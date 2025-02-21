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
            for input in self.query(".filter-input"):
                if isinstance(input, Input):
                    input.value = ""
            self.post_message(FilterChanged({}))

    def on_input_changed(self, event: Input.Changed) -> None:
        filters = {}
        for input in self.query(".filter-input"):
            if isinstance(input, Input) and input.value:
                filter_key = input.id.replace("filter-", "").replace("-", "_")  # type: ignore
                filters[filter_key] = input.value
        self.post_message(FilterChanged(filters))
