//! Tests for the application state machine.

use std::time::{Duration, Instant};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use super::*;
use crate::model::{
    Deployment, KillOutcome, KillReport, NodeHealth, NodeRole, NodeStatus, OpId, Snapshot,
};
use crate::testutil::operation_running;

fn options() -> AppOptions {
    AppOptions {
        refresh_interval: 2,
        theme: theme::default_theme(),
        truecolor: true,
        logs: LogBuffer::new(),
        title: "test".into(),
    }
}

fn info(deployment: Deployment, all_nodes: bool) -> ServerInfo {
    ServerInfo {
        target: "localhost:27017".into(),
        version: "8.0.4".into(),
        deployment,
        load_balanced: false,
        all_nodes,
    }
}

/// A test harness driving the app with a controllable clock.
struct Harness {
    app: App,
    now: Instant,
}

impl Harness {
    fn new() -> Self {
        Self {
            app: App::new(options()),
            now: Instant::now(),
        }
    }

    /// A harness connected to a standalone server, with the first fetch done.
    fn connected(ops: Vec<Operation>) -> Self {
        Self::connected_to(info(Deployment::Standalone, false), ops)
    }

    fn connected_to(server: ServerInfo, ops: Vec<Operation>) -> Self {
        let mut h = Self::new();
        assert_eq!(h.app.start(), vec![Effect::Connect]);
        let fx = h.send(AppEvent::Connected(Ok(server)));
        let generation = fetch_generation(&fx).expect("connecting starts a fetch");
        h.fetched(generation, ops);
        h
    }

    fn send(&mut self, event: AppEvent) -> Vec<Effect> {
        self.app.update(event, self.now)
    }

    fn advance(&mut self, by: Duration) -> Vec<Effect> {
        self.now += by;
        self.send(AppEvent::Tick)
    }

    fn fetched(&mut self, generation: u64, ops: Vec<Operation>) -> Vec<Effect> {
        self.send(AppEvent::Fetched {
            generation,
            result: Ok(Snapshot {
                operations: ops,
                ..Snapshot::default()
            }),
        })
    }

    fn key(&mut self, code: KeyCode) -> Vec<Effect> {
        self.send(AppEvent::Key(KeyEvent::new(code, KeyModifiers::NONE)))
    }

    fn ctrl(&mut self, c: char) -> Vec<Effect> {
        self.send(AppEvent::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::CONTROL,
        )))
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.key(KeyCode::Char(c));
        }
    }

    fn click(&mut self, x: u16, y: u16) -> Vec<Effect> {
        self.send(AppEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }))
    }

    fn toast_messages(&self) -> Vec<&str> {
        self.app.toasts.iter().map(|t| t.message.as_str()).collect()
    }

    fn opids(&self) -> Vec<String> {
        self.app
            .operations
            .iter()
            .map(|op| op.opid.to_string())
            .collect()
    }
}

fn fetch_generation(fx: &[Effect]) -> Option<u64> {
    fx.iter().find_map(|e| match e {
        Effect::Fetch { generation, .. } => Some(*generation),
        _ => None,
    })
}

fn fetch_query(fx: &[Effect]) -> Option<&FetchQuery> {
    fx.iter().find_map(|e| match e {
        Effect::Fetch { query, .. } => Some(query),
        _ => None,
    })
}

fn three_ops() -> Vec<Operation> {
    vec![
        operation_running(1, 30),
        operation_running(2, 10),
        operation_running(3, 20),
    ]
}

#[test]
fn starts_by_connecting() {
    let mut app = App::new(options());
    assert_eq!(app.start(), vec![Effect::Connect]);
    assert_eq!(app.connection, ConnectionState::Connecting);
    assert!(app.status_text().starts_with("Connecting..."));
}

#[test]
fn refresh_interval_is_clamped() {
    assert_eq!(clamp_refresh_interval(0), 1);
    assert_eq!(clamp_refresh_interval(-5), 1);
    assert_eq!(clamp_refresh_interval(5), 5);
    assert_eq!(clamp_refresh_interval(99), 10);
    let app = App::new(AppOptions {
        refresh_interval: 50,
        ..options()
    });
    assert_eq!(app.refresh_interval, MAX_REFRESH_INTERVAL);
}

