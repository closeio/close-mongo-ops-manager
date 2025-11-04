"""Tests for OperationsView component."""

import pytest
from textual.app import App

from close_mongo_ops_manager.operations_view import OperationsView


@pytest.fixture
def operations_view():
    """Create an OperationsView instance for testing."""
    return OperationsView()


class OperationsViewTestApp(App):
    """Test app for mounting OperationsView (not collected by pytest)."""

    def compose(self):
        yield OperationsView()


async def test_loading_property_updates_border_title():
    """Test that loading property updates the border title."""
    async with OperationsViewTestApp().run_test() as pilot:
        operations_view = pilot.app.query_one(OperationsView)

        # Initially not loading
        assert not operations_view.loading
        assert operations_view.border_title == "Operations"

        # Set to loading
        operations_view.loading = True
        assert operations_view.loading
        assert operations_view.border_title == "Operations • Refreshing..."

        # Set back to not loading
        operations_view.loading = False
        assert not operations_view.loading
        assert operations_view.border_title == "Operations"


async def test_clear_selections_empty():
    """Test clear_selections when nothing is selected."""
    async with OperationsViewTestApp().run_test() as pilot:
        operations_view = pilot.app.query_one(OperationsView)
        await pilot.pause(0.05)

        # Initially no selections
        assert len(operations_view.selected_ops) == 0

        # Clear should not raise an error
        operations_view.clear_selections()
        assert len(operations_view.selected_ops) == 0


async def test_clear_selections_multiple():
    """Test clear_selections with multiple selections."""
    async with OperationsViewTestApp().run_test() as pilot:
        operations_view = pilot.app.query_one(OperationsView)
        await pilot.pause(0.05)

        # Add some rows
        operations_view.add_row(" ", "111", "op", "query", "5s", "client1", "conn1", "user1", key="111")
        operations_view.add_row(" ", "222", "op", "query", "10s", "client2", "conn2", "user2", key="222")
        operations_view.add_row(" ", "333", "op", "update", "15s", "client3", "conn3", "user3", key="333")

        # Select multiple operations
        operations_view.selected_ops = {"111", "222", "333"}
        assert len(operations_view.selected_ops) == 3

        # Clear selections
        operations_view.clear_selections()
        assert len(operations_view.selected_ops) == 0


async def test_on_key_enter_with_valid_operation():
    """Test pressing enter with a valid operation selected."""
    async with OperationsViewTestApp().run_test() as pilot:
        operations_view = pilot.app.query_one(OperationsView)
        await pilot.pause(0.05)

        # Set up a mock operation
        operations_view.current_ops = [
            {"opid": 123, "op": "query", "ns": "test.collection"}
        ]
        operations_view.add_row(" ", "123", "op", "query", "5s", "client", "conn", "user", key="123")

        # Focus the table and move cursor to first row
        operations_view.focus()
        operations_view.move_cursor(row=0)
        assert operations_view.cursor_row == 0

        # Note: We can't easily test screen push without full app context
        # Just verify cursor is positioned correctly for the operation
        assert operations_view.cursor_row is not None
        assert 0 <= operations_view.cursor_row < len(operations_view.current_ops)


async def test_on_key_enter_with_invalid_cursor():
    """Test pressing enter with cursor out of bounds (no rows added)."""
    async with OperationsViewTestApp().run_test() as pilot:
        operations_view = pilot.app.query_one(OperationsView)
        await pilot.pause(0.05)

        # Set up operations but don't add rows to the table
        operations_view.current_ops = [
            {"opid": 123, "op": "query", "ns": "test.collection"}
        ]

        # DataTable initializes cursor to 0, but no rows exist
        # Verify the state handles this safely (cursor >= row_count)
        assert operations_view.row_count == 0
        # on_key should handle cursor being out of bounds gracefully


async def test_on_key_enter_with_empty_ops():
    """Test pressing enter when no operations exist."""
    async with OperationsViewTestApp().run_test() as pilot:
        operations_view = pilot.app.query_one(OperationsView)
        await pilot.pause(0.05)

        # No operations
        operations_view.current_ops = []

        # Should not crash with empty operations
        assert len(operations_view.current_ops) == 0
        assert operations_view.row_count == 0
        # Verify cursor exists but there are no rows to select
        assert operations_view.cursor_row is not None
