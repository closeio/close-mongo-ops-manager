"""Tests for ThemeScreen."""

from textual.app import App, ComposeResult
from textual.widgets import OptionList

from close_mongo_ops_manager.theme_screen import ThemeScreen


class ThemeTestApp(App):
    """Test app that pushes the ThemeScreen."""

    def __init__(self, themes: list[str], current: str):
        super().__init__()
        self.themes = themes
        self.current = current
        self.result = None

    def compose(self) -> ComposeResult:
        yield from ()

    def on_mount(self) -> None:
        def capture_result(result):
            self.result = result

        self.push_screen(
            ThemeScreen(self.themes, self.current), callback=capture_result
        )


async def test_theme_screen_renders_all_themes():
    """Test that all themes appear in the option list."""
    themes = ["textual-dark", "nord", "dracula"]
    app = ThemeTestApp(themes, "textual-dark")
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        option_list = app.screen.query_one("#theme-list", OptionList)
        assert option_list.option_count == 3


async def test_theme_screen_highlights_current_theme():
    """Test that the current theme is highlighted on mount."""
    themes = ["textual-dark", "nord", "dracula"]
    app = ThemeTestApp(themes, "nord")
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        option_list = app.screen.query_one("#theme-list", OptionList)
        assert option_list.highlighted == 1  # nord is index 1


async def test_theme_screen_select_dismisses_with_theme():
    """Test selecting a theme dismisses with the theme name."""
    themes = ["textual-dark", "nord", "dracula"]
    app = ThemeTestApp(themes, "textual-dark")
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        # Press enter to select the currently highlighted theme
        await pilot.press("enter")
        await pilot.pause(0.1)
        assert app.result == "textual-dark"


async def test_theme_screen_escape_dismisses_without_selection():
    """Test pressing escape dismisses without selecting a theme."""
    themes = ["textual-dark", "nord"]
    app = ThemeTestApp(themes, "textual-dark")
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        await pilot.press("escape")
        await pilot.pause(0.1)
        assert app.result is None
