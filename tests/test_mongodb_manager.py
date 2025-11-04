import pytest
from unittest.mock import AsyncMock, MagicMock, patch

from pymongo.errors import PyMongoError

from close_mongo_ops_manager.mongodb_manager import (
    MongoDBManager,
    MongoConnectionError,
    OperationError,
)


@pytest.fixture
def mock_async_mongo_client():
    """Fixture for a mocked AsyncMongoClient."""
    client = MagicMock()
    client.admin.command = AsyncMock(return_value={"ok": 1})
    client.close = AsyncMock()

    # Setup aggregate mock to return a cursor with an async to_list method
    mock_cursor = MagicMock()
    mock_cursor.to_list = AsyncMock(return_value=[])  # default to empty list
    client.admin.aggregate = AsyncMock(return_value=mock_cursor)

    return client


@pytest.fixture
@patch(
    "close_mongo_ops_manager.mongodb_manager.AsyncMongoClient",
    new_callable=MagicMock,
)
async def manager(mock_client_constructor, mock_async_mongo_client):
    """Fixture for a MongoDBManager instance."""
    mock_client_constructor.return_value = mock_async_mongo_client
    manager = await MongoDBManager.connect("mongodb://localhost:27017", "test_ns", True)
    return manager


async def test_connect_success(manager: MongoDBManager, mock_async_mongo_client):
    """Test successful MongoDB connection."""
    assert manager.client == mock_async_mongo_client
    manager.admin_db.command.assert_any_call("ping")


@patch(
    "close_mongo_ops_manager.mongodb_manager.AsyncMongoClient",
    new_callable=MagicMock,
)
async def test_connect_failure(mock_client_constructor):
    """Test MongoDB connection failure."""
    mock_client_constructor.side_effect = PyMongoError("Connection failed")
    with pytest.raises(MongoConnectionError):
        await MongoDBManager.connect("mongodb://localhost:27017", "test_ns", True)


async def test_get_operations(manager: MongoDBManager):
    """Test getting operations from MongoDB."""
    await manager.get_operations()
    manager.admin_db.aggregate.assert_called_once()


async def test_get_operations_with_filters(manager: MongoDBManager):
    """Test getting operations with filters."""
    filters = {"opid": "123", "running_time": "10"}
    await manager.get_operations(filters)
    manager.admin_db.aggregate.assert_called_once()
    assert "$match" in manager.admin_db.aggregate.call_args[0][0][1]


async def test_kill_operation_success(manager: MongoDBManager):
    """Test successful killing of an operation."""
    manager.admin_db.command = AsyncMock(return_value={"ok": 1})
    manager._operation_exists = AsyncMock(return_value=False)
    result = await manager.kill_operation("12345")
    assert result is True
    manager.admin_db.command.assert_called_with("killOp", op=12345)


async def test_kill_operation_sharded(manager: MongoDBManager):
    """Test killing a sharded operation."""
    manager.admin_db.command = AsyncMock(
        side_effect=[PyMongoError("TypeMismatch"), {"ok": 1}]
    )
    manager._operation_exists = AsyncMock(return_value=False)
    result = await manager.kill_operation("shard-0:12345")
    assert result is True
    assert manager.admin_db.command.call_count == 2


async def test_kill_operation_failure(manager: MongoDBManager):
    """Test failure in killing an operation."""
    manager.admin_db.command = AsyncMock(side_effect=PyMongoError("Kill failed"))
    with pytest.raises(OperationError):
        await manager.kill_operation("12345")


async def test_kill_operation_empty_opid(manager: MongoDBManager):
    """Test killing an operation with an empty opid."""
    result = await manager.kill_operation("")
    assert result is False


async def test_operation_exists(manager: MongoDBManager):
    """Test checking if an operation exists."""
    manager.admin_db.aggregate.return_value.to_list.return_value = [{"opid": "123"}]
    assert await manager._operation_exists("123") is True


async def test_operation_does_not_exist(manager: MongoDBManager):
    """Test checking if an operation does not exist."""
    manager.admin_db.aggregate.return_value.to_list.return_value = []
    assert await manager._operation_exists("123") is False


async def test_close_connection(manager: MongoDBManager):
    """Test closing the MongoDB connection."""
    await manager.close()
    manager.client.close.assert_called_once()


async def test_parse_complex_currentop_output(manager: MongoDBManager):
    """Test parsing complex $currentOp output with all metadata fields."""
    # Sample operation from MongoDB $currentOp with full metadata
    sample_operation = {
        "type": "op",
        "host": "am11-mgo-cio1-s25-dn-27.closeinfra.com:27017",
        "desc": "conn11007",
        "connectionId": 11007,
        "client": "10.11.17.243:41024",
        "clientMetadata": {
            "driver": {"name": "PyMongo", "version": "4.6.3"},
            "os": {
                "type": "Linux",
                "name": "Linux",
                "architecture": "x86_64",
                "version": "6.8.0-1031-aws",
            },
            "platform": "CPython 3.12.3.final.0",
            "mongos": {
                "host": "am11-mgo-cio1-rtr-113.closeinfra.com:27020",
                "client": "10.11.149.189:41978",
                "version": "7.0.18-11",
            },
        },
        "active": True,
        "currentOpTime": "2025-11-01T05:37:31.358+00:00",
        "effectiveUsers": [{"user": "closeio2", "db": "closeio"}],
        "runBy": [{"user": "__system", "db": "local"}],
        "threaded": True,
        "opid": 727852,
        "secs_running": 0,
        "microsecs_running": 173436,
        "op": "query",
        "ns": "closeio.activity",
        "redacted": False,
        "command": {
            "find": "activity",
            "filter": {
                "organization": "orga_wFx9LC3AImbzDu5S9UJRSIeyFU9230B8LhQlhA8lWvU",
                "_cls": "Activity.Email",
            },
            "sort": {"date_created": -1},
            "skip": 77600,
            "limit": 101,
        },
        "numYields": 21,
        "locks": {"FeatureCompatibilityVersion": "r", "Global": "r"},
        "waitingForLock": False,
        "waitingForFlowControl": False,
    }

    # Mock the aggregate method to return the sample operation
    manager.admin_db.aggregate.return_value.to_list.return_value = [sample_operation]

    # Get operations
    operations = await manager.get_operations()

    # Verify the operation was returned
    assert len(operations) == 1
    op = operations[0]

    # Verify all key fields are present and correct
    assert op["opid"] == 727852
    assert op["type"] == "op"
    assert op["op"] == "query"
    assert op["ns"] == "closeio.activity"
    assert op["desc"] == "conn11007"
    assert op["client"] == "10.11.17.243:41024"
    assert op["secs_running"] == 0
    assert op["microsecs_running"] == 173436
    assert op["active"] is True

    # Verify effective users
    assert "effectiveUsers" in op
    assert len(op["effectiveUsers"]) == 1
    assert op["effectiveUsers"][0]["user"] == "closeio2"
    assert op["effectiveUsers"][0]["db"] == "closeio"

    # Verify clientMetadata is present
    assert "clientMetadata" in op
    assert op["clientMetadata"]["driver"]["name"] == "PyMongo"
    assert op["clientMetadata"]["driver"]["version"] == "4.6.3"

    # Verify mongos metadata
    assert "mongos" in op["clientMetadata"]
    assert op["clientMetadata"]["mongos"]["host"] == "am11-mgo-cio1-rtr-113.closeinfra.com:27020"

    # Verify command details
    assert "command" in op
    assert op["command"]["find"] == "activity"
    assert op["command"]["limit"] == 101

    # Verify locks information
    assert "locks" in op
    assert op["locks"]["Global"] == "r"
