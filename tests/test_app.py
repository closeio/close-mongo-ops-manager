import pytest
from unittest.mock import AsyncMock, MagicMock, patch

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
    with (
        patch("close_mongo_ops_manager.app.MongoDBManager", new=mock_mongo_manager),
        patch("close_mongo_ops_manager.app.setup_logging"),
    ):
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
    with (
        patch("close_mongo_ops_manager.app.MongoDBManager", new=mock_mongo_manager),
        patch("close_mongo_ops_manager.app.setup_logging"),
    ):
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
        assert "No operations selected" in [n.message for n in pilot.app._notifications]


async def test_kill_selected_action_with_selection(
    app: MongoOpsManager,
):
    """Test kill selected action with selected operations."""
    async with app.run_test() as pilot:
        await pilot.pause(0.1)

        # Create a complete operation object with all expected fields
        test_operation = {
            "opid": "12345",
            "op": "query",
            "ns": "test.collection",
            "client": "127.0.0.1:12345",
            "desc": "conn123",
            "secs_running": 10,
            "command": {"find": "collection", "filter": {"test": "value"}},
        }

        app.operations_view.selected_ops = {"12345"}
        app.mongodb.get_operations.return_value = [test_operation]
        app.mongodb.kill_operation.return_value = True

        # Trigger kill action
        await pilot.press("ctrl+k")
        await pilot.pause(0.2)  # Give more time for dialog to appear

        # Click the "Yes" button directly instead of navigating
        yes_button = app.screen.query_one("#yes")
        await pilot.click("#yes")
        await pilot.pause(0.3)  # Give more time for async operations

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
    monkeypatch.setattr("sys.argv", ["close-mongo-ops-manager", "--host", "localhost", "--port", "27017"])

    main()
    mock_app_run.assert_called_once()


async def test_display_operation_with_mongos_metadata(app: MongoOpsManager):
    """Test that operations with mongos metadata are displayed correctly."""
    # Sample operation with full clientMetadata including mongos info
    sample_operation = {
        "type": "op",
        "host": "am11-mgo-cio1-s25-dn-27.closeinfra.com:27017",
        "desc": "conn11007",
        "client": "10.11.17.243:41024",
        "clientMetadata": {
            "driver": {"name": "PyMongo", "version": "4.6.3"},
            "mongos": {
                "host": "am11-mgo-cio1-rtr-113.closeinfra.com:27020",
                "client": "10.11.149.189:41978",
                "version": "7.0.18-11",
            },
        },
        "opid": 727852,
        "secs_running": 0,
        "microsecs_running": 173436,
        "op": "query",
        "ns": "closeio.activity",
        "active": True,
        "effectiveUsers": [{"user": "closeio2", "db": "closeio"}],
        "command": {
            "find": "activity",
            "filter": {"organization": "test_org"},
        },
    }

    async with app.run_test() as pilot:
        await pilot.pause(0.1)

        # Mock get_operations to return our sample operation
        app.mongodb.get_operations.return_value = [sample_operation]

        # Refresh to load the operation
        await pilot.press("ctrl+r")
        await pilot.pause(0.1)

        # Verify the operation was added to the table
        assert len(app.operations_view.current_ops) == 1
        displayed_op = app.operations_view.current_ops[0]

        # Verify all fields are present
        assert displayed_op["opid"] == 727852
        assert displayed_op["op"] == "query"
        assert displayed_op["ns"] == "closeio.activity"
        assert displayed_op["desc"] == "conn11007"
        assert displayed_op["client"] == "10.11.17.243:41024"

        # Verify clientMetadata structure
        assert "clientMetadata" in displayed_op
        assert "mongos" in displayed_op["clientMetadata"]
        assert (
            displayed_op["clientMetadata"]["mongos"]["host"]
            == "am11-mgo-cio1-rtr-113.closeinfra.com:27020"
        )

        # Verify effective users
        assert len(displayed_op["effectiveUsers"]) == 1
        assert displayed_op["effectiveUsers"][0]["user"] == "closeio2"

        # Verify command details
        assert displayed_op["command"]["find"] == "activity"

        # Check that the data table has exactly one row
        assert app.operations_view.row_count == 1


