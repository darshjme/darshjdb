"""Comprehensive tests for the DarshanDB Python SDK client."""

from __future__ import annotations

import inspect
from typing import Any
from unittest.mock import MagicMock, patch

import httpx
import pytest
import respx

from darshandb import DarshanDB, DarshanAdmin, AuthClient, StorageClient
from darshandb.exceptions import DarshanError, DarshanAPIError


# ---------------------------------------------------------------------------
#  Fixtures
# ---------------------------------------------------------------------------

SERVER = "http://localhost:7700"
API_KEY = "test-key"


@pytest.fixture
def db():
    """Create a DarshanDB client for testing."""
    client = DarshanDB(SERVER, api_key=API_KEY)
    yield client
    client.close()


@pytest.fixture
def mock_router():
    """Activate respx mock router for HTTP interception."""
    with respx.mock(base_url=SERVER) as router:
        yield router


# ---------------------------------------------------------------------------
#  Client initialization
# ---------------------------------------------------------------------------


class TestClientInit:
    def test_basic_init(self):
        db = DarshanDB("http://localhost:7700", api_key="key1")
        assert db._server_url == "http://localhost:7700"
        assert db._api_key == "key1"
        assert db._token is None
        db.close()

    def test_strips_trailing_slash(self):
        db = DarshanDB("http://localhost:7700/", api_key="key1")
        assert db._server_url == "http://localhost:7700"
        db.close()

    def test_strips_multiple_trailing_slashes(self):
        db = DarshanDB("http://localhost:7700///", api_key="key1")
        # rstrip("/") removes all trailing slashes
        assert db._server_url == "http://localhost:7700"
        db.close()

    def test_custom_timeout(self):
        db = DarshanDB(SERVER, api_key=API_KEY, timeout=60.0)
        assert db._http.timeout.connect == 60.0
        db.close()

    def test_default_timeout(self):
        db = DarshanDB(SERVER, api_key=API_KEY)
        assert db._http.timeout.connect == 30.0
        db.close()

    def test_missing_server_url_raises(self):
        with pytest.raises(ValueError, match="server_url is required"):
            DarshanDB("", api_key="key1")

    def test_missing_api_key_raises(self):
        with pytest.raises(ValueError, match="api_key is required"):
            DarshanDB(SERVER, api_key="")

    def test_none_server_url_raises(self):
        with pytest.raises((ValueError, TypeError)):
            DarshanDB(None, api_key="key1")  # type: ignore[arg-type]

    def test_none_api_key_raises(self):
        with pytest.raises((ValueError, TypeError)):
            DarshanDB(SERVER, api_key=None)  # type: ignore[arg-type]


# ---------------------------------------------------------------------------
#  Sub-clients
# ---------------------------------------------------------------------------


class TestSubClients:
    def test_auth_is_auth_client(self, db: DarshanDB):
        assert isinstance(db.auth, AuthClient)

    def test_storage_is_storage_client(self, db: DarshanDB):
        assert isinstance(db.storage, StorageClient)

    def test_auth_property_returns_same_instance(self, db: DarshanDB):
        assert db.auth is db.auth

    def test_storage_property_returns_same_instance(self, db: DarshanDB):
        assert db.storage is db.storage

    def test_auth_client_holds_reference_to_parent(self, db: DarshanDB):
        assert db.auth._client is db

    def test_storage_client_holds_reference_to_parent(self, db: DarshanDB):
        assert db.storage._client is db


# ---------------------------------------------------------------------------
#  Token management
# ---------------------------------------------------------------------------


class TestTokenManagement:
    def test_set_and_get_token(self, db: DarshanDB):
        assert db.get_token() is None
        db.set_token("tok_abc")
        assert db.get_token() == "tok_abc"

    def test_clear_token(self, db: DarshanDB):
        db.set_token("tok_abc")
        db.set_token(None)
        assert db.get_token() is None

    def test_build_headers_without_token(self, db: DarshanDB):
        headers = db._build_headers()
        assert headers["X-Api-Key"] == API_KEY
        assert "Authorization" not in headers

    def test_build_headers_with_token(self, db: DarshanDB):
        db.set_token("tok_abc")
        headers = db._build_headers()
        assert headers["Authorization"] == "Bearer tok_abc"
        assert headers["X-Api-Key"] == API_KEY

    def test_build_headers_content_type(self, db: DarshanDB):
        headers = db._build_headers()
        assert headers["Content-Type"] == "application/json"
        assert headers["Accept"] == "application/json"


