from textual.binding import Binding
from textual.app import ComposeResult
from textual.containers import (
    VerticalScroll,
)
from textual.screen import ModalScreen
from textual.widgets import TextArea
from textual.containers import ScrollableContainer


class OperationDetailsScreen(ModalScreen):
    """Screen for viewing detailed operation information."""

    BORDER_TITLE = "Operation Details"

    DEFAULT_CSS = """
    OperationDetailsScreen {
        align: center middle;
    }

    #details-container {
        width: 80%;
        height: 80%;
        max-width: 80%;
        max-height: 80%;
        border: round $primary;
        background: $surface;
    }

    #details-content {
        width: 100%;
        height: auto;
        padding: 1;
    }

    .details-text {
        width: 100%;
        height: auto;
        padding: 0 1;
    }
    """

    BINDINGS = [
        Binding("escape", "dismiss", "Close", show=False),
    ]

    def __init__(self, operation: dict) -> None:
        super().__init__()
        self.operation = operation

    def compose(self) -> ComposeResult:
        with ScrollableContainer(id="details-container"):
            with VerticalScroll(id="details-content"):
                # Format operation details
                details = []
                details.append(f"Operation ID: {self.operation.get('opid', 'N/A')}")
                details.append(f"Type: {self.operation.get('op', 'N/A')}")
                details.append(f"Namespace: {self.operation.get('ns', 'N/A')}")
                details.append(
                    f"Running Time: {self.operation.get('secs_running', 0)}s"
                )
                # Get client info with fallbacks
                client_info = (
                    self.operation.get("client_s")
                    or self.operation.get("client")
                    or "N/A"
                )

                # Add mongos host info if available
                mongos_host = (
                    self.operation.get("clientMetadata", {})
                    .get("mongos", {})
                    .get("host", "")
                )
                if mongos_host:
                    # Extract first part of hostname for brevity
                    short_host = mongos_host.split(".", 1)[0]
                    client_info = f"{client_info} ({short_host})"

                details.append(f"Client: {client_info}")

                # Format command details
                command = self.operation.get("command", {})
                if command:
                    details.append("\nCommand Details:")
                    for key, value in command.items():
                        details.append(f"  {key}: {value}")

                # Format plan summary if available
                plan_summary = self.operation.get("planSummary", "")
                if plan_summary:
                    details.append(f"\nPlan Summary: {plan_summary}")

                # Join all details with newlines
                yield TextArea(
                    "\n".join(details), classes="details-text", read_only=True
                )
