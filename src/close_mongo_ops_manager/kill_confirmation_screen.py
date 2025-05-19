from textual.app import ComposeResult
from textual.containers import (
    Container,
    Horizontal,
)
from textual.screen import ModalScreen
from textual.widgets import Button, Static


class KillConfirmation(ModalScreen[bool]):
    """Modal screen for kill operation confirmation."""

    AUTO_FOCUS = "#no"

    DEFAULT_CSS = """
    KillConfirmation {
        align: center middle;
        width: 100%;
        height: 100%;
        padding: 0;
    }

    #dialog {
        background: $surface;
        border: thick $error;
        width: auto;
        min-width: 40;
        max-width: 60;
        height: auto;
        min-height: 6;
        max-height: 10;
        padding: 1;
        align: center middle;
    }

    #question {
        padding: 0;
        text-align: center;
        width: auto;
    }

    #button-container {
        width: 100%;
        align: center middle;
        padding-top: 1;
        margin-top: 1;
    }

    Button {
        margin: 0 2;
        min-width: 10;
    }

    #yes {
        background: $error;
    }

    #no {
        background: $primary;
    }
    """

    def __init__(self, operations: list[str]) -> None:
        super().__init__()
        self.operations = operations

    def compose(self) -> ComposeResult:
        count = len(self.operations)
        op_text = "operation" if count == 1 else "operations"

        with Container(id="dialog"):
            yield Static(
                f"Are you sure you want to kill {count} {op_text}?", id="question"
            )
            with Horizontal(id="button-container"):
                yield Button("Yes", variant="error", id="yes")
                yield Button("No", variant="primary", id="no", classes="button")

    def on_mount(self) -> None:
        """Set up the dialog when mounted."""
        # Focus the No button by default (safer option)
        no_button = self.query_one("#no", Button)
        no_button.focus()

    def on_button_pressed(self, event: Button.Pressed) -> None:
        self.dismiss(event.button.id == "yes")

    def on_key(self, event) -> None:
        try:
            match event.key:
                case "escape":
                    self.dismiss(False)
                case "enter" if self.query_one("#yes").has_focus:
                    self.dismiss(True)
        except Exception as e:
            # Log the error but still try to dismiss the screen
            self.app.notify(f"Error processing key: {e}", severity="error")
            self.dismiss(False)