# ---------------------------------------------------------------------------
#  Context manager
# ---------------------------------------------------------------------------


class TestContextManager:
    def test_enter_returns_self(self):
        db = DarshanDB(SERVER, api_key=API_KEY)
        assert db.__enter__() is db
        db.close()

    def test_context_manager_closes(self):
        with DarshanDB(SERVER, api_key=API_KEY) as db:
            assert db._http is not None
        # After exit, the http client should be closed
        assert db._http.is_closed


# ---------------------------------------------------------------------------
#  Query
# ---------------------------------------------------------------------------


class TestQuery:
    def test_query_sends_post(self, db: DarshanDB, mock_router):
        route = mock_router.post("/api/query").respond(
            json={"data": [{"id": "1"}], "txId": "tx1"}
        )
        result = db.query({"collection": "posts", "limit": 10})
        assert route.called
        assert result["data"] == [{"id": "1"}]
        assert result["txId"] == "tx1"

    def test_query_sends_correct_body(self, db: DarshanDB, mock_router):
        q = {
            "collection": "posts",
            "where": [{"field": "published", "op": "=", "value": True}],
            "limit": 20,
            "offset": 5,
        }
        route = mock_router.post("/api/query").respond(json={"data": [], "txId": "tx2"})
        db.query(q)
        assert route.called
        request = route.calls[0].request
        import json
        body = json.loads(request.content)
        assert body["collection"] == "posts"
        assert body["limit"] == 20
        assert body["offset"] == 5
        assert len(body["where"]) == 1

    def test_query_includes_api_key_header(self, db: DarshanDB, mock_router):
        route = mock_router.post("/api/query").respond(json={"data": [], "txId": "tx3"})
        db.query({"collection": "test"})
        request = route.calls[0].request
        assert request.headers["x-api-key"] == API_KEY

    def test_query_includes_auth_token(self, db: DarshanDB, mock_router):
        db.set_token("my-token")
        route = mock_router.post("/api/query").respond(json={"data": [], "txId": "tx4"})
        db.query({"collection": "test"})
        request = route.calls[0].request
        assert request.headers["authorization"] == "Bearer my-token"


# ---------------------------------------------------------------------------
#  Transact
# ---------------------------------------------------------------------------


class TestTransact:
    def test_transact_sends_post(self, db: DarshanDB, mock_router):
        route = mock_router.post("/api/transact").respond(json={"txId": "tx10"})
        ops = [
            {"kind": "set", "entity": "accounts", "id": "a1", "data": {"balance": 900}},
        ]
        result = db.transact(ops)
        assert route.called
        assert result["txId"] == "tx10"

    def test_transact_wraps_ops(self, db: DarshanDB, mock_router):
        route = mock_router.post("/api/transact").respond(json={"txId": "tx11"})
        ops = [
            {"kind": "set", "entity": "x", "id": "1", "data": {"a": 1}},
            {"kind": "delete", "entity": "x", "id": "2"},
        ]
        db.transact(ops)
        import json
        body = json.loads(route.calls[0].request.content)
        assert "ops" in body
        assert len(body["ops"]) == 2
        assert body["ops"][0]["kind"] == "set"
        assert body["ops"][1]["kind"] == "delete"

    def test_transact_empty_ops(self, db: DarshanDB, mock_router):
        route = mock_router.post("/api/transact").respond(json={"txId": "tx12"})
        db.transact([])
        import json
        body = json.loads(route.calls[0].request.content)
        assert body["ops"] == []


# ---------------------------------------------------------------------------
#  Server-side functions (fn)
# ---------------------------------------------------------------------------


class TestFn:
    def test_fn_sends_to_correct_path(self, db: DarshanDB, mock_router):
        route = mock_router.post("/api/fn/generateReport").respond(
            json={"result": {"rows": 42}}
        )
        result = db.fn("generateReport", {"month": "2026-04"})
        assert route.called
        assert result == {"rows": 42}

    def test_fn_extracts_result_key(self, db: DarshanDB, mock_router):
        mock_router.post("/api/fn/myFunc").respond(json={"result": "hello"})
        assert db.fn("myFunc") == "hello"

    def test_fn_returns_full_body_if_no_result_key(self, db: DarshanDB, mock_router):
        mock_router.post("/api/fn/myFunc").respond(json={"data": "raw"})
        result = db.fn("myFunc")
        assert result == {"data": "raw"}

    def test_fn_sends_empty_dict_when_no_args(self, db: DarshanDB, mock_router):
        route = mock_router.post("/api/fn/noArgs").respond(json={"result": True})
        db.fn("noArgs")
        import json
        body = json.loads(route.calls[0].request.content)
        assert body == {}

    def test_fn_sends_provided_args(self, db: DarshanDB, mock_router):
        route = mock_router.post("/api/fn/withArgs").respond(json={"result": True})
        db.fn("withArgs", {"x": 1, "y": "two"})
        import json
        body = json.loads(route.calls[0].request.content)
        assert body == {"x": 1, "y": "two"}


