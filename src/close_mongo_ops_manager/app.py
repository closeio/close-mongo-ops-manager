from __future__ import annotations

import asyncio
import logging
import os
import argparse
import sys
import time
from urllib.parse import quote_plus
from importlib.metadata import version

from textual import work
from textual.binding import Binding
from textual.app import App, ComposeResult
from textual.reactive import reactive
from textual.widgets import DataTable, Footer, Header, Input
from textual.coordinate import Coordinate
from textual.containers import VerticalScroll

from pymongo.uri_parser import parse_uri

from close_mongo_ops_manager.filterbar import FilterBar
from close_mongo_ops_manager.help_screen import HelpScreen
from close_mongo_ops_manager.kill_confirmation_screen import KillConfirmation
from close_mongo_ops_manager.log_screen import LogScreen
from close_mongo_ops_manager.messages import (
    FilterChanged,
    OperationsLoaded,
    SelectionChanged,
)
from close_mongo_ops_manager.mongodb_manager import MongoDBManager
from close_mongo_ops_manager.operations_view import OperationsView
from close_mongo_ops_manager.statusbar import StatusBar
from close_mongo_ops_manager.theme_manager import ThemeManager
from close_mongo_ops_manager.config_manager import ConfigManager
from close_mongo_ops_manager.theme_screen import ThemeScreen


# Constants
LOG_FILE = "close_mongo_ops_manager.log"
MIN_REFRESH_INTERVAL = 1
MAX_REFRESH_INTERVAL = 10
DEFAULT_REFRESH_INTERVAL = 2
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