#[test]
fn connection_failure_is_reported_and_ctrl_r_reconnects() {
    let mut h = Harness::new();
    h.app.start();
    let fx = h.send(AppEvent::Connected(Err("no servers".into())));
    assert!(fx.is_empty());
    assert_eq!(
        h.app.connection,
        ConnectionState::Failed("no servers".into())
    );
    assert_eq!(h.toast_messages(), vec!["Failed to connect: no servers"]);
    assert!(h.app.status_text().starts_with("Disconnected"));

    assert_eq!(h.ctrl('r'), vec![Effect::Connect]);
    assert_eq!(h.app.connection, ConnectionState::Connecting);
}

#[test]
fn first_fetch_sorts_ascending_and_places_cursor() {
    let h = Harness::connected(three_ops());
    assert_eq!(h.opids(), vec!["2", "3", "1"]);
    assert_eq!(h.app.table.selected(), Some(0));
    assert!(h.app.last_refresh.is_some());
}

#[test]
fn auto_refresh_runs_after_the_interval() {
    let mut h = Harness::connected(three_ops());
    assert!(fetch_generation(&h.advance(Duration::from_millis(1900))).is_none());
    let fx = h.advance(Duration::from_millis(200));
    assert!(fetch_generation(&fx).is_some());
    // No second fetch while one is in flight.
    assert!(fetch_generation(&h.advance(Duration::from_secs(5))).is_none());
}

#[test]
fn paused_auto_refresh_does_not_fetch_and_resume_refreshes() {
    let mut h = Harness::connected(three_ops());
    h.ctrl('p');
    assert!(!h.app.auto_refresh);
    assert!(h.app.status_text().contains("Auto-refresh paused (2s)"));
    assert!(fetch_generation(&h.advance(Duration::from_secs(10))).is_none());
    let fx = h.ctrl('p');
    assert!(h.app.auto_refresh);
    assert!(fetch_generation(&fx).is_some());
}

#[test]
fn stale_fetch_results_are_ignored() {
    let mut h = Harness::connected(three_ops());
    let first = fetch_generation(&h.ctrl('r')).unwrap();
    let second = fetch_generation(&h.ctrl('r')).unwrap();
    assert!(second > first);
    h.fetched(first, vec![operation_running(9, 1)]);
    assert_eq!(h.opids(), vec!["2", "3", "1"]);
    h.fetched(second, vec![operation_running(8, 1)]);
    assert_eq!(h.opids(), vec!["8"]);
}

#[test]
fn loading_indicator_appears_after_a_delay() {
    let mut h = Harness::connected(three_ops());
    let generation = fetch_generation(&h.ctrl('r')).unwrap();
    assert!(!h.app.loading);
    h.advance(LOADING_DELAY);
    assert!(h.app.loading);
    assert!(h.app.table_title().contains("Refreshing..."));
    h.fetched(generation, three_ops());
    assert!(!h.app.loading);
}

#[test]
fn fetch_errors_are_toasted_once() {
    let mut h = Harness::connected(three_ops());
    for _ in 0..2 {
        let generation = fetch_generation(&h.ctrl('r')).unwrap();
        h.send(AppEvent::Fetched {
            generation,
            result: Err("boom".into()),
        });
    }
    assert_eq!(h.toast_messages(), vec!["Failed to refresh: boom"]);
    assert_eq!(h.app.last_error.as_deref(), Some("boom"));
    // The previous rows are kept.
    assert_eq!(h.app.operations.len(), 3);
}

#[test]
fn snapshot_warnings_are_toasted_once() {
    let mut h = Harness::connected(three_ops());
    for _ in 0..2 {
        let generation = fetch_generation(&h.ctrl('r')).unwrap();
        h.send(AppEvent::Fetched {
            generation,
            result: Ok(Snapshot {
                operations: three_ops(),
                warnings: vec!["shard02 unreachable".into()],
                ..Snapshot::default()
            }),
        });
    }
    assert_eq!(h.toast_messages(), vec!["shard02 unreachable"]);
}

