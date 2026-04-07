"""Comprehensive tests for the DarshJDB Python SDK."""

from __future__ import annotations

import json
from typing import Any

import httpx
import pytest
import respx

from darshjdb import (
    DarshDB,
    DarshDBError,
    DarshDBConnectionError,
    DarshDBAuthError,
    DarshDBQueryError,
    DarshDBAPIError,
    QueryResult,
    LiveNotification,
    LiveAction,
    AuthResponse,
    ConnectionState,
)

# ---------------------------------------------------------------------------
#  Fixtures
# ---------------------------------------------------------------------------

SERVER = "http://localhost:8080"


@pytest.fixture
def db():
    """Create a DarshDB client for testing."""
    return DarshDB(SERVER)


@pytest.fixture
def mock_router():
    """Activate respx mock router for HTTP interception."""
    with respx.mock(base_url=SERVER) as router:
        yield router


# ---------------------------------------------------------------------------
#  Client initialization
# ---------------------------------------------------------------------------


class TestInit:
    def test_basic_init(self):
        db = DarshDB("http://localhost:8080")
        assert db._url == "http://localhost:8080"
        assert db._token is None
        assert db._namespace is None
        assert db._database is None
        assert db.state == ConnectionState.CONNECTED

    def test_strips_trailing_slash(self):
        db = DarshDB("http://localhost:8080/")
        assert db._url == "http://localhost:8080"

    def test_custom_timeout(self):
        db = DarshDB(SERVER, timeout=60.0)
        assert db._http.timeout.connect == 60.0

    def test_missing_url_raises(self):
        with pytest.raises(ValueError, match="url is required"):
            DarshDB("")

    def test_context_manager(self):
        async def _test():
            async with DarshDB(SERVER) as db:
                assert db.state == ConnectionState.CONNECTED
            assert db.state == ConnectionState.DISCONNECTED

        import asyncio
        asyncio.run(_test())


# ---------------------------------------------------------------------------
#  Authentication
# ---------------------------------------------------------------------------


class TestAuth:
    @pytest.mark.asyncio
    async def test_signin_with_user_pass(self, db: DarshDB, mock_router):
        mock_router.post("/api/auth/signin").respond(
            json={"accessToken": "tok123", "user": {"id": "u1"}, "refreshToken": "ref1"}
        )
        result = await db.signin({"user": "root", "pass": "root"})
        assert isinstance(result, AuthResponse)
        assert result.token == "tok123"
        assert result.user == {"id": "u1"}
        assert result.refresh_token == "ref1"
        assert db._token == "tok123"

    @pytest.mark.asyncio
    async def test_signin_with_email_password(self, db: DarshDB, mock_router):
        route = mock_router.post("/api/auth/signin").respond(
            json={"accessToken": "tok456"}
        )
        await db.signin({"email": "alice@example.com", "password": "secret"})
        body = json.loads(route.calls[0].request.content)
        assert body["email"] == "alice@example.com"
        assert body["password"] == "secret"

    @pytest.mark.asyncio
    async def test_signin_sets_namespace_database(self, db: DarshDB, mock_router):
        mock_router.post("/api/auth/signin").respond(json={"accessToken": "tok"})
        await db.signin({"user": "root", "pass": "root", "namespace": "ns1", "database": "db1"})
        assert db._namespace == "ns1"
        assert db._database == "db1"

    @pytest.mark.asyncio
    async def test_signin_auth_failure(self, db: DarshDB, mock_router):
        mock_router.post("/api/auth/signin").respond(
            status_code=401, json={"message": "Invalid credentials"}
        )
        with pytest.raises(DarshDBAuthError):
            await db.signin({"user": "root", "pass": "wrong"})

    @pytest.mark.asyncio
    async def test_signup(self, db: DarshDB, mock_router):
        mock_router.post("/api/auth/signup").respond(
            json={"accessToken": "new-tok", "user": {"id": "u2", "email": "bob@test.com"}}
        )
        result = await db.signup({"email": "bob@test.com", "password": "pass123", "name": "Bob"})
        assert result.token == "new-tok"
        assert db._token == "new-tok"

    @pytest.mark.asyncio
    async def test_invalidate(self, db: DarshDB, mock_router):
        db._token = "active-tok"
        mock_router.post("/api/auth/signout").respond(json={})
        await db.invalidate()
        assert db._token is None

    @pytest.mark.asyncio
    async def test_authenticate(self, db: DarshDB):
        await db.authenticate("stored-token")
        assert db._token == "stored-token"


