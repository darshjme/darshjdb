"""Tests for the DarshanDB AuthClient."""

from __future__ import annotations

import inspect

import httpx
import pytest
import respx

from darshandb import DarshanDB, AuthClient
from darshandb.exceptions import DarshanAPIError


SERVER = "http://localhost:7700"
API_KEY = "test-key"


@pytest.fixture
def db():
    client = DarshanDB(SERVER, api_key=API_KEY)
    yield client
    client.close()


# ---------------------------------------------------------------------------
#  AuthClient construction
# ---------------------------------------------------------------------------


class TestAuthClientInit:
    def test_is_auth_client_instance(self, db: DarshanDB):
        assert isinstance(db.auth, AuthClient)

    def test_holds_parent_reference(self, db: DarshanDB):
        assert db.auth._client is db


# ---------------------------------------------------------------------------
#  Method existence and signatures
# ---------------------------------------------------------------------------


class TestAuthMethodsExist:
    """Verify that all expected public methods exist with correct parameters."""

    def test_sign_up_exists(self, db: DarshanDB):
        assert callable(db.auth.sign_up)

    def test_sign_in_exists(self, db: DarshanDB):
        assert callable(db.auth.sign_in)

    def test_sign_in_with_oauth_exists(self, db: DarshanDB):
        assert callable(db.auth.sign_in_with_oauth)

    def test_sign_out_exists(self, db: DarshanDB):
        assert callable(db.auth.sign_out)

    def test_get_user_exists(self, db: DarshanDB):
        assert callable(db.auth.get_user)

    def test_refresh_exists(self, db: DarshanDB):
        assert callable(db.auth.refresh)

    def test_set_token_exists(self, db: DarshanDB):
        assert callable(db.auth.set_token)

    def test_sign_up_params(self, db: DarshanDB):
        sig = inspect.signature(db.auth.sign_up)
        p = sig.parameters
        assert "email" in p
        assert "password" in p
        assert "display_name" in p
        assert p["display_name"].default is None
        assert "metadata" in p
        assert p["metadata"].default is None

    def test_sign_in_params(self, db: DarshanDB):
        sig = inspect.signature(db.auth.sign_in)
        p = sig.parameters
        assert "email" in p
        assert "password" in p
        # Only self, email, password
        positional = [k for k, v in p.items() if v.kind in (
            inspect.Parameter.POSITIONAL_ONLY,
            inspect.Parameter.POSITIONAL_OR_KEYWORD,
        )]
        assert positional == ["email", "password"]

    def test_sign_in_with_oauth_params(self, db: DarshanDB):
        sig = inspect.signature(db.auth.sign_in_with_oauth)
        p = sig.parameters
        assert "provider" in p
        assert "token" in p
        assert "redirect_uri" in p
        assert p["redirect_uri"].default == ""

    def test_refresh_params(self, db: DarshanDB):
        sig = inspect.signature(db.auth.refresh)
        p = sig.parameters
        assert "refresh_token" in p


# ---------------------------------------------------------------------------
#  Auth HTTP behavior
# ---------------------------------------------------------------------------


class TestAuthSignUp:
    def test_sign_up_posts_to_signup(self, db: DarshanDB):
        with respx.mock(base_url=SERVER) as router:
            route = router.post("/api/auth/signup").respond(json={
                "user": {"id": "u1", "email": "a@b.com"},
                "accessToken": "at1",
                "refreshToken": "rt1",
            })
            result = db.auth.sign_up("a@b.com", "pass123")
            assert route.called
            assert result["user"]["email"] == "a@b.com"

    def test_sign_up_sets_token(self, db: DarshanDB):
        with respx.mock(base_url=SERVER) as router:
            router.post("/api/auth/signup").respond(json={
                "user": {"id": "u1"},
                "accessToken": "tok-from-signup",
                "refreshToken": "rt",
            })
            db.auth.sign_up("a@b.com", "pass")
            assert db.get_token() == "tok-from-signup"

    def test_sign_up_with_display_name(self, db: DarshanDB):
        import json as jsonlib
        with respx.mock(base_url=SERVER) as router:
            route = router.post("/api/auth/signup").respond(json={
                "user": {"id": "u1"},
                "accessToken": "at",
                "refreshToken": "rt",
            })
            db.auth.sign_up("a@b.com", "pass", display_name="Alice")
            body = jsonlib.loads(route.calls[0].request.content)
            assert body["displayName"] == "Alice"

    def test_sign_up_with_metadata(self, db: DarshanDB):
        import json as jsonlib
        with respx.mock(base_url=SERVER) as router:
            route = router.post("/api/auth/signup").respond(json={
                "user": {"id": "u1"},
                "accessToken": "at",
                "refreshToken": "rt",
            })
            db.auth.sign_up("a@b.com", "pass", metadata={"role": "admin"})
            body = jsonlib.loads(route.calls[0].request.content)
            assert body["metadata"] == {"role": "admin"}

    def test_sign_up_without_optional_fields(self, db: DarshanDB):
        import json as jsonlib
        with respx.mock(base_url=SERVER) as router:
            route = router.post("/api/auth/signup").respond(json={
                "user": {"id": "u1"},
                "accessToken": "at",
                "refreshToken": "rt",
            })
            db.auth.sign_up("a@b.com", "pass")
            body = jsonlib.loads(route.calls[0].request.content)
            assert "displayName" not in body
            assert "metadata" not in body

    def test_sign_up_no_token_in_response(self, db: DarshanDB):
        """If server doesn't return accessToken, token stays None."""
        with respx.mock(base_url=SERVER) as router:
            router.post("/api/auth/signup").respond(json={
                "user": {"id": "u1"},
            })
            db.auth.sign_up("a@b.com", "pass")
            assert db.get_token() is None