#[test]
fn toasts_expire() {
    let mut h = Harness::connected(three_ops());
    h.ctrl('k');
    assert_eq!(h.toast_messages(), vec!["No operations selected"]);
    h.advance(TOAST_DURATION);
    assert!(h.app.toasts.is_empty());
}

#[test]
fn space_toggles_selection_of_cursor_row() {
    let mut h = Harness::connected(three_ops());
    h.key(KeyCode::Down);
    h.key(KeyCode::Char(' '));
    let key = h.app.operations[1].key.clone();
    assert!(h.app.selected.contains(&key));
    assert!(h.app.status_text().contains("Selected: 1"));
    h.key(KeyCode::Char(' '));
    assert!(h.app.selected.is_empty());
}

#[test]
fn ctrl_a_selects_all_then_deselects_all() {
    let mut h = Harness::connected(three_ops());
    h.ctrl('a');
    assert_eq!(h.app.selected.len(), 3);
    h.ctrl('a');
    assert!(h.app.selected.is_empty());
}

#[test]
fn selection_and_cursor_survive_refreshes() {
    let mut h = Harness::connected(three_ops());
    h.key(KeyCode::End); // opid 1, the slowest
    h.key(KeyCode::Char(' '));
    let generation = fetch_generation(&h.ctrl('r')).unwrap();
    // A new, slower operation appears; opid 1 is still running.
    let mut ops = three_ops();
    ops.push(operation_running(4, 99));
    h.fetched(generation, ops);
    assert_eq!(h.opids(), vec!["2", "3", "1", "4"]);
    assert_eq!(h.app.cursor_operation().unwrap().opid, OpId::Num(1));
    assert_eq!(h.app.selected.len(), 1);

    // opid 1 finishes: selection pruned, cursor stays at the same index.
    let generation = fetch_generation(&h.ctrl('r')).unwrap();
    h.fetched(
        generation,
        vec![
            operation_running(2, 10),
            operation_running(3, 20),
            operation_running(4, 99),
        ],
    );
    assert!(h.app.selected.is_empty());
    assert_eq!(h.app.table.selected(), Some(2));

    // Everything finishes.
    let generation = fetch_generation(&h.ctrl('r')).unwrap();
    h.fetched(generation, vec![]);
    assert_eq!(h.app.table.selected(), None);
}

#[test]
fn sort_toggle_reverses_order_and_keeps_cursor() {
    let mut h = Harness::connected(three_ops());
    assert_eq!(h.app.cursor_operation().unwrap().opid, OpId::Num(2));
    h.ctrl('s');
    assert!(!h.app.sort_ascending);
    assert_eq!(h.opids(), vec!["1", "3", "2"]);
    assert_eq!(h.app.cursor_operation().unwrap().opid, OpId::Num(2));
    assert_eq!(
        h.toast_messages(),
        vec!["Sorted by running time (descending)"]
    );
    h.ctrl('s');
    assert_eq!(h.opids(), vec!["2", "3", "1"]);
}

#[test]
fn cursor_movement_is_clamped() {
    let mut h = Harness::connected(three_ops());
    h.app.layout.table_body = Rect::new(1, 3, 80, 10);
    h.key(KeyCode::Up);
    assert_eq!(h.app.table.selected(), Some(0));
    h.key(KeyCode::PageDown);
    assert_eq!(h.app.table.selected(), Some(2));
    h.key(KeyCode::Home);
    assert_eq!(h.app.table.selected(), Some(0));
    h.key(KeyCode::Char('G'));
    assert_eq!(h.app.table.selected(), Some(2));
}

