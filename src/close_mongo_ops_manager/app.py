from __future__ import annotations

from dataclasses import dataclass
import asyncio
import logging
import os
import argparse
import sys
import time
from typing import Any
from collections.abc import Mapping
from urllib.parse import quote_plus

from textual import work
from textual.binding import Binding
from textual.app import App, ComposeResult
from textual.containers import Container, Horizontal, VerticalScroll
from textual.message import Message
from textual.reactive import reactive
from textual.screen import ModalScreen
from textual.widgets import Button, DataTable, Footer, Header, Input, Static
from textual.coordinate import Coordinate

from pymongo import AsyncMongoClient
from pymongo.asynchronous.database import AsyncDatabase
from pymongo.errors import PyMongoError
from pymongo.uri_parser import parse_uri


# Constants
LOG_FILE = "mongo_ops_manager.log"
MIN_REFRESH_INTERVAL = 1
MAX_REFRESH_INTERVAL = 10
DEFAULT_REFRESH_INTERVAL = 5
STEP_REFRESH_INTERVAL = 1  # Interval change step


# Set up logging
def setup_logging() -> logging.Logger:
    logger = logging.getLogger("mongo_ops_manager")
    logger.setLevel(logging.INFO)

    formatter = logging.Formatter("%(asctime)s (%(levelname)s): %(message)s")

    fh = logging.FileHandler(LOG_FILE, mode="w")
    fh.setFormatter(formatter)
    logger.addHandler(fh)

    return logger


logger = setup_logging()


# Custom exceptions
class MongoOpsError(Exception):
    """Base exception for Close MongoDB Operations Manager."""

    pass


class ConnectionError(MongoOpsError):
    """Exception raised for connection-related errors."""

    pass


class OperationError(MongoOpsError):
    """Exception raised for operation-related errors."""

    pass


# Message classes for better component communication
@dataclass
class FilterChanged(Message):
    """Filter criteria changed."""

    filters: dict[str, str]


@dataclass
class OperationsLoaded(Message):
    """Event emitted when operations are fully loaded."""

    count: int
    duration: float


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
        self.operations = operations
        super().__init__()

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


