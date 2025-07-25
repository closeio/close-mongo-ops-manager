from dataclasses import dataclass
from textual.theme import Theme


@dataclass
class ThemeConfig:
    """Configuration for theme preferences."""

    current_theme: str = "textual-dark"
    available_themes: list[str] = None

    def __post_init__(self):
        if self.available_themes is None:
            self.available_themes = [
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
            ]


class ThemeManager:
    """Manages theme selection and persistence."""

    def __init__(self):
        self.config = ThemeConfig()
        self._custom_themes: dict[str, Theme] = {}

        # Register custom themes
        close_mongodb_theme = self._create_close_mongodb_theme()
        self.register_custom_theme(close_mongodb_theme)

    def get_available_themes(self) -> list[str]:
        """Get list of all available themes."""
        return self.config.available_themes + list(self._custom_themes.keys())

    def register_custom_theme(self, theme: Theme) -> None:
        """Register a custom theme."""
        self._custom_themes[theme.name] = theme
        # Only add to config if not already present to avoid duplicates
        if theme.name not in self.get_available_themes():
            self.config.available_themes.append(theme.name)

    def get_current_theme(self) -> str:
        """Get current theme name."""
        return self.config.current_theme

    def set_current_theme(self, theme_name: str) -> bool:
        """Set current theme if valid."""
        if theme_name in self.get_available_themes():
            self.config.current_theme = theme_name
            return True
        return False

    def _create_close_mongodb_theme(self) -> Theme:
        """Create a Close MongoDB theme based on official brand guidelines."""
        return Theme(
            name="close-mongodb",
            primary="#00ED64",  # MongoDB Primary Green - for key highlights only
            secondary="#3d4f58",  # MongoDB Slate Gray
            foreground="#E5E5E5",  # Light gray text instead of green
            background="#001E2B",  # MongoDB Dark Blue/Black
            surface="#1a2832",  # Slightly lighter than background for surfaces
            panel="#001E2B",  # Keep consistent with background
            warning="#FFB000",  # Warm warning color
            error="#E74C3C",  # Red for errors
            success="#00ED64",  # MongoDB Green for success only
            accent="#00ED64",  # MongoDB Green for important accents
            boost="#52c787",  # Muted green for status bar
            dark=True,
        )
