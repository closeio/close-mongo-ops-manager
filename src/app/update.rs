//! Event handling for [`App`].

use std::time::Instant;

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;

use super::{
    Action, App, AppEvent, ConnectionState, ERROR_TOAST_DURATION, Effect, FILTER_DEBOUNCE,
    FilterField, Focus, LOADING_DELAY, MAX_REFRESH_INTERVAL, MAX_TOASTS, MIN_REFRESH_INTERVAL,
    Modal, RefreshStats, Severity, TOAST_DURATION, Toast,
};
use crate::model::{FetchQuery, KillReport, KillRequest, OpKey, ServerInfo, Snapshot};
use crate::theme;

/// Lines scrolled per mouse wheel step in dialogs.
const WHEEL_LINES: u16 = 3;

/// Effects accumulated while handling one event.
type Effects = Vec<Effect>;

impl App {
    /// Applies an event and returns the effects to perform.
    pub fn update(&mut self, event: AppEvent, now: Instant) -> Vec<Effect> {
        let mut fx = Vec::new();
        match event {
            AppEvent::Key(key) => self.on_key(key, now, &mut fx),
            AppEvent::Mouse(mouse) => self.on_mouse(mouse, now, &mut fx),
            AppEvent::Paste(text) => self.on_paste(&text, now),
            AppEvent::Resize | AppEvent::Tick => {}
            AppEvent::Connected(result) => self.on_connected(result, now),
            AppEvent::Fetched { generation, result } => self.on_fetched(generation, result, now),
            AppEvent::Killed(report) => self.on_killed(report, now),
        }
        if !self.should_quit {
            self.schedule(now, &mut fx);
        }
        fx
    }

    /// Timers: toast expiry, loading indicator, debounced and automatic
    /// refreshes.
    fn schedule(&mut self, now: Instant, fx: &mut Effects) {
        self.toasts.retain(|t| t.expires_at > now);
        self.loading = self
            .fetch_in_flight
            .is_some_and(|(_, started)| now.duration_since(started) >= LOADING_DELAY);

        if !matches!(self.connection, ConnectionState::Connected(_)) {
            return;
        }
        if self
            .filters_changed_at
            .is_some_and(|changed| now.duration_since(changed) >= FILTER_DEBOUNCE)
        {
            self.filters_changed_at = None;
            self.refresh_requested = true;
        }
        let auto_due = self.auto_refresh
            && self.fetch_in_flight.is_none()
            && self.next_refresh_at.is_some_and(|due| now >= due);
        if self.refresh_requested || auto_due {
            self.start_fetch(now, fx);
        }
    }

    /// Starts a fetch, superseding any fetch in flight.
    fn start_fetch(&mut self, now: Instant, fx: &mut Effects) {
        self.refresh_requested = false;
        self.next_refresh_at = None;
        self.fetch_generation += 1;
        self.fetch_in_flight = Some((self.fetch_generation, now));
        fx.push(Effect::Fetch {
            generation: self.fetch_generation,
            query: FetchQuery {
                filters: self.filters(),
                include_mongos_local: self.show_mongos_local,
            },
        });
    }

    fn request_refresh(&mut self) {
        self.refresh_requested = true;
    }

    fn notify(&mut self, message: impl Into<String>, severity: Severity, now: Instant) {
        let ttl = if severity == Severity::Error {
            ERROR_TOAST_DURATION
        } else {
            TOAST_DURATION
        };
        self.toasts.push(Toast {
            message: message.into(),
            severity,
            expires_at: now + ttl,
        });
        if self.toasts.len() > MAX_TOASTS {
            let excess = self.toasts.len() - MAX_TOASTS;
            self.toasts.drain(..excess);
        }
    }

    fn quit(&mut self, fx: &mut Effects) {
        self.should_quit = true;
        fx.push(Effect::Quit);
    }

    // ----- background results -------------------------------------------------