# ---------------------------------------------------------------------------
#  Namespace / Database
# ---------------------------------------------------------------------------


class TestUse:
    @pytest.mark.asyncio
    async def test_use_sets_ns_db(self, db: DarshDB):
        await db.use("test_ns", "test_db")
        assert db._namespace == "test_ns"
        assert db._database == "test_db"

    @pytest.mark.asyncio
    async def test_headers_include_ns_db(self, db: DarshDB, mock_router):
        await db.use("myns", "mydb")
        db._token = "tok"
        route = mock_router.post("/api/query").respond(json={"data": []})
        await db.query("SELECT * FROM users")
        req = route.calls[0].request
        assert req.headers["x-darshdb-ns"] == "myns"
        assert req.headers["x-darshdb-db"] == "mydb"
        assert req.headers["authorization"] == "Bearer tok"


# ---------------------------------------------------------------------------
#  CRUD
# ---------------------------------------------------------------------------


class TestCRUD:
    @pytest.mark.asyncio
    async def test_select_table(self, db: DarshDB, mock_router):
        mock_router.get("/api/data/users").respond(
            json={"data": [{"id": "u1", "name": "Alice"}]}
        )
        result = await db.select("users")
        assert len(result) == 1
        assert result[0]["name"] == "Alice"

    @pytest.mark.asyncio
    async def test_select_record(self, db: DarshDB, mock_router):
        mock_router.get("/api/data/users/darsh").respond(
            json={"id": "darsh", "name": "Darsh"}
        )
        result = await db.select("users:darsh")
        assert len(result) == 1
        assert result[0]["id"] == "darsh"

    @pytest.mark.asyncio
    async def test_create(self, db: DarshDB, mock_router):
        route = mock_router.post("/api/data/users").respond(
            json={"id": "u1", "name": "Darsh", "age": 30}
        )
        result = await db.create("users", {"name": "Darsh", "age": 30})
        assert result["name"] == "Darsh"
        body = json.loads(route.calls[0].request.content)
        assert body["name"] == "Darsh"

    @pytest.mark.asyncio
    async def test_create_with_id(self, db: DarshDB, mock_router):
        route = mock_router.post("/api/data/users").respond(
            json={"id": "darsh", "name": "Darsh"}
        )
        await db.create("users:darsh", {"name": "Darsh"})
        body = json.loads(route.calls[0].request.content)
        assert body["id"] == "darsh"
        assert body["name"] == "Darsh"

    @pytest.mark.asyncio
    async def test_update(self, db: DarshDB, mock_router):
        mock_router.patch("/api/data/users/darsh").respond(
            json={"id": "darsh", "age": 31}
        )
        result = await db.update("users:darsh", {"age": 31})
        assert result["age"] == 31

    @pytest.mark.asyncio
    async def test_update_requires_id(self, db: DarshDB):
        with pytest.raises(DarshDBError, match="requires a record ID"):
            await db.update("users", {"age": 31})

    @pytest.mark.asyncio
    async def test_merge(self, db: DarshDB, mock_router):
        mock_router.patch("/api/data/users/darsh").respond(
            json={"id": "darsh", "name": "Darsh", "age": 31}
        )
        result = await db.merge("users:darsh", {"age": 31})
        assert result["age"] == 31

    @pytest.mark.asyncio
    async def test_delete_record(self, db: DarshDB, mock_router):
        mock_router.delete("/api/data/users/darsh").respond(json={"ok": True})
        result = await db.delete("users:darsh")
        assert result["ok"] is True

    @pytest.mark.asyncio
    async def test_delete_table(self, db: DarshDB, mock_router):
        mock_router.delete("/api/data/users").respond(status_code=204)
        result = await db.delete("users")
        assert result == {}

    @pytest.mark.asyncio
    async def test_insert_single(self, db: DarshDB, mock_router):
        mock_router.post("/api/mutate").respond(
            json={"results": [{"id": "u1", "name": "Alice"}]}
        )
        result = await db.insert("users", {"name": "Alice"})
        assert len(result) == 1

    @pytest.mark.asyncio
    async def test_insert_batch(self, db: DarshDB, mock_router):
        route = mock_router.post("/api/mutate").respond(
            json={"results": [{"id": "u1"}, {"id": "u2"}]}
        )
        await db.insert("users", [{"name": "A"}, {"name": "B"}])
        body = json.loads(route.calls[0].request.content)
        assert len(body["mutations"]) == 2
        assert all(m["op"] == "insert" for m in body["mutations"])