class FilterBar(Container):
    """Container for filter inputs."""

    BORDER_TITLE = "Filters"

    BORDER_SUBTITLE = "Filter operations by criteria"

    DEFAULT_CSS = """
    FilterBar {
        height: auto;
        layout: horizontal;
        background: $surface;
        border: solid $primary;
        padding: 1;
        margin: 0 1;
        width: 100%;
    }

    .filter-input {
        margin: 0 1;
        width: 1fr;
        border: solid $primary;
    }

    #clear-filters {
        margin: 0 1;
        width: auto;
        background: $primary;

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


class OperationsView(DataTable):
    """Table displaying MongoDB operations."""

    BORDER_TITLE = "Operations"

    DEFAULT_CSS = """
    OperationsView {
        height: 100%;
        margin: 0 1;
        border: solid $primary;
        width: 100%;
    }

    DataTable {
        height: auto;
    }
    """

    BINDINGS = [
        Binding("enter,space", "select_cursor", "Select", show=False),
    ]

    def __init__(self) -> None:
        super().__init__()
        self.cursor_type = "row"
        self.zebra_stripes = True
        self.filters: dict[str, str] = {}
        self.sort_running_time_asc = True
        self.selected_ops: set[str] = set()
        self.can_focus = True

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

    def clear_selections(self) -> None:
        self.selected_ops.clear()
        for idx, key in enumerate(self.rows.keys()):
            coord = Coordinate(idx, 0)
            self.update_cell_at(coord, "☐")


class MongoDBManager:
    """Handles MongoDB connection and operations."""

    def __init__(self) -> None:
        self.read_client = None
        self.write_client = None
        self.admin_db: AsyncDatabase
        self.namespace: str = ""

    @classmethod
    async def connect(cls, connection_string: str, namespace: str) -> MongoDBManager:
        self = cls()
        try:
            self.namespace = namespace

            # Create client
            conn_string = f"{connection_string}?readPreference=secondaryPreferred"
            self.read_client = AsyncMongoClient(
                conn_string,
                serverSelectionTimeoutMS=5000,
                connectTimeoutMS=5000,
            )

            # Set up admin databases
            self.admin_db = self.read_client.admin

            # Verify connection
            await self.admin_db.command("ping")

            server_status = await self.admin_db.command("serverStatus")
            version = server_status.get("version", "unknown version")
            process = server_status.get("process", "unknown process")
            logger.info(f"Connected to MongoDB {version} ({process})")

            return self
        except PyMongoError as e:
            raise ConnectionError(f"Failed to connect to MongoDB: {e}")

    async def get_operations(self, filters: dict[str, str] | None = None) -> list[dict]:
        """Get current operations with appropriate handling"""
        try:
            # Base currentOp arguments
            current_op_args = {
                "allUsers": True,
                "idleConnections": False,
                "idleCursors": False,
                "idleSessions": True,
                "localOps": False,
                "backtrace": False,
            }

            pipeline = [{"$currentOp": current_op_args}]

            if filters or self.namespace:
                match_stage: Mapping[str, Any] = {"$and": []}

                if self.namespace:
                    match_stage["$and"].append(
                        {"ns": {"$regex": f"^{self.namespace}", "$options": "i"}}
                    )

                if filters:
                    if filters.get("opid"):
                        match_stage["$and"].append(
                            {"opid": {"$regex": filters["opid"], "$options": "i"}}
                        )
                    if filters.get("operation"):
                        match_stage["$and"].append(
                            {
                                "op": {
                                    "$regex": filters["operation"],
                                    "$options": "i",
                                }
                            }
                        )
                    if filters.get("client"):
                        match_stage["$and"].append(
                            {
                                "$or": [
                                    {
                                        "client": {
                                            "$regex": filters["client"],
                                            "$options": "i",
                                        }
                                    },
                                    {
                                        "client_s": {
                                            "$regex": filters["client"],
                                            "$options": "i",
                                        }
                                    },
                                ]
                            }
                        )
                    if filters.get("description"):
                        match_stage["$and"].append(
                            {
                                "desc": {
                                    "$regex": filters["description"],
                                    "$options": "i",
                                }
                            }
                        )
                    if filters.get("effective_users"):
                        match_stage["$and"].append(
                            {
                                "effectiveUsers": {
                                    "$elemMatch": {
                                        "user": {
                                            "$regex": filters["effective_users"],
                                            "$options": "i",
                                        }
                                    }
                                }
                            }
                        )
                    if (
                        filters.get("running_time")
                        and filters["running_time"].isdigit()
                    ):
                        match_stage["$and"].append(
                            {"secs_running": {"$gte": int(filters["running_time"])}}
                        )

                match_stage["$and"].append({"active": True})

                if match_stage["$and"]:
                    pipeline.append({"$match": match_stage})

            cursor = await self.admin_db.aggregate(pipeline)
            inprog = await cursor.to_list(None)

            return inprog
        except PyMongoError as e:
            raise OperationError(f"Failed to get operations: {e}")

    async def kill_operation(
        self, opid: str, max_retries: int = 3, verify_timeout: float = 5.0
    ) -> bool:
        """Kill a MongoDB operation with retries and verification."""
        try:
            # Convert string opid to numeric if possible (for non-sharded operations)
            numeric_opid = None
            if isinstance(opid, str) and ":" not in opid:
                try:
                    numeric_opid = int(opid)
                except ValueError:
                    pass

            use_opid = numeric_opid if numeric_opid is not None else opid

            # Try killing the operation with retries
            for attempt in range(max_retries):
                try:
                    # Execute killOp command
                    result = await self.admin_db.command("killOp", op=use_opid)

                    if result.get("ok") == 1:
                        # Start verification process with timeout
                        verification_start = time.monotonic()

                        while time.monotonic() - verification_start < verify_timeout:
                            # Check if operation still exists
                            current_ops = await self.get_operations()
                            operation_exists = any(
                                str(op["opid"]) == str(opid) for op in current_ops
                            )

                            if not operation_exists:
                                logger.info(
                                    f"Successfully killed and verified operation {opid}"
                                )
                                return True

                            # Brief pause before next verification check
                            await asyncio.sleep(0.5)

                        # If we reach here, operation still exists after timeout
                        logger.warning(
                            f"Operation {opid} still exists after kill attempt {attempt + 1}"
                        )

                        if attempt < max_retries - 1:
                            # Wait before retry, with exponential backoff
                            await asyncio.sleep(2**attempt)
                            continue

                except PyMongoError as e:
                    # Special handling for sharded cluster operations
                    if (
                        "TypeMismatch" in str(e)
                        and isinstance(opid, str)
                        and ":" in opid
                    ):
                        try:
                            # For sharded operations, try to extract and kill the numeric part
                            shard_id, numeric_part = opid.split(":")
                            if numeric_part.isdigit():
                                logger.info(
                                    f"Retrying kill with numeric part of sharded operation: {numeric_part}"
                                )
                                return await self.kill_operation(
                                    numeric_part,
                                    max_retries=max_retries - attempt,
                                    verify_timeout=verify_timeout,
                                )
                        except Exception as inner_e:
                            logger.error(
                                f"Error processing sharded operation ID: {inner_e}"
                            )

                    # Log the error and continue retrying if attempts remain
                    logger.error(
                        f"Attempt {attempt + 1} failed to kill operation {opid}: {e}"
                    )
                    if attempt < max_retries - 1:
                        await asyncio.sleep(2**attempt)
                        continue
                    else:
                        raise OperationError(
                            f"Failed to kill operation {opid} after {max_retries} attempts: {e}"
                        )

            # If we reach here, all attempts failed
            logger.error(
                f"Failed to kill operation {opid} after {max_retries} attempts"
            )
            return False

        except Exception as e:
            logger.error(
                f"Unexpected error killing operation {opid}: {e}", exc_info=True
            )
            raise OperationError(f"Failed to kill operation {opid}: {e}")


class StatusBar(Static):
    """Status bar widget showing current connection and refresh status."""

    DEFAULT_CSS = """
    StatusBar {
        width: 100%;
        height: 1;
        background: $boost;
        color: $text;
        content-align: left middle;
        padding: 0 1;
    }
    """

    def __init__(self) -> None:
        super().__init__()
        self._connection_status = "Connecting..."
        self._refresh_status = "Auto-refresh paused"
        self._refresh_interval = f"{DEFAULT_REFRESH_INTERVAL}s"
        self._update_text()

    def _update_text(self) -> None:
        text = f"{self._connection_status} | {self._refresh_status} ({self._refresh_interval})"
        self.update(text)

    def set_connection_status(self, connected: bool, details: str = "") -> None:
        self._connection_status = (
            f"Connected to {details}" if connected else "Disconnected"
        )
        self._update_text()

    def set_refresh_status(self, enabled: bool) -> None:
        self._refresh_status = (
            "Auto-refresh enabled" if enabled else "Auto-refresh paused"
        )
        self._update_text()

    def set_refresh_interval(self, interval: float) -> None:
        self._refresh_interval = f"{interval}s"
        self._update_text()


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
        with Container(id="help-container"):
            yield Static(
                "Close MongoDB Operations Manager Help", classes="section-title"
            )
            yield Static(
                """