async def test_refresh_operations_mongodb_none(app: MongoOpsManager):
    """Test refresh_operations when mongodb is None."""
    async with app.run_test() as pilot:
        await pilot.pause(0.1)
        # Set mongodb to None to simulate disconnection
        app.mongodb = None

        # Refresh should not raise an error, just return early
        await pilot.press("ctrl+r")
        await pilot.pause(0.1)

        # View should not be loading
        assert not app.operations_view.loading


async def test_kill_selected_partial_failure(app: MongoOpsManager):
    """Test kill selected when some kills succeed and some fail."""
    async with app.run_test() as pilot:
        await pilot.pause(0.1)

        # Create operations
        operations = [
            {
                "opid": "111",
                "op": "query",
                "ns": "test.collection",
                "client": "127.0.0.1:11111",
                "desc": "conn111",
                "secs_running": 5,
                "command": {"find": "collection"},
            },
            {
                "opid": "222",
                "op": "query",
                "ns": "test.collection",
                "client": "127.0.0.1:22222",
                "desc": "conn222",
                "secs_running": 10,
                "command": {"find": "collection"},
            },
        ]

        app.operations_view.selected_ops = {"111", "222"}
        app.mongodb.get_operations.return_value = operations

        # First kill succeeds, second fails
        app.mongodb.kill_operation.side_effect = [True, False]

        await pilot.press("ctrl+k")
        await pilot.pause(0.2)
        await pilot.click("#yes")
        await pilot.pause(0.3)

        # Should have notified about success and failure
        notifications = [n.message for n in pilot.app._notifications]
        assert any("Successfully killed 1 operation(s)" in msg for msg in notifications)
        assert any("Failed to kill 1 operation(s)" in msg for msg in notifications)


async def test_kill_selected_all_fail(app: MongoOpsManager):
    """Test kill selected when all kill operations fail."""
    async with app.run_test() as pilot:
        await pilot.pause(0.1)

        # Create operation
        operation = {
            "opid": "333",
            "op": "query",
            "ns": "test.collection",
            "client": "127.0.0.1:33333",
            "desc": "conn333",
            "secs_running": 5,
            "command": {"find": "collection"},
        }

        app.operations_view.selected_ops = {"333"}
        app.mongodb.get_operations.return_value = [operation]

        # Kill fails
        app.mongodb.kill_operation.return_value = False

        await pilot.press("ctrl+k")
        await pilot.pause(0.2)
        await pilot.click("#yes")
        await pilot.pause(0.3)

        # Should have notified about failure
        notifications = [n.message for n in pilot.app._notifications]
        assert any("Failed to kill 1 operation(s)" in msg for msg in notifications)


async def test_kill_selected_with_exception(app: MongoOpsManager):
    """Test kill selected when kill_operation raises an exception."""
    async with app.run_test() as pilot:
        await pilot.pause(0.1)

        # Create operation
        operation = {
            "opid": "444",
            "op": "query",
            "ns": "test.collection",
            "client": "127.0.0.1:44444",
            "desc": "conn444",
            "secs_running": 5,
            "command": {"find": "collection"},
        }

        app.operations_view.selected_ops = {"444"}
        app.mongodb.get_operations.return_value = [operation]

        # Kill raises exception
        app.mongodb.kill_operation.side_effect = Exception("Database error")

        await pilot.press("ctrl+k")
        await pilot.pause(0.2)
        await pilot.click("#yes")
        await pilot.pause(0.3)

        # Should have handled the exception and shown error notification
        notifications = [n.message for n in pilot.app._notifications]
        error_notification = any(
            "Failed to kill operation 444" in msg or "Database error" in msg
            for msg in notifications
        )
        assert error_notification


async def test_refresh_operations_get_operations_fails(app: MongoOpsManager):
    """Test refresh when get_operations raises an exception."""
    async with app.run_test() as pilot:
        await pilot.pause(0.1)

        # Make get_operations raise an exception
        app.mongodb.get_operations.side_effect = Exception("Query failed")

        await pilot.press("ctrl+r")
        await pilot.pause(0.2)

        # Should show error notification
        notifications = [n.message for n in pilot.app._notifications]
        assert any("Failed to refresh" in msg for msg in notifications)
