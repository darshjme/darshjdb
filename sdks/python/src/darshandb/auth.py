"""
Authentication client for DarshanDB.

Handles user registration, login, logout, and current-user retrieval.
Tokens are managed automatically on the parent :class:`DarshanDB` client.

Usage::

    db = DarshanDB(server_url="...", api_key="...")

    # Sign up
    result = db.auth.sign_up("alice@example.com", "password123", display_name="Alice")

    # Sign in
    result = db.auth.sign_in("alice@example.com", "password123")

    # Get current user
    user = db.auth.get_user()

    # Sign out
    db.auth.sign_out()
"""

from __future__ import annotations

from typing import Any, TYPE_CHECKING

if TYPE_CHECKING:
    from darshandb.client import DarshanDB


class AuthClient:
    """
    Authentication operations for DarshanDB.

    Accessed via ``db.auth`` on a :class:`~darshandb.client.DarshanDB` instance.
    """

    def __init__(self, client: DarshanDB) -> None:
        self._client = client

    def sign_up(
        self,
        email: str,
        password: str,
        *,
        display_name: str | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """
        Register a new user with email and password.

        Args:
            email: The user's email address.
            password: The user's password.
            display_name: Optional display name.
            metadata: Optional custom metadata key-value pairs.

        Returns:
            A dict with ``user``, ``accessToken``, and ``refreshToken``.

        Raises:
            DarshanAPIError: On validation or server errors.
        """
        body: dict[str, Any] = {"email": email, "password": password}
        if display_name is not None:
            body["displayName"] = display_name
        if metadata is not None:
            body["metadata"] = metadata

        result = self._client._post("/api/auth/signup", json=body)

        if "accessToken" in result:
            self._client.set_token(result["accessToken"])

        return result

    def sign_in(self, email: str, password: str) -> dict[str, Any]:
        """
        Authenticate an existing user with email and password.

        Args:
            email: The user's email address.
            password: The user's password.

        Returns:
            A dict with ``user``, ``accessToken``, and ``refreshToken``.

        Raises:
            DarshanAPIError: On invalid credentials or server errors.
        """
        result = self._client._post("/api/auth/signin", json={
            "email": email,
            "password": password,
        })

        if "accessToken" in result:
            self._client.set_token(result["accessToken"])

        return result

    def sign_in_with_oauth(
        self,
        provider: str,
        token: str,
        *,
        redirect_uri: str = "",
    ) -> dict[str, Any]:
        """
        Sign in using an OAuth2 provider token.

        Args:
            provider: OAuth provider name (``google``, ``github``, ``apple``, ``discord``).
            token: OAuth access token or authorization code.
            redirect_uri: The redirect URI used in the OAuth flow.

        Returns:
            A dict with ``user``, ``accessToken``, and ``refreshToken``.

        Raises:
            DarshanAPIError: On OAuth or server errors.
        """
        result = self._client._post("/api/auth/oauth", json={
            "provider": provider,
            "token": token,
            "redirectUri": redirect_uri,
        })

        if "accessToken" in result:
            self._client.set_token(result["accessToken"])

        return result

    def sign_out(self) -> dict[str, Any]:
        """
        Sign out the current user and invalidate the session.

        Returns:
            Server acknowledgement.
        """
        result = self._client._post("/api/auth/signout")
        self._client.set_token(None)
        return result

    def get_user(self) -> dict[str, Any] | None:
        """
        Retrieve the currently authenticated user.

        Returns:
            User dict with ``id``, ``email``, ``displayName``, etc.,
            or ``None`` if not authenticated.
        """
        from darshandb.exceptions import DarshanAPIError

        try:
            return self._client._get("/api/auth/me")
        except DarshanAPIError as exc:
            if exc.status_code == 401:
                return None
            raise

    def refresh(self, refresh_token: str) -> dict[str, Any]:
        """
        Refresh the access token using a refresh token.

        Args:
            refresh_token: The refresh token from a previous sign-in.

        Returns:
            A dict with ``accessToken``, ``refreshToken``, and ``expiresAt``.
        """
        result = self._client._post("/api/auth/refresh", json={
            "refreshToken": refresh_token,
        })

        if "accessToken" in result:
            self._client.set_token(result["accessToken"])

        return result

    def set_token(self, token: str) -> None:
        """
        Manually set the access token on the client.

        Useful when restoring sessions from stored tokens.

        Args:
            token: A valid access token.
        """
        self._client.set_token(token)