#[test]
fn enter_opens_details_of_cursor_row() {
    let mut h = Harness::connected(three_ops());
    h.key(KeyCode::Down);
    h.key(KeyCode::Enter);
    match &h.app.modal {
        Some(Modal::Details { op, scroll }) => {
            assert_eq!(op.opid, OpId::Num(3));
            assert_eq!(*scroll, 0);
        }
        other => panic!("unexpected modal {other:?}"),
    }
    h.key(KeyCode::Down);
    assert!(matches!(
        h.app.modal,
        Some(Modal::Details { scroll: 1, .. })
    ));
    h.key(KeyCode::Esc);
    assert!(h.app.modal.is_none());
}

#[test]
fn enter_without_operations_does_nothing() {
    let mut h = Harness::connected(vec![]);
    h.key(KeyCode::Enter);
    assert!(h.app.modal.is_none());
}

#[test]
fn kill_without_selection_notifies() {
    let mut h = Harness::connected(three_ops());
    assert!(h.ctrl('k').is_empty());
    assert!(h.app.modal.is_none());
    assert_eq!(h.toast_messages(), vec!["No operations selected"]);
}

#[test]
fn kill_confirmation_defaults_to_no() {
    let mut h = Harness::connected(three_ops());
    h.ctrl('a');
    h.ctrl('k');
    match &h.app.modal {
        Some(Modal::KillConfirm {
            requests,
            yes_focused,
        }) => {
            assert_eq!(requests.len(), 3);
            assert!(!yes_focused);
        }
        other => panic!("unexpected modal {other:?}"),
    }
    let fx = h.key(KeyCode::Enter);
    assert!(fx.iter().all(|e| !matches!(e, Effect::Kill(_))));
    assert!(h.app.modal.is_none());
    assert_eq!(h.app.selected.len(), 3);
}

#[test]
fn kill_confirmed_with_y_kills_selected_in_display_order() {
    let mut h = Harness::connected(three_ops());
    h.ctrl('a');
    h.ctrl('k');
    let fx = h.key(KeyCode::Char('y'));
    let requests = fx
        .iter()
        .find_map(|e| match e {
            Effect::Kill(r) => Some(r.clone()),
            _ => None,
        })
        .expect("kill effect");
    let opids: Vec<String> = requests.iter().map(|r| r.opid.to_string()).collect();
    assert_eq!(opids, vec!["2", "3", "1"]);
    assert!(h.app.kill_in_progress);
    assert!(h.app.status_text().contains("Killing..."));

    // A second kill is refused while the first runs.
    h.ctrl('k');
    assert!(
        h.toast_messages()
            .contains(&"A kill is already in progress")
    );

    let report = KillReport {
        results: vec![
            (requests[0].clone(), KillOutcome::Killed),
            (requests[1].clone(), KillOutcome::AlreadyFinished),
            (
                requests[2].clone(),
                KillOutcome::Failed("not authorized".into()),
            ),
        ],
    };
    let fx = h.send(AppEvent::Killed(report));
    assert!(!h.app.kill_in_progress);
    assert!(h.app.selected.is_empty());
    assert!(fetch_generation(&fx).is_some(), "refreshes after killing");
    let toasts = h.toast_messages();
    assert!(toasts.contains(&"Successfully killed 2 operation(s)"));
    assert!(toasts.contains(&"Failed to kill 1 operation(s)"));
    assert!(toasts.contains(&"Failed to kill operation 1: kill failed: not authorized"));
}

#[test]
fn kill_dialog_buttons_switch_focus() {
    let mut h = Harness::connected(three_ops());
    h.key(KeyCode::Char(' '));
    h.ctrl('k');
    h.key(KeyCode::Left);
    assert!(matches!(
        h.app.modal,
        Some(Modal::KillConfirm {
            yes_focused: true,
            ..
        })
    ));
    let fx = h.key(KeyCode::Enter);
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::Kill(r) if r.len() == 1))
    );
}

#[test]
fn kill_dialog_escape_cancels() {
    let mut h = Harness::connected(three_ops());
    h.key(KeyCode::Char(' '));
    h.ctrl('k');
    let fx = h.key(KeyCode::Esc);
    assert!(fx.is_empty());
    assert!(h.app.modal.is_none());
}

