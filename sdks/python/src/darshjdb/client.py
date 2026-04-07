"""
DarshDB — the main async client for DarshJDB.

Follows the SurrealDB SDK pattern: signin, use namespace/database,
then CRUD + query + live queries + graph relations.

Usage::

    from darshjdb import DarshDB

    db = DarshDB("http://localhost:8080")
    await db.signin({"user": "root", "pass": "root"})
    await db.use("test", "test")

    user = await db.create("users", {"name": "Darsh", "age": 30})
    users = await db.select("users")
    await db.update("users:darsh", {"age": 31})
    await db.delete("users:darsh")

    results = await db.query("SELECT * FROM users WHERE age > 18")

    async for change in db.live("SELECT * FROM users"):
        print(change)

    await db.relate("user:darsh", "works_at", "company:knowai")
"""

from __future__ import annotations

import asyncio
import json
import logging
from typing import Any, AsyncIterator, overload
from urllib.parse import urljoin, urlparse

import httpx

from darshjdb.exceptions import (
    DarshDBAPIError,
    DarshDBAuthError,
    DarshDBConnectionError,
    DarshDBError,
    DarshDBQueryError,
)
from darshjdb.models import (
    AuthResponse,
    ConnectionState,
    LiveAction,
    LiveNotification,
    QueryResult,
)

logger = logging.getLogger("darshjdb")


def _parse_thing(thing: str) -> tuple[str, str | None]:
    """
    Parse a SurrealDB-style record ID like 'users:darsh' into (table, id).
    If no colon, returns (thing, None).
    """
    if ":" in thing:
        parts = thing.split(":", 1)
        return parts[0], parts[1]
    return thing, None


