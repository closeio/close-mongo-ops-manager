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
        width: auto;
        height: auto;
        padding: 1;
    }

    #dialog {
        background: $surface;
        border: thick $error;
        width: auto;
        max-width: 50;
        height: auto;
        max-height: 10;
        padding: 1;
    }

    #question {
        padding: 1;
        text-align: center;
        width: auto;
    }

    #button-container {
        width: 100%;
        align: center middle;
        padding-top: 1;
    }

    Button {
        width: 8;
        margin: 0 2;
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
        self.query_one("#no")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "yes":
            self.dismiss(True)
        else:
            self.dismiss(False)

    def on_key(self, event) -> None:
        if event.key == "escape":
            self.dismiss(False)
        elif event.key == "enter" and self.query_one("#yes").has_focus:
            self.dismiss(True)
