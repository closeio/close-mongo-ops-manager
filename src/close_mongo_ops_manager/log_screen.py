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
        width: 80%;
        height: 80%;
        padding: 1;
    }
    """

    BINDINGS = [
        Binding("escape", "dismiss", "Log Screen", show=False),
    ]

    def __init__(self, log_file: str) -> None:
        super().__init__()
        self.log_file = log_file

    def compose(self) -> ComposeResult:
        yield Footer()
        with Container(id="log-container"):
            with VerticalScroll(id="log-content") as vertical_scroll:
                try:
                    with open(self.log_file) as f:
                        content = f.read()
                    yield Static(content)
                except Exception as e:
                    yield Static(f"Error reading log file: {e}")

            vertical_scroll.border_title = "Application Logs"
            vertical_scroll.border_subtitle = "ESCAPE to dismiss"
