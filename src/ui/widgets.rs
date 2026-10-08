//! Small rendering helpers shared by the screens.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use super::text::{to_u16, width, wrap_words};
use crate::theme::Palette;

/// `block` with `hint` on the right of its bottom border, when a block
/// `block_width` cells wide shows it whole.
pub fn with_hint<'a>(block: Block<'a>, hint: &'a str, block_width: u16) -> Block<'a> {
    if usize::from(block_width) >= width(hint) + 4 {
        block.title_bottom(Line::from(hint).right_aligned())
    } else {
        block
    }
}

/// Draws centered, word-wrapped messages in the middle of `area`.
pub fn render_message(frame: &mut Frame<'_>, area: Rect, messages: &[(&str, Style)]) {
    if area.is_empty() {
        return;
    }
    let lines: Vec<Line<'static>> = messages
        .iter()
        .flat_map(|&(text, style)| {
            wrap_words(text, usize::from(area.width))
                .into_iter()
                .map(move |line| Line::styled(line, style).centered())
        })
        .collect();
    let height = to_u16(lines.len()).min(area.height);
    let top = area.y + (area.height - height) / 2;
    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            y: top,
            height,
            ..area
        },
    );
}

/// Draws a vertical scrollbar in `track` (usually over a block's right
/// border) for a view of `page` rows out of `total`, scrolled to `offset`.
/// Nothing is drawn when everything fits.
pub fn render_scrollbar(
    frame: &mut Frame<'_>,
    track: Rect,
    (total, page, offset): (usize, usize, usize),
    track_style: Style,
    p: &Palette,
) {
    if track.is_empty() || page == 0 || total <= page {
        return;
    }
    // One position per possible offset, so the thumb reaches the end of the
    // track at the last page.
    let max_offset = total - page;
    let mut state = ScrollbarState::new(max_offset + 1)
        .viewport_content_length(page)
        .position(offset.min(max_offset));
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .track_style(track_style)
        .thumb_symbol("█")
        .thumb_style(Style::new().fg(p.primary));
    frame.render_stateful_widget(scrollbar, track, &mut state);
}
