from textual.binding import Binding
from textual.app import ComposeResult
from textual.containers import ScrollableContainer, VerticalScroll
from textual.screen import ModalScreen
from textual.widgets import TextArea


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
        Binding("up", "scroll_up", "Scroll Up", show=False),
        Binding("down", "scroll_down", "Scroll Down", show=False),
        Binding("pageup", "page_up", "Page Up", show=False),
        Binding("pagedown", "page_down", "Page Down", show=False),
        Binding("home", "scroll_home", "Home", show=False),
        Binding("end", "scroll_end", "End", show=False),
    ]

    def __init__(self, operation: dict) -> None:
        super().__init__()
        self.operation = operation

    def action_scroll_up(self) -> None:
        """Scroll up in the details container."""
        container = self.query_one("#details-container", ScrollableContainer)
        container.scroll_up()

    def action_scroll_down(self) -> None:
        """Scroll down in the details container."""
        container = self.query_one("#details-container", ScrollableContainer)
        container.scroll_down()

    def action_page_up(self) -> None:
        """Page up in the details container."""
        container = self.query_one("#details-container", ScrollableContainer)
        container.scroll_page_up()

    def action_page_down(self) -> None:
        """Page down in the details container."""
        container = self.query_one("#details-container", ScrollableContainer)
        container.scroll_page_down()

    def action_scroll_home(self) -> None:
        """Scroll to the top of the details container."""
        container = self.query_one("#details-container", ScrollableContainer)
        container.scroll_home()

    def action_scroll_end(self) -> None:
        """Scroll to the bottom of the details container."""
        container = self.query_one("#details-container", ScrollableContainer)
        container.scroll_end()

    def compose(self) -> ComposeResult:
        with ScrollableContainer(id="details-container"):
            with VerticalScroll(id="details-content"):
                op = self.operation
                details: list[str] = []
                details.append(f"Operation ID: {op.get('opid', 'N/A')}")
                details.append(f"Type: {op.get('op', 'N/A')}")
                details.append(f"Namespace: {op.get('ns', 'N/A')}")

                secs_running = op.get("secs_running", 0)
                microsecs_running = op.get("microsecs_running")
                if microsecs_running is not None:
                    details.append(
                        f"Running Time: {secs_running}s ({microsecs_running}µs)"
                    )
                else:
                    details.append(f"Running Time: {secs_running}s")

                if op.get("active") is not None:
                    details.append(f"Active: {op.get('active')}")

                # Get client info with fallbacks
                client_info = op.get("client_s") or op.get("client") or "N/A"

                # Add mongos host info if available
                mongos_host = (
                    op.get("clientMetadata", {}).get("mongos", {}).get("host", "")
                )
                if mongos_host:
                    short_host = mongos_host.split(".", 1)[0]
                    client_info = f"{client_info} ({short_host})"

                details.append(f"Client: {client_info}")

                app_name = op.get("appName")
                if app_name:
                    details.append(f"App Name: {app_name}")

                host = op.get("host")
                connection_id = op.get("connectionId")
                if host or connection_id is not None:
                    host_str = host or "N/A"
                    if connection_id is not None:
                        host_str = f"{host_str} (conn {connection_id})"
                    details.append(f"Server: {host_str}")

                desc = op.get("desc")
                if desc:
                    details.append(f"Description: {desc}")

                effective_users = op.get("effectiveUsers") or []
                if effective_users:
                    users_str = ", ".join(
                        f"{u.get('user', '?')}@{u.get('db', '?')}"
                        for u in effective_users
                        if u
                    )
                    details.append(f"Effective Users: {users_str}")

                msg = op.get("msg")
                if msg:
                    details.append(f"\nStatus: {msg}")

                progress = op.get("progress") or {}
                if progress:
                    done = progress.get("done")
                    total = progress.get("total")
                    if total:
                        pct = (
                            (done / total * 100)
                            if isinstance(done, (int, float))
                            else 0
                        )
                        details.append(f"Progress: {done}/{total} ({pct:.1f}%)")
                    else:
                        details.append(f"Progress: {progress}")

                num_yields = op.get("numYields")
                if num_yields is not None:
                    details.append(f"Num Yields: {num_yields}")

                query_framework = op.get("queryFramework")
                if query_framework:
                    details.append(f"Query Framework: {query_framework}")

                plan_summary = op.get("planSummary")
                if plan_summary:
                    details.append(f"Plan Summary: {plan_summary}")

                write_conflicts = op.get("writeConflicts")
                if write_conflicts:
                    details.append(f"Write Conflicts: {write_conflicts}")

                prepare_read_conflicts = op.get("prepareReadConflicts")
                if prepare_read_conflicts:
                    details.append(f"Prepare Read Conflicts: {prepare_read_conflicts}")

                throughput_last = op.get("dataThroughputLastSecond")
                throughput_avg = op.get("dataThroughputAverage")
                if throughput_last is not None or throughput_avg is not None:
                    details.append(
                        "Throughput: "
                        f"last={throughput_last} avg={throughput_avg} bytes/s"
                    )

                shard = op.get("shard")
                if shard:
                    details.append(f"Shard: {shard}")

                cursor = op.get("cursor") or {}
                if cursor:
                    details.append("\nCursor:")
                    for key in (
                        "cursorId",
                        "createdDate",
                        "lastAccessDate",
                        "nDocsReturned",
                        "nBatchesReturned",
                        "noCursorTimeout",
                        "tailable",
                        "awaitData",
                        "planSummary",
                    ):
                        if key in cursor:
                            details.append(f"  {key}: {cursor[key]}")

                transaction = op.get("transaction") or {}
                if transaction:
                    details.append("\nTransaction:")
                    params = transaction.get("parameters") or {}
                    for key in ("txnNumber", "autocommit"):
                        if key in params:
                            details.append(f"  {key}: {params[key]}")
                    read_concern = params.get("readConcern") or {}
                    if read_concern.get("level"):
                        details.append(f"  readConcern.level: {read_concern['level']}")
                    for key in (
                        "readTimestamp",
                        "startWallClockTime",
                        "timeOpenMicros",
                        "timeActiveMicros",
                        "timeInactiveMicros",
                        "expiryTime",
                    ):
                        if key in transaction:
                            details.append(f"  {key}: {transaction[key]}")

                two_pc = op.get("twoPhaseCommitCoordinator") or {}
                if two_pc:
                    details.append("\n2PC Coordinator:")
                    for key in (
                        "txnNumber",
                        "numParticipants",
                        "state",
                        "commitStartTime",
                        "hasRecoveredFromFailover",
                        "deadline",
                    ):
                        if key in two_pc:
                            details.append(f"  {key}: {two_pc[key]}")

                locks = op.get("locks") or {}
                if locks:
                    details.append("\nLocks:")
                    for key, value in locks.items():
                        details.append(f"  {key}: {value}")
                if op.get("waitingForLock"):
                    details.append("  waitingForLock: True")

                command = op.get("command") or {}
                if command:
                    details.append("\nCommand Details:")
                    for key, value in command.items():
                        details.append(f"  {key}: {value}")

                yield TextArea(
                    "\n".join(details), classes="details-text", read_only=True
                )