class MongoOpsManager(App):
    """Main application class."""

    ENABLE_COMMAND_PALETTE = False

    TITLE = f"Close MongoDB Operations Manager v{version('close-mongo-ops-manager')}"

    # Explicitly set initial focus to operations view
    AUTO_FOCUS = None  # Disable auto-focus and handle it in on_mount

    CSS = """
    MongoOpsManager {
        align: center top;
        padding: 0;
    }

    VerticalScroll {
        width: 100%;
        padding: 0;
        margin: 0;
    }

    .hidden {
        display: none;
    }
    """

    BINDINGS = [
        Binding("f1", "show_help", "Help"),
        Binding("ctrl+q", "quit", "Quit"),
        Binding("ctrl+r", "refresh", "Refresh"),
        Binding("ctrl+k", "kill_selected", "Kill Selected"),
        Binding("ctrl+p", "toggle_refresh", "Pause/Resume"),
        Binding("ctrl+s", "sort_by_time", "Sort by Time"),
        Binding("ctrl+l", "show_logs", "View Logs"),
        Binding("ctrl+a", "toggle_selection", "Toggle Selection"),
        Binding("ctrl+f", "toggle_filter_bar", "Toggle Filters"),
        Binding("ctrl+t", "change_theme", "Theme"),
        Binding(
            "ctrl+equals_sign",
            "increase_refresh",
            "Increase Refresh Interval",
            key_display="^+",
            show=False,
        ),
        Binding(
            "ctrl+minus",
            "decrease_refresh",
            "Decrease Refresh Interval",
            key_display="^-",
            show=False,
        ),
    ]

    auto_refresh: reactive[bool] = reactive(True)
    refresh_interval: reactive[int] = reactive(DEFAULT_REFRESH_INTERVAL)

    def __init__(
        self,
        connection_string: str,
        refresh_interval: int = DEFAULT_REFRESH_INTERVAL,
        namespace: str = "",
        hide_system_ops: bool = True,
        load_balanced: bool = False,
    ) -> None:
        super().__init__()
        self.connection_string = connection_string
        self.refresh_interval = refresh_interval
        self.mongodb: MongoDBManager | None = None
        self._refresh_task: asyncio.Task | None = None
        self.log_file = LOG_FILE
        self._status_bar: StatusBar | None = None
        self.namespace: str = namespace
        self.hide_system_ops = hide_system_ops
        self.load_balanced = load_balanced

        # Initialize theme management
        self.config_manager = ConfigManager()
        self.theme_manager = ThemeManager()

        # Register custom themes with Textual
        self._register_custom_themes()

        # Load saved theme configuration
        theme_config = self.config_manager.load_theme_config()
        self.theme_manager.config = theme_config

        # Apply saved theme
        self.theme = theme_config.current_theme

    def _register_custom_themes(self) -> None:
        """Register custom themes with Textual app."""
        for theme_name, theme in self.theme_manager._custom_themes.items():
            self.register_theme(theme)

    @staticmethod
    def validate_refresh_interval(value: int) -> int:
        """Validate refresh interval."""
        return max(MIN_REFRESH_INTERVAL, min(value, MAX_REFRESH_INTERVAL))

    def compose(self) -> ComposeResult:
        """Create child widgets for the app."""
        yield Header()
        yield FilterBar(classes="hidden")  # Hide FilterBar by default
        with VerticalScroll(can_focus=False, can_focus_children=True):
            yield OperationsView()
        yield StatusBar(self.refresh_interval)
        yield Footer()

    async def on_mount(self) -> None:
        self.operations_view = self.query_one(OperationsView)
        self._status_bar = self.query_one(StatusBar)
        self.operations_view.loading = True
        self._status_bar.set_refresh_interval(self.refresh_interval)
        self._status_bar.set_connection_status(False, "Connecting...")
        self._status_bar.set_refresh_status(self.auto_refresh)
        # Ensure operations view has focus when app loads
        self.operations_view.focus()
        # Defer MongoDB connection to ensure UI is fully rendered first
        self.call_after_refresh(self._start_connection)

    def action_show_help(self) -> None:
        """Show the help screen."""
        self.push_screen(HelpScreen())

    def action_show_logs(self) -> None:
        """Show the log viewer screen."""
        self.push_screen(LogScreen(self.log_file))

    def action_change_theme(self) -> None:
        """Show theme selection screen."""
        available_themes = self.theme_manager.get_available_themes()
        current_theme = self.theme_manager.get_current_theme()

        async def handle_theme_selection(selected_theme: str | None) -> None:
            if selected_theme and self.theme_manager.set_current_theme(selected_theme):
                # Apply the new theme
                self.theme = selected_theme

                # Save the configuration
                self.config_manager.save_theme_config(self.theme_manager.config)

                # Notify user
                theme_display = selected_theme.replace("-", " ").title()
                self.notify(f"Theme changed to {theme_display}")

        self.push_screen(
            ThemeScreen(available_themes, current_theme),
            callback=handle_theme_selection,
        )

    async def _start_connection(self) -> None:
        """Start the MongoDB connection process after UI is ready."""
        await self._setup()

    async def _setup(self) -> None:
        """Initialize MongoDB connection and start operation monitoring."""
        try:
            self.mongodb = await MongoDBManager.connect(
                self.connection_string, self.namespace, self.hide_system_ops, self.load_balanced
            )
            # Extract connection details for status bar
            host_info = "MongoDB server"
            try:
                parsed_uri = parse_uri(self.connection_string)

                # Safely extract host information with fallbacks
                nodelist = parsed_uri.get("nodelist")
                if nodelist and len(nodelist) > 0:
                    host, port = nodelist[0]
                    host_info = f"{host}:{port}"
                else:
                    # Fallback: try to extract from connection string directly
                    if "@" in self.connection_string:
                        cleaned_uri = self.connection_string.split("@")[-1].split("/")[
                            0
                        ]
                    else:
                        cleaned_uri = self.connection_string.replace(
                            "mongodb://", ""
                        ).split("/")[0]

                    # Remove query parameters if present
                    host_info = cleaned_uri.split("?")[0]
            except Exception as parse_error:
                logger.warning(f"Failed to parse host details: {parse_error}")
                # Keep default "MongoDB server" as host_info

            if self._status_bar:
                self._status_bar.set_connection_status(True, host_info)

            self.refresh_operations()
            self._refresh_task = asyncio.create_task(self.auto_refreshing())

            # Ensure operations view has focus after setup
            self.operations_view.focus()
        except Exception as e:
            logger.error(f"Setup error: {e}", exc_info=True)
            if self._status_bar:
                self._status_bar.set_connection_status(False)
            self.notify(f"Failed to connect: {e}", severity="error")

    def action_increase_refresh(self) -> None:
        """Increase the refresh interval."""
        new_interval = self.refresh_interval + STEP_REFRESH_INTERVAL
        new_interval = MongoOpsManager.validate_refresh_interval(new_interval)
        if new_interval != self.refresh_interval:
            self.refresh_interval = new_interval
            self._status_bar.set_refresh_interval(self.refresh_interval)

    def action_decrease_refresh(self) -> None:
        """Decrease the refresh interval."""
        new_interval = self.refresh_interval - STEP_REFRESH_INTERVAL
        new_interval = MongoOpsManager.validate_refresh_interval(new_interval)
        if new_interval != self.refresh_interval:
            self.refresh_interval = new_interval
            self._status_bar.set_refresh_interval(self.refresh_interval)

    async def auto_refreshing(self) -> None:
        """Background task for auto-refreshing functionality."""
        while True:
            try:
                if self.auto_refresh and self.mongodb:
                    self.refresh_operations()
                await asyncio.sleep(self.refresh_interval)
            except asyncio.CancelledError:
                # Re-raise CancelledError to allow proper task cancellation
                logger.info("Auto-refresh task cancelled")
                raise
            except Exception as e:
                logger.error(f"Auto-refresh error: {e}", exc_info=True)
                # Continue running but wait before retry
                await asyncio.sleep(self.refresh_interval)

    async def _update_operations_view(
        self,
        ops_data: list,
        selected_ops_before_refresh: set,
        start_time: float,
        loading_timer: asyncio.Task | None,
        cursor_position_info: dict | None = None,
    ) -> None:
        """Update the operations view with new data."""
        try:
            # Cancel loading timer if still running (it might have finished before this point)
            if loading_timer and not loading_timer.done():
                loading_timer.cancel()

            # Store the operations data in the view (after sorting)
            self.operations_view.current_ops = ops_data

            # Clear all rows to ensure correct ordering
            self.operations_view.clear()

            # Update existing rows and add new ones in sorted order
            for i, op in enumerate(ops_data):
                # Skip operations without opid
                if not op or "opid" not in op:
                    logger.warning("Skipping operation without opid")
                    continue

                op_id = str(op["opid"])

                # Get client info
                client_info = op.get("client_s") or op.get("client", "N/A")
                client_metadata = op.get("clientMetadata", {})
                mongos_info = (
                    client_metadata.get("mongos", {}) if client_metadata else {}
                )
                mongos_host = mongos_info.get("host", "") if mongos_info else ""

                if mongos_host:
                    client_info = f"{client_info} ({mongos_host.split('.', 1)[0]})"

                # Get effective users
                effective_users = op.get("effectiveUsers", [])
                users_str = (
                    ", ".join(u.get("user", "") for u in effective_users if u)
                    if effective_users
                    else "N/A"
                )

                # Check if we need to show selection
                is_selected = op_id in selected_ops_before_refresh
                selection_mark = "✓" if is_selected else " "

                row_data = (
                    selection_mark,
                    op_id,
                    op.get("type", ""),
                    op.get("op", ""),
                    f"{op.get('secs_running', 0)}s",
                    client_info,
                    op.get("desc", "N/A"),
                    users_str,
                )

                # Add row
                self.operations_view.add_row(*row_data, key=op_id)

            # Build set of current operation IDs
            current_op_ids = {str(op["opid"]) for op in ops_data if op and "opid" in op}

            # Restore selected operations
            self.operations_view.selected_ops = {
                op_id
                for op_id in selected_ops_before_refresh
                if op_id in current_op_ids
            }

            # Update status bar with selected operations count
            if self._status_bar:
                self._status_bar.set_selected_count(
                    len(self.operations_view.selected_ops)
                )

            # Restore cursor position if we saved it before refresh
            if cursor_position_info and cursor_position_info.get("opid"):
                target_opid = cursor_position_info["opid"]
                fallback_row_index = cursor_position_info.get("row_index", 0)

                # Find the new row index of the previously selected operation
                new_row_index = None
                for i, op in enumerate(ops_data):
                    if op and str(op.get("opid", "")) == target_opid:
                        new_row_index = i
                        break

                # If we found the operation in the new data, move cursor there
                if new_row_index is not None:
                    self.operations_view.move_cursor(row=new_row_index)
                elif len(ops_data) > 0:
                    # Fallback: use the previous row index, clamped to available rows
                    fallback_index = min(fallback_row_index, len(ops_data) - 1)
                    self.operations_view.move_cursor(row=fallback_index)

            # Calculate load duration and emit event
            duration = time.monotonic() - start_time
            self.operations_view.post_message(
                OperationsLoaded(count=len(ops_data), duration=duration)
            )

            # Only focus operations view if a filter input doesn't have focus
            filter_inputs = self.query(".filter-input")
            has_focus_input = any(
                input_widget.has_focus for input_widget in filter_inputs
            )
            if not has_focus_input:
                self.operations_view.focus()
        except Exception as e:
            logger.error(f"Failed to update operations view: {e}", exc_info=True)
            self.notify(f"Failed to update view: {e}", severity="error")
        finally:
            # Ensure loading is always set to False in the UI update method
            self.operations_view.loading = False

    @work(exclusive=True)
    async def refresh_operations(self) -> None:
        """Refresh the operations table with current data."""
        if not self.mongodb:
            self.operations_view.loading = False
            return

        # Save current selected operations before refreshing
        selected_ops_before_refresh = self.operations_view.selected_ops.copy()

        # Save cursor position to preserve user's position in the table
        cursor_position_info = None
        if (
            self.operations_view.cursor_row is not None
            and self.operations_view.current_ops
            and 0
            <= self.operations_view.cursor_row
            < len(self.operations_view.current_ops)
        ):
            # Save the opid of the currently selected row
            current_op = self.operations_view.current_ops[
                self.operations_view.cursor_row
            ]
            cursor_position_info = {
                "opid": str(current_op.get("opid", "")),
                "row_index": self.operations_view.cursor_row,
            }

        start_time = time.monotonic()
        loading_timer = None

        async def set_loading_after_delay():
            """Set loading state after a short delay to avoid flicker."""
            await asyncio.sleep(0.1)  # 100ms delay
            self.operations_view.loading = True

        try:
            # Start loading timer
            loading_timer = asyncio.create_task(set_loading_after_delay())

            # Fetch operations
            ops = await self.mongodb.get_operations(self.operations_view.filters)

            # Sort operations by running time if needed
            if hasattr(self.operations_view, "sort_running_time_asc"):
                ops.sort(
                    key=lambda x: float(x.get("secs_running", 0)),
                    reverse=not self.operations_view.sort_running_time_asc,
                )

            # Call the new method to update the UI
            scheduled_ok = self.call_later(
                self._update_operations_view,
                ops,
                selected_ops_before_refresh,
                start_time,
                loading_timer,
                cursor_position_info,
            )
            if not scheduled_ok:
                logger.warning(
                    "Could not schedule UI update for operations. Performing cleanup."
                )
                if loading_timer and not loading_timer.done():
                    loading_timer.cancel()
                # Ensure self.operations_view exists and is not None
                if self.operations_view:
                    self.operations_view.loading = False

        except Exception as e:
            logger.error(f"Failed to refresh operations: {e}", exc_info=True)
            self.notify(f"Failed to refresh: {e}", severity="error")
            # Ensure loading state is reset in case of error before UI update
            # This is important if get_operations fails before _update_operations_view is called
            if loading_timer and not loading_timer.done():
                loading_timer.cancel()
            self.operations_view.loading = False

    def action_refresh(self) -> None:
        """Handle refresh action."""
        self.refresh_operations()

    def action_toggle_refresh(self) -> None:
        """Toggle auto-refresh."""
        self.auto_refresh = not self.auto_refresh
        self._status_bar.set_refresh_status(self.auto_refresh)

    def action_toggle_filter_bar(self) -> None:
        """Toggle filter bar visibility."""
        filter_bar = self.query_one(FilterBar)

        # Use call_after_refresh to ensure UI updates properly
        if "hidden" in filter_bar.classes:
            # Show filter bar
            filter_bar.remove_class("hidden")
            # Focus the first input after refresh
            self.call_after_refresh(lambda: self._focus_first_filter_input())
        else:
            # Hide filter bar
            filter_bar.add_class("hidden")
            # Return focus to operations view after refresh
            # Ensure operations view is actually focusable
            if self.operations_view and not self.operations_view.loading:
                self.call_after_refresh(self.operations_view.focus)

    def _focus_first_filter_input(self) -> None:
        """Helper to focus the first filter input."""
        filter_bar = self.query_one(FilterBar)
        first_input = filter_bar.query_one(".filter-input", expect_type=Input)
        if first_input:
            first_input.focus()

    def action_deselect_all(self) -> None:
        """Deselect all selected operations."""
        if not self.operations_view.selected_ops:
            return

        # Clear the selected operations set
        self.operations_view.selected_ops.clear()

        # Update StatusBar with selected count (zero)
        self._status_bar.set_selected_count(0)

        # Emit message about selection change
        self.operations_view.post_message(SelectionChanged(count=0))

        self.refresh_operations()

    def action_toggle_selection(self) -> None:
        """Toggle between selecting all operations and deselecting all operations."""
        # If there are any selected operations, deselect them
        if self.operations_view.selected_ops:
            self.action_deselect_all()
        else:
            # Select all operations (default behavior)
            self.action_select_all()

    def action_select_all(self) -> None:
        """Select all operations in the view."""
        # Clear any existing selections first
        self.operations_view.selected_ops.clear()

        # Add all row keys to selected_ops and update checkboxes
        for idx, key in enumerate(self.operations_view.rows.keys()):
            # Convert RowKey to string value
            row_key = str(getattr(key, "value", key))
            self.operations_view.selected_ops.add(row_key)
            coord = Coordinate(idx, 0)
            self.operations_view.update_cell_at(coord, "✓")

        # Update StatusBar with selected count
        count = len(self.operations_view.selected_ops)
        self._status_bar.set_selected_count(count)

        # Emit message about selection change
        self.operations_view.post_message(SelectionChanged(count=count))

    # Fix for the issue where row selection isn't properly cleared after killing operations
    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        """Handle row selection."""
        try:
            # Get the row key value directly
            row_key = str(event.row_key.value)
            coord = Coordinate(event.cursor_row, 0)  # Get checkbox cell coordinate

            if row_key in self.operations_view.selected_ops:
                self.operations_view.selected_ops.remove(row_key)
                self.operations_view.update_cell_at(coord, " ")
            else:
                self.operations_view.selected_ops.add(row_key)
                self.operations_view.update_cell_at(coord, "✓")

            # Update StatusBar with selected count
            self._status_bar.set_selected_count(len(self.operations_view.selected_ops))
            # Emit message about selection change
            self.operations_view.post_message(
                SelectionChanged(count=len(self.operations_view.selected_ops))
            )

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

            try:
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
                            f"Failed to kill operation {opid}: {str(e)}",
                            severity="error",
                        )
                        logger.error(
                            f"Failed to kill operation {opid}: {e}", exc_info=True
                        )

                # Clear selections after all operations are processed
                self.operations_view.clear_selections()

                # Force update the status bar immediately
                self._status_bar.set_selected_count(0)

                # Refresh the view
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
            except Exception as e:
                logger.error(f"Error in kill operation handler: {e}", exc_info=True)
                self.notify(f"Error processing operations: {e}", severity="error")

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

    def on_selection_changed(self, event: SelectionChanged) -> None:
        """Handle selection changed event."""
        self._status_bar.set_selected_count(event.count)

    async def on_unmount(self) -> None:
        """Clean up resources when the application exits."""
        # Cancel the refresh task if it's running
        if (
            hasattr(self, "_refresh_task")
            and self._refresh_task
            and not self._refresh_task.done()
        ):
            self._refresh_task.cancel()
            try:
                await self._refresh_task
            except asyncio.CancelledError:
                logger.info("Refresh task cancelled successfully")
            except Exception as e:
                logger.error(f"Error cancelling refresh task: {e}")

        # Close MongoDB connections
        if hasattr(self, "mongodb") and self.mongodb:
            try:
                await self.mongodb.close()
            except Exception as e:
                logger.error(f"Error closing MongoDB connections: {e}")