    fn on_connected(&mut self, result: Result<ServerInfo, String>, now: Instant) {
        // Results of earlier fetches belong to the previous connection.
        self.fetch_in_flight = None;
        match result {
            Ok(info) => {
                self.connection = ConnectionState::Connected(info);
                self.last_error = None;
                self.request_refresh();
            }
            Err(error) => {
                log::error!("Failed to connect: {error}");
                self.notify(format!("Failed to connect: {error}"), Severity::Error, now);
                self.connection = ConnectionState::Failed(error);
            }
        }
    }

    fn on_fetched(&mut self, generation: u64, result: Result<Snapshot, String>, now: Instant) {
        let Some((in_flight, started)) = self.fetch_in_flight else {
            return;
        };
        if in_flight != generation || !matches!(self.connection, ConnectionState::Connected(_)) {
            return; // superseded
        }
        self.fetch_in_flight = None;
        self.loading = false;
        self.next_refresh_at = Some(now + self.interval());
        match result {
            Ok(snapshot) => {
                let took = now.duration_since(started);
                log::info!(
                    "Loaded {} operations in {:.2} seconds",
                    snapshot.operations.len(),
                    took.as_secs_f64()
                );
                for warning in &snapshot.warnings {
                    if !self.warnings.contains(warning) {
                        log::warn!("{warning}");
                        self.notify(warning.clone(), Severity::Warning, now);
                    }
                }
                self.last_refresh = Some(RefreshStats {
                    at: now,
                    took,
                    count: snapshot.operations.len(),
                });
                self.last_error = None;
                self.apply_snapshot(snapshot);
            }
            Err(error) => {
                log::error!("Failed to refresh operations: {error}");
                if self.last_error.as_deref() != Some(error.as_str()) {
                    self.notify(format!("Failed to refresh: {error}"), Severity::Error, now);
                }
                self.last_error = Some(error);
            }
        }
    }

    /// Replaces the operations, keeping the selection and the cursor on the
    /// same operations when they are still running.
    fn apply_snapshot(&mut self, snapshot: Snapshot) {
        let cursor_key = self.cursor_operation().map(|op| op.key.clone());
        let cursor_index = self.table.selected();
        // Opids are reused (restarts, failovers): a selected key that now
        // names a different operation is deselected.
        let replaced: Vec<OpKey> = self
            .operations
            .iter()
            .filter(|old| self.selected.contains(&old.key))
            .filter(|old| {
                snapshot
                    .operations
                    .iter()
                    .find(|new| new.key == old.key)
                    .is_some_and(|new| !same_operation(old, new))
            })
            .map(|old| old.key.clone())
            .collect();
        for key in &replaced {
            self.selected.remove(key);
        }

        self.operations = snapshot.operations;
        self.truncated = snapshot.truncated;
        self.nodes = snapshot.nodes;
        self.warnings = snapshot.warnings;
        self.sort_operations();

        let present: std::collections::HashSet<&OpKey> =
            self.operations.iter().map(|op| &op.key).collect();
        self.selected.retain(|key| present.contains(key));

        let new_cursor = if self.operations.is_empty() {
            None
        } else {
            cursor_key
                .and_then(|key| self.operations.iter().position(|op| op.key == key))
                .or_else(|| cursor_index.map(|i| i.min(self.operations.len() - 1)))
                .or(Some(0))
        };
        self.table.select(new_cursor);
    }

    /// Orders the operations by running time, keeping the cursor on the same
    /// operation.
    fn sort_operations(&mut self) {
        let cursor_key = self.cursor_operation().map(|op| op.key.clone());
        let ascending = self.sort_ascending;
        self.operations.sort_by(|a, b| {
            let order = a
                .running_micros()
                .cmp(&b.running_micros())
                .then_with(|| a.key.cmp(&b.key));
            if ascending { order } else { order.reverse() }
        });
        if let Some(key) = cursor_key {
            let index = self.operations.iter().position(|op| op.key == key);
            self.table.select(index);
        }
    }

