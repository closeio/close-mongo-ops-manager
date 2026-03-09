import json
import logging
from pathlib import Path
from .theme_manager import ThemeConfig

logger = logging.getLogger("mongo_ops_manager")


class ConfigManager:
    """Manages application configuration persistence."""

    def __init__(self):
        self.config_dir = Path.home() / ".config" / "close-mongo-ops-manager"
        self.config_file = self.config_dir / "config.json"
        self._ensure_config_dir()

    def _ensure_config_dir(self) -> None:
        """Ensure config directory exists."""
        self.config_dir.mkdir(parents=True, exist_ok=True)

    def load_theme_config(self) -> ThemeConfig:
        """Load theme configuration from file."""
        try:
            if self.config_file.exists():
                with open(self.config_file) as f:
                    data = json.load(f)
                    theme_data = data.get("theme", {})
                    return ThemeConfig(
                        current_theme=theme_data.get("current_theme", "textual-dark"),
                        available_themes=theme_data.get("available_themes"),
                    )
        except Exception as e:
            logger.warning(f"Failed to load theme config from {self.config_file}: {e}")
        return ThemeConfig()

    def save_theme_config(self, theme_config: ThemeConfig) -> None:
        """Save theme configuration to file."""
        try:
            config_data = {}
            if self.config_file.exists():
                with open(self.config_file) as f:
                    config_data = json.load(f)

            config_data["theme"] = {
                "current_theme": theme_config.current_theme,
                "available_themes": theme_config.available_themes,
            }

            with open(self.config_file, "w") as f:
                json.dump(config_data, f, indent=2)
        except Exception as e:
            logger.warning(f"Failed to save theme config to {self.config_file}: {e}")
