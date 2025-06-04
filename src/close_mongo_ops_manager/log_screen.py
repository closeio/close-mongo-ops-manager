from textual.binding import Binding
from textual.app import ComposeResult
from textual.containers import (
    Container,
    VerticalScroll,
)
from textual.screen import ModalScreen
from textual.widgets import Footer, Static
from textual.timer import Timer


class LogScreen(ModalScreen):
    """Screen for viewing application logs."""

    BORDER_TITLE = "Application Logs"
    BORDER_SUBTITLE = "ESCAPE to dismiss"

    DEFAULT_CSS = """
    LogScreen {
        align: center middle;
    }

    #log-container {
        width: 80%;
        height: 80%;
        border: round $primary;
        background: $surface;
        padding: 1;
        overflow-y: auto;
    }

    #log-content {
        width: 100%;
        height: 100%;
        padding: 1;
    }
    """

    BINDINGS = [
        Binding("escape", "dismiss", "Log Screen", show=False),
    ]

    def __init__(self, log_file: str) -> None:
        super().__init__()
        self.log_file = log_file
        self.update_timer: Timer | None = None
        self.last_position = 0

    async def on_mount(self) -> None:
        """Load log file asynchronously and start auto-refresh."""
        await self.update_log_content()
        # Start periodic updates every 0.5 seconds
        self.update_timer = self.set_interval(0.5, self.update_log_content)

    async def update_log_content(self) -> None:
        """Read and update the log file content."""
        try:
            with open(self.log_file) as f:
                content = f.read()

            log_content = self.query_one("#log-content", VerticalScroll)

            # Clear existing content
            await log_content.remove_children()

            # Add new content
            await log_content.mount(Static(content))

            # Auto-scroll to bottom if we're near the bottom
            if log_content.scroll_y >= log_content.max_scroll_y - 5:
                log_content.scroll_end()

        except Exception as e:
            log_content = self.query_one("#log-content", VerticalScroll)
            await log_content.remove_children()
            await log_content.mount(Static(f"Error reading log file: {e}"))

    def on_unmount(self) -> None:
        """Clean up the timer when screen is dismissed."""
        if self.update_timer:
            self.update_timer.stop()

    def compose(self) -> ComposeResult:
        yield Footer()
        container = Container(id="log-container")
        container.border_title = "Application Logs"
        container.border_subtitle = "ESCAPE to dismiss"

        with container:
            # We'll use the VerticalScroll widget with an ID for the content
            scroll = VerticalScroll(id="log-content")
            yield scroll
