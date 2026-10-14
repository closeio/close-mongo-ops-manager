//! Styles derived from the palette.

use ratatui::style::{Color, Modifier, Style};

use crate::app::Severity;
use crate::theme::{self, Palette};

/// Plain text on the screen background.
pub fn base(p: &Palette) -> Style {
    Style::new().fg(p.foreground).bg(p.background)
}

/// Dimmed text: placeholders, hints, empty states.
pub fn muted(p: &Palette) -> Style {
    Style::new().fg(p.muted)
}

/// `color` faded into `background`; the DIM modifier when the colors cannot
/// be mixed (no true color).
pub fn faded(color: Color, background: Color) -> Style {
    match mix(color, background, 0.5) {
        Some(c) => Style::new().fg(c),
        None => Style::new().fg(color).add_modifier(Modifier::DIM),
    }
}

/// Cursor row of a list or table with focus.
pub fn cursor(p: &Palette) -> Style {
    Style::new()
        .bg(p.cursor_bg)
        .fg(p.cursor_fg)
        .add_modifier(Modifier::BOLD)
}

/// Cursor row of a table without focus: a subtle tint of the cursor color.
pub fn cursor_unfocused(p: &Palette) -> Style {
    Style::new().bg(mix(p.cursor_bg, p.background, 0.6).unwrap_or(p.panel))
}

/// Color associated with a notification severity.
pub fn severity_color(severity: Severity, p: &Palette) -> Color {
    match severity {
        Severity::Info => p.primary,
        Severity::Warning => p.warning,
        Severity::Error => p.error,
    }
}

/// Section titles inside modals.
pub fn title(p: &Palette) -> Style {
    Style::new().fg(p.primary).add_modifier(Modifier::BOLD)
}

/// Key names (help screen, footer).
pub fn key(p: &Palette) -> Style {
    Style::new().fg(p.accent).add_modifier(Modifier::BOLD)
}

/// `a` mixed with `b` (`t = 0.0` is `a`), when both are RGB colors.
fn mix(a: Color, b: Color, t: f32) -> Option<Color> {
    matches!((a, b), (Color::Rgb(..), Color::Rgb(..))).then(|| theme::blend(a, b, t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faded_mixes_rgb_and_dims_indexed_colors() {
        let rgb = faded(Color::Rgb(200, 0, 0), Color::Rgb(0, 0, 0));
        assert_eq!(rgb.fg, Some(Color::Rgb(100, 0, 0)));
        assert!(!rgb.add_modifier.contains(Modifier::DIM));
        let indexed = faded(Color::Indexed(1), Color::Indexed(0));
        assert_eq!(indexed.fg, Some(Color::Indexed(1)));
        assert!(indexed.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn unfocused_cursor_falls_back_to_panel() {
        let p = theme::default_theme().palette(false);
        assert_eq!(cursor_unfocused(&p).bg, Some(p.panel));
        let p = theme::default_theme().palette(true);
        assert_ne!(cursor_unfocused(&p).bg, Some(p.cursor_bg));
    }
}
