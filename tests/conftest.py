"""Shared test fixtures for close-mongo-ops-manager test suite."""

import pytest
from unittest.mock import AsyncMock, MagicMock


@pytest.fixture
def sample_mongodb_operation():
    """Return a realistic MongoDB operation with all common fields."""
    return {
        "type": "op",
        "host": "mongodb-server:27017",
        "desc": "conn12345",
        "connectionId": 12345,
        "client": "192.168.1.100:54321",
        "clientMetadata": {
            "driver": {"name": "PyMongo", "version": "4.6.3"},
            "os": {
                "type": "Linux",
                "name": "Linux",
                "architecture": "x86_64",
                "version": "6.8.0-1031-aws",
            },
            "platform": "CPython 3.12.3.final.0",
        },
        "active": True,
        "currentOpTime": "2025-11-03T10:00:00.000+00:00",
        "effectiveUsers": [{"user": "testuser", "db": "testdb"}],
        "runBy": [{"user": "admin", "db": "admin"}],
        "threaded": True,
        "opid": 123456,
        "secs_running": 5,
        "microsecs_running": 5000000,
        "op": "query",
        "ns": "testdb.collection",
        "redacted": False,
        "command": {
            "find": "collection",
            "filter": {"field": "value"},
            "sort": {"_id": 1},
            "limit": 100,
        },
        "numYields": 10,
        "locks": {"Global": "r", "Database": "r", "Collection": "r"},
        "waitingForLock": False,
        "waitingForFlowControl": False,
    }


@pytest.fixture
def sample_mongodb_operation_with_mongos():
    """Return a MongoDB operation with mongos metadata (sharded cluster)."""
    return {
        "type": "op",
        "host": "shard-server-01:27017",
        "desc": "conn67890",
        "connectionId": 67890,
        "client": "10.0.1.50:45678",
        "clientMetadata": {
            "driver": {"name": "PyMongo", "version": "4.6.3"},
            "os": {"type": "Linux", "name": "Linux"},
            "platform": "CPython 3.12.3.final.0",
            "mongos": {
                "host": "mongos-router:27020",
                "client": "10.0.2.100:41978",
                "version": "7.0.18-11",
            },
        },
        "opid": 789012,
        "secs_running": 15,
        "op": "update",
        "ns": "testdb.users",
        "active": True,
        "effectiveUsers": [{"user": "appuser", "db": "testdb"}],
        "command": {
            "update": "users",
            "updates": [{"q": {"_id": 123}, "u": {"$set": {"status": "active"}}}],
        },
    }


@pytest.fixture
def sample_mongodb_operations_list(
    sample_mongodb_operation, sample_mongodb_operation_with_mongos
):
    """Return a list of varied MongoDB operations for testing."""
    return [
        sample_mongodb_operation,
        sample_mongodb_operation_with_mongos,
        {
            "opid": 111111,
            "type": "op",
            "op": "insert",
            "ns": "testdb.logs",
            "secs_running": 1,
            "client": "127.0.0.1:33333",
            "desc": "conn11111",
            "active": True,
            "effectiveUsers": [{"user": "logger", "db": "testdb"}],
            "command": {"insert": "logs", "documents": [{"msg": "test"}]},
        },
        {
            "opid": 222222,
            "type": "op",
            "op": "remove",
            "ns": "testdb.temp",
            "secs_running": 60,
            "client": "192.168.1.200:44444",
            "desc": "conn22222",
            "active": True,
            "effectiveUsers": [{"user": "cleaner", "db": "testdb"}],
            "command": {"delete": "temp", "deletes": [{"q": {"old": True}}]},
        },
    ]


@pytest.fixture
def sample_filter_values():
    """Return common filter test values."""
    return {
        "valid": {
            "opid": "123",
            "operation": "query",
            "client": "192.168",
            "description": "conn",
            "effective_users": "testuser",
            "running_time": "10",
        },
        "edge_cases": {
            "running_time_non_numeric": "abc",
            "running_time_negative": "-5",
            "running_time_zero": "0",
            "running_time_very_large": "999999999",
            "opid_with_whitespace": "  123  ",
            "empty_string": "",
        },
    }


@pytest.fixture
def mock_async_mongo_client_strict():
    """
    Return a stricter mock AsyncMongoClient that validates basic interactions.

    Unlike the permissive MagicMock, this enforces that:
    - admin.command() is called with valid commands
    - admin.aggregate() is called with valid pipelines
    - Results have expected structure
    """
    client = MagicMock()

    # Mock admin database
    admin_db = MagicMock()
    client.admin = admin_db

    # Mock command responses
    async def mock_command(cmd, **kwargs):
        if cmd == "ping":
            return {"ok": 1}
        elif cmd == "serverStatus":
            return {"ok": 1, "version": "7.0.0", "process": "mongod"}
        elif cmd == "killOp":
            return {"ok": 1}
        else:
            raise ValueError(f"Unexpected command: {cmd}")

    admin_db.command = AsyncMock(side_effect=mock_command)

    # Mock aggregate with cursor
    mock_cursor = MagicMock()
    mock_cursor.to_list = AsyncMock(return_value=[])
    admin_db.aggregate = AsyncMock(return_value=mock_cursor)

    # Mock close
    client.close = AsyncMock()

    return client