    fn on_killed(&mut self, report: KillReport, now: Instant) {
        self.kill_in_progress = false;
        let mut failures = 0;
        // The MongoDB layer logs every outcome.
        for (request, outcome) in &report.results {
            if !outcome.is_success() {
                failures += 1;
                if failures <= 3 {
                    self.notify(
                        format!("Failed to kill operation {}: {outcome}", request.opid),
                        Severity::Error,
                        now,
                    );
                }
            }
        }
        let succeeded = report.succeeded();
        if succeeded > 0 {
            self.notify(
                format!("Successfully killed {succeeded} operation(s)"),
                Severity::Info,
                now,
            );
        }
        if report.failed() > 0 {
            self.notify(
                format!("Failed to kill {} operation(s)", report.failed()),
                Severity::Error,
                now,
            );
        }
        // Keep what the user selected while the kill was running.
        for (request, _) in &report.results {
            self.selected.remove(&request.key);
        }
        self.request_refresh();
    }

    // ----- keyboard -------------------------------------------------------------

    fn on_key(&mut self, key: KeyEvent, now: Instant, fx: &mut Effects) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if is_ctrl(&key, 'q') || is_ctrl(&key, 'c') {
            self.quit(fx);
            return;
        }
        if let Some(modal) = self.modal.take() {
            self.modal = self.on_modal_key(modal, key, now, fx);
            return;
        }
        match self.focus {
            Focus::Table => self.on_table_key(key, now, fx),
            Focus::Filter(field) => self.on_filter_key(field, key, now, fx),
            Focus::ClearButton => self.on_clear_button_key(key, now, fx),
        }
    }

    fn on_table_key(&mut self, key: KeyEvent, now: Instant, fx: &mut Effects) {
        if let Some(action) = global_action(&key) {
            self.perform(action, now, fx);
            return;
        }
        if has_command_modifier(&key) {
            return;
        }
        let page = self.page_size();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::PageUp => self.move_cursor(-page),
            KeyCode::PageDown => self.move_cursor(page),
            KeyCode::Home | KeyCode::Char('g') => self.move_cursor(isize::MIN),
            KeyCode::End | KeyCode::Char('G') => self.move_cursor(isize::MAX),
            KeyCode::Enter => self.show_details(),
            KeyCode::Char(' ') => self.toggle_cursor_selection(),
            KeyCode::Char('+' | '=') => self.perform(Action::IncreaseInterval, now, fx),
            KeyCode::Char('-' | '_') => self.perform(Action::DecreaseInterval, now, fx),
            KeyCode::Char('?') => self.perform(Action::Help, now, fx),
            KeyCode::Char('/') => {
                self.filter_bar_visible = true;
                self.focus = Focus::Filter(FilterField::OpId);
            }
            KeyCode::Tab if self.filter_bar_visible => {
                self.focus = Focus::Filter(FilterField::OpId)
            }
            KeyCode::BackTab if self.filter_bar_visible => self.focus = Focus::ClearButton,
            _ => {}
        }
    }

    fn on_filter_key(&mut self, field: FilterField, key: KeyEvent, now: Instant, fx: &mut Effects) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let input = &mut self.filter_inputs[field.index()];
        // Line editing keys, as in Textual's Input; they take precedence over
        // the application bindings while typing.
        let edited = match key.code {
            KeyCode::Char('a') if ctrl => {
                input.move_home();
                Some(false)
            }
            KeyCode::Char('e') if ctrl => {
                input.move_end();
                Some(false)
            }
            KeyCode::Char('u') if ctrl => Some(input.delete_to_start()),
            KeyCode::Char('k') if ctrl => Some(input.delete_to_end()),
            KeyCode::Char('w') if ctrl => Some(input.delete_word_left()),
            KeyCode::Backspace if ctrl || alt => Some(input.delete_word_left()),
            KeyCode::Backspace => Some(input.backspace()),
            KeyCode::Delete => Some(input.delete()),
            KeyCode::Left => {
                input.move_left();
                Some(false)
            }
            KeyCode::Right => {
                input.move_right();
                Some(false)
            }
            KeyCode::Home => {
                input.move_home();
                Some(false)
            }
            KeyCode::End => {
                input.move_end();
                Some(false)
            }
            KeyCode::Char(c) if !ctrl && !alt => Some(input.insert_char(c)),
            _ => None,
        };
        if let Some(changed) = edited {
            if changed {
                self.filters_changed_at = Some(now);
            }
            return;
        }
        if let Some(action) = global_action(&key) {
            self.perform(action, now, fx);
            return;
        }
        match key.code {
            KeyCode::Tab => self.focus = next_filter_focus(Focus::Filter(field)),
            KeyCode::BackTab => self.focus = previous_filter_focus(Focus::Filter(field)),
            KeyCode::Esc | KeyCode::Down => self.focus = Focus::Table,
            KeyCode::Enter => {
                self.focus = Focus::Table;
                self.apply_pending_filters();
            }
            _ => {}
        }
    }

    fn on_clear_button_key(&mut self, key: KeyEvent, now: Instant, fx: &mut Effects) {
        if let Some(action) = global_action(&key) {
            self.perform(action, now, fx);
            return;
        }
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => self.clear_filters(),
            KeyCode::Tab => self.focus = next_filter_focus(Focus::ClearButton),
            KeyCode::BackTab => self.focus = previous_filter_focus(Focus::ClearButton),
            KeyCode::Esc | KeyCode::Down => self.focus = Focus::Table,
            KeyCode::Left => self.focus = Focus::Filter(FilterField::EffectiveUsers),
            _ => {}
        }
    }

    /// Handles a key while `modal` is open; returns the modal to keep open.
    fn on_modal_key(
        &mut self,
        modal: Modal,
        key: KeyEvent,
        now: Instant,
        fx: &mut Effects,
    ) -> Option<Modal> {
        let page = self.layout.modal_page.max(1);
        match modal {
            Modal::Help { mut scroll } => match key.code {
                KeyCode::Esc | KeyCode::Char('q' | '?') | KeyCode::F(1) => None,
                code => {
                    scroll_u16(&mut scroll, code, page);
                    Some(Modal::Help { scroll })
                }
            },
            Modal::Details { op, mut scroll } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => None,
                code => {
                    scroll_u16(&mut scroll, code, page);
                    Some(Modal::Details { op, scroll })
                }
            },
            Modal::Nodes { mut scroll } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => None,
                code => {
                    scroll_u16(&mut scroll, code, page);
                    Some(Modal::Nodes { scroll })
                }
            },
            Modal::Logs {
                mut scroll,
                mut follow,
            } => {
                let page = usize::from(page);
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => return None,
                    KeyCode::Up | KeyCode::Char('k') => {
                        follow = false;
                        scroll = scroll.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => scroll = scroll.saturating_add(1),
                    KeyCode::PageUp => {
                        follow = false;
                        scroll = scroll.saturating_sub(page);
                    }
                    KeyCode::PageDown => scroll = scroll.saturating_add(page),
                    KeyCode::Home | KeyCode::Char('g') => {
                        follow = false;
                        scroll = 0;
                    }
                    KeyCode::End | KeyCode::Char('G') => follow = true,
                    _ => {}
                }
                Some(Modal::Logs { scroll, follow })
            }
            Modal::KillConfirm {
                requests,
                yes_focused,
            } => match key.code {
                KeyCode::Esc | KeyCode::Char('n' | 'N') => None,
                KeyCode::Char('y' | 'Y') => {
                    self.confirm_kill(requests, fx);
                    None
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    if yes_focused {
                        self.confirm_kill(requests, fx);
                    }
                    None
                }
                KeyCode::Left
                | KeyCode::Right
                | KeyCode::Tab
                | KeyCode::BackTab
                | KeyCode::Char('h' | 'l') => Some(Modal::KillConfirm {
                    requests,
                    yes_focused: !yes_focused,
                }),
                _ => Some(Modal::KillConfirm {
                    requests,
                    yes_focused,
                }),
            },
            Modal::Theme { mut list, original } => {
                let last = theme::all().len().saturating_sub(1);
                let current = list.selected().unwrap_or(0);
                let target = match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => return None,
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        self.apply_theme(current, now, fx);
                        return None;
                    }
                    KeyCode::Up | KeyCode::Char('k') => current.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => (current + 1).min(last),
                    KeyCode::PageUp => current.saturating_sub(usize::from(page)),
                    KeyCode::PageDown => (current + usize::from(page)).min(last),
                    KeyCode::Home | KeyCode::Char('g') => 0,
                    KeyCode::End | KeyCode::Char('G') => last,
                    _ => current,
                };
                list.select(Some(target));
                Some(Modal::Theme { list, original })
            }
        }
    }

    // ----- mouse ------------------------------------------------------------------

    fn on_mouse(&mut self, mouse: MouseEvent, now: Instant, fx: &mut Effects) {
        let pos = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::ScrollUp => self.on_wheel(-1),
            MouseEventKind::ScrollDown => self.on_wheel(1),
            MouseEventKind::Down(MouseButton::Left) => self.on_click(pos, now, fx),
            _ => {}
        }
    }

    fn on_wheel(&mut self, direction: isize) {
        let Some(modal) = self.modal.as_mut() else {
            self.move_cursor(direction);
            return;
        };
        let lines = if direction < 0 {
            KeyCode::Up
        } else {
            KeyCode::Down
        };
        match modal {
            Modal::Help { scroll } | Modal::Details { scroll, .. } | Modal::Nodes { scroll } => {
                for _ in 0..WHEEL_LINES {
                    scroll_u16(scroll, lines, 1);
                }
            }
            Modal::Logs { scroll, follow } => {
                if direction < 0 {
                    *follow = false;
                    *scroll = scroll.saturating_sub(usize::from(WHEEL_LINES));
                } else {
                    *scroll = scroll.saturating_add(usize::from(WHEEL_LINES));
                }
            }
            Modal::Theme { list, .. } => {
                let last = theme::all().len().saturating_sub(1);
                let current = list.selected().unwrap_or(0);
                let target = if direction < 0 {
                    current.saturating_sub(1)
                } else {
                    (current + 1).min(last)
                };
                list.select(Some(target));
            }
            Modal::KillConfirm { .. } => {}
        }
    }

    fn on_click(&mut self, pos: Position, now: Instant, fx: &mut Effects) {
        // Toasts are drawn over everything: a click dismisses the toast and
        // never reaches what is underneath.
        if let Some(index) = self
            .layout
            .toasts
            .iter()
            .find(|(rect, _)| rect.contains(pos))
            .map(|(_, index)| *index)
        {
            if index < self.toasts.len() {
                self.toasts.remove(index);
            }
            self.layout.toasts.clear();
            return;
        }
        if let Some(modal) = self.modal.take() {
            self.modal = self.on_modal_click(modal, pos, now, fx);
            return;
        }
        if let Some(action) = self
            .layout
            .footer
            .iter()
            .find(|(rect, _)| rect.contains(pos))
            .map(|(_, action)| *action)
        {
            self.perform(action, now, fx);
            return;
        }
        // The recorded rects are the text lines; the whole box (border and
        // padding around the text) is clickable.
        if let Some(field) = FilterField::ALL.into_iter().find(|f| {
            let text = self.layout.filter_inputs[f.index()];
            !text.is_empty() && input_box(text).contains(pos)
        }) {
            let rect = self.layout.filter_inputs[field.index()];
            let input = &mut self.filter_inputs[field.index()];
            let offset = input.scroll_offset(usize::from(rect.width));
            let column = pos.x.clamp(rect.x, rect.right()) - rect.x;
            input.set_cursor(offset + usize::from(column));
            self.focus = Focus::Filter(field);
            return;
        }
        if self.layout.clear_button.contains(pos) {
            self.clear_filters();
            return;
        }
        let body = self.layout.table_body;
        if body.contains(pos) {
            let row = self.table.offset() + usize::from(pos.y - body.y);
            if row < self.operations.len() {
                self.focus = Focus::Table;
                self.table.select(Some(row));
                self.toggle_cursor_selection();
            }
        }
    }

    fn on_modal_click(
        &mut self,
        modal: Modal,
        pos: Position,
        now: Instant,
        fx: &mut Effects,
    ) -> Option<Modal> {
        match modal {
            Modal::KillConfirm {
                requests,
                yes_focused,
            } => {
                if self.layout.kill_yes.contains(pos) {
                    self.confirm_kill(requests, fx);
                    None
                } else if self.layout.kill_no.contains(pos) {
                    None
                } else {
                    Some(Modal::KillConfirm {
                        requests,
                        yes_focused,
                    })
                }
            }
            Modal::Theme { list, original } => {
                let area = self.layout.theme_list;
                if area.contains(pos) {
                    let index = self.layout.theme_list_offset + usize::from(pos.y - area.y);
                    if index < theme::all().len() {
                        self.apply_theme(index, now, fx);
                        return None;
                    }
                }
                Some(Modal::Theme { list, original })
            }
            other => Some(other),
        }
    }

    fn on_paste(&mut self, text: &str, now: Instant) {
        if self.modal.is_some() {
            return;
        }
        if let Focus::Filter(field) = self.focus
            && self.filter_inputs[field.index()].insert_str(text)
        {
            self.filters_changed_at = Some(now);
        }
    }

    // ----- actions ------------------------------------------------------------------

    fn perform(&mut self, action: Action, now: Instant, fx: &mut Effects) {
        match action {
            Action::Help => self.modal = Some(Modal::Help { scroll: 0 }),
            Action::Quit => self.quit(fx),
            Action::Refresh => match self.connection {
                ConnectionState::Failed(_) => {
                    self.connection = ConnectionState::Connecting;
                    self.fetch_in_flight = None;
                    fx.push(Effect::Connect);
                }
                ConnectionState::Connecting => {}
                ConnectionState::Connected(_) => self.request_refresh(),
            },
            Action::Kill => self.request_kill(now),
            Action::ToggleAutoRefresh => {
                self.auto_refresh = !self.auto_refresh;
                if self.auto_refresh && self.next_refresh_at.is_none_or(|due| due > now) {
                    // Resume with fresh data.
                    self.next_refresh_at = Some(now);
                }
            }
            Action::SortByTime => {
                self.sort_ascending = !self.sort_ascending;
                self.sort_operations();
                let direction = if self.sort_ascending {
                    "ascending"
                } else {
                    "descending"
                };
                self.notify(
                    format!("Sorted by running time ({direction})"),
                    Severity::Info,
                    now,
                );
            }
            Action::Logs => {
                self.modal = Some(Modal::Logs {
                    scroll: 0,
                    follow: true,
                });
            }
            Action::ToggleSelectAll => {
                if self.selected.is_empty() {
                    self.selected = self.operations.iter().map(|op| op.key.clone()).collect();
                } else {
                    self.selected.clear();
                }
            }
            Action::ToggleFilterBar => {
                self.filter_bar_visible = !self.filter_bar_visible;
                self.focus = if self.filter_bar_visible {
                    Focus::Filter(FilterField::OpId)
                } else {
                    Focus::Table
                };
            }
            Action::Theme => {
                let index = theme::all()
                    .iter()
                    .position(|t| t.name == self.theme.name)
                    .unwrap_or(0);
                self.modal = Some(Modal::Theme {
                    list: ListState::default().with_selected(Some(index)),
                    original: self.theme,
                });
            }
            Action::IncreaseInterval => self.set_interval(self.refresh_interval + 1, now),
            Action::DecreaseInterval => {
                self.set_interval(self.refresh_interval.saturating_sub(1), now);
            }
            Action::ToggleMongosLocal => {
                if self.server_info().is_some_and(ServerInfo::is_sharded) {
                    self.show_mongos_local = !self.show_mongos_local;
                    let message = if self.show_mongos_local {
                        "Showing the operations of mongos itself"
                    } else {
                        "Hiding the operations of mongos itself"
                    };
                    self.notify(message, Severity::Info, now);
                    self.request_refresh();
                } else {
                    self.notify(
                        "Mongos operations are only available when connected to mongos",
                        Severity::Info,
                        now,
                    );
                }
            }
            Action::Nodes => {
                if self.server_info().is_some_and(|info| info.all_nodes) {
                    self.modal = Some(Modal::Nodes { scroll: 0 });
                } else {
                    self.notify(
                        "Start with --all-nodes to poll every cluster member",
                        Severity::Info,
                        now,
                    );
                }
            }
        }
    }

    fn interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.refresh_interval)
    }

    fn set_interval(&mut self, secs: u64, now: Instant) {
        let secs = secs.clamp(MIN_REFRESH_INTERVAL, MAX_REFRESH_INTERVAL);
        if secs == self.refresh_interval {
            return;
        }
        self.refresh_interval = secs;
        if self.next_refresh_at.is_some() {
            let base = self.last_refresh.map_or(now, |stats| stats.at);
            self.next_refresh_at = Some(base + self.interval());
        }
    }

    fn request_kill(&mut self, now: Instant) {
        if self.kill_in_progress {
            self.notify("A kill is already in progress", Severity::Warning, now);
            return;
        }
        // Selected keys always name listed operations (refreshes prune the
        // selection); keep the display order.
        let requests: Vec<KillRequest> = self
            .operations
            .iter()
            .filter(|op| self.selected.contains(&op.key))
            .map(KillRequest::from_operation)
            .collect();
        if requests.is_empty() {
            self.notify("No operations selected", Severity::Info, now);
            return;
        }
        self.modal = Some(Modal::KillConfirm {
            requests,
            yes_focused: false,
        });
    }

    fn confirm_kill(&mut self, requests: Vec<KillRequest>, fx: &mut Effects) {
        self.kill_in_progress = true;
        fx.push(Effect::Kill(requests));
    }

    fn apply_theme(&mut self, index: usize, now: Instant, fx: &mut Effects) {
        let Some(selected) = theme::all().get(index) else {
            return;
        };
        self.theme = selected;
        fx.push(Effect::SaveTheme(selected.name));
        self.notify(
            format!("Theme changed to {}", theme::display_name(selected.name)),
            Severity::Info,
            now,
        );
    }

    fn show_details(&mut self) {
        if let Some(op) = self.cursor_operation() {
            self.modal = Some(Modal::Details {
                op: Box::new(op.clone()),
                scroll: 0,
            });
        }
    }

    fn toggle_cursor_selection(&mut self) {
        let Some(key) = self.cursor_operation().map(|op| op.key.clone()) else {
            return;
        };
        if !self.selected.remove(&key) {
            self.selected.insert(key);
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        if self.operations.is_empty() {
            self.table.select(None);
            return;
        }
        let last = self.operations.len() - 1;
        let current = self.table.selected().unwrap_or(0).min(last);
        let target = current.saturating_add_signed(delta).min(last);
        self.table.select(Some(target));
    }

    /// Rows per page in the table.
    fn page_size(&self) -> isize {
        isize::try_from(self.layout.table_body.height.max(2) - 1).unwrap_or(1)
    }

    fn clear_filters(&mut self) {
        let mut changed = false;
        for input in &mut self.filter_inputs {
            changed |= input.clear();
        }
        if self.filter_bar_visible {
            self.focus = Focus::Filter(FilterField::OpId);
        }
        if changed {
            self.filters_changed_at = None;
            self.request_refresh();
        }
    }

    /// Refreshes now if filters changed and the debounce is still pending.
    fn apply_pending_filters(&mut self) {
        if self.filters_changed_at.take().is_some() {
            self.request_refresh();
        }
    }
}