Keyboard Shortcuts:
------------------
Ctrl+Q  : Quit application
Ctrl+R  : Refresh operations list
Ctrl+K  : Kill selected operations
Ctrl+P  : Pause/Resume auto-refresh
Ctrl+S  : Sort by running time
Ctrl+H  : Show this help
Ctrl+L  : View application logs
Ctrl+U  : Deselect all operations
Ctrl++  : Increase refresh interval
Ctrl+-  : Decrease refresh interval

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


class LogScreen(ModalScreen):
    """Screen for viewing application logs."""

    DEFAULT_CSS = """
    LogScreen {
        align: center middle;
    }

    #log-container {
        width: auto;
        height: 80%;
        max-width: 80%;
        max-height: 80%;
        border: round $primary;
        background: $surface;
        padding: 1 2;
        overflow-y: auto;
    }

    #log-content {
        width: auto;
        max-width: 80%;
        height: auto;
        max-height: 80%;
        padding: 1 2;
    }
    """

    def __init__(self, log_file: str) -> None:
        super().__init__()
        self.log_file = log_file

    def compose(self) -> ComposeResult:
        with Container(id="log-container"):
            with VerticalScroll(id="log-content"):
                try:
                    with open(self.log_file) as f:
                        content = f.read()
                    yield Static(content)
                except Exception as e:
                    yield Static(f"Error reading log file: {e}")

    def on_key(self, event) -> None:
        if event.key == "escape":
            self.dismiss()


