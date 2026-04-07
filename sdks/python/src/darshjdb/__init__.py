"""
DarshJDB Python SDK.

Official async client library for DarshJDB — a real-time database
with graph relations, live queries, auth, and storage.

Usage::

    from darshjdb import DarshDB

    async with DarshDB("http://localhost:8080") as db:
        await db.signin({"user": "root", "pass": "root"})
        await db.use("test", "test")

        user = await db.create("users", {"name": "Darsh", "age": 30})
        users = await db.select("users")
        results = await db.query("SELECT * FROM users WHERE age > 18")
"""

from darshjdb.client import DarshDB
from darshjdb.exceptions import (
    DarshDBError,
    DarshDBConnectionError,
    DarshDBAuthError,
    DarshDBQueryError,
    DarshDBAPIError,
)
from darshjdb.models import (
    QueryResult,
    LiveNotification,
    LiveAction,
    AuthResponse,
    ConnectionState,
)

__all__ = [
    "DarshDB",
    "DarshDBError",
    "DarshDBConnectionError",
    "DarshDBAuthError",
    "DarshDBQueryError",
    "DarshDBAPIError",
    "QueryResult",
    "LiveNotification",
    "LiveAction",
    "AuthResponse",
    "ConnectionState",
]

__version__ = "0.2.0"
