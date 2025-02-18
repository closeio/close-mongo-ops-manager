# Custom exceptions
class MongoOpsError(Exception):
    """Base exception for Close MongoDB Operations Manager."""

    pass


class ConnectionError(MongoOpsError):
    """Exception raised for connection-related errors."""

    pass


class OperationError(MongoOpsError):
    """Exception raised for operation-related errors."""

    pass
