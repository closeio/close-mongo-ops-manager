import logging

from close_mongo_ops_manager.config_manager import ConfigManager
from close_mongo_ops_manager.theme_manager import ThemeConfig


def test_load_theme_config_logs_warning_on_invalid_json(tmp_path, caplog):
    manager = ConfigManager()
    manager.config_file = tmp_path / "config.json"
    manager.config_file.write_text("{invalid-json")

    with caplog.at_level(logging.WARNING, logger="mongo_ops_manager"):
        config = manager.load_theme_config()

    assert config.current_theme == "textual-dark"
    assert "Failed to load theme config" in caplog.text


def test_save_theme_config_logs_warning_on_invalid_existing_json(tmp_path, caplog):
    manager = ConfigManager()
    manager.config_file = tmp_path / "config.json"
    manager.config_file.write_text("{invalid-json")

    with caplog.at_level(logging.WARNING, logger="mongo_ops_manager"):
        manager.save_theme_config(ThemeConfig(current_theme="textual-light"))

    assert "Failed to save theme config" in caplog.text


def test_load_theme_config_nonexistent_file_returns_defaults(tmp_path):
    """Test that loading from a nonexistent file returns default ThemeConfig."""
    manager = ConfigManager()
    manager.config_file = tmp_path / "nonexistent" / "config.json"
    config = manager.load_theme_config()
    assert config.current_theme == "textual-dark"
    assert len(config.available_themes) == 11


def test_save_and_load_round_trip(tmp_path):
    """Test that saving then loading preserves theme config."""
    manager = ConfigManager()
    manager.config_dir = tmp_path
    manager.config_file = tmp_path / "config.json"

    # Save a theme config
    saved_config = ThemeConfig(current_theme="nord")
    manager.save_theme_config(saved_config)

    # Load it back
    loaded_config = manager.load_theme_config()
    assert loaded_config.current_theme == "nord"


def test_save_preserves_non_theme_keys(tmp_path):
    """Test that saving theme config preserves other keys in the config file."""
    import json

    manager = ConfigManager()
    manager.config_dir = tmp_path
    manager.config_file = tmp_path / "config.json"

    # Write a config file with extra keys
    initial_data = {"other_setting": "keep_me", "theme": {"current_theme": "dracula"}}
    manager.config_file.write_text(json.dumps(initial_data))

    # Save new theme config
    manager.save_theme_config(ThemeConfig(current_theme="nord"))

    # Verify other keys are preserved
    with open(manager.config_file) as f:
        data = json.load(f)
    assert data["other_setting"] == "keep_me"
    assert data["theme"]["current_theme"] == "nord"


def test_save_creates_config_file(tmp_path):
    """Test that saving creates the config file if it doesn't exist."""
    manager = ConfigManager()
    manager.config_dir = tmp_path
    manager.config_file = tmp_path / "config.json"

    assert not manager.config_file.exists()
    manager.save_theme_config(ThemeConfig(current_theme="monokai"))
    assert manager.config_file.exists()
