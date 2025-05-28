from textual.widgets import Static


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

    def __init__(self, refresh_interval: float) -> None:
        super().__init__()
        self._connection_status = "Connecting..."
        self._refresh_status = "Auto-refresh paused"
        self._refresh_interval = f"{str(refresh_interval)}s"
        self._selected_count = 0
        self._update_text()

    def _update_text(self) -> None:
        selected_text = (
            f" | Selected: {self._selected_count}" if self._selected_count > 0 else ""
        )
        text = f"{self._connection_status} | {self._refresh_status} ({self._refresh_interval}){selected_text}"
        self.update(text)

    def set_connection_status(self, connected: bool, details: str = "") -> None:
        if connected:
            self._connection_status = f"Connected to {details}"
        else:
            self._connection_status = details if details else "Disconnected"
        self._update_text()

    def set_refresh_status(self, enabled: bool) -> None:
        self._refresh_status = (
            "Auto-refresh enabled" if enabled else "Auto-refresh paused"
        )
        self._update_text()

    def set_refresh_interval(self, interval: float) -> None:
        self._refresh_interval = f"{interval}s"
        self._update_text()

    def set_selected_count(self, count: int) -> None:
        """Set the number of selected operations.

        Args:
            count: The number of selected operations.
        """
        self._selected_count = count
        self._update_text()
