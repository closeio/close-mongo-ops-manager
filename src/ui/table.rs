//! The operations table.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Cell, Row, Table};

use super::layout::{ColumnSpec, fit_columns, right_edge};
use super::style;
use super::text::{sanitize_owned, to_u16, truncate, width};
use super::widgets::{render_message, render_scrollbar, with_hint};
use crate::app::{App, ConnectionState, Focus};
use crate::model::Operation;
use crate::theme::Palette;

const SUBTITLE: &str = " View and manage MongoDB operations ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    Select,
    OpId,
    Shard,
    Node,
    Type,
    Operation,
    RunningTime,
    Client,
    Description,
    Users,
}

impl Column {
    /// Columns in display order.
    fn all(topology: bool) -> Vec<Self> {
        let mut columns = vec![Self::Select, Self::OpId];
        if topology {
            columns.extend([Self::Shard, Self::Node]);
        }
        columns.extend([
            Self::Type,
            Self::Operation,
            Self::RunningTime,
            Self::Client,
            Self::Description,
            Self::Users,
        ]);
        columns
    }

    fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::OpId => "OpId",
            Self::Shard => "Shard",
            Self::Node => "Node",
            Self::Type => "Type",
            Self::Operation => "Operation",
            Self::RunningTime => "Running Time",
            Self::Client => "Client",
            Self::Description => "Description",
            Self::Users => "Effective Users",
        }
    }

    /// Header text in `max` cells; the running time keeps its sort arrow.
    fn header(self, ascending: bool, max: usize) -> String {
        match self {
            Self::RunningTime => {
                let arrow = if ascending { "▲" } else { "▼" };
                let label = truncate(self.label(), max.saturating_sub(2));
                format!("{label} {arrow}")
            }
            _ => truncate(self.label(), max).into_owned(),
        }
    }

    /// `(max natural width, min width, shrink rank, hide rank)`: columns
    /// shrink and then disappear least important first.
    fn sizing(self) -> (usize, usize, Option<u8>, Option<u8>) {
        match self {
            Self::Select => (6, 6, None, Some(3)),
            Self::OpId => (24, 8, Some(7), None),
            Self::Shard => (16, 5, Some(4), Some(5)),
            Self::Node => (28, 8, Some(3), Some(4)),
            Self::Type => (12, 4, Some(6), Some(1)),
            Self::Operation => (16, 7, Some(5), Some(7)),
            Self::RunningTime => (14, 6, Some(8), None),
            Self::Client => (40, 8, Some(2), Some(6)),
            Self::Description => (40, 8, Some(1), Some(2)),
            Self::Users => (30, 6, Some(0), Some(0)),
        }
    }

    fn text(self, op: &Operation, selected: bool) -> String {
        match self {
            Self::Select => if selected { "✓" } else { "" }.to_owned(),
            Self::OpId => op.opid.to_string(),
            Self::Shard => op.shard.clone().unwrap_or_else(|| "-".to_owned()),
            Self::Node => node_label(op),
            Self::Type => op.op_type.clone(),
            Self::Operation => op.op.clone(),
            Self::RunningTime => format!("{}s", op.secs_running),
            Self::Client => op.client_display(),
            Self::Description if op.desc.is_empty() => "N/A".to_owned(),
            Self::Description => op.desc.clone(),
            Self::Users => op.users_display(),
        }
    }
}

/// Polled member and its role, e.g. `db1:27018 S`.
fn node_label(op: &Operation) -> String {
    match (op.host.as_deref(), op.node_role) {
        (Some(host), Some(role)) => format!("{host} {}", role.short()),
        (Some(host), None) => host.to_owned(),
        (None, Some(role)) => role.short().to_owned(),
        (None, None) => "-".to_owned(),
    }
}

