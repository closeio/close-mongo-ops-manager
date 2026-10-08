//! Cluster members polled with `--all-nodes` and their status.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Cell, Row, Table};

use super::{DISMISS_HINT, modal_block, open};
use crate::app::LayoutCache;
use crate::model::{NodeHealth, NodeStatus};
use crate::theme::Palette;
use crate::ui::layout::{ColumnSpec, centered, fit_columns, percent, right_edge};
use crate::ui::style;
use crate::ui::text::{sanitize_owned, to_u16, truncate, width};
use crate::ui::widgets::{render_message, render_scrollbar, with_hint};

const HEADERS: [&str; 7] = [
    "Address",
    "Shard",
    "Role",
    "Status",
    "Ops",
    "Latency (ms)",
    "Error",
];
/// `(max natural width, min width, shrink rank, hide rank)` per column.
const SIZING: [(usize, usize, Option<u8>, Option<u8>); 7] = [
    (40, 10, Some(1), None),
    (20, 5, Some(2), Some(3)),
    (10, 4, None, Some(4)),
    (19, 6, Some(3), None),
    (6, 3, None, Some(2)),
    (12, 7, None, Some(1)),
    (60, 5, Some(0), Some(0)),
];
/// Index of the status column.
const STATUS: usize = 3;
const EMPTY: &str = "Not polling individual members (start with --all-nodes)";

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    nodes: &[NodeStatus],
    scroll: &mut u16,
    p: &Palette,
    layout: &mut LayoutCache,
) {
    // Borders, the header and one row per member (or the empty message).
    let height = to_u16(nodes.len().max(1)).saturating_add(3).max(5);
    let rect = centered(
        area,
        percent(area.width, 80),
        height.min(percent(area.height, 80)),
    );
    let block = with_hint(modal_block("Cluster Members", p), DISMISS_HINT, rect.width);
    let inner = open(frame, rect, block, layout);
    if nodes.is_empty() {
        render_message(frame, inner, &[(EMPTY, style::muted(p))]);
        *scroll = 0;
        return;
    }

    let page_rows = inner.height.saturating_sub(1);
    layout.modal_page = page_rows;
    let page = usize::from(page_rows);
    let offset = usize::from(*scroll).min(nodes.len().saturating_sub(page));
    *scroll = to_u16(offset);

    let texts: Vec<[String; 7]> = nodes.iter().map(node_texts).collect();
    let widths = column_widths(&texts, inner.width);
    let header = Row::new(cells(&HEADERS, &widths, Style::new())).style(
        Style::new()
            .bg(p.panel)
            .fg(p.foreground)
            .add_modifier(Modifier::BOLD),
    );
    let rows = nodes
        .iter()
        .zip(&texts)
        .skip(offset)
        .take(page)
        .map(|(node, texts)| {
            let status = Style::new().fg(match node.health {
                NodeHealth::Ok { .. } => p.success,
                NodeHealth::Fallback { .. } => p.warning,
                NodeHealth::Failed { .. } => p.error,
            });
            Row::new(cells(texts, &widths, status))
        });
    let table = Table::new(
        rows,
        widths
            .iter()
            .flatten()
            .map(|&w| Constraint::Length(to_u16(w))),
    )
    .header(header)
    .column_spacing(1);
    frame.render_widget(table, inner);

    let body = Rect {
        y: inner.y.saturating_add(1).min(inner.bottom()),
        height: page_rows,
        ..inner
    };
    let track = right_edge(rect, body);
    render_scrollbar(
        frame,
        track,
        (nodes.len(), page, offset),
        Style::new().fg(p.primary),
        p,
    );
}

/// Cell texts of a member, in [`HEADERS`] order.
pub fn node_texts(node: &NodeStatus) -> [String; 7] {
    let (status, operations, latency, error) = match &node.health {
        NodeHealth::Ok {
            operations,
            latency,
        } => (
            "ok",
            operations.to_string(),
            latency.as_millis().to_string(),
            String::new(),
        ),
        NodeHealth::Fallback { error } => {
            ("fallback via mongos", "-".into(), "-".into(), error.clone())
        }
        NodeHealth::Failed { error } => ("failed", "-".into(), "-".into(), error.clone()),
    };
    [
        node.address.clone(),
        node.shard.clone().unwrap_or_else(|| "-".to_owned()),
        node.role.label().to_owned(),
        status.to_owned(),
        operations,
        latency,
        error,
    ]
    .map(sanitize_owned)
}

fn column_widths(texts: &[[String; 7]], available: u16) -> Vec<Option<usize>> {
    let specs: Vec<ColumnSpec> = SIZING
        .iter()
        .enumerate()
        .map(|(i, &(max, min, shrink_rank, hide_rank))| {
            let content = texts.iter().map(|t| width(&t[i])).max().unwrap_or(0);
            ColumnSpec {
                natural: content.max(width(HEADERS[i])).min(max),
                header: width(HEADERS[i]),
                min,
                shrink_rank,
                hide_rank,
            }
        })
        .collect();
    fit_columns(&specs, usize::from(available))
}

/// Cells of the visible columns, truncated to their width, the status
/// styled with `status`.
fn cells<S: AsRef<str>>(
    texts: &[S],
    widths: &[Option<usize>],
    status: Style,
) -> Vec<Cell<'static>> {
    texts
        .iter()
        .zip(widths)
        .enumerate()
        .filter_map(|(i, (text, width))| {
            let cell = Cell::from(truncate(text.as_ref(), (*width)?).into_owned());
            Some(if i == STATUS {
                cell.style(status)
            } else {
                cell
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::model::NodeRole;

    #[test]
    fn texts_by_health() {
        let mut node = NodeStatus {
            address: "db1:27018".into(),
            shard: Some("rs0".into()),
            role: NodeRole::Secondary,
            health: NodeHealth::Ok {
                operations: 4,
                latency: Duration::from_millis(12),
            },
        };
        assert_eq!(
            node_texts(&node),
            ["db1:27018", "rs0", "secondary", "ok", "4", "12", ""]
        );
        node.health = NodeHealth::Failed {
            error: "timed out".into(),
        };
        node.shard = None;
        assert_eq!(
            node_texts(&node),
            [
                "db1:27018",
                "-",
                "secondary",
                "failed",
                "-",
                "-",
                "timed out"
            ]
        );
        node.health = NodeHealth::Fallback {
            error: "auth\nfailed".into(),
        };
        assert_eq!(node_texts(&node)[3], "fallback via mongos");
        assert_eq!(node_texts(&node)[6], "auth failed");
    }
}
