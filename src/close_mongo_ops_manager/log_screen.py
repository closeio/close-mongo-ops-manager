from textual.binding import Binding
from textual.app import ComposeResult
from textual.containers import (
    Container,
    VerticalScroll,
)
from textual.screen import ModalScreen
from textual.widgets import Footer, Static


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

    async def on_mount(self) -> None:
        """Load log file asynchronously."""
        try:
            with open(self.log_file) as f:
                content = f.read()
            log_content = self.query_one("#log-content")
            log_content.mount(Static(content))
        except Exception as e:
            log_content = self.query_one("#log-content")
            log_content.mount(Static(f"Error reading log file: {e}"))

    def compose(self) -> ComposeResult:
        yield Footer()
        container = Container(id="log-container")
        container.border_title = "Application Logs"
        container.border_subtitle = "ESCAPE to dismiss"

        with container:
            # We'll use the VerticalScroll widget with an ID for the content
            scroll = VerticalScroll(id="log-content")
            yield scroll
