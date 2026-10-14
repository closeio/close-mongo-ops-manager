//! Rendering tests on an in-memory terminal.

use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier};
use ratatui::widgets::ListState;

use super::{draw, style};
use crate::app::{
    Action, App, AppOptions, ConnectionState, FilterField, Focus, Modal, RefreshStats, Severity,
    Toast,
};
use crate::logging::LogBuffer;
use crate::model::{
    Deployment, KillRequest, NodeHealth, NodeRole, NodeStatus, OpId, Operation, ServerInfo,
};
use crate::testutil::{operation_running, sample_operation};
use crate::theme;

const TITLE: &str = "Close MongoDB Operations Manager v0.6.0";

fn new_app() -> App {
    App::new(AppOptions {
        refresh_interval: 2,
        theme: theme::default_theme(),
        truecolor: true,
        logs: LogBuffer::new(),
        title: TITLE.into(),
    })
}

fn server(deployment: Deployment, all_nodes: bool) -> ServerInfo {
    ServerInfo {
        target: "localhost:27017".into(),
        version: "8.0.4".into(),
        deployment,
        load_balanced: false,
        all_nodes,
    }
}

/// Connected to a standalone server, listing `operations`, cursor on the
/// first row.
fn connected(operations: Vec<Operation>) -> App {
    let mut app = new_app();
    app.connection = ConnectionState::Connected(server(Deployment::Standalone, false));
    if !operations.is_empty() {
        app.table.select(Some(0));
    }
    app.operations = operations;
    app
}

/// Operations 1..=count, operation `i` running for `i` seconds.
fn ops(count: i64) -> Vec<Operation> {
    (1..=count).map(|i| operation_running(i, i)).collect()
}

fn render(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    terminal
}

fn row(terminal: &Terminal<TestBackend>, y: u16) -> String {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.width)
        .map(|x| buffer.cell((x, y)).map_or(" ", Cell::symbol))
        .collect()
}

fn rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
    (0..terminal.backend().buffer().area.height)
        .map(|y| row(terminal, y))
        .collect()
}

fn screen(terminal: &Terminal<TestBackend>) -> String {
    rows(terminal).join("\n")
}

