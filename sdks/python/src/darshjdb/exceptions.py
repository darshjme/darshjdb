"""Exception hierarchy for the DarshJDB Python SDK."""

from __future__ import annotations

from typing import Any


class DarshDBError(Exception):
    """Base exception for all DarshJDB SDK errors."""

    def __init__(self, message: str = "An unknown error occurred") -> None:
        super().__init__(message)
        self.message = message


class DarshDBConnectionError(DarshDBError):
    """Raised when the SDK cannot connect to the DarshJDB server."""

    def __init__(self, message: str = "Failed to connect to DarshJDB server") -> None:
        super().__init__(message)


class DarshDBAuthError(DarshDBError):
    """Raised when authentication fails (invalid credentials, expired token, etc.)."""

    def __init__(self, message: str = "Authentication failed") -> None:
        super().__init__(message)


class DarshDBQueryError(DarshDBError):
    """Raised when a query fails to parse or execute."""

    def __init__(
        self,
        message: str = "Query execution failed",
        *,
        query: str | None = None,
    ) -> None:
        super().__init__(message)
        self.query = query


class DarshDBAPIError(DarshDBError):
    """
    Raised when the DarshJDB server returns an HTTP error response.

    Attributes:
        status_code: HTTP status code from the server.
        error_code: Application-level error code (if provided).
        error_body: Full parsed JSON error payload.
    """

    def __init__(
        self,
        message: str,
        *,
        status_code: int = 0,
        error_code: str | None = None,
        error_body: dict[str, Any] | None = None,
    ) -> None:
        super().__init__(message)
        self.status_code = status_code
        self.error_code = error_code
        self.error_body = error_body or {}

    def __repr__(self) -> str:
        return (
            f"DarshDBAPIError({self.message!r}, "
            f"status_code={self.status_code}, "
            f"error_code={self.error_code!r})"
        )
