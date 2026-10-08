//! Application log viewer.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{DISMISS_HINT, modal_block, open};
use crate::app::LayoutCache;
use crate::theme::Palette;
use crate::ui::layout::{centered, percent, right_edge};
use crate::ui::style;
use crate::ui::text::{sanitize, wrap_chars, wrapped_rows};
use crate::ui::widgets::{render_message, render_scrollbar, with_hint};

/// Draws the log lines, wrapped to the width. Follows the end of the log
/// while `follow` is set; scrolling back to the end sets it again. `scroll`
/// counts wrapped rows.
pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    logs: &[String],
    (scroll, follow): (&mut usize, &mut bool),
    p: &Palette,
    layout: &mut LayoutCache,
) {
    let rect = centered(area, percent(area.width, 80), percent(area.height, 80));
    let block = with_hint(modal_block("Application Logs", p), DISMISS_HINT, rect.width);
    let inner = open(frame, rect, block, layout);
    if inner.is_empty() {
        return;
    }
    if logs.is_empty() {
        render_message(frame, inner, &[("No log messages yet", style::muted(p))]);
        *scroll = 0;
        *follow = true;
        return;
    }

    // Only the visible lines are wrapped and styled: the log can be long.
    let (width, page) = (usize::from(inner.width), usize::from(inner.height));
    let rows: Vec<usize> = logs.iter().map(|l| wrapped_rows(l, width)).collect();
    let total: usize = rows.iter().sum();
    let last_page = total.saturating_sub(page);
    let top = if *follow {
        last_page
    } else {
        (*scroll).min(last_page)
    };
    *scroll = top;
    *follow = top >= last_page;

    let (first, skip) = locate(&rows, top);
    let lines: Vec<Line<'static>> = logs[first..]
        .iter()
        .flat_map(|line| log_line(line, width, p))
        .skip(skip)
        .take(page)
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
    let track = right_edge(rect, inner);
    render_scrollbar(
        frame,
        track,
        (total, page, top),
        Style::new().fg(p.primary),
        p,
    );
}

/// One log line wrapped to `width`: errors and warnings colored.
pub fn log_line(line: &str, width: usize, p: &Palette) -> Vec<Line<'static>> {
    let style = if line.contains("(ERROR)") {
        Style::new().fg(p.error)
    } else if line.contains("(WARNING)") {
        Style::new().fg(p.warning)
    } else {
        Style::new()
    };
    wrap_chars(&[Span::styled(sanitize(line), style)], width)
}

/// The item holding row `row` of items `rows` rows tall, and the rows of
/// that item before it.
fn locate(rows: &[usize], mut row: usize) -> (usize, usize) {
    for (i, &n) in rows.iter().enumerate() {
        if row < n {
            return (i, row);
        }
        row -= n;
    }
    (rows.len(), 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;

    #[test]
    fn lines_are_wrapped_and_colored_by_level() {
        let p = theme::default_theme().palette(true);
        let info = log_line("2026-10-08 12:00:00,000 (INFO): connected", 20, &p);
        let text: Vec<String> = info.iter().map(ToString::to_string).collect();
        assert_eq!(text, ["2026-10-08 12:00:00,", "000 (INFO): connecte", "d"]);
        assert_eq!(info[0].spans[0].style.fg, None);
        let error = log_line("2026-10-08 12:00:01,000 (ERROR): boom", 20, &p);
        assert_eq!(error.len(), 2);
        assert!(error.iter().all(|l| l.spans[0].style.fg == Some(p.error)));
        let warning = log_line("(WARNING): careful", 20, &p);
        assert_eq!(warning[0].spans[0].style.fg, Some(p.warning));
    }

    #[test]
    fn locate_finds_the_line_of_a_row() {
        let rows = [3, 1, 2];
        assert_eq!(locate(&rows, 0), (0, 0));
        assert_eq!(locate(&rows, 2), (0, 2));
        assert_eq!(locate(&rows, 3), (1, 0));
        assert_eq!(locate(&rows, 5), (2, 1));
        assert_eq!(locate(&rows, 6), (3, 0));
    }
}