/// Text of `rect` (one line per row).
fn text_in(terminal: &Terminal<TestBackend>, rect: Rect) -> String {
    let buffer = terminal.backend().buffer();
    rect.rows()
        .map(|r| {
            r.positions()
                .map(|p| buffer.cell(p).map_or(" ", Cell::symbol))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Position of the first occurrence of `needle`; every cell holds one char
/// in these tests.
fn find(terminal: &Terminal<TestBackend>, needle: &str) -> Option<Position> {
    rows(terminal).iter().zip(0..).find_map(|(line, y)| {
        line.find(needle).map(|byte| {
            let x = line[..byte].chars().count();
            Position::new(u16::try_from(x).unwrap(), y)
        })
    })
}

fn found(terminal: &Terminal<TestBackend>, needle: &str) -> Position {
    find(terminal, needle)
        .unwrap_or_else(|| panic!("{needle:?} not found in\n{}", screen(terminal)))
}

fn cell(terminal: &Terminal<TestBackend>, position: Position) -> Cell {
    terminal.backend().buffer().cell(position).unwrap().clone()
}

fn toast(message: &str, severity: Severity) -> Toast {
    Toast {
        message: message.into(),
        severity,
        expires_at: Instant::now() + Duration::from_secs(5),
    }
}

fn node(address: &str, health: NodeHealth) -> NodeStatus {
    NodeStatus {
        address: address.into(),
        shard: Some("rs0".into()),
        role: NodeRole::Secondary,
        health,
    }
}

// Main screen ---------------------------------------------------------------

#[test]
fn header_shows_the_title_on_the_panel_color() {
    let mut app = connected(ops(1));
    let terminal = render(&mut app, 100, 20);
    let at = found(&terminal, TITLE);
    assert_eq!(at.y, 0);
    assert_eq!(at.x, (100 - 39) / 2);
    assert_eq!(cell(&terminal, Position::new(0, 0)).bg, app.palette().panel);
}

#[test]
fn table_lists_operations_under_the_headers() {
    let mut app = connected(vec![operation_running(1, 5), operation_running(2, 10)]);
    let terminal = render(&mut app, 120, 20);
    assert!(row(&terminal, 1).contains(" Operations (2) "));
    let header = row(&terminal, 2);
    for label in [
        "Select",
        "OpId",
        "Type",
        "Operation",
        "Running Time ▲",
        "Client",
        "Description",
        "Effective Users",
    ] {
        assert!(header.contains(label), "{label} missing in {header:?}");
    }
    assert!(!header.contains("Shard") && !header.contains("Node"));
    let first = row(&terminal, 3);
    for text in ["1", "op", "query", "5s", "10.0.0.1:5000", "conn42", "alice"] {
        assert!(first.contains(text), "{text} missing in {first:?}");
    }
    assert!(row(&terminal, 4).contains("10s"));
    let p = app.palette();
    let header_cell = cell(&terminal, found(&terminal, "OpId"));
    assert_eq!(header_cell.bg, p.panel);
    assert!(header_cell.modifier.contains(Modifier::BOLD));
}

#[test]
fn table_header_shows_the_sort_direction() {
    let mut app = connected(ops(2));
    app.sort_ascending = false;
    let terminal = render(&mut app, 120, 20);
    assert!(row(&terminal, 2).contains("Running Time ▼"));
    assert!(!screen(&terminal).contains('▲'));
}

#[test]
fn empty_cells_show_placeholders() {
    let mut op = operation_running(1, 0);
    op.desc.clear();
    op.client.clear();
    op.effective_users.clear();
    let mut app = connected(vec![op]);
    let terminal = render(&mut app, 120, 20);
    let line = row(&terminal, 3);
    assert_eq!(line.matches("N/A").count(), 3, "{line:?}");
    assert!(line.contains("0s"));
}

#[test]
fn topology_columns_only_for_sharded_clusters_and_all_nodes() {
    for (deployment, all_nodes, expected) in [
        (Deployment::Standalone, false, false),
        (Deployment::ReplicaSet { name: "rs0".into() }, false, false),
        (Deployment::ReplicaSet { name: "rs0".into() }, true, true),
        (Deployment::Sharded, false, true),
    ] {
        let mut op = sample_operation(OpId::Str("shard01:42".into()));
        op.shard = Some("shard01".into());
        op.node_role = Some(NodeRole::Secondary);
        let mut app = connected(vec![op]);
        app.connection = ConnectionState::Connected(server(deployment, all_nodes));
        let terminal = render(&mut app, 160, 20);
        let header = row(&terminal, 2);
        assert_eq!(header.contains("Shard"), expected, "{header:?}");
        assert_eq!(header.contains("Node"), expected, "{header:?}");
        let line = row(&terminal, 3);
        assert!(line.contains("shard01:42"));
        assert_eq!(line.contains("db1:27017 S"), expected, "{line:?}");
    }
}

#[test]
fn node_cell_without_host_or_shard() {
    let mut op = operation_running(1, 1);
    op.host = None;
    let mut app = connected(vec![op]);
    app.connection = ConnectionState::Connected(server(Deployment::Sharded, false));
    let terminal = render(&mut app, 160, 20);
    let header = row(&terminal, 2);
    let line = row(&terminal, 3);
    let shard_x = header.find("Shard").unwrap();
    let node_x = header.find("Node").unwrap();
    assert_eq!(&line[shard_x..shard_x + 1], "-");
    assert_eq!(&line[node_x..node_x + 1], "-");
}

#[test]
fn selected_rows_have_a_check_mark_and_accent_text() {
    let mut app = connected(ops(3));
    app.selected.insert(app.operations[1].key.clone());
    let terminal = render(&mut app, 120, 20);
    let p = app.palette();
    assert!(!row(&terminal, 3).contains('✓'));
    assert!(row(&terminal, 4).contains('✓'));
    assert!(!row(&terminal, 5).contains('✓'));
    // The OpId column starts after the 6-cell Select column and a gap.
    assert_eq!(cell(&terminal, Position::new(8, 4)).symbol(), "2");
    assert_eq!(cell(&terminal, Position::new(8, 4)).fg, p.accent);
    assert_eq!(cell(&terminal, Position::new(8, 5)).fg, p.foreground);
}

#[test]
fn rows_have_zebra_stripes() {
    let mut app = connected(ops(4));
    app.table.select(None);
    let terminal = render(&mut app, 120, 20);
    let p = app.palette();
    assert_eq!(cell(&terminal, Position::new(10, 3)).bg, p.background);
    assert_eq!(cell(&terminal, Position::new(10, 4)).bg, p.zebra);
    assert_eq!(cell(&terminal, Position::new(10, 5)).bg, p.background);
    assert_eq!(cell(&terminal, Position::new(10, 6)).bg, p.zebra);
}

#[test]
fn cursor_row_is_highlighted_by_focus() {
    let mut app = connected(ops(3));
    app.table.select(Some(1));
    let terminal = render(&mut app, 120, 20);
    let p = app.palette();
    let cursor = cell(&terminal, Position::new(10, 4));
    assert_eq!(cursor.bg, p.cursor_bg);
    assert_eq!(cursor.fg, p.cursor_fg);
    assert!(cursor.modifier.contains(Modifier::BOLD));
    assert_eq!(cell(&terminal, Position::new(10, 3)).bg, p.background);

    app.focus = Focus::Filter(FilterField::OpId);
    app.filter_bar_visible = true;
    let terminal = render(&mut app, 120, 25);
    // The table moved down by the filter bar: rows start at y = 8.
    let unfocused = cell(&terminal, Position::new(10, 9));
    assert_eq!(Some(unfocused.bg), style::cursor_unfocused(&p).bg);
    assert_ne!(unfocused.bg, p.cursor_bg);
}

#[test]
fn table_border_dims_without_focus() {
    let mut app = connected(ops(1));
    let terminal = render(&mut app, 100, 20);
    let p = app.palette();
    assert_eq!(cell(&terminal, Position::new(0, 1)).fg, p.primary);
    app.focus = Focus::ClearButton;
    let terminal = render(&mut app, 100, 20);
    let dimmed = cell(&terminal, Position::new(0, 1));
    assert_ne!(dimmed.fg, p.primary);
    assert!(matches!(dimmed.fg, Color::Rgb(..)));
}

#[test]
fn killing_operations_show_their_running_time_as_a_warning() {
    let mut operations = ops(2);
    operations[1].kill_pending = true;
    let mut app = connected(operations);
    let terminal = render(&mut app, 120, 20);
    let p = app.palette();
    let line = row(&terminal, 4);
    let x = line.find("2s").unwrap();
    assert_eq!(cell(&terminal, Position::new(x as u16, 4)).fg, p.warning);
    let line = row(&terminal, 3);
    let x = line.find("1s").unwrap();
    assert_ne!(cell(&terminal, Position::new(x as u16, 3)).fg, p.warning);
}

#[test]
fn table_body_is_below_the_header_row() {
    let mut app = connected(ops(3));
    let terminal = render(&mut app, 100, 20);
    assert_eq!(app.layout.table_body, Rect::new(1, 3, 98, 14));
    assert!(row(&terminal, 2).contains("OpId"));
    assert_eq!(cell(&terminal, Position::new(8, 3)).symbol(), "1");

    app.filter_bar_visible = true;
    render(&mut app, 100, 20);
    assert_eq!(app.layout.table_body, Rect::new(1, 8, 98, 9));
}

#[test]
fn table_scrolls_to_the_cursor_with_a_scrollbar() {
    let mut app = connected(ops(30));
    app.table.select(Some(20));
    let terminal = render(&mut app, 100, 12);
    let body = app.layout.table_body;
    assert_eq!(body, Rect::new(1, 3, 98, 6));
    assert_eq!(app.table.offset(), 15);
    // Clicks map rows with `offset + (y - body.y)`.
    let cursor_y = body.y + 5;
    assert_eq!(app.table.offset() + usize::from(cursor_y - body.y), 20);
    assert_eq!(cell(&terminal, Position::new(8, cursor_y)).symbol(), "2");
    assert_eq!(cell(&terminal, Position::new(9, cursor_y)).symbol(), "1");
    let track = text_in(&terminal, Rect::new(99, body.y, 1, body.height));
    assert_eq!(track.matches('█').count(), 1, "{track:?}");
    assert_eq!(track.matches('│').count(), 5, "{track:?}");
}

#[test]
fn no_scrollbar_when_rows_fit() {
    let mut app = connected(ops(3));
    let terminal = render(&mut app, 100, 20);
    assert!(!screen(&terminal).contains('█'));
}

#[test]
fn out_of_range_cursor_is_clamped_by_the_table() {
    let mut app = connected(ops(3));
    app.table.select(Some(10));
    render(&mut app, 100, 20);
    assert_eq!(app.table.selected(), Some(2));
}

#[test]
fn narrow_terminals_hide_less_important_columns() {
    let mut app = connected(ops(3));
    let terminal = render(&mut app, 40, 12);
    let header = row(&terminal, 2);
    assert!(header.contains("OpId"), "{header:?}");
    assert!(header.contains('▲'), "{header:?}");
    assert!(!header.contains("Effective Users"), "{header:?}");
    for line in rows(&terminal) {
        assert_eq!(line.chars().count(), 40);
    }
}

// Empty states --------------------------------------------------------------

#[test]
fn connecting_message() {
    let mut app = new_app();
    let terminal = render(&mut app, 100, 20);
    let at = found(&terminal, "Connecting to MongoDB...");
    // Centered in the body (y 3..17).
    assert_eq!(at.y, 9);
    assert_eq!(at.x, 1 + (98 - 24) / 2);
    assert!(row(&terminal, 2).contains("OpId"));
}

#[test]
fn failed_connection_message() {
    let mut app = new_app();
    app.connection = ConnectionState::Failed("No servers available".into());
    let terminal = render(&mut app, 100, 20);
    let p = app.palette();
    let error = found(&terminal, "Failed to connect: No servers available");
    let retry = found(&terminal, "Press Ctrl+R to retry");
    assert_eq!(retry.y, error.y + 1);
    assert_eq!(cell(&terminal, error).fg, p.error);
    assert!(row(&terminal, 18).contains("Disconnected"));
}

#[test]
fn no_operations_messages() {
    let mut app = connected(Vec::new());
    let terminal = render(&mut app, 100, 20);
    assert!(find(&terminal, "No operations").is_some());
    assert!(find(&terminal, "match the filters").is_none());

    app.filter_inputs[FilterField::Client.index()].set_value("10.0");
    let terminal = render(&mut app, 100, 20);
    assert!(find(&terminal, "No operations match the filters").is_some());

    app.loading = true;
    let terminal = render(&mut app, 100, 20);
    assert!(find(&terminal, "Loading operations...").is_some());
    assert!(row(&terminal, 1).contains("Refreshing..."));
}

#[test]
fn empty_table_keeps_the_cursor() {
    let mut app = connected(Vec::new());
    app.table.select(Some(3));
    render(&mut app, 100, 20);
    assert_eq!(app.table.selected(), Some(3));
}

// Filter bar ----------------------------------------------------------------

#[test]
fn hidden_filter_bar_has_no_areas() {
    let mut app = connected(ops(1));
    app.layout.filter_inputs = [Rect::new(1, 1, 1, 1); 6];
    app.layout.clear_button = Rect::new(1, 1, 1, 1);
    let terminal = render(&mut app, 100, 20);
    assert_eq!(app.layout.filter_inputs, [Rect::default(); 6]);
    assert_eq!(app.layout.clear_button, Rect::default());
    assert!(find(&terminal, "Filters").is_none());
}

#[test]
fn filter_bar_shows_placeholders_and_records_areas() {
    let mut app = connected(ops(1));
    app.filter_bar_visible = true;
    let terminal = render(&mut app, 120, 24);
    assert_eq!(found(&terminal, " Filters ").y, 1);
    assert_eq!(found(&terminal, " Filter operations by criteria ").y, 5);
    let inputs = app.layout.filter_inputs;
    for (field, rect) in FilterField::ALL.iter().zip(inputs) {
        assert_eq!(rect.height, 1);
        assert_eq!(rect.y, 3);
        let text = text_in(&terminal, rect);
        assert!(
            text.starts_with(&field.placeholder()[..field.placeholder().len().min(4)]),
            "{field:?}: {text:?}"
        );
    }
    for pair in inputs.windows(2) {
        assert!(pair[0].right() < pair[1].x, "{pair:?}");
    }
    let button = app.layout.clear_button;
    assert_eq!(button.height, 3);
    assert!(inputs[5].right() < button.x);
    assert!(text_in(&terminal, button).contains("Clear"));
    let p = app.palette();
    assert_eq!(cell(&terminal, button.as_position()).bg, p.primary);
    assert_eq!(cell(&terminal, inputs[0].as_position()).fg, p.muted);
    assert!(!terminal.backend().cursor_visible());
}

#[test]
fn focused_filter_input_shows_the_cursor() {
    let mut app = connected(ops(1));
    app.filter_bar_visible = true;
    app.focus = Focus::Filter(FilterField::Client);
    app.filter_inputs[FilterField::Client.index()].set_value("10.0");
    let terminal = render(&mut app, 100, 24);
    let rect = app.layout.filter_inputs[FilterField::Client.index()];
    assert_eq!(rect, Rect::new(47, 3, 9, 1));
    assert!(text_in(&terminal, rect).starts_with("10.0"));
    assert!(terminal.backend().cursor_visible());
    assert_eq!(terminal.backend().cursor_position(), Position::new(51, 3));
    let p = app.palette();
    assert_eq!(cell(&terminal, Position::new(45, 2)).fg, p.accent);
    assert_eq!(cell(&terminal, Position::new(2, 2)).fg, p.muted);
}

#[test]
fn long_filter_values_scroll_to_the_cursor() {
    let mut app = connected(ops(1));
    app.filter_bar_visible = true;
    app.focus = Focus::Filter(FilterField::Client);
    app.filter_inputs[FilterField::Client.index()].set_value("0123456789abcdef");
    let terminal = render(&mut app, 100, 24);
    let rect = app.layout.filter_inputs[FilterField::Client.index()];
    assert_eq!(text_in(&terminal, rect), "89abcdef ");
    assert_eq!(terminal.backend().cursor_position(), Position::new(55, 3));

    app.filter_inputs[FilterField::Client.index()].move_home();
    let terminal = render(&mut app, 100, 24);
    assert_eq!(text_in(&terminal, rect), "012345678");
    assert_eq!(terminal.backend().cursor_position(), Position::new(47, 3));
}

#[test]
fn focused_clear_button_is_highlighted() {
    let mut app = connected(ops(1));
    app.filter_bar_visible = true;
    app.focus = Focus::ClearButton;
    let terminal = render(&mut app, 100, 24);
    let p = app.palette();
    let button = cell(&terminal, app.layout.clear_button.as_position());
    assert_eq!(button.bg, p.accent);
    assert!(!terminal.backend().cursor_visible());
}

#[test]
fn modals_take_the_focus_from_the_filter_bar() {
    let mut app = connected(ops(1));
    app.filter_bar_visible = true;
    app.focus = Focus::Filter(FilterField::OpId);
    app.modal = Some(Modal::Help { scroll: 0 });
    let terminal = render(&mut app, 100, 24);
    assert!(!terminal.backend().cursor_visible());
}

// Status bar and footer -------------------------------------------------------

#[test]
fn status_bar_shows_the_status_text_and_refresh_summary() {
    let mut app = connected(ops(1));
    let terminal = render(&mut app, 140, 20);
    let p = app.palette();
    let status = row(&terminal, 18);
    assert!(
        status.starts_with(&format!(" {}", app.status_text())),
        "{status:?}"
    );
    assert_eq!(cell(&terminal, Position::new(0, 18)).bg, p.status_bg);

    app.last_refresh = Some(RefreshStats {
        at: Instant::now(),
        took: Duration::from_millis(120),
        count: 42,
    });
    let terminal = render(&mut app, 140, 20);
    assert!(row(&terminal, 18).ends_with("42 ops in 0.12s "));

    app.last_error = Some("timeout".into());
    let terminal = render(&mut app, 140, 20);
    assert!(row(&terminal, 18).ends_with("42 ops in 0.12s · refresh failed "));
    let marker = found(&terminal, "refresh failed");
    assert_eq!(cell(&terminal, marker).fg, p.error);
}

#[test]
fn status_bar_keeps_the_error_marker_when_narrow() {
    let mut app = connected(ops(1));
    app.last_refresh = Some(RefreshStats {
        at: Instant::now(),
        took: Duration::from_millis(120),
        count: 42,
    });
    app.last_error = Some("timeout".into());
    let terminal = render(&mut app, 60, 20);
    let status = row(&terminal, 18);
    assert!(status.ends_with(" refresh failed "), "{status:?}");
    assert!(status.starts_with(" Connected to"), "{status:?}");
    assert!(!status.contains("42 ops"), "{status:?}");
    assert!(status.contains('…'), "{status:?}");
}

#[test]
fn footer_lists_actions_and_records_their_areas() {
    let mut app = connected(ops(1));
    let terminal = render(&mut app, 200, 20);
    let footer = &app.layout.footer;
    let actions: Vec<Action> = footer.iter().map(|&(_, a)| a).collect();
    assert_eq!(actions, app.footer_actions());
    let mut x = 0;
    for &(rect, action) in footer {
        assert_eq!(rect.y, 19);
        assert_eq!(rect.height, 1);
        assert_eq!(rect.x, x, "entries are adjacent");
        x = rect.right();
        let text = text_in(&terminal, rect);
        assert_eq!(
            text,
            format!(" {} {} ", action.key_label(), action.description())
        );
    }
    assert!(x <= 200);
    let p = app.palette();
    let key = found(&terminal, "^q");
    assert_eq!(key.y, 19);
    assert_eq!(cell(&terminal, key).fg, p.accent);
    assert!(cell(&terminal, key).modifier.contains(Modifier::BOLD));
    assert_eq!(cell(&terminal, Position::new(199, 19)).bg, p.panel);
}

#[test]
fn footer_stops_at_the_last_entry_that_fits() {
    let mut app = connected(ops(1));
    render(&mut app, 40, 20);
    let footer = &app.layout.footer;
    assert_eq!(
        footer.iter().map(|&(_, a)| a).collect::<Vec<_>>(),
        [Action::Help, Action::Quit, Action::Refresh]
    );
    assert!(footer.iter().all(|(r, _)| r.right() <= 40));
}

#[test]
fn footer_includes_cluster_actions() {
    let mut app = connected(ops(1));
    app.connection = ConnectionState::Connected(server(Deployment::Sharded, true));
    let terminal = render(&mut app, 250, 20);
    assert!(row(&terminal, 19).contains("^o Mongos Ops"));
    assert!(row(&terminal, 19).contains("^n Nodes"));
}

// Toasts --------------------------------------------------------------------

#[test]
fn toasts_stack_newest_at_the_bottom() {
    let mut app = connected(ops(1));
    app.toasts
        .push(toast("Theme changed to Nord", Severity::Info));
    app.toasts.push(toast("Kill failed: boom", Severity::Error));
    let terminal = render(&mut app, 100, 30);
    let p = app.palette();
    let info = found(&terminal, "Theme changed to Nord");
    let error = found(&terminal, "Kill failed: boom");
    assert!(info.y < error.y);
    // Above the table's bottom border and the status bar.
    assert_eq!(error.y, 25);
    // Error toast: 24 cells wide, 2 cells from the right edge.
    let corner = Position::new(100 - 24 - 2, 24);
    assert_eq!(cell(&terminal, corner).symbol(), "╭");
    assert_eq!(cell(&terminal, corner).fg, p.error);
    assert!(row(&terminal, 24).contains(" Error "));
    let info_corner = Position::new(100 - 25 - 2, 21);
    assert_eq!(cell(&terminal, info_corner).fg, p.primary);
}

#[test]
fn at_most_five_toasts_are_shown() {
    let mut app = connected(ops(1));
    for i in 0..7 {
        app.toasts
            .push(toast(&format!("toast number {i}"), Severity::Warning));
    }
    let terminal = render(&mut app, 100, 40);
    for i in 0..2 {
        assert!(find(&terminal, &format!("toast number {i}")).is_none());
    }
    for i in 2..7 {
        assert!(find(&terminal, &format!("toast number {i}")).is_some());
    }
    assert_eq!(screen(&terminal).matches(" Warning ").count(), 5);
}

#[test]
fn toasts_that_do_not_fit_are_skipped() {
    let mut app = connected(ops(1));
    for i in 0..5 {
        app.toasts
            .push(toast(&format!("toast number {i}"), Severity::Info));
    }
    // 7 rows between the header and the table's bottom border: two toasts.
    let terminal = render(&mut app, 100, 11);
    assert!(find(&terminal, "toast number 4").is_some());
    assert!(find(&terminal, "toast number 3").is_some());
    assert!(find(&terminal, "toast number 2").is_none());
}

#[test]
fn toasts_are_drawn_over_modals() {
    let mut app = connected(ops(1));
    app.modal = Some(Modal::Logs {
        scroll: 0,
        follow: true,
    });
    app.toasts.push(toast("Killed 1 operation", Severity::Info));
    let terminal = render(&mut app, 100, 30);
    assert!(find(&terminal, "Killed 1 operation").is_some());
}

// Modals ----------------------------------------------------------------------

#[test]
fn no_modal_clears_the_modal_areas() {
    let mut app = connected(ops(1));
    app.modal = Some(Modal::KillConfirm {
        requests: vec![KillRequest::from_operation(&app.operations[0])],
        yes_focused: false,
    });
    render(&mut app, 100, 30);
    assert_ne!(app.layout.modal_area, Rect::default());
    assert_ne!(app.layout.kill_yes, Rect::default());
    app.modal = None;
    render(&mut app, 100, 30);
    assert_eq!(app.layout.modal_area, Rect::default());
    assert_eq!(app.layout.modal_page, 0);
    assert_eq!(app.layout.kill_yes, Rect::default());
    assert_eq!(app.layout.kill_no, Rect::default());
    assert_eq!(app.layout.theme_list, Rect::default());
}

#[test]
fn help_modal() {
    let mut app = connected(ops(1));
    app.modal = Some(Modal::Help { scroll: 0 });
    let terminal = render(&mut app, 100, 40);
    let area = app.layout.modal_area;
    assert_eq!((area.x, area.width), (12, 76));
    assert_eq!(app.layout.modal_page, area.height - 2);
    let text = text_in(&terminal, area);
    for needle in [
        " Help ",
        "Keyboard Shortcuts",
        "Show this help",
        "Ctrl+Q, Ctrl+C",
        "Usage",
        "- Use arrow keys or mouse to navigate",
        " ESC to dismiss ",
    ] {
        assert!(text.contains(needle), "{needle} missing in\n{text}");
    }
    let p = app.palette();
    assert_eq!(cell(&terminal, found(&terminal, "Ctrl+Q")).fg, p.accent);
    assert_eq!(cell(&terminal, area.as_position()).bg, p.surface);
    assert_eq!(cell(&terminal, area.as_position()).symbol(), "╭");
}

#[test]
fn help_scroll_is_clamped() {
    let mut app = connected(ops(1));
    app.modal = Some(Modal::Help { scroll: 500 });
    let terminal = render(&mut app, 100, 16);
    let p = app.palette();
    let total = super::modals::help::help_lines(72, &p).len();
    let page = usize::from(app.layout.modal_page);
    assert_eq!(page, 11);
    let Some(Modal::Help { scroll }) = app.modal else {
        panic!("help closed");
    };
    assert_eq!(usize::from(scroll), total - page);
    assert!(find(&terminal, "Confirm kill operations when prompted").is_some());
    assert!(find(&terminal, "Keyboard Shortcuts").is_none());
}

#[test]
fn logs_modal_follows_the_end() {
    let mut app = connected(ops(1));
    for i in 0..100 {
        app.logs
            .push(&format!("2026-10-08 12:00:00,000 (INFO): entry {i:03}"));
    }
    app.logs
        .push("2026-10-08 12:00:01,000 (ERROR): entry failed");
    app.modal = Some(Modal::Logs {
        scroll: 0,
        follow: true,
    });
    let terminal = render(&mut app, 100, 20);
    let p = app.palette();
    let area = app.layout.modal_area;
    assert_eq!(area, Rect::new(10, 2, 80, 16));
    let page = app.layout.modal_page;
    assert_eq!(page, 14);
    assert!(text_in(&terminal, area).contains(" Application Logs "));
    assert!(find(&terminal, "entry 099").is_some());
    assert!(find(&terminal, "entry 000").is_none());
    let error = found(&terminal, "entry failed");
    assert_eq!(cell(&terminal, error).fg, p.error);
    assert!(matches!(
        app.modal,
        Some(Modal::Logs {
            scroll: 87,
            follow: true
        })
    ));
}

#[test]
fn logs_modal_scrolls_back_and_refollows_at_the_end() {
    let mut app = connected(ops(1));
    for i in 0..100 {
        app.logs.push(&format!("entry {i:03}"));
    }
    app.modal = Some(Modal::Logs {
        scroll: 0,
        follow: false,
    });
    let terminal = render(&mut app, 100, 20);
    assert!(find(&terminal, "entry 000").is_some());
    assert!(matches!(
        app.modal,
        Some(Modal::Logs {
            scroll: 0,
            follow: false
        })
    ));

    app.modal = Some(Modal::Logs {
        scroll: 10_000,
        follow: false,
    });
    let terminal = render(&mut app, 100, 20);
    assert!(find(&terminal, "entry 099").is_some());
    assert!(matches!(
        app.modal,
        Some(Modal::Logs {
            scroll: 86,
            follow: true
        })
    ));
}

#[test]
fn logs_modal_wraps_long_lines_and_colors_warnings() {
    let mut app = connected(ops(1));
    app.logs
        .push(&format!("2026-10-08 (WARNING): {}", "x".repeat(150)));
    app.modal = Some(Modal::Logs {
        scroll: 0,
        follow: true,
    });
    let terminal = render(&mut app, 100, 20);
    let p = app.palette();
    let start = found(&terminal, "2026-10-08 (WARNING)");
    assert_eq!(cell(&terminal, start).fg, p.warning);
    // 76 cells per line: the line continues below.
    let next = row(&terminal, start.y + 1);
    assert!(next.contains("xxxxxxxx"), "{next:?}");
    assert_eq!(
        cell(&terminal, Position::new(start.x, start.y + 1)).fg,
        p.warning
    );
}

#[test]
fn empty_logs_modal() {
    let mut app = connected(ops(1));
    app.modal = Some(Modal::Logs {
        scroll: 3,
        follow: false,
    });
    let terminal = render(&mut app, 100, 20);
    assert!(find(&terminal, "No log messages yet").is_some());
    assert!(matches!(
        app.modal,
        Some(Modal::Logs {
            scroll: 0,
            follow: true
        })
    ));
}

#[test]
fn details_modal() {
    let mut app = connected(ops(1));
    app.modal = Some(Modal::Details {
        op: Box::new(app.operations[0].clone()),
        scroll: 1000,
    });
    let terminal = render(&mut app, 120, 60);
    let text = text_in(&terminal, app.layout.modal_area);
    for needle in [
        " Operation Details ",
        "Operation ID: 1",
        "Type: query",
        "Namespace: app.users",
        "Running Time: 1s",
        "Client: 10.0.0.1:5000",
        "Command Details:",
        "\"find\": \"users\",",
        "Full $currentOp document:",
        "\"command\": {",
        " ESC to dismiss ",
    ] {
        assert!(text.contains(needle), "{needle} missing in\n{text}");
    }
    // Everything fits: the scroll is clamped to the top.
    assert!(matches!(app.modal, Some(Modal::Details { scroll: 0, .. })));
    let p = app.palette();
    assert_eq!(
        cell(&terminal, found(&terminal, "Operation ID:")).fg,
        p.accent
    );
    assert_eq!(
        cell(&terminal, found(&terminal, "Command Details:")).fg,
        p.primary
    );
}

#[test]
fn details_scroll_is_clamped_to_the_last_page() {
    let mut app = connected(ops(1));
    let op = app.operations[0].clone();
    let total = super::details_lines(&op).len();
    app.modal = Some(Modal::Details {
        op: Box::new(op),
        scroll: 1000,
    });
    let terminal = render(&mut app, 60, 16);
    let page = usize::from(app.layout.modal_page);
    assert_eq!(page, 10);
    let Some(Modal::Details { scroll, .. }) = app.modal else {
        panic!("details closed");
    };
    assert_eq!(usize::from(scroll), total - page);
    assert!(find(&terminal, "Operation ID").is_none());
    // The thumb sits at the bottom of the modal's right border.
    let area = app.layout.modal_area;
    let thumb = Position::new(area.right() - 1, area.bottom() - 2);
    assert_eq!(cell(&terminal, thumb).symbol(), "█");
}

#[test]
fn kill_confirmation_modal() {
    let mut app = connected(ops(3));
    app.modal = Some(Modal::KillConfirm {
        requests: vec![KillRequest::from_operation(&app.operations[0])],
        yes_focused: false,
    });
    let terminal = render(&mut app, 100, 30);
    let p = app.palette();
    assert_eq!(app.layout.modal_area, Rect::new(25, 11, 50, 7));
    assert!(find(&terminal, "Are you sure you want to kill 1 operation?").is_some());
    assert_eq!(app.layout.kill_yes, Rect::new(39, 15, 9, 1));
    assert_eq!(app.layout.kill_no, Rect::new(52, 15, 9, 1));
    assert_eq!(text_in(&terminal, app.layout.kill_yes).trim(), "Yes");
    assert_eq!(text_in(&terminal, app.layout.kill_no).trim(), "▸ No ◂");
    assert_eq!(
        cell(&terminal, app.layout.kill_yes.as_position()).bg,
        p.error
    );
    assert_eq!(
        cell(&terminal, app.layout.kill_no.as_position()).bg,
        p.primary
    );
    let corner = cell(&terminal, Position::new(25, 11));
    assert_eq!(corner.symbol(), "┏");
    assert_eq!(corner.fg, p.error);

    app.modal = Some(Modal::KillConfirm {
        requests: app
            .operations
            .iter()
            .map(KillRequest::from_operation)
            .collect(),
        yes_focused: true,
    });
    let terminal = render(&mut app, 100, 30);
    assert!(find(&terminal, "Are you sure you want to kill 3 operations?").is_some());
    assert_eq!(text_in(&terminal, app.layout.kill_yes).trim(), "▸ Yes ◂");
    assert_eq!(text_in(&terminal, app.layout.kill_no).trim(), "No");
    let yes = found(&terminal, "Yes");
    assert!(cell(&terminal, yes).modifier.contains(Modifier::UNDERLINED));
}

#[test]
fn theme_modal_previews_the_highlighted_theme() {
    let mut app = connected(ops(1));
    let nord = theme::by_name("nord").unwrap();
    let nord_index = theme::all().iter().position(|t| t.name == "nord").unwrap();
    app.modal = Some(Modal::Theme {
        list: ListState::default().with_selected(Some(nord_index)),
        original: app.theme,
    });
    let terminal = render(&mut app, 100, 30);
    let p = nord.palette(true);
    assert_eq!(app.layout.modal_area, Rect::new(20, 5, 60, 20));
    assert_eq!(app.layout.theme_list, Rect::new(22, 7, 56, 16));
    assert_eq!(app.layout.theme_list_offset, 0);
    let text = text_in(&terminal, app.layout.modal_area);
    assert!(text.contains(" Select Theme "));
    assert!(text.contains("Textual Dark ✓"), "{text}");
    for t in theme::all() {
        assert!(text.contains(&theme::display_name(t.name)));
    }
    let highlighted = Position::new(22, 7 + u16::try_from(nord_index).unwrap());
    assert!(row(&terminal, highlighted.y).contains("Nord"));
    assert_eq!(cell(&terminal, highlighted).bg, p.cursor_bg);
    // The whole screen uses the previewed theme.
    assert_eq!(cell(&terminal, Position::new(0, 0)).bg, p.panel);
}

#[test]
fn theme_list_scrolls_to_the_highlighted_theme() {
    let mut app = connected(ops(1));
    let last = theme::all().len() - 1;
    app.modal = Some(Modal::Theme {
        list: ListState::default().with_selected(Some(last)),
        original: app.theme,
    });
    let terminal = render(&mut app, 100, 12);
    let list = app.layout.theme_list;
    assert_eq!(list.height, 8);
    assert_eq!(app.layout.theme_list_offset, last + 1 - 8);
    assert_eq!(app.layout.modal_page, 8);
    let name = theme::display_name(theme::all()[last].name);
    assert!(row(&terminal, list.bottom() - 1).contains(&name));
}

#[test]
fn nodes_modal() {
    let mut app = connected(ops(1));
    app.connection = ConnectionState::Connected(server(Deployment::Sharded, true));
    app.nodes = vec![
        node(
            "db1:27018",
            NodeHealth::Ok {
                operations: 4,
                latency: Duration::from_millis(12),
            },
        ),
        node(
            "db2:27018",
            NodeHealth::Failed {
                error: "timed out".into(),
            },
        ),
        node(
            "db3:27018",
            NodeHealth::Fallback {
                error: "auth failed".into(),
            },
        ),
    ];
    app.modal = Some(Modal::Nodes { scroll: 0 });
    let terminal = render(&mut app, 120, 30);
    let p = app.palette();
    let text = text_in(&terminal, app.layout.modal_area);
    for needle in [
        " Cluster Members ",
        "Address",
        "Latency (ms)",
        "db1:27018",
        "secondary",
        "timed out",
        "auth failed",
    ] {
        assert!(text.contains(needle), "{needle} missing in\n{text}");
    }
    let ok = found(&terminal, "ok ");
    assert_eq!(cell(&terminal, ok).fg, p.success);
    assert_eq!(cell(&terminal, found(&terminal, "failed ")).fg, p.error);
    assert_eq!(
        cell(&terminal, found(&terminal, "fallback via mongos")).fg,
        p.warning
    );
    assert!(row(&terminal, ok.y).contains(" 12 "));
}

#[test]
fn nodes_modal_without_nodes() {
    let mut app = connected(ops(1));
    app.modal = Some(Modal::Nodes { scroll: 4 });
    let terminal = render(&mut app, 120, 30);
    assert!(
        find(
            &terminal,
            "Not polling individual members (start with --all-nodes)"
        )
        .is_some()
    );
    assert!(matches!(app.modal, Some(Modal::Nodes { scroll: 0 })));
}

#[test]
fn nodes_scroll_is_clamped() {
    let mut app = connected(ops(1));
    app.nodes = (0..50)
        .map(|i| {
            node(
                &format!("db{i:02}:27018"),
                NodeHealth::Ok {
                    operations: i,
                    latency: Duration::from_millis(1),
                },
            )
        })
        .collect();
    app.modal = Some(Modal::Nodes { scroll: 1000 });
    let terminal = render(&mut app, 120, 20);
    assert_eq!(app.layout.modal_page, 13);
    assert!(matches!(app.modal, Some(Modal::Nodes { scroll: 37 })));
    assert!(find(&terminal, "db49:27018").is_some());
    assert!(find(&terminal, "db36:27018").is_none());
    assert!(find(&terminal, "db37:27018").is_some());
}

// Robustness ----------------------------------------------------------------

fn every_modal(app: &App) -> Vec<Option<Modal>> {
    vec![
        None,
        Some(Modal::Help { scroll: 3 }),
        Some(Modal::Logs {
            scroll: 5,
            follow: false,
        }),
        Some(Modal::Logs {
            scroll: 0,
            follow: true,
        }),
        Some(Modal::Details {
            op: Box::new(app.operations[0].clone()),
            scroll: 7,
        }),
        Some(Modal::KillConfirm {
            requests: vec![KillRequest::from_operation(&app.operations[0])],
            yes_focused: true,
        }),
        Some(Modal::Theme {
            list: ListState::default().with_selected(Some(5)),
            original: app.theme,
        }),
        Some(Modal::Nodes { scroll: 2 }),
    ]
}

#[test]
fn renders_at_any_size_without_panicking() {
    let mut app = connected(ops(60));
    app.connection = ConnectionState::Connected(server(Deployment::Sharded, true));
    app.selected.insert(app.operations[3].key.clone());
    app.operations[2].kill_pending = true;
    app.operations[4].desc = "tab\there 日本語 \u{1b}[31m".into();
    app.table.select(Some(40));
    app.toasts.push(toast("Info toast", Severity::Info));
    app.toasts
        .push(toast(&"long error ".repeat(30), Severity::Error));
    app.nodes = vec![node(
        "db1:27018",
        NodeHealth::Failed {
            error: "x".repeat(200),
        },
    )];
    app.last_error = Some("boom".into());
    for i in 0..50 {
        app.logs
            .push(&format!("2026-10-08 (ERROR): line {i} {}", "y".repeat(i)));
    }
    app.filter_inputs[0].set_value("12345678901234567890");
    let widths = [0, 1, 2, 3, 4, 5, 8, 10, 13, 20, 33, 40, 61, 80, 120, 300];
    let heights = [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 16, 24, 40, 80];
    let mut cases: Vec<(Option<Modal>, bool, Focus)> = every_modal(&app)
        .into_iter()
        .map(|modal| (modal, true, Focus::Filter(FilterField::OpId)))
        .collect();
    cases.push((None, false, Focus::Table));
    cases.push((None, true, Focus::ClearButton));
    for (modal, filter_bar, focus) in cases {
        for &w in &widths {
            for &h in &heights {
                app.filter_bar_visible = filter_bar;
                app.focus = focus;
                app.modal = modal.clone();
                let terminal = render(&mut app, w, h);
                assert_layout_inside(&app, &terminal, Rect::new(0, 0, w, h));
            }
        }
    }
}

/// Recorded areas and the cursor are inside the screen.
fn assert_layout_inside(app: &App, terminal: &Terminal<TestBackend>, area: Rect) {
    let layout = &app.layout;
    let rects = layout
        .filter_inputs
        .iter()
        .chain([
            &layout.clear_button,
            &layout.kill_yes,
            &layout.kill_no,
            &layout.theme_list,
            &layout.modal_area,
            &layout.table_body,
        ])
        .chain(layout.footer.iter().map(|(r, _)| r));
    for rect in rects {
        if !rect.is_empty() {
            assert_eq!(area.intersection(*rect), *rect, "{area:?}: {rect:?}");
        }
    }
    if terminal.backend().cursor_visible() {
        assert!(area.contains(terminal.backend().cursor_position()));
    }
}

#[test]
fn renders_with_256_colors() {
    let mut app = connected(ops(3));
    app.truecolor = false;
    app.focus = Focus::Filter(FilterField::OpId);
    let terminal = render(&mut app, 100, 20);
    let border = cell(&terminal, Position::new(0, 1));
    assert!(matches!(border.fg, Color::Indexed(_)));
    assert!(border.modifier.contains(Modifier::DIM));
    for t in theme::all() {
        app.theme = t;
        render(&mut app, 100, 20);
    }
}
