"""Tests for OperationDetailsScreen."""

from textual.app import App, ComposeResult
from textual.widgets import TextArea

from close_mongo_ops_manager.operation_details_screen import OperationDetailsScreen


class DetailsTestApp(App):
    """Test app that pushes the OperationDetailsScreen."""

    def __init__(self, operation: dict):
        super().__init__()
        self.operation = operation

    def compose(self) -> ComposeResult:
        yield from ()

    def on_mount(self) -> None:
        self.push_screen(OperationDetailsScreen(self.operation))


async def test_details_displays_basic_fields(sample_mongodb_operation):
    """Test that basic operation fields are displayed."""
    app = DetailsTestApp(sample_mongodb_operation)
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        text_area = app.screen.query_one(TextArea)
        content = text_area.text
        assert "123456" in content
        assert "query" in content
        assert "testdb.collection" in content
        assert "5s" in content


async def test_details_displays_client_info(sample_mongodb_operation):
    """Test that client info is displayed."""
    app = DetailsTestApp(sample_mongodb_operation)
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        text_area = app.screen.query_one(TextArea)
        content = text_area.text
        assert "192.168.1.100:54321" in content


async def test_details_displays_mongos_metadata(sample_mongodb_operation_with_mongos):
    """Test that mongos metadata is extracted and displayed."""
    app = DetailsTestApp(sample_mongodb_operation_with_mongos)
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        text_area = app.screen.query_one(TextArea)
        content = text_area.text
        # Should contain the short hostname from mongos
        assert "mongos-router" in content


async def test_details_displays_command_details(sample_mongodb_operation):
    """Test that command details are formatted as key-value pairs."""
    app = DetailsTestApp(sample_mongodb_operation)
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        text_area = app.screen.query_one(TextArea)
        content = text_area.text
        assert "Command Details:" in content
        assert "find: collection" in content


async def test_details_displays_plan_summary():
    """Test that plan summary is displayed when present."""
    op = {
        "opid": 999,
        "op": "query",
        "ns": "test.col",
        "secs_running": 1,
        "client": "127.0.0.1:1234",
        "command": {"find": "col"},
        "planSummary": "IXSCAN { _id: 1 }",
    }
    app = DetailsTestApp(op)
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        text_area = app.screen.query_one(TextArea)
        content = text_area.text
        assert "Plan Summary: IXSCAN { _id: 1 }" in content


async def test_details_missing_fields_show_fallback():
    """Test that missing fields show N/A fallback."""
    op = {"opid": 1}  # Minimal operation
    app = DetailsTestApp(op)
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        text_area = app.screen.query_one(TextArea)
        content = text_area.text
        assert "Type: N/A" in content
        assert "Namespace: N/A" in content


async def test_details_escape_dismisses():
    """Test that pressing Escape dismisses the details screen."""
    op = {"opid": 1, "op": "query", "ns": "test.col"}
    app = DetailsTestApp(op)
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        assert len(app.screen_stack) > 1
        await pilot.press("escape")
        await pilot.pause(0.1)
        assert len(app.screen_stack) == 1