class MongoOpsManager(App):
    """Main application class."""

    ENABLE_COMMAND_PALETTE = False

    TITLE = "Close MongoDB Operations Manager"

    AUTO_FOCUS = "OperationsView"

    CSS = """
    Screen {
        align: center top;
        padding: 0;
    }

    VerticalScroll {
        width: 100%;
        padding: 0;
        margin: 0;
    }

    Container {
        width: 100%;
        padding: 0;
        margin: 0;
    }
    """

    BINDINGS = [
        Binding("ctrl+q", "quit", "Quit"),
        Binding("ctrl+r", "refresh", "Refresh"),
        Binding("ctrl+k", "kill_selected", "Kill Selected"),
        Binding("ctrl+p", "toggle_refresh", "Pause/Resume"),
        Binding("ctrl+s", "sort_by_time", "Sort by Time"),
        Binding("ctrl+h", "show_help", "Help"),
        Binding("ctrl+l", "show_logs", "View Logs"),
        Binding("ctrl+u", "deselect_all", "Deselect All"),
        Binding(
            "ctrl+equals_sign",
            "increase_refresh",
            "Increase Refresh Interval",
            key_display="^+",
        ),
        Binding(
            "ctrl+minus",
            "decrease_refresh",
            "Decrease Refresh Interval",
            key_display="^-",
        ),
    ]

    auto_refresh: reactive[bool] = reactive(False)
    refresh_interval: reactive[float] = reactive(DEFAULT_REFRESH_INTERVAL)

    def __init__(
        self,
        connection_string: str,
        refresh_interval: float = DEFAULT_REFRESH_INTERVAL,
        namespace: str = "",
    ) -> None:
        super().__init__()
        self.connection_string = connection_string
        self.refresh_interval = refresh_interval
        self.mongodb: MongoDBManager | None = None
        self._refresh_task: asyncio.Task | None = None
        self.log_file = LOG_FILE
        self._status_bar: StatusBar
        self.namespace: str = namespace

    def validate_refresh_interval(self, value: float) -> float:
        """Validate refresh interval."""
        return max(MIN_REFRESH_INTERVAL, min(value, MAX_REFRESH_INTERVAL))

    def compose(self) -> ComposeResult:
        """Create child widgets for the app."""
        yield Header()
        with Container():
            yield FilterBar()
            with VerticalScroll():
                yield OperationsView()
        yield StatusBar()
        yield Footer()

    async def on_mount(self) -> None:
        self.operations_view = self.query_one(OperationsView)
        self._status_bar = self.query_one(StatusBar)
        self.operations_view.loading = True
        self._status_bar.set_refresh_interval(self.refresh_interval)
        asyncio.create_task(self._setup())

    def action_show_help(self) -> None:
        """Show the help screen."""
        self.push_screen(HelpScreen())

    def action_show_logs(self) -> None:
        """Show the log viewer screen."""
        self.push_screen(LogScreen(self.log_file))

    async def _setup(self) -> None:
        """Initialize MongoDB connection and start operation monitoring."""
        try:
            self.mongodb = await MongoDBManager.connect(
                self.connection_string, self.namespace
            )
            # Extract connection details for status bar
            parsed_uri = parse_uri(self.connection_string)

            # Safely extract host information with fallbacks
            host_info = "unknown host"
            try:
                nodelist = parsed_uri.get("nodelist")
                if nodelist and len(nodelist) > 0:
                    host, port = nodelist[0]
                    host_info = f"{host}:{port}"
                else:
                    # Fallback: try to extract from connection string directly
                    cleaned_uri = self.connection_string.split("@")[-1].split("/")[0]
                    host_info = cleaned_uri.split("?")[
                        0
                    ]  # Remove query parameters if present
            except Exception as parse_error:
                logger.warning(f"Failed to parse host details: {parse_error}")
                # Use a generic connection success message
                host_info = "MongoDB server"

            self._status_bar.set_connection_status(True, host_info)

            self.refresh_operations()
            self._refresh_task = asyncio.create_task(self.auto_refreshing())
        except Exception as e:
            logger.error(f"Setup error: {e}", exc_info=True)
            self._status_bar.set_connection_status(False)
            self.notify(f"Failed to connect: {e}", severity="error")

    def action_increase_refresh(self) -> None:
        """Increase the refresh interval."""
        new_interval = min(
            MAX_REFRESH_INTERVAL, self.refresh_interval + STEP_REFRESH_INTERVAL
        )
        if new_interval != self.refresh_interval:
            self.refresh_interval = new_interval
            self.notify(f"Refresh interval increased to {self.refresh_interval:.1f}s")
            self._status_bar.set_refresh_interval(self.refresh_interval)

    def action_decrease_refresh(self) -> None:
        """Decrease the refresh interval."""
        new_interval = max(
            MIN_REFRESH_INTERVAL, self.refresh_interval - STEP_REFRESH_INTERVAL
        )
        if new_interval != self.refresh_interval:
            self.refresh_interval = new_interval
            self.notify(f"Refresh interval decreased to {self.refresh_interval:.1f}s")
            self._status_bar.set_refresh_interval(self.refresh_interval)

    async def auto_refreshing(self) -> None:
        """Background task for auto-refreshing functionality."""
        while True:
            try:
                if self.auto_refresh:
                    self.refresh_operations()
                await asyncio.sleep(self.refresh_interval)
            except Exception as e:
                logger.error(f"Auto-refresh error: {e}", exc_info=True)
                await asyncio.sleep(self.refresh_interval)

    @work(exclusive=True)
    async def refresh_operations(self) -> None:
        """Refresh the operations table with current data."""
        if not self.mongodb:
            self.operations_view.loading = False
            return

        start_time = time.monotonic()
        try:
            self.operations_view.loading = True
            ops = await self.mongodb.get_operations(self.operations_view.filters)

            # Clear the operations table
            self.operations_view.clear()

            # Sort operations by running time if needed
            if hasattr(self.operations_view, "sort_running_time_asc"):
                ops.sort(
                    key=lambda x: float(str(x.get("secs_running", 0)).rstrip("s")),
                    reverse=not self.operations_view.sort_running_time_asc,
                )

            for op in ops:
                # Get client info
                client_info = op.get("client_s") or op.get("client", "N/A")
                client_metadata = op.get("clientMetadata", {})
                mongos_info = client_metadata.get("mongos", {})
                mongos_host = mongos_info.get("host", "")

                if mongos_host:
                    client_info = f"{client_info} ({mongos_host.split('.', 1)[0]})"

                # Get effective users
                effective_users = op.get("effectiveUsers", [])
                users_str = (
                    ", ".join(u.get("user", "") for u in effective_users)
                    if effective_users
                    else "N/A"
                )

                row = (
                    "☐",
                    str(op["opid"]),
                    op.get("type", ""),
                    op.get("op", ""),
                    f"{op.get('secs_running', 0)}s",
                    client_info,
                    op.get("desc", "N/A"),
                    users_str,
                )
                self.operations_view.add_row(*row, key=str(op["opid"]))

            # Calculate load duration and emit event
            duration = time.monotonic() - start_time
            self.operations_view.post_message(
                OperationsLoaded(count=len(ops), duration=duration)
            )

        except Exception as e:
            self.notify(f"Failed to refresh: {e}", severity="error")

        finally:
            self.operations_view.loading = False

    def action_refresh(self) -> None:
        """Handle refresh action."""
        self.refresh_operations()

    def action_toggle_refresh(self) -> None:
        """Toggle auto-refresh."""
        self.auto_refresh = not self.auto_refresh
        self._status_bar.set_refresh_status(self.auto_refresh)
        status = "enabled" if self.auto_refresh else "paused"
        self.notify(f"Auto-refresh {status}")

    def action_deselect_all(self) -> None:
        """Deselect all selected operations."""
        if not self.operations_view.selected_ops:
            return

        # Remember selected ops before clearing
        count = len(self.operations_view.selected_ops)

        # Clear the selected operations set
        self.operations_view.selected_ops.clear()

        self.refresh_operations()

        # Show notification
        self.notify(f"Deselected {count} operations")

    # FIXME: When refreshing the table after killing an operation
    # the selected row is keep selected and the checkbox is not unchecked.
    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        """Handle row selection."""
        try:
            # Get the row key value directly
            row_key = str(event.row_key.value)
            coord = Coordinate(event.cursor_row, 0)  # Get checkbox cell coordinate

            if row_key in self.operations_view.selected_ops:
                self.operations_view.selected_ops.remove(row_key)
                self.operations_view.update_cell_at(coord, "☐")
            else:
                self.operations_view.selected_ops.add(row_key)
                self.operations_view.update_cell_at(coord, "☒")

        except Exception as e:
            logger.error(f"Error handling row selection: {e}", exc_info=True)
            self.notify("Error selecting row", severity="error")

    async def action_kill_selected(self) -> None:
        """Kill selected operations with confirmation."""
        if not self.operations_view.selected_ops:
            self.notify("No operations selected")
            return

        async def handle_confirmation(confirmed: bool | None) -> None:
            if not confirmed or not self.mongodb:
                return

            # Get operation details before killing
            current_ops = await self.mongodb.get_operations()
            selected_ops = [
                op
                for op in current_ops
                if str(op["opid"]) in self.operations_view.selected_ops
            ]

            for op in selected_ops:
                command = op.get("command", {})
                query_info = {
                    "find": command.get("find"),
                    "filter": command.get("filter"),
                    "ns": op.get("ns"),
                    "client": op.get("client"),
                }
                logger.info(
                    f"Preparing to kill operation {op['opid']}. Query details: {query_info}"
                )

            success_count = 0
            error_count = 0

            for opid in list(self.operations_view.selected_ops):
                try:
                    if await self.mongodb.kill_operation(opid):
                        success_count += 1
                    else:
                        error_count += 1
                        logger.error(
                            f"Failed to kill operation {opid}: Operation not found"
                        )
                except Exception as e:
                    error_count += 1
                    self.notify(
                        f"Failed to kill operation {opid}: {str(e)}", severity="error"
                    )
                    logger.error(f"Failed to kill operation {opid}: {e}", exc_info=True)

            # Clear selections after all operations are processed
            self.operations_view.clear_selections()

            # Refresh the view after a brief delay to allow operations to be killed
            await asyncio.sleep(1.0)
            self.refresh_operations()

            # Show summary
            if success_count > 0:
                self.notify(
                    f"Successfully killed {success_count} operation(s)",
                    severity="information",
                )
            if error_count > 0:
                self.notify(
                    f"Failed to kill {error_count} operation(s)", severity="error"
                )

        await self.push_screen(
            KillConfirmation(list(self.operations_view.selected_ops)),
            callback=handle_confirmation,
        )

    async def on_filter_changed(self, event: FilterChanged) -> None:
        """Handle filter changes."""
        self.operations_view.filters = event.filters
        self.refresh_operations()

    def action_sort_by_time(self) -> None:
        """Sort operations by running time."""
        self.operations_view.sort_running_time_asc = not getattr(
            self.operations_view, "sort_running_time_asc", True
        )
        direction = (
            "ascending" if self.operations_view.sort_running_time_asc else "descending"
        )
        self.notify(f"Sorted by running time ({direction})")
        self.refresh_operations()

    def on_operations_loaded(self, event: OperationsLoaded) -> None:
        """Handle operations loaded event."""
        logger.info(f"Loaded {event.count} operations in {event.duration:.2f} seconds")
        if event.count > 0:
            # Ensure focus is on the operations view after loading
            self.operations_view.focus()


