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


async def test_details_displays_mongodb_8_fields():
    """MongoDB 8.0 fields should render when present."""
    op = {
        "opid": 42,
        "op": "getmore",
        "ns": "test.col",
        "secs_running": 3,
        "microsecs_running": 3_456_789,
        "client": "10.0.0.1:5000",
        "appName": "ETL-runner",
        "host": "shard-0:27017",
        "connectionId": 88,
        "numYields": 7,
        "queryFramework": "sbe",
        "msg": "Index Build: scanning collection",
        "progress": {"done": 250, "total": 1000},
        "writeConflicts": 4,
        "dataThroughputLastSecond": 1024,
        "dataThroughputAverage": 2048,
        "shard": "shard-0",
        "cursor": {
            "cursorId": 99,
            "nDocsReturned": 500,
            "tailable": False,
        },
        "transaction": {
            "parameters": {
                "txnNumber": 5,
                "autocommit": False,
                "readConcern": {"level": "snapshot"},
            },
            "timeOpenMicros": 12345,
        },
        "command": {"getMore": 99, "collection": "col"},
    }
    app = DetailsTestApp(op)
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        content = app.screen.query_one(TextArea).text

    assert "App Name: ETL-runner" in content
    assert "Server: shard-0:27017 (conn 88)" in content
    assert "Num Yields: 7" in content
    assert "Query Framework: sbe" in content
    assert "Status: Index Build: scanning collection" in content
    assert "Progress: 250/1000 (25.0%)" in content
    assert "Write Conflicts: 4" in content
    assert "Throughput: last=1024 avg=2048 bytes/s" in content
    assert "Shard: shard-0" in content
    assert "Cursor:" in content
    assert "cursorId: 99" in content
    assert "Transaction:" in content
    assert "txnNumber: 5" in content
    assert "readConcern.level: snapshot" in content


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
