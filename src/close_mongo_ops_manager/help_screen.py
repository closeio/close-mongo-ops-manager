from textual.binding import Binding
from textual.app import ComposeResult
from textual.containers import (
    VerticalScroll,
    Center,
)
from textual.screen import ModalScreen
from textual.widgets import Footer, Static


class HelpScreen(ModalScreen):
    """Help screen showing keyboard shortcuts and usage information."""

    DEFAULT_CSS = """
    HelpScreen {
        align: center middle;
    }

    #help-container {
        width: 50%;
        height: auto;
        max-width: 50%;
        max-height: 80%;
        border: round $primary;
        background: $surface;
        padding: 1;
        overflow-y: auto;
    }

    .help-content {
        width: 50%;
        height: auto;
        padding: 1;
    }

    .section-title {
        text-style: bold;
        color: $text;
        padding: 1;
    }
    """

    BINDINGS = [
        Binding("escape", "dismiss", "Help", show=False),
    ]

    def compose(self) -> ComposeResult:
        yield Footer()
        with VerticalScroll(id="help-container") as vertical_scroll:
            with Center():
                yield Static(
                    "Close MongoDB Operations Manager", classes="section-title"
                )
                yield Static(
                    """
    Keyboard Shortcuts:
    ------------------
    f1      : Show this help
    Ctrl+Q  : Quit application
    Ctrl+R  : Refresh operations list
    Ctrl+K  : Kill selected operations
    Ctrl+P  : Pause/Resume auto-refresh
    Ctrl+S  : Sort by running time
    Ctrl+L  : View application logs
    Ctrl+U  : Deselect all operations
    Ctrl+A  : Select all operations
    Ctrl++  : Increase refresh interval
    Ctrl+-  : Decrease refresh interval
    Enter   : See operation details
    Space   : Select operations

    Usage:
    ------
    - Use arrow keys or mouse to navigate
    - Space/Click to select operations
    - Filter operations using the input fields
    - Clear filters with the Clear button
    - Confirm kill operations when prompted
                """,
                    classes="help-content",
                )
        vertical_scroll.border_title = "Help"
        vertical_scroll.border_subtitle = "ESCAPE to dismiss"
