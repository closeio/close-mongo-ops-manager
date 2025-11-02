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
    manager = await MongoDBManager.connect(
        "mongodb://localhost:27017", "test_ns", True
    )
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