/// Whether two listings with the same key are the same operation: same start
/// time (within a second: it is computed from the running time), or the same
/// connection (the start time of multi-statement writes moves with each
/// statement). Unknown values compare equal.
fn same_operation(a: &crate::model::Operation, b: &crate::model::Operation) -> bool {
    let parse = |t: &Option<String>| {
        t.as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
    };
    let same_start = match (parse(&a.current_op_time), parse(&b.current_op_time)) {
        (Some(a), Some(b)) => (a - b).num_milliseconds().abs() <= 1000,
        _ => true,
    };
    same_start || (!a.desc.is_empty() && a.desc == b.desc)
}

/// Application-wide key bindings.
fn global_action(key: &KeyEvent) -> Option<Action> {
    if key.code == KeyCode::F(1) {
        return Some(Action::Help);
    }
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    Some(match c.to_ascii_lowercase() {
        'r' => Action::Refresh,
        'k' => Action::Kill,
        'p' => Action::ToggleAutoRefresh,
        's' => Action::SortByTime,
        'l' => Action::Logs,
        'a' => Action::ToggleSelectAll,
        'f' => Action::ToggleFilterBar,
        't' => Action::Theme,
        'o' => Action::ToggleMongosLocal,
        'n' => Action::Nodes,
        '=' | '+' => Action::IncreaseInterval,
        '-' | '_' => Action::DecreaseInterval,
        _ => return None,
    })
}