# ---------------------------------------------------------------------------
#  Query
# ---------------------------------------------------------------------------


class TestQuery:
    @pytest.mark.asyncio
    async def test_query_returns_query_result(self, db: DarshDB, mock_router):
        mock_router.post("/api/query").respond(
            json={"data": [{"id": "1", "age": 25}], "meta": {"count": 1, "duration_ms": 0.5}}
        )
        results = await db.query("SELECT * FROM users WHERE age > 18")
        assert len(results) == 1
        assert isinstance(results[0], QueryResult)
        assert results[0].count == 1
        assert results[0].data[0]["age"] == 25

    @pytest.mark.asyncio
    async def test_query_sends_correct_body(self, db: DarshDB, mock_router):
        route = mock_router.post("/api/query").respond(json={"data": []})
        await db.query("SELECT * FROM users", vars={"min_age": 18})
        body = json.loads(route.calls[0].request.content)
        assert body["query"] == "SELECT * FROM users"
        assert body["vars"] == {"min_age": 18}

    @pytest.mark.asyncio
    async def test_query_error_raises(self, db: DarshDB, mock_router):
        mock_router.post("/api/query").respond(
            status_code=400, json={"message": "Parse error at line 1"}
        )
        with pytest.raises(DarshDBQueryError) as exc_info:
            await db.query("INVALID QUERY")
        assert exc_info.value.query == "INVALID QUERY"

    @pytest.mark.asyncio
    async def test_query_raw(self, db: DarshDB, mock_router):
        mock_router.post("/api/query").respond(
            json={"data": [], "meta": {"count": 0}, "extra": "field"}
        )
        raw = await db.query_raw("SELECT * FROM users")
        assert "extra" in raw


# ---------------------------------------------------------------------------
#  Graph
# ---------------------------------------------------------------------------


class TestGraph:
    @pytest.mark.asyncio
    async def test_relate(self, db: DarshDB, mock_router):
        route = mock_router.post("/api/mutate").respond(
            json={"results": [{"id": "r1"}]}
        )
        result = await db.relate("user:darsh", "works_at", "company:knowai")
        body = json.loads(route.calls[0].request.content)
        mutation = body["mutations"][0]
        assert mutation["op"] == "insert"
        assert mutation["entity"] == "works_at"
        assert mutation["data"]["from_entity"] == "user"
        assert mutation["data"]["from_id"] == "darsh"
        assert mutation["data"]["to_entity"] == "company"
        assert mutation["data"]["to_id"] == "knowai"

    @pytest.mark.asyncio
    async def test_relate_with_data(self, db: DarshDB, mock_router):
        route = mock_router.post("/api/mutate").respond(json={"results": [{}]})
        await db.relate("user:darsh", "works_at", "company:knowai", {"role": "CEO"})
        body = json.loads(route.calls[0].request.content)
        assert body["mutations"][0]["data"]["role"] == "CEO"


# ---------------------------------------------------------------------------
#  Server-side functions
# ---------------------------------------------------------------------------


class TestRun:
    @pytest.mark.asyncio
    async def test_run_function(self, db: DarshDB, mock_router):
        mock_router.post("/api/fn/generateReport").respond(
            json={"result": {"rows": 42}}
        )
        result = await db.run("generateReport", {"month": "2026-04"})
        assert result == {"rows": 42}

    @pytest.mark.asyncio
    async def test_run_extracts_result(self, db: DarshDB, mock_router):
        mock_router.post("/api/fn/ping").respond(json={"result": "pong"})
        assert await db.run("ping") == "pong"

    @pytest.mark.asyncio
    async def test_run_no_result_key(self, db: DarshDB, mock_router):
        mock_router.post("/api/fn/raw").respond(json={"data": "test"})
        result = await db.run("raw")
        assert result == {"data": "test"}


