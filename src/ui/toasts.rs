//! Notifications stacked at the bottom-right corner.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

use super::style::severity_color;
use super::text::{sanitize, to_u16, truncate, width, wrap_words};
use crate::app::{MAX_TOASTS, Severity, Toast};
use crate::theme::Palette;

const MAX_WIDTH: u16 = 60;
const MIN_WIDTH: u16 = 24;
/// Longer messages are cut.
const MAX_LINES: usize = 6;
/// Cells taken by the border and padding on both sides of the text.
const CHROME: u16 = 4;
/// Cells kept free on the right of the toasts.
const MARGIN: u16 = 2;

/// Draws the newest toasts at the bottom-right of `area`, older ones above,
/// as many as fit. Returns the drawn boxes with their index in `toasts`.
pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    toasts: &[Toast],
    p: &Palette,
) -> Vec<(Rect, usize)> {
    let mut drawn = Vec::new();
    let max_width = MAX_WIDTH.min(area.width.saturating_sub(2 * MARGIN));
    if max_width <= CHROME {
        return drawn;
    }
    let mut bottom = area.bottom();
    for (index, toast) in toasts.iter().enumerate().rev().take(MAX_TOASTS) {
        let lines = message_lines(&toast.message, usize::from(max_width - CHROME));
        let text_width = lines.iter().map(|l| width(l)).max().unwrap_or(0);
        let box_width = to_u16(text_width)
            .saturating_add(CHROME)
            .clamp(MIN_WIDTH.min(max_width), max_width);
        let height = to_u16(lines.len()).saturating_add(2);
        if bottom - area.y < height {
            break;
        }
        bottom -= height;
        let rect = Rect::new(area.right() - box_width - MARGIN, bottom, box_width, height);
        render_toast(frame, rect, toast.severity, lines, p);
        drawn.push((rect, index));
    }
    drawn
}

/// The message wrapped to `max` cells, at most [`MAX_LINES`] lines.
fn message_lines(message: &str, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = message
        .lines()
        .flat_map(|line| wrap_words(&sanitize(line), max))
        .collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    if lines.len() > MAX_LINES {
        lines.truncate(MAX_LINES);
        let last = &mut lines[MAX_LINES - 1];
        *last = truncate(&format!("{last} …"), max).into_owned();
    }
    lines
}

fn render_toast(
    frame: &mut Frame<'_>,
    area: Rect,
    severity: Severity,
    lines: Vec<String>,
    p: &Palette,
) {
    let color = severity_color(severity, p);
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .style(Style::new().bg(p.panel).fg(p.foreground))
        .padding(Padding::horizontal(1));
    let title = match severity {
        Severity::Info => None,
        Severity::Warning => Some(" Warning "),
        Severity::Error => Some(" Error "),
    };
    if let Some(title) = title {
        block = block.title(Line::styled(
            title,
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>()).block(block),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_messages_are_wrapped_and_cut() {
        assert_eq!(message_lines("short", 20), ["short"]);
        assert_eq!(message_lines("", 20), [""]);
        assert_eq!(message_lines("two\nlines", 20), ["two", "lines"]);
        let long = "word ".repeat(100);
        let lines = message_lines(&long, 20);
        assert_eq!(lines.len(), MAX_LINES);
        assert!(lines[MAX_LINES - 1].ends_with('…'), "{lines:?}");
        assert_eq!(
            message_lines(&"ab ".repeat(20), 8)[MAX_LINES - 1],
            "ab ab a…"
        );
        assert_eq!(
            message_lines(&"ab ".repeat(20), 10)[MAX_LINES - 1],
            "ab ab ab …"
        );
        assert!(lines.iter().all(|l| width(l) <= 20), "{lines:?}");
    }
}
