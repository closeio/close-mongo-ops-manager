//! Header, status bar and footer.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;

use super::style;
use super::text::{sanitize, to_u16, truncate, width};
use crate::app::{Action, App};
use crate::theme::Palette;

/// Gap between the status text and the refresh summary.
const STATUS_GAP: usize = 2;
/// Status text kept visible when making room for the refresh error marker.
const MIN_STATUS_TEXT: usize = 12;
const REFRESH_FAILED: &str = "refresh failed";

/// The application title, centered.
pub fn render_header(frame: &mut Frame<'_>, area: Rect, title: &str, p: &Palette) {
    let style = Style::new().bg(p.panel).fg(p.foreground);
    frame.render_widget(Block::new().style(style), area);
    frame.render_widget(Line::from(sanitize(title).into_owned()).centered(), area);
}

/// Connection and refresh status, with the last refresh summary on the
/// right when it fits.
pub fn render_status(frame: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    frame.render_widget(
        Block::new().style(Style::new().bg(p.status_bg).fg(p.status_fg)),
        area,
    );
    // One cell of padding on both sides.
    let inner = Rect {
        x: area.x.saturating_add(1),
        width: area.width.saturating_sub(2),
        ..area
    };
    if inner.is_empty() {
        return;
    }
    let status = sanitize(&app.status_text()).into_owned();
    let summary = refresh_summary(app, p, usize::from(inner.width), width(&status));

    let summary_width = to_u16(summary.width());
    let gap = if summary_width > 0 { STATUS_GAP } else { 0 };
    let left = Rect {
        width: inner
            .width
            .saturating_sub(summary_width.saturating_add(to_u16(gap))),
        ..inner
    };
    frame.render_widget(
        Line::from(truncate(&status, usize::from(left.width)).into_owned()),
        left,
    );
    if summary_width > 0 {
        let right = Rect {
            x: inner.right() - summary_width,
            width: summary_width,
            ..inner
        };
        frame.render_widget(summary, right);
    }
}

/// `"42 ops in 0.12s · refresh failed"` when it fits next to the status
/// text. The error marker matters most: it may take room from the status
/// text when the whole summary does not fit.
fn refresh_summary(app: &App, p: &Palette, available: usize, status_width: usize) -> Line<'static> {
    let failed = app.last_error.is_some();
    let marker = Span::styled(
        REFRESH_FAILED,
        Style::new().fg(p.error).add_modifier(Modifier::BOLD),
    );
    let mut spans = Vec::new();
    if let Some(stats) = app.last_refresh {
        let noun = if stats.count == 1 { "op" } else { "ops" };
        spans.push(Span::raw(format!(
            "{} {noun} in {:.2}s",
            stats.count,
            stats.took.as_secs_f64()
        )));
    }
    if failed {
        if !spans.is_empty() {
            spans.push(Span::raw(" · "));
        }
        spans.push(marker.clone());
    }
    let full = Line::from(spans);
    if status_width + STATUS_GAP + full.width() <= available {
        return full;
    }
    let kept_status = status_width.min(MIN_STATUS_TEXT);
    if failed && kept_status + STATUS_GAP + width(REFRESH_FAILED) <= available {
        return Line::from(marker);
    }
    Line::default()
}

/// Key bindings, as many as fit. Returns the area of each entry.
pub fn render_footer(
    frame: &mut Frame<'_>,
    area: Rect,
    actions: &[Action],
    p: &Palette,
) -> Vec<(Rect, Action)> {
    frame.render_widget(
        Block::new().style(Style::new().bg(p.panel).fg(p.foreground)),
        area,
    );
    let mut entries = Vec::new();
    if area.is_empty() {
        return entries;
    }
    let mut x = area.x;
    for &action in actions {
        let entry = Line::from(vec![
            Span::raw(" "),
            Span::styled(action.key_label(), style::key(p)),
            Span::raw(" "),
            Span::raw(action.description()),
            Span::raw(" "),
        ]);
        let entry_width = to_u16(entry.width());
        if area.right() - x < entry_width {
            break;
        }
        let rect = Rect::new(x, area.y, entry_width, 1);
        frame.render_widget(entry, rect);
        entries.push((rect, action));
        x += entry_width;
    }
    entries
}
