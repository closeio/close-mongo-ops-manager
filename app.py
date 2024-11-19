#!/usr/bin/env python3
import re
from typing import Any
from dataclasses import dataclass
from datetime import datetime, timedelta
import asyncio
from urllib.parse import quote_plus
from collections.abc import Mapping

from textual.app import App, ComposeResult
from textual.containers import Container, Horizontal
from textual.widgets import DataTable, Footer, Button, Static, Input, Header
from textual.binding import Binding
from textual.reactive import reactive
from textual import work
from textual.screen import ModalScreen
from textual.coordinate import Coordinate
from rich.text import Text

from pymongo import AsyncMongoClient
from pymongo.errors import PyMongoError
from pymongo.asynchronous.database import AsyncDatabase
import pymongo.uri_parser
import logging
import sys
import time
import argparse

LOG_FILE = "mongo_ops_manager.log"


# Set up logging
def setup_logging() -> logging.Logger:
    logger = logging.getLogger()
    logger.setLevel(logging.INFO)

    formatter = logging.Formatter("%(asctime)s (%(levelname)s): %(message)s")

    fh = logging.FileHandler(LOG_FILE)
    fh.setFormatter(formatter)
    logger.addHandler(fh)
    return logger


logger = setup_logging()


@dataclass
class MongoOperation:
    """Represents a MongoDB operation."""

    opid: int | str
    type: str
    host: str
    desc: str
    client: str | None
    op: str
    ns: str
    secs_running: int
    microsecs_running: int
    active: bool
    command: dict[str, Any]
    effective_users: list[dict[str, str]]


class MongoConnectionError(Exception):
    """Custom exception for MongoDB connection errors."""

    pass


