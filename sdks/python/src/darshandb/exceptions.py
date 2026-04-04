"""Exception types for the DarshanDB SDK."""

from __future__ import annotations

from typing import Any


class DarshanError(Exception):
    """Base exception for all DarshanDB SDK errors."""


class DarshanAPIError(DarshanError):
    """
    Raised when the DarshanDB server returns an error response.

    Attributes:
        status_code: HTTP status code from the server.
        error_body: Parsed JSON error payload (if available).
    """

    def __init__(
        self,
        message: str,
        *,
        status_code: int | None = None,
        error_body: dict[str, Any] | None = None,
    ) -> None:
        super().__init__(message)
        self.status_code = status_code
        self.error_body = error_body or {}

    def __repr__(self) -> str:
        return (
            f"DarshanAPIError({self.args[0]!r}, "
            f"status_code={self.status_code}, "
            f"error_body={self.error_body})"
        )