#[test]
fn kill_uses_the_operations_as_listed_when_the_dialog_opened() {
    let mut h = Harness::connected(three_ops());
    h.ctrl('a');
    h.ctrl('k');
    // A refresh lands while the dialog is open: opid 3 finished and opid 2
    // now names another operation (it started later).
    let generation = fetch_generation(&h.advance(Duration::from_secs(2))).unwrap();
    let mut reused = operation_running(2, 1);
    reused.current_op_time = Some("2026-10-08T10:05:00.000+00:00".into());
    h.fetched(generation, vec![operation_running(1, 30), reused]);
    let fx = h.key(KeyCode::Char('y'));
    let requests = fx
        .iter()
        .find_map(|e| match e {
            Effect::Kill(r) => Some(r.clone()),
            _ => None,
        })
        .unwrap();
    // All three are sent as they were seen; the MongoDB layer reports the
    // finished one as already finished and refuses the reused opid.
    assert_eq!(requests.len(), 3);
    let opid2 = requests.iter().find(|r| r.opid == OpId::Num(2)).unwrap();
    assert_eq!(
        opid2.current_op_time.as_deref(),
        Some("2026-10-08T10:00:00.000+00:00")
    );
    h.send(AppEvent::Killed(KillReport {
        results: requests
            .into_iter()
            .map(|r| (r, KillOutcome::AlreadyFinished))
            .collect(),
    }));
    assert!(
        h.toast_messages()
            .contains(&"Successfully killed 3 operation(s)")
    );
}

#[test]
fn selection_is_dropped_when_an_opid_is_reused() {
    let mut h = Harness::connected(three_ops());
    h.ctrl('a');
    let generation = fetch_generation(&h.ctrl('r')).unwrap();
    let mut reused = operation_running(2, 1);
    reused.current_op_time = Some("2026-10-08T11:00:00.000+00:00".into());
    reused.desc = "conn77".into();
    let mut same = operation_running(3, 21);
    // Rounding of the computed start time between listings.
    same.current_op_time = Some("2026-10-08T10:00:00.400+00:00".into());
    // Same connection, start moved: a multi-statement write, still selected.
    let mut next_statement = operation_running(1, 2);
    next_statement.current_op_time = Some("2026-10-08T10:07:00.000+00:00".into());
    h.fetched(generation, vec![next_statement, reused, same]);
    let selected: Vec<String> = h
        .app
        .operations
        .iter()
        .filter(|op| h.app.selected.contains(&op.key))
        .map(|op| op.opid.to_string())
        .collect();
    assert_eq!(selected, vec!["1", "3"]);
}

#[test]
fn kill_results_only_deselect_the_killed_operations() {
    let mut h = Harness::connected(three_ops());
    h.key(KeyCode::Char(' ')); // opid 2
    h.ctrl('k');
    let fx = h.key(KeyCode::Char('y'));
    let requests = fx
        .iter()
        .find_map(|e| match e {
            Effect::Kill(r) => Some(r.clone()),
            _ => None,
        })
        .unwrap();
    // The user selects another operation while the kill runs.
    h.key(KeyCode::Down);
    h.key(KeyCode::Char(' ')); // opid 3
    h.send(AppEvent::Killed(KillReport {
        results: requests
            .into_iter()
            .map(|r| (r, KillOutcome::Killed))
            .collect(),
    }));
    assert_eq!(h.app.selected.len(), 1);
    assert!(h.app.selected.contains(&h.app.operations[1].key));
}

#[test]
fn clicking_a_toast_dismisses_it_without_clicking_below() {
    let mut h = Harness::connected(three_ops());
    h.key(KeyCode::Char(' '));
    h.ctrl('k');
    h.app.layout.kill_yes = Rect::new(10, 10, 7, 1);
    h.app.toasts.push(Toast {
        message: "covering".into(),
        severity: Severity::Error,
        expires_at: h.now + Duration::from_secs(8),
    });
    let index = h.app.toasts.len() - 1;
    h.app.layout.toasts = vec![(Rect::new(8, 9, 20, 3), index)];
    let fx = h.click(12, 10);
    assert!(fx.iter().all(|e| !matches!(e, Effect::Kill(_))));
    assert!(matches!(h.app.modal, Some(Modal::KillConfirm { .. })));
    assert!(!h.toast_messages().contains(&"covering"));
}

