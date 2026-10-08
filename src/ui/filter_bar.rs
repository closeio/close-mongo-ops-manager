//! Filter bar: one input per filter field and a Clear button.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Padding};

use super::style;
use super::text::{to_u16, truncate, width};
use super::widgets::with_hint;
use crate::app::{App, FilterField, Focus, TextInput};
use crate::theme::Palette;

const TITLE: &str = " Filters ";
const SUBTITLE: &str = " Filter operations by criteria ";
const BUTTON_LABEL: &str = "Clear";
const BUTTON_WIDTH: u16 = 11;
/// Cells between two inputs, and between the last input and the button.
const GAP: u16 = 1;
/// Inputs at least this wide get a cell of padding around the text.
const MIN_PADDED_INPUT: u16 = 8;

/// Draws the filter bar in `area` (zero-sized when hidden) and records the
/// input and button areas.
pub fn render(frame: &mut Frame<'_>, area: Rect, app: &mut App, p: &Palette) {
    app.layout.filter_inputs = [Rect::default(); 6];
    app.layout.clear_button = Rect::default();
    if area.is_empty() {
        return;
    }
    let block = Block::bordered()
        .border_style(Style::new().fg(p.primary))
        .style(Style::new().bg(p.surface).fg(p.foreground))
        .title(TITLE);
    let block = with_hint(block, SUBTITLE, area.width);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let row = Rect {
        x: inner.x.saturating_add(1),
        width: inner.width.saturating_sub(2),
        height: inner.height.min(3),
        ..inner
    };
    let (inputs, button) = split_row(row);
    // Widgets only show focus while no dialog is open.
    let focus = app.modal.is_none().then_some(app.focus);
    for (i, field) in FilterField::ALL.into_iter().enumerate() {
        let focused = focus == Some(Focus::Filter(field));
        app.layout.filter_inputs[i] =
            render_input(frame, inputs[i], &app.filter_inputs[i], field, focused, p);
    }
    render_button(frame, button, focus == Some(Focus::ClearButton), p);
    app.layout.clear_button = button;
}

/// Splits the row into six equal inputs and the button, `GAP` cells apart.
fn split_row(row: Rect) -> ([Rect; 6], Rect) {
    let button_width = BUTTON_WIDTH.min(row.width);
    let inputs_width = row.width.saturating_sub(button_width + 6 * GAP);
    let (base, extra) = (inputs_width / 6, inputs_width % 6);
    let mut inputs = [Rect::default(); 6];
    let mut x = row.x;
    for (i, input) in (0..).zip(inputs.iter_mut()) {
        let w = base + u16::from(i < extra);
        *input = Rect::new(x, row.y, w, row.height).intersection(row);
        x = x.saturating_add(w + GAP);
    }
    let button = Rect::new(x, row.y, button_width, row.height).intersection(row);
    (inputs, button)
}

/// Draws one input box. Returns its text area.
fn render_input(
    frame: &mut Frame<'_>,
    area: Rect,
    input: &TextInput,
    field: FilterField,
    focused: bool,
    p: &Palette,
) -> Rect {
    let border = if focused { p.accent } else { p.muted };
    let mut block = Block::bordered().border_style(Style::new().fg(border));
    if area.width >= MIN_PADDED_INPUT {
        block = block.padding(Padding::horizontal(1));
    }
    let text_area = block.inner(area);
    frame.render_widget(block, area);
    if text_area.is_empty() {
        return text_area;
    }

    let columns = usize::from(text_area.width);
    let offset = input.scroll_offset(columns);
    let line = if input.value().is_empty() {
        Line::styled(
            truncate(field.placeholder(), columns).into_owned(),
            style::muted(p),
        )
    } else {
        Line::raw(input.value().chars().skip(offset).collect::<String>())
    };
    frame.render_widget(line, text_area);

    if focused {
        let before: String = input
            .value()
            .chars()
            .skip(offset)
            .take(input.cursor().saturating_sub(offset))
            .collect();
        let x = to_u16(width(&before)).min(text_area.width - 1);
        frame.set_cursor_position(Position::new(text_area.x + x, text_area.y));
    }
    text_area
}

/// Draws the Clear button, label on its middle line.
fn render_button(frame: &mut Frame<'_>, area: Rect, focused: bool, p: &Palette) {
    if area.is_empty() {
        return;
    }
    let style = if focused {
        Style::new()
            .bg(p.accent)
            .fg(p.on_accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().bg(p.primary).fg(p.on_primary)
    };
    frame.render_widget(Block::new().style(style), area);
    let label = Rect {
        y: area.y + area.height / 2,
        height: 1,
        ..area
    };
    frame.render_widget(Line::from(BUTTON_LABEL).centered(), label);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_share_the_row_equally() {
        let row = Rect::new(2, 3, 100, 3);
        let (inputs, button) = split_row(row);
        assert_eq!(button, Rect::new(91, 3, 11, 3));
        // (100 - 11 - 6) / 6 = 13 rest 5: the first five get one more cell.
        assert_eq!(inputs[0], Rect::new(2, 3, 14, 3));
        assert_eq!(inputs[5], Rect::new(77, 3, 13, 3));
        for pair in inputs.windows(2) {
            assert_eq!(pair[0].right() + GAP, pair[1].x);
        }
        assert_eq!(inputs[5].right() + GAP, button.x);
    }

    #[test]
    fn narrow_rows_stay_inside_the_area() {
        for w in 0..30 {
            let row = Rect::new(1, 1, w, 3);
            let (inputs, button) = split_row(row);
            for rect in inputs.iter().chain([&button]) {
                assert!(
                    rect.width == 0 || row.contains(rect.as_position()),
                    "{w}: {rect:?}"
                );
                assert!(rect.right() <= row.right().max(rect.x), "{w}: {rect:?}");
            }
        }
    }
}
