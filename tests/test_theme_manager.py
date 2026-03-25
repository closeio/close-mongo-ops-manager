"""Tests for ThemeManager and ThemeConfig."""

from close_mongo_ops_manager.theme_manager import ThemeConfig, ThemeManager


def test_theme_config_defaults():
    """Test ThemeConfig initializes with correct defaults."""
    config = ThemeConfig()
    assert config.current_theme == "textual-dark"
    assert "textual-dark" in config.available_themes
    assert "textual-light" in config.available_themes
    assert len(config.available_themes) == 11


def test_theme_manager_has_builtin_themes():
    """Test ThemeManager includes all built-in themes."""
    manager = ThemeManager()
    themes = manager.get_available_themes()
    assert "textual-dark" in themes
    assert "nord" in themes
    assert "dracula" in themes


def test_theme_manager_registers_close_mongodb_theme():
    """Test ThemeManager auto-registers the custom close-mongodb theme."""
    manager = ThemeManager()
    themes = manager.get_available_themes()
    assert "close-mongodb" in themes


def test_register_custom_theme_adds_to_list():
    """Test registering a custom theme makes it available."""
    from textual.theme import Theme

    manager = ThemeManager()
    custom = Theme(name="my-custom", primary="#ff0000")
    manager.register_custom_theme(custom)
    assert "my-custom" in manager.get_available_themes()


def test_register_custom_theme_duplicate_is_idempotent():
    """Test registering the same theme twice doesn't create duplicates."""
    from textual.theme import Theme

    manager = ThemeManager()
    custom = Theme(name="dupe-theme", primary="#ff0000")
    manager.register_custom_theme(custom)
    manager.register_custom_theme(custom)
    count = manager.get_available_themes().count("dupe-theme")
    assert count == 1


def test_set_current_theme_valid():
    """Test setting a valid theme returns True and updates current."""
    manager = ThemeManager()
    result = manager.set_current_theme("nord")
    assert result is True
    assert manager.get_current_theme() == "nord"


def test_set_current_theme_invalid():
    """Test setting an invalid theme returns False and keeps current."""
    manager = ThemeManager()
    result = manager.set_current_theme("nonexistent-theme")
    assert result is False
    assert manager.get_current_theme() == "textual-dark"


def test_close_mongodb_theme_properties():
    """Test the custom Close MongoDB theme has expected properties."""
    manager = ThemeManager()
    theme = manager._custom_themes["close-mongodb"]
    assert theme.name == "close-mongodb"
    assert theme.dark is True
    assert theme.primary == "#00ED64"
    assert theme.background == "#001E2B"
