//! Color themes. Names and colors follow the Textual themes used by the
//! Python version, so saved preferences keep working.

use ratatui::style::Color;

/// Theme used when none is configured.
pub const DEFAULT_THEME: &str = "textual-dark";

/// A color theme, defined by its base colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub name: &'static str,
    pub dark: bool,
    pub primary: Color,
    pub secondary: Color,
    pub accent: Color,
    pub foreground: Color,
    pub background: Color,
    pub surface: Color,
    pub panel: Color,
    /// Status bar background; derived from background and foreground when
    /// `None`.
    pub boost: Option<Color>,
    pub warning: Color,
    pub error: Color,
    pub success: Color,
}

/// Concrete colors to render with, derived from a [`Theme`] and adapted to the
/// terminal's color support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub dark: bool,
    pub primary: Color,
    pub secondary: Color,
    pub accent: Color,
    pub foreground: Color,
    /// Dimmed text (placeholders, hints, borders of unfocused widgets).
    pub muted: Color,
    pub background: Color,
    pub surface: Color,
    pub panel: Color,
    pub warning: Color,
    pub error: Color,
    pub success: Color,
    /// Status bar background and text.
    pub status_bg: Color,
    pub status_fg: Color,
    /// Background of every other table row.
    pub zebra: Color,
    /// Table cursor row.
    pub cursor_bg: Color,
    pub cursor_fg: Color,
    /// Text drawn on top of `primary`, `accent`, `error`, ... backgrounds.
    pub on_primary: Color,
    pub on_accent: Color,
    pub on_error: Color,
    pub on_warning: Color,
    pub on_success: Color,
}

