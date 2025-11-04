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
                "organization": "orga_b1ae86c0973cc6f0210b53d508ca3641fb6d0c56823",
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


async def test_get_operations_with_missing_opid(manager: MongoDBManager):
    """Test handling of operations without opid field."""
    operations_without_opid = [
        {"op": "query", "ns": "test.collection"},  # Missing opid
        {"opid": 12345, "op": "update", "ns": "test.users"},  # Valid
    ]
    manager.admin_db.aggregate.return_value.to_list.return_value = (
        operations_without_opid
    )

    # Should return both, but app logic should handle missing opid gracefully
    operations = await manager.get_operations()
    assert len(operations) == 2


async def test_get_operations_with_null_values(manager: MongoDBManager):
    """Test handling of None/null values in operation fields."""
    operation_with_nulls = {
        "opid": 12345,
        "op": None,  # Null operation type
        "ns": None,  # Null namespace
        "client": None,
        "desc": None,
        "secs_running": 0,
        "effectiveUsers": None,
        "command": None,
    }
    manager.admin_db.aggregate.return_value.to_list.return_value = [
        operation_with_nulls
    ]

    operations = await manager.get_operations()
    assert len(operations) == 1
    assert operations[0]["opid"] == 12345
    assert operations[0]["op"] is None


async def test_get_operations_empty_result(manager: MongoDBManager):
    """Test get_operations when MongoDB returns empty array."""
    manager.admin_db.aggregate.return_value.to_list.return_value = []

    operations = await manager.get_operations()
    assert operations == []
    manager.admin_db.aggregate.assert_called_once()


async def test_get_operations_database_not_initialized(manager: MongoDBManager):
    """Test get_operations when admin_db is None."""
    manager.admin_db = None

    operations = await manager.get_operations()
    assert operations == []


async def test_filter_running_time_non_numeric(manager: MongoDBManager):
    """Test running_time filter with non-numeric value."""
    manager.admin_db.aggregate.return_value.to_list.return_value = []

    # Non-numeric running_time should be ignored
    filters = {"running_time": "abc"}
    _ = await manager.get_operations(filters)

    # Verify aggregate was called
    call_args = manager.admin_db.aggregate.call_args[0][0]

    # The pipeline should NOT have a secs_running filter since "abc" is not a digit
    match_stages = [stage for stage in call_args if "$match" in stage]
    if match_stages:
        match_stage = match_stages[0]["$match"]
        # Should not contain secs_running condition
        and_conditions = match_stage.get("$and", [])
        has_secs_running = any("secs_running" in cond for cond in and_conditions)
        assert not has_secs_running


async def test_filter_running_time_negative(manager: MongoDBManager):
    """Test running_time filter with negative value."""
    manager.admin_db.aggregate.return_value.to_list.return_value = []

    # Negative running_time (technically isdigit() returns False for negative)
    filters = {"running_time": "-5"}
    _ = await manager.get_operations(filters)

    # Verify the filter is not applied since "-5".isdigit() is False
    call_args = manager.admin_db.aggregate.call_args[0][0]
    match_stages = [stage for stage in call_args if "$match" in stage]
    if match_stages:
        match_stage = match_stages[0]["$match"]
        and_conditions = match_stage.get("$and", [])
        has_secs_running = any("secs_running" in cond for cond in and_conditions)
        assert not has_secs_running


async def test_filter_running_time_zero(manager: MongoDBManager):
    """Test running_time filter with zero value."""
    manager.admin_db.aggregate.return_value.to_list.return_value = []

    # Zero is a valid filter value
    filters = {"running_time": "0"}
    _ = await manager.get_operations(filters)

    # Verify the filter was applied with value 0
    call_args = manager.admin_db.aggregate.call_args[0][0]
    match_stages = [stage for stage in call_args if "$match" in stage]
    assert match_stages
    match_stage = match_stages[0]["$match"]
    and_conditions = match_stage.get("$and", [])

    # Find the secs_running condition
    secs_running_cond = next(
        (cond for cond in and_conditions if "secs_running" in cond), None
    )
    assert secs_running_cond is not None
    assert secs_running_cond["secs_running"]["$gte"] == 0


async def test_filter_multiple_criteria_combined(manager: MongoDBManager):
    """Test filtering with all filter criteria combined."""
    manager.admin_db.aggregate.return_value.to_list.return_value = []

    # Apply all filters at once
    filters = {
        "opid": "123",
        "operation": "query",
        "client": "192.168",
        "description": "conn",
        "effective_users": "testuser",
        "running_time": "10",
    }
    _ = await manager.get_operations(filters)

    # Verify all filters were added to the pipeline
    call_args = manager.admin_db.aggregate.call_args[0][0]
    match_stages = [stage for stage in call_args if "$match" in stage]
    assert match_stages
    match_stage = match_stages[0]["$match"]
    and_conditions = match_stage.get("$and", [])

    # Should have multiple conditions (system ops filter + namespace + all our filters)
    # At minimum: 1 (system ops) + 1 (our filters) = 2, likely more
    assert len(and_conditions) >= 2

    # Verify specific filter conditions exist
    has_opid = any("opid" in cond for cond in and_conditions)
    has_op = any("op" in cond for cond in and_conditions)
    has_client = any(
        "$or" in cond and any("client" in or_cond for or_cond in cond["$or"])
        for cond in and_conditions
    )
    has_desc = any("desc" in cond for cond in and_conditions)
    has_effective_users = any("effectiveUsers" in cond for cond in and_conditions)
    has_secs_running = any("secs_running" in cond for cond in and_conditions)

    assert has_opid
    assert has_op
    assert has_client
    assert has_desc
    assert has_effective_users
    assert has_secs_running


