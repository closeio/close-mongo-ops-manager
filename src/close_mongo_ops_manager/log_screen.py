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
    id = "log_screen"
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
        self._content = ""

    async def on_mount(self) -> None:
        """Load log file asynchronously and start auto-refresh."""
        await self.update_log_content()
        # Start periodic updates every 0.5 seconds
        self.update_timer = self.set_interval(0.5, self.update_log_content)

    def _read_new_log_content(self) -> tuple[str, bool]:
        """Read only appended log content and detect file truncation."""
        with open(self.log_file) as f:
            f.seek(0, 2)
            current_size = f.tell()

            file_truncated = current_size < self.last_position
            if file_truncated:
                self.last_position = 0

            f.seek(self.last_position)
            content = f.read()
            self.last_position = f.tell()

        return content, file_truncated

    async def update_log_content(self) -> None:
        """Read and update the log file content."""
        try:
            log_content = self.query_one("#log-content", VerticalScroll)
            log_text = self.query_one("#log-text", Static)

            is_near_bottom = (
                log_content.max_scroll_y <= 0
                or log_content.scroll_y >= log_content.max_scroll_y - 5
            )

            new_content, file_truncated = self._read_new_log_content()

            if file_truncated:
                self._content = ""

            if not new_content and not file_truncated:
                return

            self._content += new_content
            log_text.update(self._content)

            # Auto-scroll to bottom if we're near the bottom
            if is_near_bottom:
                log_content.scroll_end()

        except Exception as e:
            error_content = f"Error reading log file: {e}"
            if self._content == error_content:
                return

            self._content = error_content
            self.last_position = 0

            try:
                log_text = self.query_one("#log-text", Static)
            except Exception:
                return

            log_text.update(error_content)

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
            with VerticalScroll(id="log-content"):
                yield Static("", id="log-text")