const fn hex(v: u32) -> Color {
    Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// All themes, in the order they are offered in the theme picker.
pub static THEMES: [Theme; 12] = [
    Theme {
        name: "textual-dark",
        dark: true,
        primary: hex(0x0178D4),
        secondary: hex(0x004578),
        accent: hex(0xFFA62B),
        foreground: hex(0xE0E0E0),
        background: hex(0x121212),
        surface: hex(0x1E1E1E),
        panel: hex(0x1B2730),
        boost: None,
        warning: hex(0xFFA62B),
        error: hex(0xBA3C5B),
        success: hex(0x4EBF71),
    },
    Theme {
        name: "textual-light",
        dark: false,
        primary: hex(0x004578),
        secondary: hex(0x0178D4),
        accent: hex(0xFFA62B),
        foreground: hex(0x1E1E1E),
        background: hex(0xE0E0E0),
        surface: hex(0xD8D8D8),
        panel: hex(0xDFDFDF),
        boost: None,
        warning: hex(0xFFA62B),
        error: hex(0xBA3C5B),
        success: hex(0x4EBF71),
    },
    Theme {
        name: "nord",
        dark: true,
        primary: hex(0x88C0D0),
        secondary: hex(0x81A1C1),
        accent: hex(0xB48EAD),
        foreground: hex(0xD8DEE9),
        background: hex(0x2E3440),
        surface: hex(0x3B4252),
        panel: hex(0x434C5E),
        boost: None,
        warning: hex(0xEBCB8B),
        error: hex(0xBF616A),
        success: hex(0xA3BE8C),
    },
    Theme {
        name: "gruvbox",
        dark: true,
        primary: hex(0x85A598),
        secondary: hex(0xA89A85),
        accent: hex(0xFABD2F),
        foreground: hex(0xFBF1C7),
        background: hex(0x282828),
        surface: hex(0x3C3836),
        panel: hex(0x504945),
        boost: None,
        warning: hex(0xFE8019),
        error: hex(0xFB4934),
        success: hex(0xB8BB26),
    },
    Theme {
        name: "tokyo-night",
        dark: true,
        primary: hex(0xBB9AF7),
        secondary: hex(0x7AA2F7),
        accent: hex(0xFF9E64),
        foreground: hex(0xA9B1D6),
        background: hex(0x1A1B26),
        surface: hex(0x24283B),
        panel: hex(0x414868),
        boost: None,
        warning: hex(0xE0AF68),
        error: hex(0xF7768E),
        success: hex(0x9ECE6A),
    },
    Theme {
        name: "solarized-light",
        dark: false,
        primary: hex(0x268BD2),
        secondary: hex(0x2AA198),
        accent: hex(0x6C71C4),
        foreground: hex(0x586E75),
        background: hex(0xFDF6E3),
        surface: hex(0xEEE8D5),
        panel: hex(0xEEE8D5),
        boost: None,
        warning: hex(0xCB4B16),
        error: hex(0xDC322F),
        success: hex(0x859900),
    },
    Theme {
        name: "dracula",
        dark: true,
        primary: hex(0xBD93F9),
        secondary: hex(0x6272A4),
        accent: hex(0xFF79C6),
        foreground: hex(0xF8F8F2),
        background: hex(0x282A36),
        surface: hex(0x2B2E3B),
        panel: hex(0x313442),
        boost: None,
        warning: hex(0xFFB86C),
        error: hex(0xFF5555),
        success: hex(0x50FA7B),
    },
    Theme {
        name: "monokai",
        dark: true,
        primary: hex(0xAE81FF),
        secondary: hex(0xF92672),
        accent: hex(0x66D9EF),
        foreground: hex(0xD6D6D6),
        background: hex(0x272822),
        surface: hex(0x2E2E2E),
        panel: hex(0x3E3D32),
        boost: None,
        warning: hex(0xFD971F),
        error: hex(0xF92672),
        success: hex(0xA6E22E),
    },
    Theme {
        name: "flexoki",
        dark: true,
        primary: hex(0x205EA6),
        secondary: hex(0x24837B),
        accent: hex(0x9B76C8),
        foreground: hex(0xFFFCF0),
        background: hex(0x100F0F),
        surface: hex(0x1C1B1A),
        panel: hex(0x282726),
        boost: None,
        warning: hex(0xAD8301),
        error: hex(0xAF3029),
        success: hex(0x66800B),
    },
    Theme {
        name: "catppuccin-mocha",
        dark: true,
        primary: hex(0xF5C2E7),
        secondary: hex(0xCBA6F7),
        accent: hex(0xFAB387),
        foreground: hex(0xCDD6F4),
        background: hex(0x181825),
        surface: hex(0x313244),
        panel: hex(0x45475A),
        boost: None,
        warning: hex(0xFAE3B0),
        error: hex(0xF28FAD),
        success: hex(0xABE9B3),
    },
    Theme {
        name: "catppuccin-latte",
        dark: false,
        primary: hex(0x8839EF),
        secondary: hex(0xDC8A78),
        accent: hex(0xFE640B),
        foreground: hex(0x4C4F69),
        background: hex(0xEFF1F5),
        surface: hex(0xE6E9EF),
        panel: hex(0xCCD0DA),
        boost: None,
        warning: hex(0xDF8E1D),
        error: hex(0xD20F39),
        success: hex(0x40A02B),
    },
    // Close theme with the MongoDB brand colors.
    Theme {
        name: "close-mongodb",
        dark: true,
        primary: hex(0x00ED64),
        secondary: hex(0x3D4F58),
        accent: hex(0x00ED64),
        foreground: hex(0xE5E5E5),
        background: hex(0x001E2B),
        surface: hex(0x1A2832),
        panel: hex(0x001E2B),
        boost: Some(hex(0x52C787)),
        warning: hex(0xFFB000),
        error: hex(0xE74C3C),
        success: hex(0x00ED64),
    },
];

/// All available themes.
pub fn all() -> &'static [Theme] {
    &THEMES
}

/// Looks a theme up by name.
pub fn by_name(name: &str) -> Option<&'static Theme> {
    THEMES.iter().find(|t| t.name == name)
}

