"""
Main DarshanDB client for Python applications.

Provides a synchronous interface to the DarshanDB server including
authentication, querying, transactions, server-side functions, and storage.

Usage::

    from darshandb import DarshanDB

    db = DarshanDB(server_url="https://db.example.com", api_key="your-key")

    # Authenticate
    db.auth.sign_in("user@example.com", "password")

    # Query
    result = db.query({
        "collection": "posts",
        "where": [{"field": "published", "op": "=", "value": True}],
        "limit": 20,
    })

    # Transactions
    db.transact([
        {"kind": "set", "entity": "accounts", "id": "a1", "data": {"balance": 900}},
        {"kind": "set", "entity": "accounts", "id": "a2", "data": {"balance": 1100}},
    ])

    # Server-side functions
    report = db.fn("generateReport", {"month": "2026-04"})

    # Close when done
    db.close()
"""

from __future__ import annotations

from typing import Any

import httpx

from darshandb.auth import AuthClient
from darshandb.exceptions import DarshanAPIError, DarshanError
from darshandb.storage import StorageClient


class DarshanDB:
    """
    DarshanDB client — the main entry point for all server interactions.

    Args:
        server_url: Base URL of the DarshanDB server (e.g. ``https://db.example.com``).
        api_key: Application API key from the DarshanDB dashboard.
        timeout: Request timeout in seconds (default: 30).

    Example::

        db = DarshanDB(
            server_url="https://db.example.com",
            api_key="dsk_abc123",
        )
    """

    def __init__(
        self,
        server_url: str,
        api_key: str,
        *,
        timeout: float = 30.0,
    ) -> None:
        if not server_url:
            raise ValueError("server_url is required.")
        if not api_key:
            raise ValueError("api_key is required.")

        self._server_url = server_url.rstrip("/")
        self._api_key = api_key
        self._token: str | None = None

        self._http = httpx.Client(
            base_url=self._server_url,
            timeout=timeout,
            headers={
                "Content-Type": "application/json",
                "Accept": "application/json",
            },
        )

        self._auth = AuthClient(self)
        self._storage = StorageClient(self)

    # ------------------------------------------------------------------
    #  Properties
    # ------------------------------------------------------------------

    @property
    def auth(self) -> AuthClient:
        """Authentication client for sign-up, sign-in, sign-out, and user retrieval."""
        return self._auth

    @property
    def storage(self) -> StorageClient:
        """File storage client for uploads, URL retrieval, and deletion."""
        return self._storage

    # ------------------------------------------------------------------
    #  Query
    # ------------------------------------------------------------------

    def query(self, darshan_ql: dict[str, Any]) -> dict[str, Any]:
        """
        Execute a DarshanQL query against the server.

        Args:
            darshan_ql: A query descriptor dict with keys like ``collection``,
                ``where``, ``order``, ``limit``, ``offset``, ``select``.

        Returns:
            A dict with ``data`` (list of records) and ``txId`` (transaction ID).

        Raises:
            DarshanAPIError: On server errors.
        """
        return self._post("/api/query", json=darshan_ql)

    # ------------------------------------------------------------------
    #  Transactions
    # ------------------------------------------------------------------

    def transact(self, ops: list[dict[str, Any]]) -> dict[str, Any]:
        """
        Execute a batch transaction.

        Each operation is a dict with at least ``kind``, ``entity``, and ``id``.
        Supported kinds: ``set``, ``merge``, ``delete``, ``link``, ``unlink``.

        Args:
            ops: List of transaction operation dicts.

        Returns:
            A dict with ``txId`` confirming the committed transaction.

        Raises:
            DarshanAPIError: On validation or server errors.
        """
        return self._post("/api/transact", json={"ops": ops})

    # ------------------------------------------------------------------
    #  Server-side functions
    # ------------------------------------------------------------------

    def fn(self, name: str, args: dict[str, Any] | None = None) -> Any:
        """
        Invoke a server-side function by name.

        Args:
            name: The registered function name.
            args: Arguments to pass to the function.

        Returns:
            The function's return value (type depends on the function).

        Raises:
            DarshanAPIError: On server errors.
        """
        result = self._post(f"/api/fn/{name}", json=args or {})
        return result.get("result", result)

    # ------------------------------------------------------------------
    #  Data helpers (convenience wrappers around query/transact)
    # ------------------------------------------------------------------

    def get(
        self,
        entity: str,
        *,
        where: list[dict[str, Any]] | None = None,
        order: list[dict[str, Any]] | None = None,
        limit: int | None = None,
        offset: int | None = None,
        select: list[str] | None = None,
    ) -> dict[str, Any]:
        """
        Convenience method to query an entity with keyword arguments.

        Args:
            entity: Collection/entity name.
            where: Filter clauses.
            order: Sort clauses.
            limit: Max records to return.
            offset: Records to skip.
            select: Fields to include (projection).

        Returns:
            Query result with ``data`` and ``txId``.
        """
        q: dict[str, Any] = {"collection": entity}
        if where is not None:
            q["where"] = where
        if order is not None:
            q["order"] = order
        if limit is not None:
            q["limit"] = limit
        if offset is not None:
            q["offset"] = offset
        if select is not None:
            q["select"] = select
        return self.query(q)

    def create(self, entity: str, data: dict[str, Any]) -> dict[str, Any]:
        """
        Create a new record in a collection.

        Args:
            entity: Collection name.
            data: Record data.

        Returns:
            The created record (with server-assigned ID).
        """
        return self._post(f"/api/data/{entity}", json=data)

    def update(self, entity: str, record_id: str, data: dict[str, Any]) -> dict[str, Any]:
        """
        Update an existing record by ID (merge semantics).

        Args:
            entity: Collection name.
            record_id: The record's unique identifier.
            data: Fields to update.

        Returns:
            The updated record.
        """
        return self._post(f"/api/data/{entity}/{record_id}", json=data)

    def delete(self, entity: str, record_id: str) -> dict[str, Any]:
        """
        Delete a record by ID.

        Args:
            entity: Collection name.
            record_id: The record's unique identifier.

        Returns:
            Server acknowledgement.
        """
        return self._request("DELETE", f"/api/data/{entity}/{record_id}")

    # ------------------------------------------------------------------
    #  Lifecycle
    # ------------------------------------------------------------------

    def close(self) -> None:
        """Close the underlying HTTP client and release resources."""
        self._http.close()

    def __enter__(self) -> DarshanDB:
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()

    # ------------------------------------------------------------------
    #  Internal HTTP helpers
    # ------------------------------------------------------------------

    def set_token(self, token: str | None) -> None:
        """
        Set or clear the authentication token.

        .. note::
            Normally managed automatically by :class:`AuthClient`.
        """
        self._token = token

    def get_token(self) -> str | None:
        """Return the current authentication token, or ``None``."""
        return self._token

    def _build_headers(self) -> dict[str, str]:
        """Build request headers with API key and optional auth token."""
        headers: dict[str, str] = {
            "X-Api-Key": self._api_key,
            "Accept": "application/json",
            "Content-Type": "application/json",
        }
        if self._token:
            headers["Authorization"] = f"Bearer {self._token}"
        return headers

    def _post(self, path: str, *, json: dict[str, Any] | None = None) -> dict[str, Any]:
        """Send a POST request and return the parsed JSON response."""
        return self._request("POST", path, json=json)

    def _get(self, path: str, *, params: dict[str, Any] | None = None) -> dict[str, Any]:
        """Send a GET request and return the parsed JSON response."""
        return self._request("GET", path, params=params)

    def _delete(self, path: str, *, json: dict[str, Any] | None = None) -> dict[str, Any]:
        """Send a DELETE request and return the parsed JSON response."""
        return self._request("DELETE", path, json=json)

    def _request(
        self,
        method: str,
        path: str,
        *,
        json: dict[str, Any] | None = None,
        params: dict[str, Any] | None = None,
        content: bytes | None = None,
        files: dict[str, Any] | None = None,
        extra_headers: dict[str, str] | None = None,
    ) -> dict[str, Any]:
        """
        Execute an HTTP request against the DarshanDB server.

        Raises:
            DarshanAPIError: On 4xx/5xx responses.
            DarshanError: On network or decoding errors.
        """
        headers = self._build_headers()
        if files:
            # Let httpx set multipart content-type
            headers.pop("Content-Type", None)
        if extra_headers:
            headers.update(extra_headers)

        try:
            response = self._http.request(
                method,
                path,
                headers=headers,
                json=json if not files else None,
                params=params,
                content=content,
                files=files,
            )
        except httpx.HTTPError as exc:
            raise DarshanError(f"Network error: {exc}") from exc

        if response.status_code >= 400:
            try:
                body = response.json()
            except Exception:
                body = {"raw": response.text}

            message = body.get("message") or body.get("error") or response.text
            raise DarshanAPIError(
                str(message),
                status_code=response.status_code,
                error_body=body,
            )

        if response.status_code == 204:
            return {}

        try:
            return response.json()  # type: ignore[no-any-return]
        except Exception as exc:
            raise DarshanError(f"Invalid JSON response: {exc}") from exc