class DarshDB:
    """
    Async client for DarshJDB.

    Provides the full DarshJDB API: authentication, namespace/database
    selection, CRUD, queries, live queries via WebSocket, graph relations,
    server-side functions, and storage.

    Args:
        url: Base URL of the DarshJDB server (e.g. ``http://localhost:8080``).
        timeout: HTTP request timeout in seconds.

    Example::

        async with DarshDB("http://localhost:8080") as db:
            await db.signin({"user": "root", "pass": "root"})
            await db.use("test", "test")
            users = await db.select("users")
    """

    def __init__(
        self,
        url: str,
        *,
        timeout: float = 30.0,
    ) -> None:
        if not url:
            raise ValueError("url is required")

        self._url = url.rstrip("/")
        self._timeout = timeout
        self._token: str | None = None
        self._namespace: str | None = None
        self._database: str | None = None
        self._state = ConnectionState.DISCONNECTED

        self._http = httpx.AsyncClient(
            base_url=self._url,
            timeout=timeout,
            headers={
                "Content-Type": "application/json",
                "Accept": "application/json",
            },
        )
        self._state = ConnectionState.CONNECTED

    # ------------------------------------------------------------------
    #  Connection lifecycle
    # ------------------------------------------------------------------

    async def close(self) -> None:
        """Close the HTTP client and release all resources."""
        if self._state == ConnectionState.CLOSING:
            return
        self._state = ConnectionState.CLOSING
        await self._http.aclose()
        self._state = ConnectionState.DISCONNECTED

    async def __aenter__(self) -> DarshDB:
        return self

    async def __aexit__(self, *_: Any) -> None:
        await self.close()

    @property
    def state(self) -> ConnectionState:
        """Current connection state."""
        return self._state

    # ------------------------------------------------------------------
    #  Authentication
    # ------------------------------------------------------------------

    async def signin(self, credentials: dict[str, Any]) -> AuthResponse:
        """
        Sign in to the DarshJDB server.

        Accepts either root credentials (``user``/``pass``) or email/password
        authentication (``email``/``password``).

        Args:
            credentials: Dict with authentication fields. Supported keys:
                - ``user`` + ``pass``: Root/system authentication
                - ``email`` + ``password``: User-level authentication
                - ``namespace``, ``database``: Optional scope

        Returns:
            AuthResponse with the JWT token and user data.

        Raises:
            DarshDBAuthError: On invalid credentials.
        """
        # Normalize to server format
        body: dict[str, Any] = {}
        if "user" in credentials and "pass" in credentials:
            body["email"] = credentials["user"]
            body["password"] = credentials["pass"]
        elif "email" in credentials and "password" in credentials:
            body["email"] = credentials["email"]
            body["password"] = credentials["password"]
        else:
            body = credentials.copy()

        if "namespace" in credentials:
            self._namespace = credentials["namespace"]
        if "database" in credentials:
            self._database = credentials["database"]

        try:
            result = await self._post("/api/auth/signin", json=body)
        except DarshDBAPIError as exc:
            if exc.status_code in (401, 403):
                raise DarshDBAuthError(str(exc)) from exc
            raise

        token = result.get("accessToken", result.get("token", ""))
        if token:
            self._token = token

        return AuthResponse(
            token=token,
            user=result.get("user", {}),
            refresh_token=result.get("refreshToken", ""),
        )

    async def signup(self, credentials: dict[str, Any]) -> AuthResponse:
        """
        Create a new account and sign in.

        Args:
            credentials: Dict with ``email``, ``password``, and optionally
                ``name``, ``namespace``, ``database``.

        Returns:
            AuthResponse with the JWT token and new user data.
        """
        body: dict[str, Any] = {}
        if "email" in credentials:
            body["email"] = credentials["email"]
        if "password" in credentials:
            body["password"] = credentials["password"]
        if "name" in credentials:
            body["name"] = credentials["name"]

        try:
            result = await self._post("/api/auth/signup", json=body)
        except DarshDBAPIError as exc:
            if exc.status_code in (401, 403, 409):
                raise DarshDBAuthError(str(exc)) from exc
            raise

        token = result.get("accessToken", result.get("token", ""))
        if token:
            self._token = token

        if "namespace" in credentials:
            self._namespace = credentials["namespace"]
        if "database" in credentials:
            self._database = credentials["database"]

        return AuthResponse(
            token=token,
            user=result.get("user", {}),
            refresh_token=result.get("refreshToken", ""),
        )

    async def invalidate(self) -> None:
        """
        Sign out and invalidate the current session.
        Clears the stored token.
        """
        if self._token:
            try:
                await self._post("/api/auth/signout")
            except DarshDBError:
                pass
        self._token = None

    async def authenticate(self, token: str) -> None:
        """
        Set the authentication token directly (e.g., from a stored session).

        Args:
            token: A valid JWT access token.
        """
        self._token = token

    # ------------------------------------------------------------------
    #  Namespace / Database
    # ------------------------------------------------------------------

    async def use(self, namespace: str, database: str) -> None:
        """
        Set the active namespace and database for subsequent operations.

        Args:
            namespace: The namespace to use.
            database: The database within the namespace.
        """
        self._namespace = namespace
        self._database = database

    # ------------------------------------------------------------------
    #  CRUD operations
    # ------------------------------------------------------------------

    async def select(self, thing: str) -> list[dict[str, Any]]:
        """
        Select all records from a table, or a specific record by ID.

        Args:
            thing: Table name (``"users"``) or record ID (``"users:darsh"``).

        Returns:
            A list of records. For a specific ID, a single-element list.
        """
        table, record_id = _parse_thing(thing)
        if record_id:
            result = await self._get(f"/api/data/{table}/{record_id}")
            return [result] if result else []
        else:
            result = await self._get(f"/api/data/{table}")
            if isinstance(result, list):
                return result
            return result.get("data", [result])

    async def create(
        self,
        thing: str,
        data: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """
        Create a new record in a table.

        Args:
            thing: Table name (``"users"``) or record ID (``"users:darsh"``).
            data: Record data.

        Returns:
            The created record with server-assigned ID.
        """
        table, record_id = _parse_thing(thing)
        body = data or {}
        if record_id:
            body = {**body, "id": record_id}
        return await self._post(f"/api/data/{table}", json=body)

    async def insert(
        self,
        table: str,
        data: dict[str, Any] | list[dict[str, Any]],
    ) -> list[dict[str, Any]]:
        """
        Insert one or more records into a table.

        Args:
            table: Target table name.
            data: A single record dict or a list of record dicts.

        Returns:
            List of created records.
        """
        records = data if isinstance(data, list) else [data]
        result = await self._post("/api/mutate", json={
            "mutations": [
                {"op": "insert", "entity": table, "data": record}
                for record in records
            ]
        })
        return result.get("results", records)

    async def update(
        self,
        thing: str,
        data: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """
        Update an existing record (full replacement).

        Args:
            thing: Record ID (``"users:darsh"``).
            data: New record data (replaces all fields).

        Returns:
            The updated record.
        """
        table, record_id = _parse_thing(thing)
        if not record_id:
            raise DarshDBError(
                f"update() requires a record ID like '{table}:id', got '{thing}'"
            )
        return await self._patch(f"/api/data/{table}/{record_id}", json=data or {})

    async def merge(
        self,
        thing: str,
        data: dict[str, Any],
    ) -> dict[str, Any]:
        """
        Merge data into an existing record (partial update).

        Args:
            thing: Record ID (``"users:darsh"``).
            data: Fields to merge into the existing record.

        Returns:
            The merged record.
        """
        table, record_id = _parse_thing(thing)
        if not record_id:
            raise DarshDBError(
                f"merge() requires a record ID like '{table}:id', got '{thing}'"
            )
        return await self._patch(f"/api/data/{table}/{record_id}", json=data)

    async def delete(self, thing: str) -> dict[str, Any]:
        """
        Delete a record or all records in a table.

        Args:
            thing: Record ID (``"users:darsh"``) or table name (``"users"``).

        Returns:
            Server acknowledgement.
        """
        table, record_id = _parse_thing(thing)
        if record_id:
            return await self._delete(f"/api/data/{table}/{record_id}")
        else:
            return await self._delete(f"/api/data/{table}")

    # ------------------------------------------------------------------
    #  Query
    # ------------------------------------------------------------------

    async def query(
        self,
        sql: str,
        vars: dict[str, Any] | None = None,
    ) -> list[QueryResult]:
        """
        Execute a DarshJQL query string.

        Args:
            sql: The query string (e.g., ``"SELECT * FROM users WHERE age > 18"``).
            vars: Optional bind variables.

        Returns:
            A list of QueryResult objects (one per statement in the query).

        Raises:
            DarshDBQueryError: On parse or execution errors.
        """
        body: dict[str, Any] = {"query": sql}
        if vars:
            body["vars"] = vars

        try:
            result = await self._post("/api/query", json=body)
        except DarshDBAPIError as exc:
            raise DarshDBQueryError(str(exc), query=sql) from exc

        # Server may return a single result or an array of results
        if isinstance(result, list):
            return [
                QueryResult(
                    data=r.get("data", [r]) if isinstance(r, dict) else [r],
                    meta=r.get("meta", {}) if isinstance(r, dict) else {},
                )
                for r in result
            ]

        return [
            QueryResult(
                data=result.get("data", []),
                meta=result.get("meta", {}),
            )
        ]

    async def query_raw(
        self,
        sql: str,
        vars: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """
        Execute a query and return the raw server response without parsing.

        Args:
            sql: The query string.
            vars: Optional bind variables.

        Returns:
            Raw JSON response from the server.
        """
        body: dict[str, Any] = {"query": sql}
        if vars:
            body["vars"] = vars
        return await self._post("/api/query", json=body)

    # ------------------------------------------------------------------
    #  Live queries (WebSocket)
    # ------------------------------------------------------------------

    async def live(
        self,
        query_or_table: str,
        *,
        diff: bool = False,
    ) -> AsyncIterator[LiveNotification]:
        """
        Subscribe to a live query via WebSocket.

        Yields LiveNotification objects whenever records matching the
        query are created, updated, or deleted.

        Args:
            query_or_table: A SQL query (``"SELECT * FROM users"``) or
                just a table name (``"users"``).
            diff: If True, receive diff patches instead of full records.

        Yields:
            LiveNotification with action and result data.

        Example::

            async for change in db.live("SELECT * FROM users"):
                print(f"{change.action}: {change.result}")
        """
        import websockets

        # Build WebSocket URL from HTTP URL
        parsed = urlparse(self._url)
        ws_scheme = "wss" if parsed.scheme == "https" else "ws"
        ws_url = f"{ws_scheme}://{parsed.netloc}/ws"

        async for ws in websockets.connect(ws_url, max_size=1024 * 1024):
            try:
                # Authenticate
                if self._token:
                    await ws.send(json.dumps({
                        "type": "auth",
                        "token": self._token,
                    }))
                    auth_response = json.loads(await ws.recv())
                    if auth_response.get("type") == "auth-err":
                        raise DarshDBAuthError(
                            auth_response.get("error", "WebSocket auth failed")
                        )

                # Determine if this is a raw table name or a query
                if query_or_table.strip().upper().startswith("SELECT"):
                    sub_query = {"query": query_or_table}
                else:
                    sub_query = {"query": f"SELECT * FROM {query_or_table}"}

                if diff:
                    sub_query["diff"] = True  # type: ignore[assignment]

                # Subscribe
                sub_id = f"live_{id(query_or_table)}"
                await ws.send(json.dumps({
                    "type": "sub",
                    "id": sub_id,
                    "query": sub_query,
                }))

                sub_response = json.loads(await ws.recv())
                if sub_response.get("type") == "sub-err":
                    raise DarshDBQueryError(
                        sub_response.get("error", "Subscription failed"),
                        query=query_or_table,
                    )

                # Listen for diffs
                async for raw_msg in ws:
                    try:
                        msg = json.loads(raw_msg)
                    except (json.JSONDecodeError, TypeError):
                        continue

                    msg_type = msg.get("type", "")

                    if msg_type == "diff":
                        changes = msg.get("changes", {})
                        for action_key in ("inserted", "updated", "deleted"):
                            records = changes.get(action_key, [])
                            action_map = {
                                "inserted": LiveAction.CREATE,
                                "updated": LiveAction.UPDATE,
                                "deleted": LiveAction.DELETE,
                            }
                            for record in records:
                                yield LiveNotification(
                                    action=action_map[action_key],
                                    result=record,
                                )

                    elif msg_type == "pub-event":
                        event = msg.get("event", "updated")
                        action_map_pub = {
                            "created": LiveAction.CREATE,
                            "updated": LiveAction.UPDATE,
                            "deleted": LiveAction.DELETE,
                        }
                        yield LiveNotification(
                            action=action_map_pub.get(event, LiveAction.UPDATE),
                            result=msg,
                        )

                    elif msg_type == "pong":
                        continue

                    elif msg_type == "error":
                        raise DarshDBError(msg.get("error", "Unknown WebSocket error"))

            except websockets.ConnectionClosed:
                logger.warning("WebSocket connection closed, stopping live query")
                return

    async def subscribe(
        self,
        table: str,
        callback: Any = None,
    ) -> str:
        """
        Subscribe to changes on a table via SSE (Server-Sent Events).

        For WebSocket-based live queries, use ``live()`` instead.
        This method sets up an SSE subscription and returns a subscription ID.

        Args:
            table: The table to subscribe to.
            callback: Optional async callback(event_dict) for each event.

        Returns:
            The subscription channel identifier.
        """
        channel = f"entity:{table}:*"
        if callback:
            asyncio.create_task(self._sse_listener(table, callback))
        return channel

    async def _sse_listener(self, table: str, callback: Any) -> None:
        """Internal SSE listener that calls the callback for each event."""
        try:
            async with self._http.stream(
                "GET",
                "/api/subscribe",
                params={"table": table},
                headers=self._build_headers(),
                timeout=None,
            ) as response:
                async for line in response.aiter_lines():
                    if line.startswith("data:"):
                        try:
                            data = json.loads(line[5:].strip())
                            await callback(data)
                        except (json.JSONDecodeError, Exception) as e:
                            logger.warning("SSE parse error: %s", e)
        except Exception as e:
            logger.error("SSE listener error: %s", e)

    # ------------------------------------------------------------------
    #  Graph relations
    # ------------------------------------------------------------------

    async def relate(
        self,
        from_thing: str,
        relation: str,
        to_thing: str,
        data: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """
        Create a graph relation between two records.

        Args:
            from_thing: Source record ID (``"user:darsh"``).
            relation: Relation type (``"works_at"``).
            to_thing: Target record ID (``"company:knowai"``).
            data: Optional data to attach to the relation edge.

        Returns:
            The created relation record.

        Example::

            await db.relate("user:darsh", "works_at", "company:knowai")
        """
        from_table, from_id = _parse_thing(from_thing)
        to_table, to_id = _parse_thing(to_thing)

        body: dict[str, Any] = {
            "mutations": [
                {
                    "op": "insert",
                    "entity": relation,
                    "data": {
                        "from_entity": from_table,
                        "from_id": from_id or from_thing,
                        "to_entity": to_table,
                        "to_id": to_id or to_thing,
                        **(data or {}),
                    },
                }
            ]
        }
        result = await self._post("/api/mutate", json=body)
        return result.get("results", [{}])[0] if isinstance(result, dict) else result

    # ------------------------------------------------------------------
    #  Server-side functions
    # ------------------------------------------------------------------

    async def run(
        self,
        name: str,
        args: dict[str, Any] | None = None,
    ) -> Any:
        """
        Invoke a server-side function.

        Args:
            name: Registered function name.
            args: Arguments to pass.

        Returns:
            The function's return value.
        """
        result = await self._post(f"/api/fn/{name}", json=args or {})
        return result.get("result", result)

    # ------------------------------------------------------------------
    #  Batch operations
    # ------------------------------------------------------------------

    async def batch(
        self,
        operations: list[dict[str, Any]],
    ) -> list[dict[str, Any]]:
        """
        Execute multiple operations in a single batch request.

        Args:
            operations: List of operation dicts, each with ``method``,
                ``path``, and optionally ``body``.

        Returns:
            List of results corresponding to each operation.
        """
        result = await self._post("/api/batch", json={"operations": operations})
        return result.get("results", [])

    # ------------------------------------------------------------------
    #  Storage
    # ------------------------------------------------------------------

    async def upload(
        self,
        path: str,
        content: bytes,
        filename: str,
        *,
        content_type: str | None = None,
    ) -> dict[str, Any]:
        """
        Upload a file to DarshJDB storage.

        Args:
            path: Storage path (e.g. ``"/avatars/photo.jpg"``).
            content: File content as bytes.
            filename: The filename.
            content_type: Optional MIME type.

        Returns:
            Dict with ``path``, ``url``, ``size``.
        """
        files = {"file": (filename, content)}
        form_data: dict[str, str] = {"path": path}
        if content_type:
            form_data["contentType"] = content_type

        headers = self._build_headers()
        headers.pop("Content-Type", None)  # Let httpx set multipart boundary

        response = await self._http.post(
            "/api/storage/upload",
            headers=headers,
            data=form_data,
            files=files,
        )
        self._check_response(response)
        return response.json()

    async def download(self, path: str) -> bytes:
        """
        Download a file from DarshJDB storage.

        Args:
            path: Storage path.

        Returns:
            File content as bytes.
        """
        response = await self._http.get(
            f"/api/storage/{path.lstrip('/')}",
            headers=self._build_headers(),
        )
        self._check_response(response)
        return response.content

    # ------------------------------------------------------------------
    #  Info / health
    # ------------------------------------------------------------------

    async def health(self) -> bool:
        """
        Check if the server is reachable.

        Returns:
            True if the server responded successfully.
        """
        try:
            response = await self._http.get("/api/health")
            return response.status_code < 400
        except Exception:
            return False

    async def version(self) -> str:
        """
        Get the server version string.

        Returns:
            Version string (e.g. ``"0.1.0"``).
        """
        result = await self._get("/api/health")
        return result.get("version", "unknown")

    # ------------------------------------------------------------------
    #  Internal HTTP helpers
    # ------------------------------------------------------------------

    def _build_headers(self) -> dict[str, str]:
        """Build request headers with auth token and namespace/database."""
        headers: dict[str, str] = {
            "Content-Type": "application/json",
            "Accept": "application/json",
        }
        if self._token:
            headers["Authorization"] = f"Bearer {self._token}"
        if self._namespace:
            headers["X-DarshDB-NS"] = self._namespace
        if self._database:
            headers["X-DarshDB-DB"] = self._database
        return headers

    async def _post(
        self,
        path: str,
        *,
        json: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Send a POST request and return parsed JSON."""
        return await self._request("POST", path, json=json)

    async def _get(
        self,
        path: str,
        *,
        params: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Send a GET request and return parsed JSON."""
        return await self._request("GET", path, params=params)

    async def _patch(
        self,
        path: str,
        *,
        json: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Send a PATCH request and return parsed JSON."""
        return await self._request("PATCH", path, json=json)

    async def _delete(
        self,
        path: str,
        *,
        json: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Send a DELETE request and return parsed JSON."""
        return await self._request("DELETE", path, json=json)

    async def _request(
        self,
        method: str,
        path: str,
        *,
        json: dict[str, Any] | None = None,
        params: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """
        Execute an HTTP request against the DarshJDB server.

        Raises:
            DarshDBAPIError: On 4xx/5xx responses.
            DarshDBConnectionError: On network errors.
        """
        headers = self._build_headers()

        try:
            response = await self._http.request(
                method,
                path,
                headers=headers,
                json=json,
                params=params,
            )
        except httpx.ConnectError as exc:
            raise DarshDBConnectionError(f"Cannot connect to {self._url}: {exc}") from exc
        except httpx.HTTPError as exc:
            raise DarshDBConnectionError(f"Network error: {exc}") from exc

        self._check_response(response)

        if response.status_code == 204:
            return {}

        try:
            return response.json()  # type: ignore[no-any-return]
        except Exception as exc:
            raise DarshDBError(f"Invalid JSON response: {exc}") from exc

    def _check_response(self, response: httpx.Response) -> None:
        """Raise DarshDBAPIError for non-success status codes."""
        if response.status_code < 400:
            return

        try:
            body = response.json()
        except Exception:
            body = {"raw": response.text}

        message = (
            body.get("error", {}).get("message")
            if isinstance(body.get("error"), dict)
            else body.get("message")
            or body.get("error")
            or response.text
        )

        if response.status_code in (401, 403):
            raise DarshDBAuthError(str(message))

        raise DarshDBAPIError(
            str(message),
            status_code=response.status_code,
            error_code=body.get("error", {}).get("code")
            if isinstance(body.get("error"), dict)
            else None,
            error_body=body,
        )
