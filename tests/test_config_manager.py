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