/// The default theme.
pub fn default_theme() -> &'static Theme {
    by_name(DEFAULT_THEME).expect("default theme exists")
}

/// Display name of a theme: `"catppuccin-mocha"` -> `"Catppuccin Mocha"`.
pub fn display_name(name: &str) -> String {
    name.split('-')
        .filter(|w| !w.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

impl Theme {
    /// Concrete colors for rendering. Without true color support, colors are
    /// mapped to the nearest entry of the 256-color palette.
    pub fn palette(&self, truecolor: bool) -> Palette {
        let status_bg = self
            .boost
            .unwrap_or_else(|| blend(self.background, self.foreground, 0.10));
        let zebra = blend(
            self.background,
            self.foreground,
            if self.dark { 0.05 } else { 0.06 },
        );
        let a = |c: Color| adapt(c, truecolor);
        Palette {
            dark: self.dark,
            primary: a(self.primary),
            secondary: a(self.secondary),
            accent: a(self.accent),
            foreground: a(self.foreground),
            muted: a(blend(self.foreground, self.background, 0.45)),
            background: a(self.background),
            surface: a(self.surface),
            panel: a(self.panel),
            warning: a(self.warning),
            error: a(self.error),
            success: a(self.success),
            status_bg: a(status_bg),
            status_fg: a(contrast_text(status_bg)),
            zebra: a(zebra),
            cursor_bg: a(self.primary),
            cursor_fg: a(contrast_text(self.primary)),
            on_primary: a(contrast_text(self.primary)),
            on_accent: a(contrast_text(self.accent)),
            on_error: a(contrast_text(self.error)),
            on_warning: a(contrast_text(self.warning)),
            on_success: a(contrast_text(self.success)),
        }
    }
}

/// Mixes `b` into `a`: `t = 0.0` is `a`, `t = 1.0` is `b`. Non-RGB colors are
/// returned unchanged (`a`).
pub fn blend(a: Color, b: Color, t: f32) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let mix = |x: u8, y: u8| -> u8 {
                (f32::from(x) + (f32::from(y) - f32::from(x)) * t.clamp(0.0, 1.0)).round() as u8
            };
            Color::Rgb(mix(r1, r2), mix(g1, g2), mix(b1, b2))
        }
        _ => a,
    }
}

/// Black or white, whichever reads better on `bg`.
pub fn contrast_text(bg: Color) -> Color {
    match bg {
        Color::Rgb(r, g, b) => {
            let lum = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
            if lum > 140.0 {
                Color::Rgb(0x10, 0x10, 0x10)
            } else {
                Color::Rgb(0xF5, 0xF5, 0xF5)
            }
        }
        _ => Color::Reset,
    }
}

/// Returns `color` unchanged with true color support, otherwise the nearest
/// color of the xterm 256-color palette.
pub fn adapt(color: Color, truecolor: bool) -> Color {
    match color {
        Color::Rgb(r, g, b) if !truecolor => Color::Indexed(nearest_xterm_256(r, g, b)),
        other => other,
    }
}

/// Whether the terminal advertises 24-bit color support.
pub fn truecolor_supported() -> bool {
    let env = |key: &str| std::env::var(key).unwrap_or_default().to_ascii_lowercase();
    let colorterm = env("COLORTERM");
    if colorterm.contains("truecolor") || colorterm.contains("24bit") {
        return true;
    }
    let term_program = env("TERM_PROGRAM");
    if [
        "iterm.app",
        "wezterm",
        "ghostty",
        "vscode",
        "hyper",
        "tabby",
        "rio",
        "warpterminal",
    ]
    .contains(&term_program.as_str())
    {
        return true;
    }
    let term = env("TERM");
    ["kitty", "alacritty", "foot", "ghostty", "wezterm", "direct"]
        .iter()
        .any(|t| term.contains(t))
}