async def test_kill_operation_verification_timeout(manager: MongoDBManager):
    """Test kill operation when verification times out (operation never dies)."""
    manager.admin_db.command = AsyncMock(return_value={"ok": 1})
    # Operation always exists (never dies)
    manager._operation_exists = AsyncMock(return_value=True)

    # Use very short timeout for testing
    result = await manager.kill_operation("12345", max_retries=1, verify_timeout=0.1)

    # Should return False when operation doesn't die within timeout
    assert result is False
    manager.admin_db.command.assert_called_with("killOp", op=12345)


async def test_kill_operation_immediate_verification(manager: MongoDBManager):
    """Test kill operation when operation dies immediately."""
    manager.admin_db.command = AsyncMock(return_value={"ok": 1})
    # Operation dies immediately
    manager._operation_exists = AsyncMock(return_value=False)

    result = await manager.kill_operation("12345")

    assert result is True
    manager.admin_db.command.assert_called_with("killOp", op=12345)
    manager._operation_exists.assert_called_once_with("12345")


async def test_kill_operation_negative_opid(manager: MongoDBManager):
    """Test kill operation with negative opid string."""
    manager.admin_db.command = AsyncMock(return_value={"ok": 1})
    manager._operation_exists = AsyncMock(return_value=False)

    # Negative opids: "-123".isdigit() returns False, so string is used
    result = await manager.kill_operation("-123")

    assert result is True
    # Should use string, not convert to int
    manager.admin_db.command.assert_called_with("killOp", op="-123")


async def test_kill_operation_very_large_opid(manager: MongoDBManager):
    """Test kill operation with very large opid that could cause overflow."""
    manager.admin_db.command = AsyncMock(return_value={"ok": 1})
    manager._operation_exists = AsyncMock(return_value=False)

    # Very large number that fits in Python int but might overflow in other contexts
    large_opid = "999999999999999999"
    result = await manager.kill_operation(large_opid)

    assert result is True
    # Should convert to int successfully
    manager.admin_db.command.assert_called_with("killOp", op=999999999999999999)


async def test_kill_operation_whitespace_opid(manager: MongoDBManager):
    """Test kill operation with opid containing whitespace."""
    manager.admin_db.command = AsyncMock(return_value={"ok": 1})
    manager._operation_exists = AsyncMock(return_value=False)

    # Opid with leading/trailing whitespace
    result = await manager.kill_operation("  12345  ")

    assert result is True
    # Should strip whitespace and convert to int
    manager.admin_db.command.assert_called_with("killOp", op=12345)


async def test_kill_operation_database_not_initialized(manager: MongoDBManager):
    """Test kill operation when admin_db is None."""
    manager.admin_db = None

    result = await manager.kill_operation("12345")

    assert result is False


async def test_get_operations_pymongo_error(manager: MongoDBManager):
    """Test get_operations when MongoDB query fails."""
    from close_mongo_ops_manager.exceptions import OperationError

    # Simulate database error
    manager.admin_db.aggregate = AsyncMock(side_effect=PyMongoError("Connection lost"))

    with pytest.raises(OperationError) as exc_info:
        await manager.get_operations()

    assert "Failed to get operations" in str(exc_info.value)
    assert "Connection lost" in str(exc_info.value)


async def test_operation_exists_pymongo_error(manager: MongoDBManager):
    """Test _operation_exists when database query fails."""
    # Simulate PyMongoError
    mock_cursor = MagicMock()
    mock_cursor.to_list = AsyncMock(side_effect=PyMongoError("Query failed"))
    manager.admin_db.aggregate = AsyncMock(return_value=mock_cursor)

    result = await manager._operation_exists("12345")

    # Should return False on error (logged but not raised)
    assert result is False


async def test_operation_exists_unexpected_error(manager: MongoDBManager):
    """Test _operation_exists when unexpected exception occurs."""
    # Simulate unexpected error (not PyMongoError)
    mock_cursor = MagicMock()
    mock_cursor.to_list = AsyncMock(side_effect=RuntimeError("Unexpected error"))
    manager.admin_db.aggregate = AsyncMock(return_value=mock_cursor)

    result = await manager._operation_exists("12345")

    # Should return False on error (logged but not raised)
    assert result is False


async def test_kill_operation_all_retries_exhausted(manager: MongoDBManager):
    """Test kill operation when all retries fail."""
    from close_mongo_ops_manager.exceptions import OperationError

    # All attempts fail with PyMongoError
    manager.admin_db.command = AsyncMock(
        side_effect=PyMongoError("Command failed persistently")
    )

    with pytest.raises(OperationError) as exc_info:
        await manager.kill_operation("12345", max_retries=2, verify_timeout=0.1)

    assert "Failed to kill operation" in str(exc_info.value)
    assert "2 attempts" in str(exc_info.value)
    # Should have tried twice
    assert manager.admin_db.command.call_count == 2