#[test]
fn filter_bar_toggle_and_debounced_filtering() {
    let mut h = Harness::connected(three_ops());
    h.ctrl('f');
    assert!(h.app.filter_bar_visible);
    assert_eq!(h.app.focus, Focus::Filter(FilterField::OpId));

    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Filter(FilterField::Client));
    h.type_text("10.0");
    assert!(fetch_generation(&h.advance(Duration::from_millis(100))).is_none());
    let fx = h.advance(FILTER_DEBOUNCE);
    let query = fetch_query(&fx).expect("debounced refresh");
    assert_eq!(query.filters.client, "10.0");
    assert!(query.filters.opid.is_empty());

    // Typing `k`, `a`, ... goes to the input, not to bindings.
    h.type_text("q");
    assert_eq!(
        h.app.filter_inputs[FilterField::Client.index()].value(),
        "10.0q"
    );
    assert!(!h.app.should_quit());

    h.ctrl('f');
    assert!(!h.app.filter_bar_visible);
    assert_eq!(h.app.focus, Focus::Table);
    // Hidden filters stay applied.
    assert_eq!(h.app.filters().client, "10.0q");
}

#[test]
fn enter_in_filter_applies_immediately() {
    let mut h = Harness::connected(three_ops());
    h.key(KeyCode::Char('/'));
    h.type_text("12");
    let fx = h.key(KeyCode::Enter);
    assert_eq!(fetch_query(&fx).unwrap().filters.opid, "12");
    assert_eq!(h.app.focus, Focus::Table);
}

#[test]
fn filter_line_editing_keys() {
    let mut h = Harness::connected(three_ops());
    h.ctrl('f');
    h.type_text("abc def");
    h.ctrl('w');
    assert_eq!(h.app.filter_inputs[0].value(), "abc ");
    h.ctrl('a');
    h.ctrl('k');
    assert_eq!(h.app.filter_inputs[0].value(), "");
    h.send(AppEvent::Paste("x\ny".into()));
    assert_eq!(h.app.filter_inputs[0].value(), "x y");
}

#[test]
fn clear_button_clears_filters_and_refreshes() {
    let mut h = Harness::connected(three_ops());
    h.ctrl('f');
    h.type_text("7");
    h.advance(FILTER_DEBOUNCE);
    for _ in 0..6 {
        h.key(KeyCode::Tab);
    }
    assert_eq!(h.app.focus, Focus::ClearButton);
    let fx = h.key(KeyCode::Enter);
    assert!(fetch_query(&fx).is_some_and(|q| q.filters.is_empty()));
    assert_eq!(h.app.focus, Focus::Filter(FilterField::OpId));
    h.key(KeyCode::BackTab);
    assert_eq!(h.app.focus, Focus::Table);
}

#[test]
fn escape_leaves_the_filter_bar() {
    let mut h = Harness::connected(three_ops());
    h.ctrl('f');
    h.key(KeyCode::Esc);
    assert_eq!(h.app.focus, Focus::Table);
    assert!(h.app.filter_bar_visible);
}

#[test]
fn refresh_interval_keys() {
    let mut h = Harness::connected(three_ops());
    h.key(KeyCode::Char('+'));
    assert_eq!(h.app.refresh_interval, 3);
    assert!(h.app.status_text().contains("(3s)"));
    h.ctrl('-');
    h.ctrl('-');
    h.ctrl('-');
    assert_eq!(h.app.refresh_interval, MIN_REFRESH_INTERVAL);
    for _ in 0..20 {
        h.ctrl('=');
    }
    assert_eq!(h.app.refresh_interval, MAX_REFRESH_INTERVAL);
}