# ---------------------------------------------------------------------------
#  Data helpers (get, create, update, delete)
# ---------------------------------------------------------------------------


class TestDataHelpers:
    def test_get_builds_minimal_query(self, db: DarshanDB, mock_router):
        route = mock_router.post("/api/query").respond(json={"data": [], "txId": "t1"})
        db.get("users")
        import json
        body = json.loads(route.calls[0].request.content)
        assert body == {"collection": "users"}

    def test_get_with_all_params(self, db: DarshanDB, mock_router):
        route = mock_router.post("/api/query").respond(json={"data": [], "txId": "t2"})
        db.get(
            "users",
            where=[{"field": "active", "op": "=", "value": True}],
            order=[{"field": "name", "direction": "asc"}],
            limit=10,
            offset=5,
            select=["id", "name"],
        )
        import json
        body = json.loads(route.calls[0].request.content)
        assert body["collection"] == "users"
        assert body["where"] == [{"field": "active", "op": "=", "value": True}]
        assert body["order"] == [{"field": "name", "direction": "asc"}]
        assert body["limit"] == 10
        assert body["offset"] == 5
        assert body["select"] == ["id", "name"]

    def test_get_omits_none_params(self, db: DarshanDB, mock_router):
        route = mock_router.post("/api/query").respond(json={"data": [], "txId": "t3"})
        db.get("users", limit=5)
        import json
        body = json.loads(route.calls[0].request.content)
        assert "where" not in body
        assert "order" not in body
        assert "offset" not in body
        assert "select" not in body
        assert body["limit"] == 5

    def test_create_sends_post(self, db: DarshanDB, mock_router):
        route = mock_router.post("/api/data/users").respond(
            json={"id": "u1", "name": "Alice"}
        )
        result = db.create("users", {"name": "Alice"})
        assert result["id"] == "u1"

    def test_update_sends_post(self, db: DarshanDB, mock_router):
        route = mock_router.post("/api/data/users/u1").respond(
            json={"id": "u1", "name": "Bob"}
        )
        result = db.update("users", "u1", {"name": "Bob"})
        assert result["name"] == "Bob"

    def test_delete_sends_delete_method(self, db: DarshanDB, mock_router):
        route = mock_router.delete("/api/data/users/u1").respond(json={"ok": True})
        result = db.delete("users", "u1")
        assert result["ok"] is True


# ---------------------------------------------------------------------------
#  Error handling
# ---------------------------------------------------------------------------


class TestErrorHandling:
    def test_4xx_raises_api_error(self, db: DarshanDB, mock_router):
        mock_router.post("/api/query").respond(
            status_code=400,
            json={"message": "Bad query"},
        )
        with pytest.raises(DarshanAPIError) as exc_info:
            db.query({"collection": "x"})
        assert exc_info.value.status_code == 400
        assert "Bad query" in str(exc_info.value)

    def test_5xx_raises_api_error(self, db: DarshanDB, mock_router):
        mock_router.post("/api/query").respond(
            status_code=500,
            json={"error": "Internal error"},
        )
        with pytest.raises(DarshanAPIError) as exc_info:
            db.query({"collection": "x"})
        assert exc_info.value.status_code == 500

    def test_401_raises_api_error(self, db: DarshanDB, mock_router):
        mock_router.post("/api/query").respond(
            status_code=401,
            json={"message": "Unauthorized"},
        )
        with pytest.raises(DarshanAPIError) as exc_info:
            db.query({"collection": "x"})
        assert exc_info.value.status_code == 401

    def test_error_body_preserved(self, db: DarshanDB, mock_router):
        mock_router.post("/api/query").respond(
            status_code=403,
            json={"message": "Forbidden", "code": "PERM_DENIED"},
        )
        with pytest.raises(DarshanAPIError) as exc_info:
            db.query({"collection": "x"})
        assert exc_info.value.error_body["code"] == "PERM_DENIED"

    def test_non_json_error_body(self, db: DarshanDB, mock_router):
        mock_router.post("/api/query").respond(
            status_code=502,
            text="Bad Gateway",
            headers={"content-type": "text/plain"},
        )
        with pytest.raises(DarshanAPIError) as exc_info:
            db.query({"collection": "x"})
        assert exc_info.value.status_code == 502
        assert "raw" in exc_info.value.error_body

    def test_network_error_raises_darshan_error(self, db: DarshanDB, mock_router):
        mock_router.post("/api/query").mock(side_effect=httpx.ConnectError("refused"))
        with pytest.raises(DarshanError, match="Network error"):
            db.query({"collection": "x"})

    def test_invalid_json_response_raises_darshan_error(self, db: DarshanDB, mock_router):
        mock_router.post("/api/query").respond(
            status_code=200,
            text="not json at all",
            headers={"content-type": "text/plain"},
        )
        with pytest.raises(DarshanError, match="Invalid JSON"):
            db.query({"collection": "x"})

    def test_204_returns_empty_dict(self, db: DarshanDB, mock_router):
        mock_router.delete("/api/data/users/u1").respond(status_code=204)
        result = db.delete("users", "u1")
        assert result == {}


