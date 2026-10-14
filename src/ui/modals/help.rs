//! Help screen: key bindings and usage notes from [`HELP`].

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use super::{DISMISS_HINT, modal_block, open, render_scrolled};
use crate::app::{HELP, LayoutCache};
use crate::theme::Palette;
use crate::ui::layout::{centered, percent};
use crate::ui::style;
use crate::ui::text::{to_u16, width, wrap_words};
use crate::ui::widgets::with_hint;

const MAX_WIDTH: u16 = 76;
/// Narrowest description column before descriptions go below their keys.
const MIN_DESCRIPTION_WIDTH: usize = 16;
const INDENT: &str = "  ";

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    scroll: &mut u16,
    p: &Palette,
    layout: &mut LayoutCache,
) {
    let width = MAX_WIDTH.min(percent(area.width, 90));
    // Border and padding take two cells on each side.
    let lines = help_lines(usize::from(width.saturating_sub(4)), p);
    let height = to_u16(lines.len())
        .saturating_add(2)
        .min(percent(area.height, 85));
    let rect = centered(area, width, height);
    let block = with_hint(modal_block("Help", p), DISMISS_HINT, rect.width);
    let inner = open(frame, rect, block, layout);
    let shown = render_scrolled(frame, (rect, inner), lines, usize::from(*scroll), p);
    *scroll = to_u16(shown);
}

/// Lines of the help screen for a content area `width` cells wide.
///
/// Entries without keys are section titles, or plain notes when they start
/// with `-`.
pub fn help_lines(width: usize, p: &Palette) -> Vec<Line<'static>> {
    let key_width = HELP
        .iter()
        .map(|(keys, _)| self::width(keys))
        .max()
        .unwrap_or(0);
    let mut lines = Vec::new();
    for &(keys, description) in HELP {
        if keys.is_empty() && !description.starts_with('-') {
            if !lines.is_empty() {
                lines.push(Line::default());
            }
            lines.push(Line::styled(
                description,
                style::title(p).add_modifier(Modifier::UNDERLINED),
            ));
        } else if keys.is_empty() {
            push_wrapped(&mut lines, description, width, INDENT, "    ");
        } else if width.saturating_sub(key_width + 4) >= MIN_DESCRIPTION_WIDTH {
            let columns = width - key_width - 4;
            for (i, text) in wrap_words(description, columns).into_iter().enumerate() {
                let keys = if i == 0 { keys } else { "" };
                lines.push(Line::from(vec![
                    Span::raw(INDENT),
                    Span::styled(format!("{keys:<key_width$}"), style::key(p)),
                    Span::raw("  "),
                    Span::raw(text),
                ]));
            }
        } else {
            lines.push(Line::from(vec![
                Span::raw(INDENT),
                Span::styled(keys, style::key(p)),
            ]));
            push_wrapped(&mut lines, description, width, "    ", "    ");
        }
    }
    lines
}

/// Appends `text` wrapped to `width` cells, the first line indented with
/// `first`, the others with `rest`.
fn push_wrapped(lines: &mut Vec<Line<'static>>, text: &str, width: usize, first: &str, rest: &str) {
    for (i, text) in wrap_words(text, width.saturating_sub(rest.len()))
        .into_iter()
        .enumerate()
    {
        let indent = if i == 0 { first } else { rest };
        lines.push(Line::from(vec![
            Span::raw(indent.to_owned()),
            Span::raw(text),
        ]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.to_string().trim_end().to_owned())
            .collect()
    }

    #[test]
    fn sections_keys_and_notes() {
        let p = theme::default_theme().palette(true);
        let lines = help_lines(72, &p);
        let text = text(&lines);
        assert_eq!(text[0], "Keyboard Shortcuts");
        assert_eq!(text[1], format!("  {:<14}  Show this help", "F1, ?"));
        assert!(text.contains(&format!("  {:<14}  Kill selected operations", "Ctrl+K")));
        let usage = text
            .iter()
            .position(|l| l == "Usage")
            .expect("usage section");
        assert_eq!(text[usage - 1], "");
        assert_eq!(text[usage + 1], "  - Use arrow keys or mouse to navigate");
        assert_eq!(
            lines[0].style,
            style::title(&p).add_modifier(Modifier::UNDERLINED)
        );
        assert_eq!(lines[1].spans[1].style, style::key(&p));
        assert!(lines.iter().all(|l| l.width() <= 72));
    }

    #[test]
    fn narrow_help_puts_descriptions_below_keys() {
        let p = theme::default_theme().palette(true);
        let lines = help_lines(24, &p);
        let text = text(&lines);
        assert_eq!(text[1], "  F1, ?");
        assert_eq!(text[2], "    Show this help");
        assert!(lines.iter().all(|l| l.width() <= 24), "{text:?}");
    }
}
