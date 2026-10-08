//! Rendering.
//!
//! [`draw`] renders the whole screen from the [`App`] state, top to bottom:
//! header, filter bar, operations table, status bar and footer, then the
//! open modal and toasts. It also records the clickable areas in
//! `app.layout` and clamps the modals' scroll positions to their content.

mod bars;
mod filter_bar;
mod layout;
mod modals;
mod style;
mod table;
mod text;
mod toasts;
mod widgets;

#[cfg(test)]
mod tests;

use ratatui::Frame;
use ratatui::widgets::Block;

pub use modals::details_lines;
pub use text::pretty_json;

use crate::app::App;
use layout::MainAreas;

/// Draws the whole screen and records clickable areas in `app.layout`.
pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let palette = app.palette();
    let area = frame.area();
    frame.render_widget(Block::new().style(style::base(&palette)), area);

    let areas = MainAreas::new(area, app.filter_bar_visible);
    bars::render_header(frame, areas.header, &app.title, &palette);
    filter_bar::render(frame, areas.filter_bar, app, &palette);
    table::render(frame, areas.table, app, &palette);
    bars::render_status(frame, areas.status, app, &palette);
    app.layout.footer = bars::render_footer(frame, areas.footer, &app.footer_actions(), &palette);
    modals::render(frame, area, app, &palette);
    // Notifications stay readable over dialogs.
    app.layout.toasts = toasts::render(frame, areas.toasts, &app.toasts, &palette);
}
