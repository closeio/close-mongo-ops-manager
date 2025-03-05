from pymongo.errors import PyMongoError


# Custom exceptions
class MongoOpsError(PyMongoError):
    """Base class for exceptions raised by the MongoOpsManager class."""

    pass


class MongoConnectionError(MongoOpsError):
    """Exception raised for MongoDB connection errors."""

    pass


class OperationError(Exception):
    """Exception raised for operation-related errors."""

    pass
