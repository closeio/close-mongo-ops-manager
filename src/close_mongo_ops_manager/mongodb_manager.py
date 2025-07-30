import asyncio
import time
from typing import Any

from pymongo import AsyncMongoClient
from pymongo.asynchronous.database import AsyncDatabase
from pymongo.errors import PyMongoError

from close_mongo_ops_manager.exceptions import MongoConnectionError, OperationError

import logging

logger = logging.getLogger("mongo_ops_manager")


class MongoDBManager:
    """Handles MongoDB connection and operations."""

    def __init__(self) -> None:
        self.client: AsyncMongoClient | None = None
        self.admin_db: AsyncDatabase | None = None
        self.namespace: str = ""
        self.hide_system_ops: bool = True

    async def close(self) -> None:
        """Close all MongoDB connections properly."""
        logger.info("Closing MongoDB connections")

        try:
            if self.client is not None:
                await self.client.close()
                logger.debug("Closed main MongoDB connection")
        except Exception as e:
            logger.warning(f"Error closing main MongoDB connection: {e}")

    @classmethod
    async def connect(
        cls, connection_string: str, namespace: str, hide_system_ops: bool = True, load_balanced: bool = False
    ) -> "MongoDBManager":
        self = cls()
        try:
            self.namespace = namespace
            self.hide_system_ops = hide_system_ops

            # Create client
            separator = "&" if "?" in connection_string else "?"
            conn_string = f"{connection_string}{separator}readPreference=secondaryPreferred"
            if load_balanced:
                conn_string += "&loadBalanced=true"
            self.client = AsyncMongoClient(
                conn_string,
                serverSelectionTimeoutMS=5000,
                connectTimeoutMS=5000,
            )

            # Set up admin databases
            self.admin_db = self.client.admin

            # Verify connection
            await self.admin_db.command("ping")

            server_status = await self.admin_db.command("serverStatus")
            version = server_status.get("version", "unknown version")
            process = server_status.get("process", "unknown process")
            logger.info(f"Connected to MongoDB {version} ({process})")

            return self
        except PyMongoError as e:
            raise MongoConnectionError(f"Failed to connect to MongoDB: {e}")

    async def get_operations(self, filters: dict[str, str] | None = None) -> list[dict]:
        """Get current operations with appropriate handling"""
        if self.admin_db is None:
            logger.error("Admin database not initialized")
            return []

        try:
            # Base currentOp arguments
            current_op_args = {
                "allUsers": True,
                "idleConnections": False,
                "idleCursors": False,
                "idleSessions": True,
                "localOps": False,
                "backtrace": False,
            }

            pipeline = [{"$currentOp": current_op_args}]

            if filters or self.namespace:
                match_stage: dict[str, Any] = {"$and": []}

                # Add system operations filter
                if self.hide_system_ops:
                    match_stage["$and"].append(
                        {
                            "$nor": [
                                {"ns": {"$regex": "^admin\\.", "$options": "i"}},
                                {"ns": {"$regex": "^config\\.", "$options": "i"}},
                                {"ns": {"$regex": "^local\\.", "$options": "i"}},
                                {"op": "none"},  # Filter out no-op operations
                                {
                                    "effectiveUsers.user": "__system"
                                },  # exclude system users
                                {
                                    "op": "command",
                                    "command.cursor": {"$exists": True},
                                },  # Filter cursor operations
                            ]
                        }
                    )

                if self.namespace:
                    match_stage["$and"].append(
                        {"ns": {"$regex": f"^{self.namespace}", "$options": "i"}}
                    )

                if filters:
                    if filters.get("opid"):
                        match_stage["$and"].append(
                            {"opid": {"$regex": filters["opid"], "$options": "i"}}
                        )
                    if filters.get("operation"):
                        match_stage["$and"].append(
                            {
                                "op": {
                                    "$regex": filters["operation"],
                                    "$options": "i",
                                }
                            }
                        )
                    if filters.get("client"):
                        match_stage["$and"].append(
                            {
                                "$or": [
                                    {
                                        "client": {
                                            "$regex": filters["client"],
                                            "$options": "i",
                                        }
                                    },
                                    {
                                        "client_s": {
                                            "$regex": filters["client"],
                                            "$options": "i",
                                        }
                                    },
                                ]
                            }
                        )
                    if filters.get("description"):
                        match_stage["$and"].append(
                            {
                                "desc": {
                                    "$regex": filters["description"],
                                    "$options": "i",
                                }
                            }
                        )
                    if filters.get("effective_users"):
                        match_stage["$and"].append(
                            {
                                "effectiveUsers": {
                                    "$elemMatch": {
                                        "user": {
                                            "$regex": filters["effective_users"],
                                            "$options": "i",
                                        }
                                    }
                                }
                            }
                        )
                    if (
                        filters.get("running_time")
                        and filters["running_time"].isdigit()
                    ):
                        match_stage["$and"].append(
                            {"secs_running": {"$gte": int(filters["running_time"])}}
                        )

                match_stage["$and"].append({"active": True})

                if match_stage["$and"]:
                    pipeline.append({"$match": match_stage})

            # Project only the fields we need to reduce data transfer
            pipeline.append(
                {
                    "$project": {
                        "opid": 1,
                        "type": 1,
                        "op": 1,
                        "secs_running": 1,
                        "client": 1,
                        "client_s": 1,
                        "clientMetadata.mongos.host": 1,
                        "desc": 1,
                        "effectiveUsers": 1,
                        "active": 1,
                        "ns": 1,
                        "command": 1,
                        "planSummary": 1,
                        "currentOpTime": 1,
                        "microsecs_running": 1,
                        "killPending": 1,
                        "WiredTigerTxn": 1,
                        "lsid": 1,
                        "transaction": 1,
                        "locks": 1,
                        "waitingForLock": 1,
                        "lockStats": 1,
                    }
                }
            )

            # Limit results to prevent overwhelming the UI
            pipeline.append({"$limit": 1000})

            cursor = await self.admin_db.aggregate(pipeline)
            inprog = await cursor.to_list(None)

            return inprog
        except PyMongoError as e:
            raise OperationError(f"Failed to get operations: {e}")

    async def _operation_exists(self, opid: str) -> bool:
        """Check if operation still exists with minimal query."""
        try:
            pipeline = [
                {"$currentOp": {"allUsers": True}},
                {"$match": {"opid": {"$eq": str(opid)}}},
                {"$project": {"_id": 0, "opid": 1}},
                {"$limit": 1},
            ]

            result = await self.admin_db.aggregate(pipeline)
            result_list = await result.to_list(1)
            return len(result_list) > 0  # Operation exists if result is not empty
        except PyMongoError as e:
            logger.warning(f"Error checking operation existence: {e}")
            return False
        except Exception as e:
            logger.error(f"Unexpected error checking operation: {e}", exc_info=True)
            return False

    async def kill_operation(
        self, opid: str, max_retries: int = 2, verify_timeout: float = 5.0
    ) -> bool:
        """Kill a MongoDB operation with retries and verification."""
        if not opid or opid.strip() == "":
            logger.error("Cannot kill operation with empty opid")
            return False

        if self.admin_db is None:
            logger.error("Admin database not initialized")
            return False

        # Validate input parameters
        if max_retries < 1:
            max_retries = max(1, max_retries)

        if verify_timeout < 1.0:
            verify_timeout = max(1.0, verify_timeout)

        try:
            # Convert string opid to numeric if possible (for non-sharded operations)
            use_opid = opid
            if isinstance(opid, str):
                opid = opid.strip()
                if ":" not in opid and opid.isdigit():
                    try:
                        numeric_opid = int(opid)
                        if numeric_opid > 0:  # Ensure it's a valid positive number
                            use_opid = numeric_opid
                    except (ValueError, OverflowError) as e:
                        logger.warning(f"Failed to convert opid to numeric: {e}")
                        use_opid = opid

            # Try killing the operation with retries
            for attempt in range(max_retries):
                try:
                    # Execute killOp command
                    result = await self.admin_db.command("killOp", op=use_opid)

                    if result.get("ok") == 1:
                        # Start verification process with timeout
                        verification_start = time.monotonic()

                        while time.monotonic() - verification_start < verify_timeout:
                            # Check if operation still exists
                            operation_exists = await self._operation_exists(opid)

                            if not operation_exists:
                                logger.info(
                                    f"Successfully killed and verified operation {opid}"
                                )
                                return True

                            # Brief pause before next verification check
                            await asyncio.sleep(1.0)

                        # If we reach here, operation still exists after timeout
                        logger.warning(
                            f"Operation {opid} still exists after kill attempt {attempt + 1}"
                        )
                        if attempt < max_retries - 1:
                            # Wait before retry, with exponential backoff (capped at 10 seconds)
                            await asyncio.sleep(min(2**attempt, 10))
                            continue

                except PyMongoError as e:
                    # Special handling for sharded cluster operations
                    if (
                        "TypeMismatch" in str(e)
                        and isinstance(opid, str)
                        and ":" in opid
                    ):
                        try:
                            # For sharded operations, try to extract and kill the numeric part
                            shard_id, numeric_part = opid.split(":")
                            if numeric_part.isdigit():
                                logger.info(
                                    f"Retrying kill with numeric part of sharded operation: {numeric_part}"
                                )
                                return await self.kill_operation(
                                    numeric_part,
                                    max_retries=max_retries - attempt,
                                    verify_timeout=verify_timeout,
                                )
                        except Exception as inner_e:
                            logger.error(
                                f"Error processing sharded operation ID: {inner_e}"
                            )

                    # Log the error and continue retrying if attempts remain
                    logger.error(
                        f"Attempt {attempt + 1} failed to kill operation {opid}: {e}",
                        exc_info=True,
                    )
                    if attempt < max_retries - 1:
                        await asyncio.sleep(2**attempt)
                        continue
                    else:
                        raise OperationError(
                            f"Failed to kill operation {opid} after {max_retries} attempts: {e}"
                        )

            # If we reach here, all attempts failed
            logger.error(
                f"Failed to kill operation {opid} after {max_retries} attempts"
            )
            return False

        except Exception as e:
            logger.error(
                f"Unexpected error killing operation {opid}: {e}", exc_info=True
            )
            raise OperationError(f"Failed to kill operation {opid}: {e}")