const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn nearest_xterm_256(r: u8, g: u8, b: u8) -> u8 {
    let dist = |(r1, g1, b1): (u8, u8, u8)| -> u32 {
        let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).unsigned_abs();
        d(r, r1).pow(2) + d(g, g1).pow(2) + d(b, b1).pow(2)
    };
    let level = |v: u8| -> usize {
        CUBE_LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, l)| (i32::from(**l) - i32::from(v)).unsigned_abs())
            .map(|(i, _)| i)
            .unwrap_or(0)
    };
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = (CUBE_LEVELS[ri], CUBE_LEVELS[gi], CUBE_LEVELS[bi]);
    let cube_index = 16 + 36 * ri + 6 * gi + bi;

    let avg = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let gray_step = (avg.saturating_sub(8) / 10).min(23) as u8;
    let gray_level = 8 + 10 * gray_step;
    let gray = (gray_level, gray_level, gray_level);
    let gray_index = 232 + usize::from(gray_step);

    if dist(gray) < dist(cube) {
        gray_index as u8
    } else {
        cube_index as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_names_are_unique_and_include_python_themes() {
        let names: Vec<&str> = all().iter().map(|t| t.name).collect();
        for expected in [
            "textual-dark",
            "textual-light",
            "nord",
            "gruvbox",
            "tokyo-night",
            "solarized-light",
            "dracula",
            "monokai",
            "flexoki",
            "catppuccin-mocha",
            "catppuccin-latte",
            "close-mongodb",
        ] {
            assert!(names.contains(&expected), "{expected} missing");
        }
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
    }

    #[test]
    fn default_theme_exists() {
        assert_eq!(default_theme().name, DEFAULT_THEME);
        assert!(by_name("does-not-exist").is_none());
    }

    #[test]
    fn close_mongodb_theme_uses_brand_colors() {
        let t = by_name("close-mongodb").unwrap();
        assert_eq!(t.primary, Color::Rgb(0x00, 0xED, 0x64));
        assert_eq!(t.background, Color::Rgb(0x00, 0x1E, 0x2B));
        assert_eq!(t.boost, Some(Color::Rgb(0x52, 0xC7, 0x87)));
        assert!(t.dark);
    }

    #[test]
    fn display_names() {
        assert_eq!(display_name("catppuccin-mocha"), "Catppuccin Mocha");
        assert_eq!(display_name("close-mongodb"), "Close Mongodb");
        assert_eq!(display_name("nord"), "Nord");
    }

    #[test]
    fn blend_and_contrast() {
        assert_eq!(
            blend(Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255), 0.5),
            Color::Rgb(128, 128, 128)
        );
        assert_eq!(blend(Color::Reset, Color::Rgb(1, 2, 3), 0.5), Color::Reset);
        assert_eq!(
            contrast_text(Color::Rgb(255, 255, 255)),
            Color::Rgb(0x10, 0x10, 0x10)
        );
        assert_eq!(
            contrast_text(Color::Rgb(0, 0, 0)),
            Color::Rgb(0xF5, 0xF5, 0xF5)
        );
    }

    #[test]
    fn adapt_maps_to_256_colors_without_truecolor() {
        assert_eq!(adapt(Color::Rgb(255, 0, 0), true), Color::Rgb(255, 0, 0));
        assert_eq!(adapt(Color::Rgb(255, 0, 0), false), Color::Indexed(196));
        assert_eq!(adapt(Color::Rgb(0, 0, 0), false), Color::Indexed(16));
        assert_eq!(adapt(Color::Rgb(128, 128, 128), false), Color::Indexed(244));
        assert_eq!(adapt(Color::Reset, false), Color::Reset);
    }

    #[test]
    fn palettes_are_fully_resolved() {
        for theme in all() {
            let p = theme.palette(false);
            for c in [p.primary, p.background, p.status_bg, p.cursor_fg, p.zebra] {
                assert!(matches!(c, Color::Indexed(_)), "{}: {c:?}", theme.name);
            }
        }
    }
}
