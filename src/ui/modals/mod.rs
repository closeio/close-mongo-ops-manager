//! Modal dialogs, drawn centered over the main screen.

mod details;
pub(super) mod help;
mod kill;
mod logs;
mod nodes;
mod theme_picker;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

pub use details::details_lines;

use super::layout::right_edge;
use super::widgets::render_scrollbar;
use crate::app::{App, LayoutCache, Modal};
use crate::theme::Palette;

const DISMISS_HINT: &str = " ESC to dismiss ";

/// Draws the open modal, if any, and records its areas. Scroll positions are
/// clamped to the content in place.
pub fn render(frame: &mut Frame<'_>, area: Rect, app: &mut App, p: &Palette) {
    let layout = &mut app.layout;
    layout.modal_area = Rect::default();
    layout.modal_page = 0;
    layout.kill_yes = Rect::default();
    layout.kill_no = Rect::default();
    layout.theme_list = Rect::default();
    layout.theme_list_offset = 0;

    let Some(mut modal) = app.modal.take() else {
        return;
    };
    let layout = &mut app.layout;
    match &mut modal {
        Modal::Help { scroll } => help::render(frame, area, scroll, p, layout),
        Modal::Logs { scroll, follow } => {
            logs::render(frame, area, &app.logs.lines(), (scroll, follow), p, layout);
        }
        Modal::Details { op, scroll } => details::render(frame, area, op, scroll, p, layout),
        Modal::KillConfirm {
            requests,
            yes_focused,
        } => {
            kill::render(frame, area, requests.len(), *yes_focused, p, layout);
        }
        Modal::Theme { list, .. } => theme_picker::render(frame, area, list, app.theme, p, layout),
        Modal::Nodes { scroll } => nodes::render(frame, area, &app.nodes, scroll, p, layout),
    }
    app.modal = Some(modal);
}

/// The frame of a modal: rounded primary border on the surface color, a
/// bold title and one cell of horizontal padding.
fn modal_block<'a>(title: &'a str, p: &Palette) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(p.primary))
        .style(Style::new().bg(p.surface).fg(p.foreground))
        .title(
            Line::from(vec![" ".into(), title.into(), " ".into()])
                .style(Style::new().add_modifier(Modifier::BOLD)),
        )
        .padding(Padding::horizontal(1))
}

/// Clears `area`, draws `block` there and returns its inner area.
fn open(frame: &mut Frame<'_>, area: Rect, block: Block<'_>, layout: &mut LayoutCache) -> Rect {
    frame.render_widget(Clear, area);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    layout.modal_area = area;
    layout.modal_page = inner.height;
    inner
}

/// Draws the `inner` rows of `lines` starting at `scroll`, clamped so the
/// last page is full, with a scrollbar over the right border of `area` when
/// they overflow. Returns the clamped scroll.
fn render_scrolled(
    frame: &mut Frame<'_>,
    (area, inner): (Rect, Rect),
    lines: Vec<Line<'static>>,
    scroll: usize,
    p: &Palette,
) -> usize {
    let page = usize::from(inner.height);
    let total = lines.len();
    let scroll = scroll.min(total.saturating_sub(page));
    let visible: Vec<Line<'static>> = lines.into_iter().skip(scroll).take(page).collect();
    frame.render_widget(Paragraph::new(visible), inner);
    let track = right_edge(area, inner);
    render_scrollbar(
        frame,
        track,
        (total, page, scroll),
        Style::new().fg(p.primary),
        p,
    );
    scroll
}