# ---------------------------------------------------------------------------
#  Error handling
# ---------------------------------------------------------------------------


class TestErrors:
    @pytest.mark.asyncio
    async def test_4xx_raises_api_error(self, db: DarshDB, mock_router):
        mock_router.post("/api/query").respond(
            status_code=400, json={"message": "Bad query"}
        )
        with pytest.raises(DarshDBQueryError):
            await db.query("BAD")

    @pytest.mark.asyncio
    async def test_5xx_raises_api_error(self, db: DarshDB, mock_router):
        mock_router.get("/api/data/users").respond(
            status_code=500, json={"error": "Internal error"}
        )
        with pytest.raises(DarshDBAPIError) as exc_info:
            await db.select("users")
        assert exc_info.value.status_code == 500

    @pytest.mark.asyncio
    async def test_401_raises_auth_error(self, db: DarshDB, mock_router):
        mock_router.get("/api/data/users").respond(
            status_code=401, json={"message": "Unauthorized"}
        )
        with pytest.raises(DarshDBAuthError):
            await db.select("users")

    @pytest.mark.asyncio
    async def test_network_error(self, db: DarshDB, mock_router):
        mock_router.get("/api/data/users").mock(side_effect=httpx.ConnectError("refused"))
        with pytest.raises(DarshDBConnectionError):
            await db.select("users")

    @pytest.mark.asyncio
    async def test_204_returns_empty(self, db: DarshDB, mock_router):
        mock_router.delete("/api/data/users/u1").respond(status_code=204)
        result = await db.delete("users:u1")
        assert result == {}


# ---------------------------------------------------------------------------
#  Models
# ---------------------------------------------------------------------------


class TestModels:
    def test_query_result_iteration(self):
        qr = QueryResult(data=[{"a": 1}, {"a": 2}], meta={"count": 2})
        assert len(qr) == 2
        assert list(qr) == [{"a": 1}, {"a": 2}]
        assert qr.first() == {"a": 1}
        assert qr.count == 2
        assert bool(qr) is True

    def test_query_result_empty(self):
        qr = QueryResult()
        assert len(qr) == 0
        assert qr.first() is None
        assert bool(qr) is False

    def test_live_notification_from_dict(self):
        notif = LiveNotification.from_dict({"action": "CREATE", "result": {"id": "u1"}})
        assert notif.action == LiveAction.CREATE
        assert notif.result == {"id": "u1"}

    def test_live_notification_from_dict_event_key(self):
        notif = LiveNotification.from_dict({"event": "deleted", "data": {"id": "u2"}})
        assert notif.action == LiveAction.DELETE
        assert notif.result == {"id": "u2"}

    def test_auth_response(self):
        ar = AuthResponse(token="tok", user={"id": "1"}, refresh_token="ref")
        assert ar.token == "tok"
        assert ar.user == {"id": "1"}
        assert ar.refresh_token == "ref"


# ---------------------------------------------------------------------------
#  Batch
# ---------------------------------------------------------------------------


class TestBatch:
    @pytest.mark.asyncio
    async def test_batch_operations(self, db: DarshDB, mock_router):
        route = mock_router.post("/api/batch").respond(
            json={"results": [{"id": "u1"}, {"data": []}]}
        )
        results = await db.batch([
            {"method": "POST", "path": "/api/data/users", "body": {"name": "A"}},
            {"method": "GET", "path": "/api/data/users"},
        ])
        assert len(results) == 2


# ---------------------------------------------------------------------------
#  Health
# ---------------------------------------------------------------------------


class TestHealth:
    @pytest.mark.asyncio
    async def test_health_true(self, db: DarshDB, mock_router):
        mock_router.get("/api/health").respond(json={"status": "ok"})
        assert await db.health() is True

    @pytest.mark.asyncio
    async def test_health_false_on_error(self, db: DarshDB, mock_router):
        mock_router.get("/api/health").mock(side_effect=httpx.ConnectError("down"))
        assert await db.health() is False

    @pytest.mark.asyncio
    async def test_version(self, db: DarshDB, mock_router):
        mock_router.get("/api/health").respond(json={"version": "0.2.0"})
        assert await db.version() == "0.2.0"
