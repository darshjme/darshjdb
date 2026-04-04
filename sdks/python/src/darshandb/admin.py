"""
Admin client for DarshanDB.

Provides privileged operations using an admin token: user impersonation,
server-side subscriptions via SSE, and administrative queries.

Usage::

    from darshandb import DarshanAdmin

    admin = DarshanAdmin(
        server_url="https://db.example.com",
        admin_token="dsk_admin_...",
    )

    # Impersonate a user
    user_db = admin.as_user("alice@example.com")
    posts = user_db.query({"collection": "posts"})

    # Subscribe to real-time changes (async)
    async for event in admin.subscribe({"collection": "orders"}):
        print("New event:", event)
"""

from __future__ import annotations

import json
from typing import Any, AsyncIterator

import httpx

from darshandb.client import DarshanDB
from darshandb.exceptions import DarshanAPIError, DarshanError


class DarshanAdmin:
    """
    Admin client for privileged DarshanDB operations.

    Authenticated with an admin token (not a user access token).
    Provides impersonation, real-time subscriptions, and admin-level queries.

    Args:
        server_url: Base URL of the DarshanDB server.
        admin_token: Admin API token from the DarshanDB dashboard.
        timeout: Request timeout in seconds (default: 30).
    """

    def __init__(
        self,
        server_url: str,
        admin_token: str,
        *,
        timeout: float = 30.0,
    ) -> None:
        if not server_url:
            raise ValueError("server_url is required.")
        if not admin_token:
            raise ValueError("admin_token is required.")

        self._server_url = server_url.rstrip("/")
        self._admin_token = admin_token
        self._timeout = timeout

        self._http = httpx.Client(
            base_url=self._server_url,
            timeout=timeout,
            headers=self._build_headers(),
        )

    # ------------------------------------------------------------------
    #  Impersonation
    # ------------------------------------------------------------------

    def as_user(self, email_or_token: str) -> DarshanDB:
        """
        Create a DarshanDB client impersonating a specific user.

        If ``email_or_token`` looks like a JWT (contains dots), it is used
        directly as the auth token. Otherwise, the admin endpoint is called
        to mint an impersonation token for the given email.

        Args:
            email_or_token: User email address or an existing access token.

        Returns:
            A :class:`DarshanDB` instance authenticated as the target user.

        Raises:
            DarshanAPIError: If the impersonation request fails.
        """
        if "." in email_or_token and "@" not in email_or_token:
            # Looks like a token
            token = email_or_token
        else:
            # Request an impersonation token from the admin endpoint
            result = self._post("/api/admin/impersonate", json={
                "email": email_or_token,
            })
            token = result["accessToken"]

        db = DarshanDB(
            server_url=self._server_url,
            api_key=self._admin_token,
            timeout=self._timeout,
        )
        db.set_token(token)
        return db

    # ------------------------------------------------------------------
    #  Queries
    # ------------------------------------------------------------------

    def query(self, darshan_ql: dict[str, Any]) -> dict[str, Any]:
        """
        Execute a DarshanQL query with admin privileges.

        Bypasses per-user permission rules.

        Args:
            darshan_ql: Query descriptor dict.

        Returns:
            Query result with ``data`` and ``txId``.
        """
        return self._post("/api/query", json=darshan_ql)

    def transact(self, ops: list[dict[str, Any]]) -> dict[str, Any]:
        """
        Execute a batch transaction with admin privileges.

        Args:
            ops: List of transaction operation dicts.

        Returns:
            A dict with ``txId``.
        """
        return self._post("/api/transact", json={"ops": ops})

    def fn(self, name: str, args: dict[str, Any] | None = None) -> Any:
        """
        Invoke a server-side function with admin privileges.

        Args:
            name: The function name.
            args: Arguments to pass.

        Returns:
            The function's return value.
        """
        result = self._post(f"/api/fn/{name}", json=args or {})
        return result.get("result", result)

    # ------------------------------------------------------------------
    #  Real-time subscriptions (SSE)
    # ------------------------------------------------------------------

    async def subscribe(
        self,
        query: dict[str, Any],
    ) -> AsyncIterator[dict[str, Any]]:
        """
        Subscribe to real-time query changes via Server-Sent Events.

        Yields events as dicts whenever the query result changes on the server.
        Requires the ``httpx-sse`` extra: ``pip install darshandb[sse]``.

        Args:
            query: DarshanQL query descriptor to subscribe to.

        Yields:
            Event dicts with ``data`` and ``txId`` keys.

        Example::

            async for event in admin.subscribe({"collection": "orders"}):
                print(f"Update: {event['data']}")
        """
        try:
            from httpx_sse import aconnect_sse
        except ImportError:
            raise DarshanError(
                "httpx-sse is required for SSE subscriptions. "
                "Install it with: pip install darshandb[sse]"
            )

        async with httpx.AsyncClient(
            base_url=self._server_url,
            timeout=None,  # SSE connections are long-lived
            headers=self._build_headers(),
        ) as client:
            async with aconnect_sse(
                client,
                "POST",
                "/api/subscribe",
                json=query,
            ) as event_source:
                async for sse in event_source.aiter_sse():
                    if sse.data:
                        try:
                            yield json.loads(sse.data)
                        except json.JSONDecodeError:
                            yield {"raw": sse.data}

    # ------------------------------------------------------------------
    #  Lifecycle
    # ------------------------------------------------------------------

    def close(self) -> None:
        """Close the underlying HTTP client."""
        self._http.close()

    def __enter__(self) -> DarshanAdmin:
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()

    # ------------------------------------------------------------------
    #  Internal
    # ------------------------------------------------------------------

    def _build_headers(self) -> dict[str, str]:
        return {
            "Authorization": f"Bearer {self._admin_token}",
            "Content-Type": "application/json",
            "Accept": "application/json",
        }

    def _post(self, path: str, *, json: dict[str, Any] | None = None) -> dict[str, Any]:
        return self._request("POST", path, json=json)

    def _get(self, path: str, *, params: dict[str, Any] | None = None) -> dict[str, Any]:
        return self._request("GET", path, params=params)

    def _request(
        self,
        method: str,
        path: str,
        *,
        json: dict[str, Any] | None = None,
        params: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        try:
            response = self._http.request(
                method,
                path,
                json=json,
                params=params,
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
