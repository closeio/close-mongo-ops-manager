//! Confirmation before killing the selected operations.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph};

use super::open;
use crate::app::LayoutCache;
use crate::theme::Palette;
use crate::ui::layout::centered;
use crate::ui::text::{to_u16, wrap_words};

const WIDTH: u16 = 50;
const BUTTON_WIDTH: u16 = 9;
const BUTTON_GAP: u16 = 4;

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    count: usize,
    yes_focused: bool,
    p: &Palette,
    layout: &mut LayoutCache,
) {
    let width = WIDTH.min(area.width);
    // Border and padding take two cells on each side.
    let text = wrap_words(&question(count), usize::from(width.saturating_sub(4)));
    // Border, padding, the question, a blank line and the buttons.
    let height = to_u16(text.len()).saturating_add(6).min(area.height);
    let rect = centered(area, width, height);
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(Style::new().fg(p.error))
        .style(Style::new().bg(p.surface).fg(p.foreground))
        .title(Line::from(" Kill Operations ").style(Style::new().add_modifier(Modifier::BOLD)))
        .padding(Padding::uniform(1));
    let inner = open(frame, rect, block, layout);
    layout.modal_page = 0;

    let question_height = to_u16(text.len()).min(inner.height);
    let lines: Vec<Line<'static>> = text.into_iter().map(|l| Line::from(l).centered()).collect();
    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            height: question_height,
            ..inner
        },
    );

    let buttons = Rect {
        y: inner.y.saturating_add(question_height).saturating_add(1),
        height: 1,
        ..inner
    }
    .intersection(inner);
    let (yes, no) = button_areas(buttons);
    render_button(frame, yes, "Yes", yes_focused, (p.error, p.on_error));
    render_button(frame, no, "No", !yes_focused, (p.primary, p.on_primary));
    layout.kill_yes = yes;
    layout.kill_no = no;
}

/// `"Are you sure you want to kill 3 operations?"`
pub fn question(count: usize) -> String {
    let noun = if count == 1 {
        "operation"
    } else {
        "operations"
    };
    format!("Are you sure you want to kill {count} {noun}?")
}

/// The Yes and No buttons, centered in `row`.
fn button_areas(row: Rect) -> (Rect, Rect) {
    let total = 2 * BUTTON_WIDTH + BUTTON_GAP;
    let x = row.x + row.width.saturating_sub(total) / 2;
    let yes = Rect::new(x, row.y, BUTTON_WIDTH, row.height).intersection(row);
    let no_x = x.saturating_add(BUTTON_WIDTH + BUTTON_GAP);
    let no = Rect::new(no_x, row.y, BUTTON_WIDTH, row.height).intersection(row);
    (yes, no)
}

/// A button; the focused one is bold, underlined and marked `▸ … ◂`.
fn render_button(
    frame: &mut Frame<'_>,
    area: Rect,
    label: &'static str,
    focused: bool,
    (bg, fg): (Color, Color),
) {
    if area.is_empty() {
        return;
    }
    let style = Style::new().bg(bg).fg(fg);
    let line = if focused {
        Line::from(vec![
            Span::raw("▸ "),
            Span::styled(label, Style::new().add_modifier(Modifier::UNDERLINED)),
            Span::raw(" ◂"),
        ])
        .style(style.add_modifier(Modifier::BOLD))
    } else {
        Line::from(label).style(style)
    };
    frame.render_widget(Block::new().style(style), area);
    frame.render_widget(line.centered(), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn question_is_singular_for_one_operation() {
        assert_eq!(question(1), "Are you sure you want to kill 1 operation?");
        assert_eq!(question(3), "Are you sure you want to kill 3 operations?");
    }

    #[test]
    fn buttons_are_centered_and_clipped() {
        let (yes, no) = button_areas(Rect::new(10, 5, 42, 1));
        assert_eq!(yes, Rect::new(20, 5, 9, 1));
        assert_eq!(no, Rect::new(33, 5, 9, 1));
        let (yes, no) = button_areas(Rect::new(0, 0, 12, 1));
        assert_eq!(yes, Rect::new(0, 0, 9, 1));
        assert_eq!(no.width, 0);
    }
}