/// Draws the operations table, or a message when there is nothing to list,
/// and records where the data rows are.
pub fn render(frame: &mut Frame<'_>, area: Rect, app: &mut App, p: &Palette) {
    let focused = app.focus == Focus::Table;
    let border = if focused {
        Style::new().fg(p.primary)
    } else {
        style::faded(p.primary, p.background)
    };
    let block = Block::bordered()
        .border_style(border)
        .style(style::base(p))
        .title(format!(" {} ", app.table_title()));
    let block = with_hint(block, SUBTITLE, area.width);
    let inner = block.inner(area);
    let body = Rect {
        y: inner.y.saturating_add(1).min(inner.bottom()),
        height: inner.height.saturating_sub(1),
        ..inner
    };
    app.layout.table_body = body;

    let columns = Column::all(app.topology_columns());
    let texts: Vec<Vec<String>> = app
        .operations
        .iter()
        .map(|op| {
            let selected = app.selected.contains(&op.key);
            columns
                .iter()
                .map(|c| sanitize_owned(c.text(op, selected)))
                .collect()
        })
        .collect();
    let widths = column_widths(&columns, &texts, inner.width, app.sort_ascending);
    let visible: Vec<(usize, Column, usize)> = columns
        .iter()
        .zip(&widths)
        .enumerate()
        .filter_map(|(i, (&c, w))| w.map(|w| (i, c, w)))
        .collect();

    let header = Row::new(
        visible
            .iter()
            .map(|&(_, c, w)| Cell::from(c.header(app.sort_ascending, w))),
    )
    .style(
        Style::new()
            .bg(p.panel)
            .fg(p.foreground)
            .add_modifier(Modifier::BOLD),
    );
    let rows = rows(app, texts, &visible, p);
    let table = Table::new(
        rows,
        visible
            .iter()
            .map(|&(_, _, w)| Constraint::Length(to_u16(w))),
    )
    .header(header)
    .block(block)
    .column_spacing(1)
    .row_highlight_style(if focused {
        style::cursor(p)
    } else {
        style::cursor_unfocused(p)
    });

    if app.operations.is_empty() {
        // Without rows, ratatui would reset the cursor: leave it alone.
        frame.render_widget(table, area);
        render_empty_state(frame, body, app, p);
    } else {
        frame.render_stateful_widget(table, area, &mut app.table);
        let view = (
            app.operations.len(),
            usize::from(body.height),
            app.table.offset(),
        );
        render_scrollbar(frame, right_edge(area, body), view, border, p);
    }
}

/// Column widths fitted to `available` cells; `None` for hidden columns.
fn column_widths(
    columns: &[Column],
    texts: &[Vec<String>],
    available: u16,
    ascending: bool,
) -> Vec<Option<usize>> {
    let specs: Vec<ColumnSpec> = columns
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let (max, min, shrink_rank, hide_rank) = c.sizing();
            let content = texts.iter().map(|row| width(&row[i])).max().unwrap_or(0);
            let header = width(&c.header(ascending, usize::MAX));
            ColumnSpec {
                natural: content.max(header).min(max),
                header,
                min,
                shrink_rank,
                hide_rank,
            }
        })
        .collect();
    fit_columns(&specs, usize::from(available))
}

/// Table rows: zebra stripes, accent text for selected operations, warning
/// color for the running time of operations being killed.
fn rows(
    app: &App,
    texts: Vec<Vec<String>>,
    visible: &[(usize, Column, usize)],
    p: &Palette,
) -> Vec<Row<'static>> {
    app.operations
        .iter()
        .zip(texts)
        .enumerate()
        .map(|(n, (op, texts))| {
            let selected = app.selected.contains(&op.key);
            let mut row_style = Style::new().fg(if selected { p.accent } else { p.foreground });
            if n % 2 == 1 {
                row_style = row_style.bg(p.zebra);
            }
            let cells = visible.iter().map(|&(i, c, w)| {
                let line = Line::from(truncate(&texts[i], w).into_owned());
                let cell = Cell::from(if c == Column::Select {
                    line.centered()
                } else {
                    line
                });
                if c == Column::RunningTime && op.kill_pending {
                    cell.style(Style::new().fg(p.warning))
                } else {
                    cell
                }
            });
            Row::new(cells.collect::<Vec<_>>()).style(row_style)
        })
        .collect()
}

/// Message shown instead of rows.
fn render_empty_state(frame: &mut Frame<'_>, body: Rect, app: &App, p: &Palette) {
    let muted = style::muted(p);
    match &app.connection {
        ConnectionState::Connecting => {
            render_message(frame, body, &[("Connecting to MongoDB...", muted)]);
        }
        ConnectionState::Failed(error) => {
            let error = format!("Failed to connect: {error}");
            render_message(
                frame,
                body,
                &[
                    (&error, Style::new().fg(p.error)),
                    ("Press Ctrl+R to retry", muted),
                ],
            );
        }
        ConnectionState::Connected(_) => {
            let message = if app.loading {
                "Loading operations..."
            } else if app.filters().is_empty() {
                "No operations"
            } else {
                "No operations match the filters"
            };
            render_message(frame, body, &[(message, muted)]);
        }
    }
}