fn is_ctrl(key: &KeyEvent, c: char) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(k) if k.eq_ignore_ascii_case(&c))
}

/// Ctrl/Alt/Super combinations that are not bindings are ignored rather than
/// treated as plain characters.
fn has_command_modifier(key: &KeyEvent) -> bool {
    key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

/// The box drawn around a filter input's text line: one cell of border and
/// one of padding on the sides, one row of border above and below.
fn input_box(text: Rect) -> Rect {
    Rect::new(
        text.x.saturating_sub(2),
        text.y.saturating_sub(1),
        text.width.saturating_add(4),
        text.height.saturating_add(2),
    )
}

/// Focus order with Tab: the filter inputs, the Clear button, the table.
fn next_filter_focus(focus: Focus) -> Focus {
    match focus {
        Focus::Filter(field) => FilterField::ALL
            .get(field.index() + 1)
            .map_or(Focus::ClearButton, |f| Focus::Filter(*f)),
        Focus::ClearButton => Focus::Table,
        Focus::Table => Focus::Filter(FilterField::OpId),
    }
}

fn previous_filter_focus(focus: Focus) -> Focus {
    match focus {
        Focus::Filter(field) => field
            .index()
            .checked_sub(1)
            .map_or(Focus::Table, |i| Focus::Filter(FilterField::ALL[i])),
        Focus::ClearButton => Focus::Filter(FilterField::EffectiveUsers),
        Focus::Table => Focus::ClearButton,
    }
}

/// Applies a scrolling key to a requested scroll offset (clamped by the UI).
fn scroll_u16(scroll: &mut u16, code: KeyCode, page: u16) {
    *scroll = match code {
        KeyCode::Up | KeyCode::Char('k') => scroll.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => scroll.saturating_add(1),
        KeyCode::PageUp => scroll.saturating_sub(page),
        KeyCode::PageDown | KeyCode::Char(' ') => scroll.saturating_add(page),
        KeyCode::Home | KeyCode::Char('g') => 0,
        KeyCode::End | KeyCode::Char('G') => u16::MAX,
        _ => *scroll,
    };
}