# ---------------------------------------------------------------------------
#  Auth client method signatures
# ---------------------------------------------------------------------------


class TestAuthClientSignatures:
    def test_sign_up_signature(self, db: DarshanDB):
        sig = inspect.signature(db.auth.sign_up)
        params = list(sig.parameters.keys())
        assert "email" in params
        assert "password" in params
        assert "display_name" in params
        assert "metadata" in params

    def test_sign_in_signature(self, db: DarshanDB):
        sig = inspect.signature(db.auth.sign_in)
        params = list(sig.parameters.keys())
        assert "email" in params
        assert "password" in params

    def test_sign_in_with_oauth_signature(self, db: DarshanDB):
        sig = inspect.signature(db.auth.sign_in_with_oauth)
        params = list(sig.parameters.keys())
        assert "provider" in params
        assert "token" in params
        assert "redirect_uri" in params

    def test_sign_out_signature(self, db: DarshanDB):
        sig = inspect.signature(db.auth.sign_out)
        params = list(sig.parameters.keys())
        assert params == []

    def test_get_user_signature(self, db: DarshanDB):
        sig = inspect.signature(db.auth.get_user)
        params = list(sig.parameters.keys())
        assert params == []

    def test_refresh_signature(self, db: DarshanDB):
        sig = inspect.signature(db.auth.refresh)
        params = list(sig.parameters.keys())
        assert "refresh_token" in params

    def test_set_token_signature(self, db: DarshanDB):
        sig = inspect.signature(db.auth.set_token)
        params = list(sig.parameters.keys())
        assert "token" in params


# ---------------------------------------------------------------------------
#  Storage client method signatures
# ---------------------------------------------------------------------------


class TestStorageClientSignatures:
    def test_upload_signature(self, db: DarshanDB):
        sig = inspect.signature(db.storage.upload)
        params = list(sig.parameters.keys())
        assert "path" in params
        assert "file_path" in params
        assert "content_type" in params
        assert "metadata" in params

    def test_upload_bytes_signature(self, db: DarshanDB):
        sig = inspect.signature(db.storage.upload_bytes)
        params = list(sig.parameters.keys())
        assert "path" in params
        assert "content" in params
        assert "filename" in params

    def test_get_url_signature(self, db: DarshanDB):
        sig = inspect.signature(db.storage.get_url)
        params = list(sig.parameters.keys())
        assert "path" in params
        assert "expiry" in params

    def test_delete_signature(self, db: DarshanDB):
        sig = inspect.signature(db.storage.delete)
        params = list(sig.parameters.keys())
        assert "path" in params

    def test_list_signature(self, db: DarshanDB):
        sig = inspect.signature(db.storage.list)
        params = list(sig.parameters.keys())
        assert "prefix" in params
        assert "limit" in params
        assert "cursor" in params


# ---------------------------------------------------------------------------
#  DarshanAdmin
# ---------------------------------------------------------------------------