def main() -> None:
    parser = argparse.ArgumentParser(description="Close MongoDB Operations Manager")
    parser.add_argument(
        "--host",
        default=os.environ.get("MONGODB_HOST", "localhost"),
        type=str,
        help="MongoDB host",
    )
    parser.add_argument(
        "--port",
        default=os.environ.get("MONGODB_PORT", "27017"),
        type=str,
        help="MongoDB port",
    )
    parser.add_argument(
        "--username",
        default=os.environ.get("MONGODB_USERNAME"),
        type=str,
        help="MongoDB username",
    )
    parser.add_argument(
        "--password",
        default=os.environ.get("MONGODB_PASSWORD"),
        type=str,
        help="MongoDB password",
    )
    parser.add_argument(
        "--namespace", help="MongoDB namespace to monitor", type=str, default=".*"
    )
    parser.add_argument(
        "--refresh-interval",
        type=int,
        default=int(
            os.environ.get("MONGODB_REFRESH_INTERVAL", str(DEFAULT_REFRESH_INTERVAL))
        ),
        help=f"Refresh interval in seconds (min: {MIN_REFRESH_INTERVAL}, max: {MAX_REFRESH_INTERVAL})",
    )

    args = parser.parse_args()

    # Build connection string
    username = args.username or os.environ.get("MONGODB_USERNAME")
    password = args.password or os.environ.get("MONGODB_PASSWORD")
    host = args.host or os.environ.get("MONGODB_HOST", "localhost")
    port = args.port or os.environ.get("MONGODB_PORT", "27017")

    try:
        # Build connection string based on authentication settings
        if username and password:
            # Use authenticated connection
            username = quote_plus(username)
            password = quote_plus(password)
            connection_string = f"mongodb://{username}:{password}@{host}:{port}/"
        else:
            # Use unauthenticated connection
            connection_string = f"mongodb://{host}:{port}/"
            logger.info("Using unauthenticated connection")

        # Validate refresh interval
        refresh_interval = max(
            MIN_REFRESH_INTERVAL, min(args.refresh_interval, MAX_REFRESH_INTERVAL)
        )
        if refresh_interval != args.refresh_interval:
            if args.refresh_interval < MIN_REFRESH_INTERVAL:
                logger.warning(
                    f"Refresh interval too low, setting to minimum ({MIN_REFRESH_INTERVAL} seconds)"
                )
            else:
                logger.warning(
                    f"Refresh interval too high, setting to maximum ({MAX_REFRESH_INTERVAL} seconds)"
                )

        # Start the application
        app = MongoOpsManager(
            connection_string=connection_string,
            refresh_interval=refresh_interval,
            namespace=args.namespace,
        )
        app.run()

    except Exception as e:
        logger.error(f"Startup error: {e}", exc_info=True)
        print(f"\nError: {e}")
        print(f"Please check {LOG_FILE} for details")
        sys.exit(1)


if __name__ == "__main__":
    main()
