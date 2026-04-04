"""
DarshanDB Python SDK.

Official client library for interacting with a DarshanDB server.
Provides authentication, querying, transactions, server-side functions,
admin operations, and file storage.

Usage::

    from darshandb import DarshanDB

    db = DarshanDB(server_url="https://db.example.com", api_key="your-key")
    db.auth.sign_in("user@example.com", "password")
    posts = db.query({"collection": "posts", "limit": 10})
"""

from darshandb.client import DarshanDB
from darshandb.admin import DarshanAdmin
from darshandb.auth import AuthClient
from darshandb.storage import StorageClient
from darshandb.exceptions import DarshanError, DarshanAPIError

__all__ = [
    "DarshanDB",
    "DarshanAdmin",
    "AuthClient",
    "StorageClient",
    "DarshanError",
    "DarshanAPIError",
]

__version__ = "0.1.0"