class TestAuthSignIn:
    def test_sign_in_posts_to_signin(self, db: DarshanDB):
        with respx.mock(base_url=SERVER) as router:
            route = router.post("/api/auth/signin").respond(json={
                "user": {"id": "u1"},
                "accessToken": "at-signin",
                "refreshToken": "rt",
            })
            result = db.auth.sign_in("a@b.com", "pass")
            assert route.called
            assert result["accessToken"] == "at-signin"

    def test_sign_in_sets_token(self, db: DarshanDB):
        with respx.mock(base_url=SERVER) as router:
            router.post("/api/auth/signin").respond(json={
                "user": {"id": "u1"},
                "accessToken": "tok-signin",
                "refreshToken": "rt",
            })
            db.auth.sign_in("a@b.com", "pass")
            assert db.get_token() == "tok-signin"

    def test_sign_in_sends_correct_body(self, db: DarshanDB):
        import json as jsonlib
        with respx.mock(base_url=SERVER) as router:
            route = router.post("/api/auth/signin").respond(json={
                "user": {"id": "u1"},
                "accessToken": "at",
                "refreshToken": "rt",
            })
            db.auth.sign_in("user@test.com", "secret")
            body = jsonlib.loads(route.calls[0].request.content)
            assert body == {"email": "user@test.com", "password": "secret"}


class TestAuthOAuth:
    def test_oauth_posts_to_oauth(self, db: DarshanDB):
        with respx.mock(base_url=SERVER) as router:
            route = router.post("/api/auth/oauth").respond(json={
                "user": {"id": "u1"},
                "accessToken": "at-oauth",
                "refreshToken": "rt",
            })
            result = db.auth.sign_in_with_oauth("google", "goog-tok")
            assert route.called
            assert result["accessToken"] == "at-oauth"

    def test_oauth_sends_correct_body(self, db: DarshanDB):
        import json as jsonlib
        with respx.mock(base_url=SERVER) as router:
            route = router.post("/api/auth/oauth").respond(json={
                "user": {"id": "u1"},
                "accessToken": "at",
                "refreshToken": "rt",
            })
            db.auth.sign_in_with_oauth("github", "gh-tok", redirect_uri="http://app/cb")
            body = jsonlib.loads(route.calls[0].request.content)
            assert body["provider"] == "github"
            assert body["token"] == "gh-tok"
            assert body["redirectUri"] == "http://app/cb"

    def test_oauth_sets_token(self, db: DarshanDB):
        with respx.mock(base_url=SERVER) as router:
            router.post("/api/auth/oauth").respond(json={
                "user": {"id": "u1"},
                "accessToken": "oauth-tok",
                "refreshToken": "rt",
            })
            db.auth.sign_in_with_oauth("google", "tok")
            assert db.get_token() == "oauth-tok"


class TestAuthSignOut:
    def test_sign_out_clears_token(self, db: DarshanDB):
        db.set_token("existing-token")
        with respx.mock(base_url=SERVER) as router:
            router.post("/api/auth/signout").respond(json={"ok": True})
            db.auth.sign_out()
            assert db.get_token() is None

    def test_sign_out_posts_to_signout(self, db: DarshanDB):
        with respx.mock(base_url=SERVER) as router:
            route = router.post("/api/auth/signout").respond(json={"ok": True})
            db.auth.sign_out()
            assert route.called


class TestAuthGetUser:
    def test_get_user_returns_user(self, db: DarshanDB):
        with respx.mock(base_url=SERVER) as router:
            router.get("/api/auth/me").respond(json={
                "id": "u1", "email": "a@b.com", "displayName": "Alice",
            })
            user = db.auth.get_user()
            assert user is not None
            assert user["email"] == "a@b.com"

    def test_get_user_returns_none_on_401(self, db: DarshanDB):
        with respx.mock(base_url=SERVER) as router:
            router.get("/api/auth/me").respond(
                status_code=401, json={"message": "Unauthorized"}
            )
            user = db.auth.get_user()
            assert user is None

    def test_get_user_raises_on_other_errors(self, db: DarshanDB):
        with respx.mock(base_url=SERVER) as router:
            router.get("/api/auth/me").respond(
                status_code=500, json={"message": "Server error"}
            )
            with pytest.raises(DarshanAPIError) as exc_info:
                db.auth.get_user()
            assert exc_info.value.status_code == 500


class TestAuthRefresh:
    def test_refresh_posts_to_refresh(self, db: DarshanDB):
        with respx.mock(base_url=SERVER) as router:
            route = router.post("/api/auth/refresh").respond(json={
                "accessToken": "new-at",
                "refreshToken": "new-rt",
                "expiresAt": 9999999999,
            })
            result = db.auth.refresh("old-rt")
            assert route.called
            assert result["accessToken"] == "new-at"

    def test_refresh_updates_token(self, db: DarshanDB):
        with respx.mock(base_url=SERVER) as router:
            router.post("/api/auth/refresh").respond(json={
                "accessToken": "refreshed-tok",
                "refreshToken": "rt2",
            })
            db.auth.refresh("rt1")
            assert db.get_token() == "refreshed-tok"

    def test_refresh_sends_correct_body(self, db: DarshanDB):
        import json as jsonlib
        with respx.mock(base_url=SERVER) as router:
            route = router.post("/api/auth/refresh").respond(json={
                "accessToken": "at",
                "refreshToken": "rt",
            })
            db.auth.refresh("my-refresh-token")
            body = jsonlib.loads(route.calls[0].request.content)
            assert body == {"refreshToken": "my-refresh-token"}


class TestAuthSetToken:
    def test_set_token_delegates_to_client(self, db: DarshanDB):
        db.auth.set_token("manual-tok")
        assert db.get_token() == "manual-tok"
