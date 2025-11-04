"""Tests for StatusBar component."""

from textual.app import App

from close_mongo_ops_manager.statusbar import StatusBar


class StatusBarTestApp(App):
    """Test app for mounting StatusBar (not collected by pytest)."""

    def compose(self):
        yield StatusBar(refresh_interval=2.0)


async def test_set_connection_status_connected():
    """Test setting connection status to connected."""
    async with StatusBarTestApp().run_test() as pilot:
        statusbar = pilot.app.query_one(StatusBar)
        await pilot.pause(0.05)

        # Set connected status
        statusbar.set_connection_status(True, "mongodb://localhost:27017")

        # Verify internal state was updated
        assert statusbar._connection_status == "Connected to mongodb://localhost:27017"


async def test_set_connection_status_disconnected():
    """Test setting connection status to disconnected."""
    async with StatusBarTestApp().run_test() as pilot:
        statusbar = pilot.app.query_one(StatusBar)
        await pilot.pause(0.05)

        # Set disconnected status
        statusbar.set_connection_status(False)

        # Verify internal state shows disconnected
        assert statusbar._connection_status == "Disconnected"


async def test_set_selected_count_updates_display():
    """Test that selected count updates the status display."""
    async with StatusBarTestApp().run_test() as pilot:
        statusbar = pilot.app.query_one(StatusBar)
        await pilot.pause(0.05)

        # Initially no selections
        statusbar.set_selected_count(0)
        assert statusbar._selected_count == 0

        # Set to 5 selections
        statusbar.set_selected_count(5)
        assert statusbar._selected_count == 5


async def test_status_text_formatting():
    """Test that status text includes all expected components."""
    async with StatusBarTestApp().run_test() as pilot:
        statusbar = pilot.app.query_one(StatusBar)
        await pilot.pause(0.05)

        # Set all status components
        statusbar.set_connection_status(True, "mongodb://localhost:27017")
        statusbar.set_refresh_status(True)
        statusbar.set_refresh_interval(5)
        statusbar.set_selected_count(3)

        # Verify all internal states were updated
        assert "mongodb://localhost:27017" in statusbar._connection_status
        assert "enabled" in statusbar._refresh_status
        assert statusbar._refresh_interval == "5s"
        assert statusbar._selected_count == 3