#[test]
fn help_and_logs_modals() {
    let mut h = Harness::connected(three_ops());
    h.key(KeyCode::F(1));
    assert!(matches!(h.app.modal, Some(Modal::Help { scroll: 0 })));
    h.key(KeyCode::PageDown);
    h.key(KeyCode::Esc);
    assert!(h.app.modal.is_none());

    h.ctrl('l');
    assert!(matches!(
        h.app.modal,
        Some(Modal::Logs { follow: true, .. })
    ));
    h.key(KeyCode::Up);
    assert!(matches!(
        h.app.modal,
        Some(Modal::Logs { follow: false, .. })
    ));
    h.key(KeyCode::End);
    assert!(matches!(
        h.app.modal,
        Some(Modal::Logs { follow: true, .. })
    ));
    // Bindings other than quit are inactive while a dialog is open.
    h.ctrl('t');
    assert!(matches!(h.app.modal, Some(Modal::Logs { .. })));
    h.key(KeyCode::Esc);
    assert!(h.app.modal.is_none());
}

#[test]
fn theme_picker_previews_selects_and_cancels() {
    let mut h = Harness::connected(three_ops());
    let original = h.app.palette();
    h.ctrl('t');
    h.key(KeyCode::Down);
    assert_ne!(h.app.palette(), original, "highlighted theme is previewed");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.palette(), original);
    assert_eq!(h.app.theme.name, theme::DEFAULT_THEME);

    h.ctrl('t');
    h.key(KeyCode::Down);
    h.key(KeyCode::Down);
    let fx = h.key(KeyCode::Enter);
    let chosen = theme::all()[2].name;
    assert_eq!(h.app.theme.name, chosen);
    assert!(fx.contains(&Effect::SaveTheme(chosen)));
    assert!(h.toast_messages().contains(&"Theme changed to Nord"));
}

#[test]
fn quit_keys() {
    let mut h = Harness::connected(three_ops());
    assert_eq!(h.ctrl('q'), vec![Effect::Quit]);
    assert!(h.app.should_quit());
    let mut h = Harness::connected(three_ops());
    h.ctrl('t');
    assert_eq!(h.ctrl('c'), vec![Effect::Quit], "quits from dialogs too");
}

#[test]
fn mongos_ops_toggle_requires_sharded_cluster() {
    let mut h = Harness::connected(three_ops());
    h.ctrl('o');
    assert!(!h.app.show_mongos_local);
    assert!(h.toast_messages()[0].contains("only available when connected to mongos"));

    let mut h = Harness::connected_to(info(Deployment::Sharded, false), three_ops());
    let fx = h.ctrl('o');
    assert!(h.app.show_mongos_local);
    assert!(fetch_query(&fx).unwrap().include_mongos_local);
    assert!(h.app.footer_actions().contains(&Action::ToggleMongosLocal));
    assert!(!h.app.footer_actions().contains(&Action::Nodes));
    assert!(h.app.topology_columns());
}

#[test]
fn nodes_modal_requires_all_nodes() {
    let mut h = Harness::connected(three_ops());
    h.ctrl('n');
    assert!(h.app.modal.is_none());

    let mut h = Harness::connected_to(
        info(Deployment::ReplicaSet { name: "rs0".into() }, true),
        three_ops(),
    );
    h.ctrl('n');
    assert!(matches!(h.app.modal, Some(Modal::Nodes { .. })));
    assert!(h.app.footer_actions().contains(&Action::Nodes));
    assert!(h.app.topology_columns());
}

#[test]
fn node_health_in_status_bar() {
    let mut h = Harness::connected_to(info(Deployment::Sharded, true), vec![]);
    let generation = fetch_generation(&h.ctrl('r')).unwrap();
    let node = |health| NodeStatus {
        address: "localhost:37021".into(),
        shard: Some("shard01".into()),
        role: NodeRole::Primary,
        health,
    };
    h.send(AppEvent::Fetched {
        generation,
        result: Ok(Snapshot {
            nodes: vec![
                node(NodeHealth::Ok {
                    operations: 1,
                    latency: Duration::from_millis(3),
                }),
                node(NodeHealth::Failed {
                    error: "timeout".into(),
                }),
            ],
            ..Snapshot::default()
        }),
    });
    assert!(h.app.status_text().contains("Nodes: 1/2 ok"));
}

