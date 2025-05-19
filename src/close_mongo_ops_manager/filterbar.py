from textual.app import ComposeResult
from textual.containers import (
    Horizontal,
)
from textual.widgets import Button, Input

from close_mongo_ops_manager.messages import FilterChanged


class FilterBar(Horizontal):
    """Container for filter inputs."""

    BORDER_TITLE = "Filters"

    BORDER_SUBTITLE = "Filter operations by criteria"

    # Define key bindings - will be used when the filter bar itself has focus
    BINDINGS = [
        ("ctrl+f", "toggle_filter_bar", "Toggle Filters"),
    ]

    DEFAULT_CSS = """
    FilterBar {
        height: auto;
        background: $surface;
        border: solid $primary;
        width: 100%;
    }

    .filter-input {
        height: auto;
        width: 10fr;
        border: tall $primary;
    }

    #clear-filters {
        padding: 0 2;
        width: auto;
        background: $primary;
        height: auto;
        &:hover {
            background: $primary-darken-2;
        }
    }
    """

    def compose(self) -> ComposeResult:
        yield Input(placeholder="OpId", id="filter-opid", classes="filter-input")
        yield Input(
            placeholder="Operation", id="filter-operation", classes="filter-input"
        )
        yield Input(
            placeholder="Running Time ≥ sec",
            id="filter-running-time",
            classes="filter-input",
        )
        yield Input(placeholder="Client", id="filter-client", classes="filter-input")
        yield Input(
            placeholder="Description", id="filter-description", classes="filter-input"
        )
        yield Input(
            placeholder="Effective Users",
            id="filter-effective-users",
            classes="filter-input",
        )
        yield Button("Clear", id="clear-filters")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "clear-filters":
            # Find the first input
            first_input = None
            inputs = self.query(".filter-input")
            if inputs:
                first_input = inputs.first()

            # Clear all inputs
            for input in inputs:
                if isinstance(input, Input):
                    input.value = ""

            # Send filter change message
            self.post_message(FilterChanged({}))

            # Focus the first input after clearing
            if first_input:
                self.call_after_refresh(first_input.focus)

    def on_input_changed(self, event: Input.Changed) -> None:
        # Get the currently focused input
        current_input = event.input

        # Collect all non-empty filters
        filters = {}
        for input in self.query(".filter-input"):
            if isinstance(input, Input) and input.value:
                filter_key = input.id.replace("filter-", "").replace("-", "_")  # type: ignore
                filters[filter_key] = input.value

        # Send the filter changed message with current filters
        self.post_message(FilterChanged(filters))

        # Make sure the current input keeps focus after any refresh operations
        # Use call_after_refresh to ensure it happens after any screen updates
        self.call_after_refresh(current_input.focus)

    def action_toggle_filter_bar(self) -> None:
        """Delegate to the parent app to toggle the filter bar."""
        # The action gets bubbled up to the parent app which has the same action name
        self.app.action_toggle_filter_bar()  # type: ignore # Pylance doesn't recognize the dynamic method

    def on_key(self, event) -> None:
        """Handle key events to make sure Ctrl+F works to toggle the filter bar."""
        # Check if Ctrl+F is pressed
        if event.key == "ctrl+f":
            # Prevent the key event from being propagated
            event.prevent_default()
            event.stop()
            # Call the toggle action on the app (our parent)
            self.app.action_toggle_filter_bar()  # type: ignore # Pylance doesn't recognize the dynamic method

    def on_mount(self) -> None:
        """Setup event handling when the widget is mounted."""
        # Capture key events from children (including inputs)
        self.capture_keys = True
