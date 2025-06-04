from textual.binding import Binding
from textual.widgets import DataTable
from textual.coordinate import Coordinate

from close_mongo_ops_manager.messages import SelectionChanged
from close_mongo_ops_manager.operation_details_screen import OperationDetailsScreen


class OperationsView(DataTable):
    """Table displaying MongoDB operations."""

    BORDER_TITLE = "Operations"

    BORDER_SUBTITLE = "View and manage MongoDB operations"

    DEFAULT_CSS = """
    OperationsView {
        height: 100%;
        margin: 0;
        border: solid $primary;
        width: 100%;
    }
    """

    BINDINGS = [
        Binding("space", "select_cursor", "Select", show=False),
    ]

    def __init__(self) -> None:
        self._loading = False  # Initialize before calling super()
        super().__init__()
        self.cursor_type = "row"
        self.zebra_stripes = True
        self.filters: dict[str, str] = {}
        self.sort_running_time_asc = True
        self.selected_ops: set[str] = set()
        self.can_focus = True
        self.current_ops: list[dict] = []

    @property
    def loading(self) -> bool:
        return self._loading

    @loading.setter
    def loading(self, value: bool) -> None:
        self._loading = value
        # Update border title to show loading state
        if value:
            self.border_title = "Operations • Refreshing..."
        else:
            self.border_title = "Operations"

    def on_mount(self) -> None:
        self.add_columns(
            "Select",
            "OpId",
            "Type",
            "Operation",
            "Running Time",
            "Client",
            "Description",
            "Effective Users",
        )
        # Explicitly focus this widget on mount
        self.focus()

    def clear_selections(self) -> None:
        self.selected_ops.clear()
        for idx, key in enumerate(self.rows.keys()):
            coord = Coordinate(idx, 0)
            self.update_cell_at(coord, " ")
        # Emit message about selection change
        self.post_message(SelectionChanged(count=0))

    def on_key(self, event) -> None:
        if event.key == "enter":
            # Get the current row's operation data
            if (
                self.cursor_row is not None
                and self.current_ops
                and 0 <= self.cursor_row < len(self.current_ops)
            ):
                op = self.current_ops[self.cursor_row]
                self.show_operation_details(op)

    def show_operation_details(self, op: dict) -> None:
        """Show detailed view of the operation."""
        self.app.push_screen(OperationDetailsScreen(op))