#[test]
fn mouse_click_selects_rows_and_triggers_footer_actions() {
    let mut h = Harness::connected(three_ops());
    h.app.layout.table_body = Rect::new(1, 3, 80, 10);
    h.app.layout.footer = vec![(Rect::new(0, 20, 10, 1), Action::Help)];

    h.click(5, 4); // second row
    assert_eq!(h.app.table.selected(), Some(1));
    assert!(h.app.selected.contains(&h.app.operations[1].key));
    h.click(5, 4);
    assert!(h.app.selected.is_empty());
    h.click(5, 12); // below the rows
    assert_eq!(h.app.table.selected(), Some(1));

    h.click(2, 20);
    assert!(matches!(h.app.modal, Some(Modal::Help { .. })));
}

#[test]
fn mouse_click_on_kill_dialog_and_filter_input() {
    let mut h = Harness::connected(three_ops());
    h.key(KeyCode::Char(' '));
    h.ctrl('k');
    h.app.layout.kill_yes = Rect::new(10, 10, 7, 1);
    h.app.layout.kill_no = Rect::new(20, 10, 6, 1);
    let fx = h.click(12, 10);
    assert!(fx.iter().any(|e| matches!(e, Effect::Kill(_))));

    let mut h = Harness::connected(three_ops());
    h.ctrl('f');
    h.type_text("abcdef");
    h.app.layout.filter_inputs[FilterField::Client.index()] = Rect::new(40, 2, 10, 1);
    h.click(41, 2);
    assert_eq!(h.app.focus, Focus::Filter(FilterField::Client));

    // The border row of another input focuses it too, cursor at the start.
    h.app.layout.filter_inputs[FilterField::OpId.index()] = Rect::new(2, 2, 10, 1);
    h.click(1, 1);
    assert_eq!(h.app.focus, Focus::Filter(FilterField::OpId));
    assert_eq!(h.app.filter_inputs[FilterField::OpId.index()].cursor(), 0);
}

#[test]
fn mouse_wheel_moves_cursor_or_scrolls_dialogs() {
    let mut h = Harness::connected(three_ops());
    let wheel = |kind| {
        AppEvent::Mouse(MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        })
    };
    h.send(wheel(MouseEventKind::ScrollDown));
    assert_eq!(h.app.table.selected(), Some(1));
    h.key(KeyCode::F(1));
    h.send(wheel(MouseEventKind::ScrollDown));
    assert!(matches!(h.app.modal, Some(Modal::Help { scroll: 3 })));
}

#[test]
fn key_release_events_are_ignored() {
    let mut h = Harness::connected(three_ops());
    let mut key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    key.kind = KeyEventKind::Release;
    assert!(h.send(AppEvent::Key(key)).is_empty());
    assert!(!h.app.should_quit());
}

#[test]
fn status_text_matches_python_format() {
    let mut h = Harness::connected(three_ops());
    assert_eq!(
        h.app.status_text(),
        "Connected to localhost:27017 (standalone, MongoDB 8.0.4) | Auto-refresh enabled (2s)"
    );
    h.key(KeyCode::Char(' '));
    assert!(h.app.status_text().ends_with("| Selected: 1"));
}

#[test]
fn reconnect_ignores_results_from_the_old_connection() {
    let mut h = Harness::new();
    h.app.start();
    h.send(AppEvent::Connected(Err("down".into())));
    h.ctrl('r');
    let fx = h.send(AppEvent::Connected(Ok(info(Deployment::Standalone, false))));
    let generation = fetch_generation(&fx).unwrap();
    h.fetched(generation, three_ops());
    assert_eq!(h.app.operations.len(), 3);
}
