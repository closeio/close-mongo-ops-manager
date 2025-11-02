import pytest
from unittest.mock import AsyncMock, MagicMock, patch

from textual import events
from textual.pilot import Pilot
from pymongo.errors import PyMongoError

from close_mongo_ops_manager.app import MongoOpsManager, main
from close_mongo_ops_manager.mongodb_manager import MongoDBManager


@pytest.fixture
def mock_mongo_manager():
    """Fixture for a mocked MongoDBManager."""
    manager = MagicMock(spec=MongoDBManager)
    manager.connect = AsyncMock(return_value=manager)
    manager.get_operations = AsyncMock(return_value=[])
    manager.kill_operation = AsyncMock(return_value=True)
    manager.close = AsyncMock()
    return manager


@pytest.fixture
async def app(mock_mongo_manager):
    """Fixture for the MongoOpsManager app."""
    with patch(
        "close_mongo_ops_manager.app.MongoDBManager", new=mock_mongo_manager
    ), patch("close_mongo_ops_manager.app.setup_logging"):
        app = MongoOpsManager(connection_string="mongodb://localhost:27017")
        yield app


async def test_app_initialization(app: MongoOpsManager):
    """Test that the app initializes correctly."""
    assert app.title.startswith("Close MongoDB Operations Manager")
    assert app.connection_string == "mongodb://localhost:27017"


async def test_app_mount_and_connection(app: MongoOpsManager):
    """Test app mounting and MongoDB connection."""
    async with app.run_test() as pilot:
        await pilot.pause(0.1)  # Allow on_mount to complete
        assert app.mongodb is not None
        app.mongodb.connect.assert_called_once()
        assert app.operations_view.loading is False


async def test_app_connection_failure(mock_mongo_manager):
    """Test app behavior on MongoDB connection failure."""
    mock_mongo_manager.connect.side_effect = PyMongoError("Connection failed")
    with patch(
        "close_mongo_ops_manager.app.MongoDBManager", new=mock_mongo_manager
    ), patch("close_mongo_ops_manager.app.setup_logging"):
        app = MongoOpsManager(connection_string="mongodb://localhost:27017")
        async with app.run_test() as pilot:
            await pilot.pause(0.1)
            # Check for the specific error message
            assert any(
                "Failed to connect: Connection failed" in n.message
                for n in pilot.app._notifications
            )


async def test_refresh_action(app: MongoOpsManager):
    """Test the refresh action."""
    async with app.run_test() as pilot:
        await pilot.pause(0.1)  # Allow connection to establish
        app.mongodb.get_operations.reset_mock()
        await pilot.press("ctrl+r")
        app.mongodb.get_operations.assert_called_once()


async def test_toggle_refresh_action(app: MongoOpsManager):
    """Test toggling auto-refresh."""
    async with app.run_test() as pilot:
        assert app.auto_refresh is True
        await pilot.press("ctrl+p")
        assert app.auto_refresh is False
        await pilot.press("ctrl+p")
        assert app.auto_refresh is True


async def test_kill_selected_action_no_selection(
    app: MongoOpsManager,
):
    """Test kill selected action with no operations selected."""
    async with app.run_test() as pilot:
        await pilot.press("ctrl+k")
        assert "No operations selected" in [
            n.message for n in pilot.app._notifications
        ]


async def test_kill_selected_action_with_selection(
    app: MongoOpsManager,
):
    """Test kill selected action with selected operations."""
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        app.operations_view.selected_ops = {"12345"}
        app.mongodb.get_operations.return_value = [{"opid": "12345"}]
        app.mongodb.kill_operation.return_value = True

        await pilot.press("ctrl+k")
        await pilot.pause(0.1)
        await pilot.press("left")  # Move focus to "Yes" button
        await pilot.press("enter")
        await pilot.pause(0.1)

        app.mongodb.kill_operation.assert_called_with("12345")
        assert "Successfully killed 1 operation(s)" in [
            n.message for n in pilot.app._notifications
        ]
        assert not app.operations_view.selected_ops


async def test_show_help_action(app: MongoOpsManager):
    """Test showing the help screen."""
    async with app.run_test() as pilot:
        await pilot.press("f1")
        assert app.screen_stack[-1].id == "help_screen"


async def test_show_logs_action(app: MongoOpsManager):
    """Test showing the log viewer screen."""
    async with app.run_test() as pilot:
        await pilot.press("ctrl+l")
        assert app.screen_stack[-1].id == "log_screen"


async def test_change_theme_action(app: MongoOpsManager):
    """Test showing the theme selection screen."""
    async with app.run_test() as pilot:
        await pilot.press("ctrl+t")
        assert app.screen_stack[-1].id == "theme_screen"


async def test_increase_decrease_refresh_interval(
    app: MongoOpsManager,
):
    """Test increasing and decreasing the refresh interval."""
    async with app.run_test() as pilot:
        initial_interval = app.refresh_interval
        await pilot.press("ctrl+equals_sign")
        assert app.refresh_interval == initial_interval + 1
        await pilot.press("ctrl+minus")
        assert app.refresh_interval == initial_interval


async def test_toggle_filter_bar_action(app: MongoOpsManager):
    """Test toggling the filter bar visibility."""
    async with app.run_test() as pilot:
        filter_bar = app.query_one("FilterBar")
        assert filter_bar.has_class("hidden")
        await pilot.press("ctrl+f")
        assert not filter_bar.has_class("hidden")
        await pilot.press("ctrl+f")
        assert filter_bar.has_class("hidden")


async def test_sort_by_time_action(app: MongoOpsManager):
    """Test sorting operations by running time."""
    async with app.run_test() as pilot:
        assert getattr(app.operations_view, "sort_running_time_asc", True)
        await pilot.press("ctrl+s")
        assert not app.operations_view.sort_running_time_asc
        await pilot.press("ctrl+s")
        assert app.operations_view.sort_running_time_asc


async def test_toggle_selection_action(app: MongoOpsManager):
    """Test toggling selection of all operations."""
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        app.mongodb.get_operations.return_value = [
            {"opid": "1"},
            {"opid": "2"},
        ]
        await pilot.press("ctrl+r")
        await pilot.pause(0.1)

        assert not app.operations_view.selected_ops
        await pilot.press("ctrl+a")
        await pilot.pause(0.1)  # need to wait for rows to be added.
        assert len(app.operations_view.selected_ops) == 2

        await pilot.press("ctrl+a")
        assert not app.operations_view.selected_ops


def test_main_function(monkeypatch):
    """Test the main function of the app."""
    mock_app_run = MagicMock()
    monkeypatch.setattr("close_mongo_ops_manager.app.MongoOpsManager.run", mock_app_run)
    monkeypatch.setattr("sys.argv", ["close-mongo-ops-manager", "--host", "test_host"])

    main()
    mock_app_run.assert_called_once()
