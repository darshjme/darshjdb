"""
File storage client for DarshanDB.

Provides upload, URL retrieval, listing, and deletion of files
stored on the DarshanDB server.

Usage::

    db = DarshanDB(server_url="...", api_key="...")

    # Upload from file path
    result = db.storage.upload("/avatars/photo.jpg", "/tmp/photo.jpg")
    print(result["url"])

    # Upload from bytes
    result = db.storage.upload_bytes("/data/report.csv", b"col1,col2\\n1,2", "report.csv")

    # Get URL
    url = db.storage.get_url("/avatars/photo.jpg")

    # Delete
    db.storage.delete("/avatars/photo.jpg")
"""

from __future__ import annotations

from pathlib import Path
from typing import Any, TYPE_CHECKING

if TYPE_CHECKING:
    from darshandb.client import DarshanDB


class StorageClient:
    """
    File storage operations for DarshanDB.

    Accessed via ``db.storage`` on a :class:`~darshandb.client.DarshanDB` instance.
    """

    def __init__(self, client: DarshanDB) -> None:
        self._client = client

    def upload(
        self,
        path: str,
        file_path: str,
        *,
        content_type: str | None = None,
        metadata: dict[str, str] | None = None,
    ) -> dict[str, Any]:
        """
        Upload a file from the local filesystem to DarshanDB storage.

        Args:
            path: Remote storage path (e.g. ``/avatars/photo.jpg``).
            file_path: Local filesystem path to the file.
            content_type: Optional MIME type override.
            metadata: Optional custom metadata key-value pairs.

        Returns:
            A dict with ``path``, ``url``, ``size``, and ``contentType``.

        Raises:
            FileNotFoundError: If the local file does not exist.
            DarshanAPIError: On upload failure.
        """
        local = Path(file_path)
        if not local.exists():
            raise FileNotFoundError(f"File not found: {file_path}")

        files: dict[str, Any] = {
            "file": (local.name, open(local, "rb")),
        }
        # Send path and options as additional form fields via data
        data: dict[str, str] = {"path": path}
        if content_type:
            data["contentType"] = content_type
        if metadata:
            import json
            data["metadata"] = json.dumps(metadata)

        return self._client._request(
            "POST",
            "/api/storage/upload",
            files=files,
        )

    def upload_bytes(
        self,
        path: str,
        content: bytes,
        filename: str,
        *,
        content_type: str | None = None,
        metadata: dict[str, str] | None = None,
    ) -> dict[str, Any]:
        """
        Upload raw bytes to DarshanDB storage.

        Args:
            path: Remote storage path.
            content: File content as bytes.
            filename: The filename to use.
            content_type: Optional MIME type override.
            metadata: Optional custom metadata.

        Returns:
            A dict with ``path``, ``url``, ``size``, and ``contentType``.
        """
        files: dict[str, Any] = {
            "file": (filename, content),
        }

        return self._client._request(
            "POST",
            "/api/storage/upload",
            files=files,
        )

    def get_url(self, path: str, *, expiry: int = 3600) -> str:
        """
        Get a (signed) URL for a stored file.

        Args:
            path: Remote storage path.
            expiry: URL expiry time in seconds (default: 3600).

        Returns:
            The file URL as a string.
        """
        result = self._client._get("/api/storage/url", params={
            "path": path,
            "expiry": expiry,
        })
        return result.get("url", "")

    def delete(self, path: str) -> dict[str, Any]:
        """
        Delete a file from storage.

        Args:
            path: Remote storage path.

        Returns:
            Server acknowledgement.
        """
        return self._client._delete("/api/storage/delete", json={"path": path})

    def list(
        self,
        prefix: str = "/",
        *,
        limit: int = 100,
        cursor: str | None = None,
    ) -> dict[str, Any]:
        """
        List files under a given prefix.

        Args:
            prefix: Directory prefix to list (e.g. ``/avatars/``).
            limit: Maximum files to return (default: 100).
            cursor: Pagination cursor from a previous response.

        Returns:
            A dict with ``files`` list and optional ``cursor`` for pagination.
        """
        params: dict[str, Any] = {"prefix": prefix, "limit": limit}
        if cursor:
            params["cursor"] = cursor
        return self._client._get("/api/storage/list", params=params)