class TestDarshanAdmin:
    def test_init(self):
        admin = DarshanAdmin(SERVER, admin_token="admin-tok")
        assert admin._server_url == "http://localhost:7700"
        assert admin._admin_token == "admin-tok"
        admin.close()

    def test_strips_trailing_slash(self):
        admin = DarshanAdmin("http://localhost:7700/", admin_token="admin-tok")
        assert admin._server_url == "http://localhost:7700"
        admin.close()

    def test_missing_server_url_raises(self):
        with pytest.raises(ValueError, match="server_url is required"):
            DarshanAdmin("", admin_token="admin-tok")

    def test_missing_admin_token_raises(self):
        with pytest.raises(ValueError, match="admin_token is required"):
            DarshanAdmin(SERVER, admin_token="")

    def test_custom_timeout(self):
        admin = DarshanAdmin(SERVER, admin_token="tok", timeout=120.0)
        assert admin._timeout == 120.0
        admin.close()

    def test_context_manager(self):
        with DarshanAdmin(SERVER, admin_token="tok") as admin:
            assert admin._http is not None
        assert admin._http.is_closed

    def test_build_headers(self):
        admin = DarshanAdmin(SERVER, admin_token="admin-tok")
        headers = admin._build_headers()
        assert headers["Authorization"] == "Bearer admin-tok"
        assert headers["Content-Type"] == "application/json"
        admin.close()


class TestDarshanAdminAsUser:
    def test_as_user_with_email(self):
        """as_user with email calls impersonate endpoint."""
        admin = DarshanAdmin(SERVER, admin_token="admin-tok")
        with respx.mock(base_url=SERVER) as router:
            router.post("/api/admin/impersonate").respond(
                json={"accessToken": "user-tok-123"}
            )
            user_db = admin.as_user("alice@example.com")
            assert isinstance(user_db, DarshanDB)
            assert user_db.get_token() == "user-tok-123"
            user_db.close()
        admin.close()

    def test_as_user_with_token(self):
        """as_user with a JWT-like string uses it directly."""
        admin = DarshanAdmin(SERVER, admin_token="admin-tok")
        user_db = admin.as_user("eyJhbGciOiJIUzI1NiJ9.payload.signature")
        assert isinstance(user_db, DarshanDB)
        assert user_db.get_token() == "eyJhbGciOiJIUzI1NiJ9.payload.signature"
        user_db.close()
        admin.close()

    def test_as_user_sets_admin_token_as_api_key(self):
        """Impersonated client uses admin token as the api_key."""
        admin = DarshanAdmin(SERVER, admin_token="admin-tok")
        user_db = admin.as_user("eyJ.payload.sig")
        assert user_db._api_key == "admin-tok"
        user_db.close()
        admin.close()


class TestDarshanAdminQuery:
    def test_admin_query(self):
        admin = DarshanAdmin(SERVER, admin_token="admin-tok")
        with respx.mock(base_url=SERVER) as router:
            route = router.post("/api/query").respond(
                json={"data": [{"id": "1"}], "txId": "tx1"}
            )
            result = admin.query({"collection": "users"})
            assert result["data"] == [{"id": "1"}]
            assert route.called
        admin.close()

    def test_admin_transact(self):
        admin = DarshanAdmin(SERVER, admin_token="admin-tok")
        with respx.mock(base_url=SERVER) as router:
            route = router.post("/api/transact").respond(json={"txId": "tx20"})
            result = admin.transact([{"kind": "set", "entity": "x", "id": "1", "data": {}}])
            assert result["txId"] == "tx20"
        admin.close()

    def test_admin_fn(self):
        admin = DarshanAdmin(SERVER, admin_token="admin-tok")
        with respx.mock(base_url=SERVER) as router:
            router.post("/api/fn/cleanup").respond(json={"result": "done"})
            result = admin.fn("cleanup")
            assert result == "done"
        admin.close()

    def test_admin_error_handling(self):
        admin = DarshanAdmin(SERVER, admin_token="admin-tok")
        with respx.mock(base_url=SERVER) as router:
            router.post("/api/query").respond(
                status_code=403, json={"message": "Admin only"}
            )
            with pytest.raises(DarshanAPIError) as exc_info:
                admin.query({"collection": "x"})
            assert exc_info.value.status_code == 403
        admin.close()

    def test_admin_network_error(self):
        admin = DarshanAdmin(SERVER, admin_token="admin-tok")
        with respx.mock(base_url=SERVER) as router:
            router.post("/api/query").mock(side_effect=httpx.ConnectError("fail"))
            with pytest.raises(DarshanError, match="Network error"):
                admin.query({"collection": "x"})
        admin.close()

    def test_admin_204_returns_empty(self):
        admin = DarshanAdmin(SERVER, admin_token="admin-tok")
        with respx.mock(base_url=SERVER) as router:
            router.post("/api/transact").respond(status_code=204)
            result = admin.transact([])
            assert result == {}
        admin.close()
