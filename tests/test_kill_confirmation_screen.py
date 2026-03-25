"""Tests for KillConfirmation screen."""

from textual.app import App, ComposeResult

from close_mongo_ops_manager.kill_confirmation_screen import KillConfirmation


class KillConfirmTestApp(App):
    """Test app that pushes the KillConfirmation screen."""

    def __init__(self, operations: list[str]):
        super().__init__()
        self.operations = operations
        self.result = None

    def compose(self) -> ComposeResult:
        yield from ()

    def on_mount(self) -> None:
        def capture_result(result):
            self.result = result

        self.push_screen(KillConfirmation(self.operations), callback=capture_result)


async def test_kill_confirmation_renders_with_single_op():
    """Test dialog renders correct text for a single operation."""
    app = KillConfirmTestApp(["12345"])
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        question = app.screen.query_one("#question")
        assert "1 operation?" in str(question.render())


async def test_kill_confirmation_renders_with_multiple_ops():
    """Test dialog renders correct plural text for multiple operations."""
    app = KillConfirmTestApp(["111", "222", "333"])
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        question = app.screen.query_one("#question")
        assert "3 operations?" in str(question.render())


async def test_kill_confirmation_yes_button_dismisses_true():
    """Test clicking Yes button dismisses with True."""
    app = KillConfirmTestApp(["12345"])
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        await pilot.click("#yes")
        await pilot.pause(0.1)
        assert app.result is True


async def test_kill_confirmation_no_button_dismisses_false():
    """Test clicking No button dismisses with False."""
    app = KillConfirmTestApp(["12345"])
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        await pilot.click("#no")
        await pilot.pause(0.1)
        assert app.result is False


async def test_kill_confirmation_escape_dismisses_false():
    """Test pressing Escape dismisses with False."""
    app = KillConfirmTestApp(["12345"])
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        await pilot.press("escape")
        await pilot.pause(0.1)
        assert app.result is False