def main() -> None:
    parser = argparse.ArgumentParser(
        description=f"Close MongoDB Operations Manager v{version('close-mongo-ops-manager')}"
    )
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
    parser.add_argument(
        "--show-system-ops",
        action="store_true",
        help="Show system operations (disabled by default)",
    )
    parser.add_argument(
        "--load-balanced",
        action="store_true",
        help="Enable load balancer support for MongoDB connections",
    )
    parser.add_argument(
        "--version",
        action="version",
        version=f"v{version('close-mongo-ops-manager')}",
        help="Show the version of the application",
    )

    args = parser.parse_args()

    logger.info(
        f"Starting Close MongoDB Operations Manager v{version('close-mongo-ops-manager')}"
    )

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
            logger.warning("Using unauthenticated connection")

        # Validate refresh interval
        refresh_interval = MongoOpsManager.validate_refresh_interval(
            args.refresh_interval
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
            hide_system_ops=not args.show_system_ops,
            load_balanced=args.load_balanced,
        )
        app.run()

        logger.info("Exiting Close MongoDB Operations Manager. Hasta luego!")

    except Exception as e:
        logger.error(f"Startup error: {e}", exc_info=True)
        print(f"\nError: {e}")
        print(f"Please check {LOG_FILE} for details")
        sys.exit(1)


if __name__ == "__main__":
    main()