class MongoDBConnection:
    """Handles MongoDB connection and operations."""

    def __init__(self) -> None:
        self.read_client = None
        self.write_client = None
        self.connection_string = ""
        self.read_admin_db: AsyncDatabase
        self.write_admin_db: AsyncDatabase

    @classmethod
    async def create(cls, connection_string: str) -> "MongoDBConnection":
        """Create a MongoDBConnection instance."""
        instance = cls()
        instance.connection_string = connection_string
        try:
            # Create read client with secondary preference
            read_conn_string = f"{connection_string}?readPreference=secondary"
            instance.read_client = AsyncMongoClient(
                read_conn_string,
                serverSelectionTimeoutMS=5000,
                connectTimeoutMS=5000,
            )

            # Create write client that targets primary
            write_conn_string = f"{connection_string}?readPreference=primary"
            instance.write_client = AsyncMongoClient(
                write_conn_string,
                serverSelectionTimeoutMS=5000,
                connectTimeoutMS=5000,
            )

            # Verify connections
            instance.read_admin_db = instance.read_client.admin
            instance.write_admin_db = instance.write_client.admin

            await instance.read_admin_db.command("ping")
            await instance.write_admin_db.command("ping")

            # Log connection success
            parsed_uri = pymongo.uri_parser.parse_uri(connection_string)
            auth_str = (
                "authenticated" if parsed_uri.get("username") else "unauthenticated"
            )
            logger.info(
                f"Successfully connected to MongoDB ({auth_str}) at {parsed_uri['nodelist'][0][0]}:{parsed_uri['nodelist'][0][1]}"
            )
            return instance

        except PyMongoError as e:
            error_msg = str(e)
            if "Authentication failed" in error_msg:
                raise MongoConnectionError(
                    "Authentication failed. Please check your username and password."
                )
            elif "Connection refused" in error_msg:
                raise MongoConnectionError(
                    "Could not connect to MongoDB server. Please verify the host and port are correct and the server is running."
                )
            elif "timed out" in error_msg:
                raise MongoConnectionError(
                    "Connection timed out. Please check your network connection and MongoDB server status."
                )
            else:
                raise MongoConnectionError(f"Failed to connect to MongoDB: {error_msg}")

    async def is_mongos(self) -> bool:
        """Check if connected to a mongos instance using read client."""
        try:
            hello = await self.read_admin_db.command("hello")
            return "isdbgrid" in hello.get("msg", "")
        except PyMongoError:
            return False

    async def get_current_ops(self, filters=None) -> list[MongoOperation]:
        """Retrieve current operations based on deployment type and filters."""
        try:
            # Prepare base currentOp command
            current_op_args = {
                "allUsers": True,
                "idleConnections": False,
                "idleCursors": False,
                "idleSessions": True,
                "localOps": False,
                "backtrace": False,
            }

            if self.is_mongos():
                # For mongos, use aggregate with $currentOp
                pipeline = [{"$currentOp": current_op_args}]

                # Add filters if present
                if filters:
                    match_stage: Mapping[str, Any] = {"$and": []}

                    if filters.get("opid"):
                        match_stage["$and"].append(
                            {"opid": {"$regex": filters["opid"], "$options": "i"}}
                        )
                    if filters.get("operation"):
                        match_stage["$and"].append(
                            {"op": {"$regex": filters["operation"], "$options": "i"}}
                        )
                    if filters.get("namespace"):
                        match_stage["$and"].append(
                            {"ns": {"$regex": filters["namespace"], "$options": "i"}}
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
                        match_stage["$and"].append({
                            "effectiveUsers": {
                                "$elemMatch": {
                                    "user": {"$regex": filters["effective_users"], "$options": "i"},
                                }
                            }
                        })
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

                cursor = await self.read_admin_db.aggregate(pipeline)
                inprog = await cursor.to_list()
            else:
                # For mongod, use currentOp command directly
                result = await self.read_admin_db.command("currentOp", current_op_args)
                inprog = result.get("inprog", [])

                # Apply filters manually for mongod
                if filters:
                    filtered_inprog = []
                    for op in inprog:
                        matches_all = True
                        if filters.get("opid"):
                            matches_all &= bool(
                                re.search(
                                    filters["opid"],
                                    str(op.get("opid", "")),
                                    re.IGNORECASE,
                                )
                            )
                        if filters.get("operation"):
                            matches_all &= bool(
                                re.search(
                                    filters["operation"],
                                    op.get("op", ""),
                                    re.IGNORECASE,
                                )
                            )
                        if filters.get("namespace"):
                            matches_all &= bool(
                                re.search(
                                    filters["namespace"],
                                    op.get("ns", ""),
                                    re.IGNORECASE,
                                )
                            )
                        if filters.get("description"):
                            matches_all &= bool(
                                re.search(
                                    filters["description"],
                                    op.get("desc", ""),
                                    re.IGNORECASE,
                                )
                            )
                        if filters.get("effective_users"):
                            matches_all &= bool(
                                re.search(
                                    filters["effective_users"],
                                    op.get("effective_users", ""),
                                    re.IGNORECASE,
                                )
                            )
                        if filters.get("client"):
                            matches_all &= any(
                                re.search(
                                    filters["client"], str(client_value), re.IGNORECASE
                                )
                                for client_value in [
                                    op.get("client"),
                                    op.get("client_s", ""),
                                ]
                            )
                        if (
                            filters.get("running_time")
                            and filters["running_time"].isdigit()
                        ):
                            secs_running = op.get("secs_running", 0)
                            matches_all &= isinstance(
                                secs_running, (int, float)
                            ) and secs_running >= int(filters["running_time"])

                        if matches_all:
                            filtered_inprog.append(op)

                    inprog = filtered_inprog

            # Filter out system operations
            ops = []
            for op in inprog:
                ops.append(
                    MongoOperation(
                        opid=op.get("opid"),
                        type=op.get("type", ""),
                        host=op.get("host", ""),
                        desc=op.get("desc", ""),
                        client=op.get("client", op.get("client_s", "")),
                        op=op.get("op", ""),
                        ns=op.get("ns", ""),
                        secs_running=op.get("secs_running", 0),
                        microsecs_running=op.get("microsecs_running", 0),
                        active=op.get("active", True),
                        command=op.get("command", {}),
                        effective_users=op.get("effectiveUsers", []),
                    )
                )
            return ops

        except PyMongoError as e:
            logger.error(f"Error getting current operations: {e}")
            raise

    async def kill_operation(self, opid: int | str) -> bool:
        """Kill a specific operation using killOp command."""
        try:
            # Convert string opid to numeric if possible (for non-sharded operations)
            numeric_opid = None
            if isinstance(opid, str) and ":" not in opid:
                try:
                    numeric_opid = int(opid)
                except ValueError:
                    pass

            # Use the admin database to run killOp command
            use_opid = numeric_opid if numeric_opid is not None else opid
            result = await self.write_admin_db.command("killOp", op=use_opid)

            # Check if the operation was killed successfully
            if result.get("ok") == 1:
                # Verify the operation was killed by checking if it still exists
                time.sleep(2.0)  # Wait briefly for kill to take effect

                # Check if operation still exists
                current_ops = await self.get_current_ops()
                for op in current_ops:
                    if str(op.opid) == str(opid):
                        logger.warning(
                            f"Operation {opid} still exists after kill attempt"
                        )
                        return False

                logger.info(f"Successfully killed operation {opid}")
                return True
            else:
                logger.error(
                    f"Failed to kill operation {opid}: {result.get('errmsg', "")}"
                )

            return False

        except PyMongoError as e:
            logger.error(f"Error killing MongoDB operation {opid}: {e}")
            # Special handling for sharded cluster operations
            if "TypeMismatch" in str(e) and isinstance(opid, str) and ":" in opid:
                try:
                    # For sharded operations, try to extract and kill the numeric part
                    shard_id, numeric_part = opid.split(":")
                    if numeric_part.isdigit():
                        logger.info(
                            f"Retrying kill with numeric part of sharded operation: {numeric_part}"
                        )
                        return await self.kill_operation(int(numeric_part))
                except Exception as inner_e:
                    logger.error(f"Error processing sharded operation ID: {inner_e}")
            return False


class KillConfirmation(ModalScreen[bool]):
    """Modal screen for kill operation confirmation."""

    DEFAULT_CSS = """
    KillConfirmation {
        align: center middle;
    }

    #dialog-container {
        grid-size: 2;
        grid-gutter: 1 2;
        grid-rows: 4;
        padding: 1 2;
        width: 60;
        height: 11;
        border: thick $error;
        background: $surface;
    }

    #question {
        column-span: 2;
        height: 1fr;
        width: 100%;
        content-align: center middle;
        text-style: bold;
    }

    #button-container {
        column-span: 2;
        height: 3;
        align: center middle;
    }

    Button {
        width: 16;
    }

    #no {
        margin-left: 2;
    }
    """

    def __init__(self, operations: list) -> None:
        super().__init__()
        self.operations = operations

    def compose(self) -> ComposeResult:
        count = len(self.operations)
        op_text = "operation" if count == 1 else "operations"

        with Container(id="dialog-container"):
            yield Static(
                f"Are you sure you want to kill {count} {op_text}?", id="question"
            )
            with Horizontal(id="button-container"):
                yield Button("Yes", variant="error", id="yes")
                yield Button("No", variant="primary", id="no")

    def on_mount(self) -> None:
        # Set focus to "No" button by default
        self.query_one("#no").focus()

    def on_button_pressed(self, event: Button.Pressed) -> None:
        self.dismiss(event.button.id == "yes")

    def on_key(self, event) -> None:
        if event.key == "escape":
            self.dismiss(False)
        elif event.key == "enter" and self.query_one("#yes").has_focus:
            self.dismiss(True)


class OperationsTable(DataTable):
    """Table displaying MongoDB operations."""

    def __init__(self) -> None:
        super().__init__()
        self.cursor_type = "row"
        self.zebra_stripes = True
        self.can_focus = True
        self.filters = {
            "opid": "",
            "operation": "",
            "namespace": "",
            "running_time": "",
            "client": "",
            "description": "",
            "effective_users": "",
        }
        self.sort_running_time_asc = True

    def on_mount(self) -> None:
        """Set up the table columns when the widget is mounted."""
        self.add_columns(
            "Select",
            "OpId",
            "Type",
            "Operation",
            "Running Time",
            "Active",
            "Client",
            "Description",
            "Effective Users",
        )
        self.focus()  # Request focus when mounted

    def add_row(self, *values, **kwargs):
        """Override add_row to ensure consistent key handling."""
        if "key" in kwargs:
            # Ensure the key is always a string
            kwargs["key"] = str(kwargs["key"])
        return super().add_row(*values, **kwargs)


class FiltersContainer(Container):
    """Container for filter inputs."""

    BORDER_TITLE = "Filters"
    BORDER_SUBTITLE = "Filter operations by criteria"

    CSS = """
    FiltersContainer {
        height: auto;
        margin: 1;
        padding: 1;
        background: $surface;
        border: solid $primary;
    }

    #filters-container {
        height: auto;
        width: 100%;
        align: center middle;
        padding: 1;
    }

    #filters-container > Input {
        margin: 1 2;
        width: 1fr;
        border: tall $primary;
        background: $surface;
    }

    #clear-filters {
        margin: 1 2;
        min-width: 35;
    }
    """

    def compose(self) -> ComposeResult:
        yield Horizontal(
            Input(placeholder="OpId...", id="filter-opid", classes="filter-input"),
            Input(
                placeholder="Operation...",
                id="filter-operation",
                classes="filter-input",
            ),
            Input(
                placeholder="Running Time (>=s)...",
                id="filter-running-time",
                classes="filter-input",
            ),
            Input(placeholder="Client...", id="filter-client", classes="filter-input"),
            Input(
                placeholder="Description...",
                id="filter-description",
                classes="filter-input",
            ),
            Input(
                placeholder="Effective Users...",
                id="filter-effective-users",
                classes="filter-input",
            ),
            Button(
                "Clear Filters", id="clear-filters", variant="primary"
            ),  # TODO: Improve layout
            id="filters-container",
        )

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "clear-filters":
            for input_widget in self.query(".filter-input"):
                if isinstance(input_widget, Input):
                    input_widget.value = ""


class MongoOpsManager(App):
    """Main application for managing MongoDB operations."""

    TITLE = "Close Mongo Operations Manager"

    ENABLE_COMMAND_PALETTE = False

    CSS = """
    #refresh-status {
        background: $surface;
        color: $error;
        text-align: center;
        padding: 1;
        margin: 0 2;
        text-style: bold;
        border: heavy $warning;
        display: none;
    }

    FiltersContainer {
        height: auto;
        margin: 1;
        padding: 1;
        background: $surface;
        border: solid $primary;
    }

    #filters-container {
        height: auto;
        width: 100%;
        align: center middle;
        padding: 1;
    }

    #filters-container > Input {
        margin: 1 2;
        width: 1fr;
        border: tall $primary;
        background: $surface;
    }

    DataTable {
        height: 1fr;
        margin: 1;
    }

    DataTable > .selected-row {
        background: $accent;
        color: $text;
        text-style: bold;
    }

    DataTable .selected-row:odd {
        background: $accent;
    }

    DataTable .selected-row:even {
        background: $accent;
    }

    .paused {
        color: $warning;
        text-style: bold;
    }
    """

    BINDINGS = [
        Binding("q", "quit", "Quit"),
        Binding("r", "refresh", "Refresh"),
        Binding("k", "kill_selected", "Kill Selected"),
        Binding("p", "toggle_refresh", "Pause/Resume"),
        Binding("u", "deselect_all", "Deselect All"),
        Binding("s", "sort_by_running_time", "Sort By Running Time"),
    ]

    selected_ops: reactive[set] = reactive(set())
    auto_refresh_enabled: reactive[bool] = reactive(True)

    def __init__(self, args) -> None:
        super().__init__()
        self._auto_refresh_task: asyncio.Task | None = None
        self.last_refresh = datetime.now()
        self.namespace = args.namespace
        self.connection_string = ""
        self.refresh_interval = 5.0
        self.mongo: MongoDBConnection
        self.theme: str = "textual-dark"  # 0.86.0+ uses themes instead of dark/light mode. So only dark for now.
        try:
            self.load_config_from_args(args)
        except Exception as e:
            logger.error(f"Initialization error: {e}")
            self.display_error_and_exit(f"Failed to initialize application: {e}")

    @classmethod
    async def create(cls, args) -> "MongoOpsManager":
        """Create a new instance of the application."""
        self = cls(args)
        try:
            self.mongo = await MongoDBConnection.create(self.connection_string)
        except MongoConnectionError as e:
            logger.error(f"MongoDB connection error: {e}")
            self.display_error_and_exit(str(e))
        except Exception as e:
            logger.error(f"Initialization error: {e}")
            self.display_error_and_exit(f"Failed to initialize application: {e}")
        return self

    def display_error_and_exit(self, message: str) -> None:
        """Display error message and exit the application."""
        print(f"\nError: {message}")
        print("Please check your configuration and try again.")
        print("The application will now exit.")
        exit(1)

    def load_config_from_args(self, args) -> None:
        """Load configuration from command line arguments."""
        try:
            # Build connection string based on authentication settings
            if args.username and args.password:
                # Use authenticated connection
                username = quote_plus(args.username)
                password = quote_plus(args.password)
                self.connection_string = (
                    f"mongodb://{username}:{password}@{args.host}:{args.port}/"
                )
            else:
                # Use unauthenticated connection
                self.connection_string = f"mongodb://{args.host}:{args.port}/"
                logger.info("Using unauthenticated connection")

            # Application settings
            refresh_interval = args.refresh_interval
            if refresh_interval < 0.5:
                logger.warning(
                    "Refresh interval too low, setting to minimum (0.5 seconds)"
                )
                refresh_interval = 0.5
            elif refresh_interval > 60:
                logger.warning(
                    "Refresh interval too high, setting to maximum (60 seconds)"
                )
                refresh_interval = 60
            self.refresh_interval = refresh_interval

        except Exception as e:
            raise Exception(f"Error processing arguments: {e}")

    def compose(self) -> ComposeResult:
        """Create and compose the app layout."""
        yield Header()
        yield Static("", id="refresh-status")
        yield FiltersContainer()
        yield Container(OperationsTable(), id="main-container")
        yield Footer()

    def on_mount(self) -> None:
        """Initialize the app when mounted."""
        self.table = self.query_one(OperationsTable)
        self.status = self.query_one("#refresh-status")
        self.refresh_ops()
        # self.start_auto_refresh()
        self.set_focus(self.table)
        self.update_refresh_status()

    def action_deselect_all(self) -> None:
        """Deselect all selected operations."""
        if not self.selected_ops:
            return

        # Remember selected ops before clearing
        count = len(self.selected_ops)

        # Find and update all selected rows
        for row_key in self.table.rows.keys():
            row_data = self.table.get_row(row_key)
            if (
                row_data and row_data[1] in self.selected_ops
            ):  # Check OpId column (index 1)
                # Find the row index
                for idx, key in enumerate(self.table.rows.keys()):
                    if key == row_key:
                        coord = Coordinate(idx, 0)
                        self.table.update_cell_at(coord, "☐")
                        break

        # Clear the selected operations set
        self.selected_ops.clear()

        # Show notification
        self.notify(f"Deselected {count} operations")

    def update_refresh_status(self) -> None:
        """Update the refresh status indicator."""
        status_text = "AUTO-REFRESH PAUSED" if not self.auto_refresh_enabled else ""
        self.status.styles.display = (
            "block" if not self.auto_refresh_enabled else "none"
        )
        self.status.update(Text(status_text, style="bold red"))  # type: ignore
        # Also update title to show status
        status_suffix = " (PAUSED)" if not self.auto_refresh_enabled else ""
        self.sub_title = status_suffix

    def action_toggle_refresh(self) -> None:
        """Toggle auto-refresh on/off."""
        self.auto_refresh_enabled = not self.auto_refresh_enabled
        self.update_refresh_status()
        status = "enabled" if self.auto_refresh_enabled else "paused"
        self.notify(f"Auto-refresh {status}")

    # def stop_refresh(self) -> None:
    #     """Stop auto-refresh."""
    #     self.auto_refresh_enabled = False
    #     self.update_refresh_status()
    #     self.notify("Auto-refresh stopped")

    # def start_refresh(self) -> None:
    #     """Start auto-refresh."""
    #     self.auto_refresh_enabled = True
    #     self.update_refresh_status()
    #     self.notify("Auto-refresh started")

    def on_input_changed(self, event: Input.Changed) -> None:
        """Handle changes to filter inputs."""
        input_id = event.input.id
        if input_id:
            filter_key = input_id.replace("filter-", "").replace("-", "_")
            self.table.filters[filter_key] = event.value
            self.refresh_ops()  # Refresh with new filters

    async def auto_refresh(self) -> None:
        """Automatically refresh operations at configured interval."""
        while True:
            if self.auto_refresh_enabled:
                current_time = datetime.now()
                if current_time - self.last_refresh >= timedelta(
                    seconds=self.refresh_interval
                ):
                    self.refresh_ops()
                    self.last_refresh = current_time
            await asyncio.sleep(0.1)

    def start_auto_refresh(self) -> None:
        """Start the auto-refresh task."""
        if self._auto_refresh_task is None or self._auto_refresh_task.done():
            self._auto_refresh_task = asyncio.create_task(self.auto_refresh())

    def stop_auto_refresh(self) -> None:
        """Stop the auto-refresh task."""
        if self._auto_refresh_task and not self._auto_refresh_task.done():
            self._auto_refresh_task.cancel()
            self._auto_refresh_task = None

    @work(exclusive=True)
    async def refresh_ops(self) -> None:
        """Refresh the operations table with filters."""
        try:
            # Get filters from inputs
            filters = {}
            for filter_id, value in self.table.filters.items():
                if value.strip():  # Only add non-empty filters
                    filters[filter_id] = value.strip()

            # Add namespace filter from command line
            filters["namespace"] = self.namespace

            # Get current operations based on filters
            ops = await self.mongo.get_current_ops(filters)

            # Clear table and selected_ops set
            self.table.clear()
            self.selected_ops.clear()

            # Add operations to the table
            for op in ops:
                # Format effective users as comma-separated string
                effective_users_str = (
                    ", ".join(f"{user.get('user', '')}" for user in op.effective_users)
                    if op.effective_users
                    else "N/A"
                )

                running_time = f"{op.secs_running}s"
                row = (
                    "☐",
                    str(op.opid),
                    op.type,
                    op.op,
                    running_time,
                    "✓" if op.active else "✗",
                    op.client or "N/A",
                    op.desc or "N/A",
                    effective_users_str,
                )
                self.table.add_row(*row, key=str(op.opid))

        except Exception as e:
            logger.error(f"Error refreshing operations: {e}")
            self.notify("Error refreshing operations", severity="error")

    def action_sort_by_running_time(self) -> None:
        """Sort operations by running time."""
        try:
            rows_data = []

            # Clear selections before sorting
            self.selected_ops.clear()

            # Collect data with running time values
            for row_key in self.table.rows.keys():
                row = self.table.get_row(row_key)
                if row:
                    # Extract seconds value, handling potential format issues
                    # TODO: Do not use index-based access
                    time_str = row[4].rstrip("s") if len(row) > 4 else "0"
                    try:
                        running_time = int(time_str)
                        # Create new row with cleared selection state
                        new_row = list(row)
                        new_row[0] = "☐"  # Clear checkbox
                        rows_data.append((running_time, new_row, str(row_key.value)))
                    except ValueError:
                        running_time = 0
                        # If conversion fails, treat as 0
                        new_row = list(row)
                        new_row[0] = "☐"  # Clear checkbox
                        rows_data.append((running_time, new_row, str(row_key.value)))

            # Sort the data
            reverse_sort = getattr(self.table, "sort_running_time_asc", True)
            rows_data.sort(key=lambda x: x[0], reverse=reverse_sort)

            # Update sort direction for next time
            self.table.sort_running_time_asc = not reverse_sort

            # Clear and rebuild table
            self.table.clear()

            # Rebuild the table with cleared selections
            for _, row, opid in rows_data:
                self.table.add_row(*row, key=opid)

            # Show sorting notification
            direction = "descending" if reverse_sort else "ascending"
            self.notify(f"Sorted by running time ({direction})")

        except Exception as e:
            logger.error(f"Error sorting table: {e}")
            self.notify("Error sorting operations", severity="error")

    def action_refresh(self) -> None:
        """Handle refresh action."""
        self.refresh_ops()

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        """Handle row selection."""
        try:
            opid = event.row_key.value  # Get the actual value from the RowKey
            row = self.table.get_row(event.row_key)
            if row is None:
                return

            # Create coordinate for the checkbox cell (row, column)
            coord = Coordinate(event.cursor_row, 0)

            if opid in self.selected_ops:
                self.selected_ops.remove(opid)
                self.table.update_cell_at(coord, "☐")
            else:
                self.selected_ops.add(opid)
                self.table.update_cell_at(coord, "☒")

        except Exception as e:
            logger.error(f"Error handling row selection: {e}")
            self.notify("Error selecting row", severity="error")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        """Handle button press events."""
        if event.button.id == "clear-filters":
            # Clear all filters in the table
            for key in self.table.filters:
                self.table.filters[key] = ""
            # Refresh the operations with cleared filters
            self.refresh_ops()
            self.notify("Filters cleared")

    async def action_kill_selected(self) -> None:
        """Kill selected operations with confirmation."""
        if not self.selected_ops:
            self.notify("No operations selected")
            return

        async def check_kill_confirmation(confirmed: bool | None) -> None:
            """Check if kill operation is confirmed."""
            if not confirmed:
                return

            killed = []
            failed = []

            try:
                for opid_key in self.selected_ops:
                    try:
                        # Extract the actual operation ID from the RowKey's value
                        if hasattr(opid_key, "value"):
                            opid = opid_key.value
                        else:
                            opid = str(opid_key)  # Fallback if it's already a string

                        # Kill operation using db.killOp()
                        if await self.mongo.kill_operation(opid):
                            killed.append(opid)
                        else:
                            failed.append(opid)
                    except Exception as e:
                        logger.error(f"Error killing operation {opid}: {e}")
                        failed.append(opid)

                # Show notifications based on results
                if killed:
                    killed_count = len(killed)
                    op_text = "operation" if killed_count == 1 else "operations"
                    self.notify(
                        f"Successfully killed {killed_count} {op_text}: {', '.join(str(x) for x in killed)}",
                        severity="information",
                        timeout=3,
                    )

                if failed:
                    failed_count = len(failed)
                    op_text = "operation" if failed_count == 1 else "operations"
                    self.notify(
                        f"Failed to kill {failed_count} {op_text}: {', '.join(str(x) for x in failed)}",
                        severity="error",
                        timeout=5,
                    )

                # Clear selected operations
                self.selected_ops.clear()

                # Refresh operations
                self.refresh_ops()

            except Exception as e:
                logger.error(f"Error in kill operation: {e}")
                self.notify(f"Error during kill operation: {str(e)}", severity="error")

            # Regardless of success/failure, refresh the list
            finally:
                self.refresh_ops()

        self.push_screen(
            KillConfirmation(list(self.selected_ops)), check_kill_confirmation
        )


def parse_args() -> argparse.Namespace:
    """Parse command line arguments."""
    parser = argparse.ArgumentParser(
        description="Close Mongo Operations Manager - Monitor and kill MongoDB operations",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )

    # MongoDB connection settings
    parser.add_argument("--host", default="localhost", type=str, help="MongoDB host")
    parser.add_argument("--port", default="27017", type=str, help="MongoDB port")
    parser.add_argument("--username", type=str, help="MongoDB username")
    parser.add_argument("--password", type=str, help="MongoDB password")
    parser.add_argument(
        "--namespace", type=str, required=True, help="MongoDB namespace to monitor"
    )

    # Application settings
    parser.add_argument(
        "--refresh-interval",
        type=float,
        default=5.0,
        help="Refresh interval in seconds (min: 0.5, max: 60)",
    )

    return parser.parse_args()


async def main() -> None:
    """Main entry point for the application."""
    try:
        # Parse command line arguments
        args = parse_args()

        app = await MongoOpsManager.create(args)
        await app.run_async()

    except MongoConnectionError as e:
        logger.error(f"MongoDB connection error: {e}")
        print(f"\nError connecting to MongoDB: {e}")
        sys.exit(1)
    except Exception as e:
        logger.error(f"Unexpected error: {e}", exc_info=True)
        print(f"\nUnexpected error occurred: {e}")
        print(f"Check {LOG_FILE} for more details")
        sys.exit(1)


if __name__ == "__main__":
    asyncio.run(main())
