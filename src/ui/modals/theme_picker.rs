//! Theme picker. The whole screen previews the highlighted theme.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{List, ListItem, ListState, Padding};

use super::{modal_block, open};
use crate::app::LayoutCache;
use crate::theme::{self, Palette, Theme};
use crate::ui::layout::centered;
use crate::ui::style;
use crate::ui::widgets::with_hint;

const WIDTH: u16 = 60;
const HEIGHT: u16 = 20;
const HINT: &str = " Enter to apply · ESC to cancel ";

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    list: &mut ListState,
    applied: &Theme,
    p: &Palette,
    layout: &mut LayoutCache,
) {
    let rect = centered(area, WIDTH, HEIGHT);
    let block =
        with_hint(modal_block("Select Theme", p), HINT, rect.width).padding(Padding::uniform(1));
    let inner = open(frame, rect, block, layout);
    let items: Vec<ListItem<'static>> = theme_names(applied)
        .into_iter()
        .map(ListItem::new)
        .collect();
    let widget = List::new(items).highlight_style(style::cursor(p));
    frame.render_stateful_widget(widget, inner, list);
    layout.theme_list = inner;
    layout.theme_list_offset = list.offset();
}

/// Display names of all themes, the applied one marked with `✓`.
pub fn theme_names(applied: &Theme) -> Vec<String> {
    theme::all()
        .iter()
        .map(|t| {
            let name = theme::display_name(t.name);
            if t.name == applied.name {
                format!("{name} ✓")
            } else {
                name
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applied_theme_is_marked() {
        let names = theme_names(theme::by_name("nord").unwrap());
        assert_eq!(names.len(), theme::all().len());
        assert!(names.contains(&"Nord ✓".to_owned()));
        assert!(names.contains(&"Textual Dark".to_owned()));
        assert_eq!(names.iter().filter(|n| n.ends_with('✓')).count(), 1);
    }
}
