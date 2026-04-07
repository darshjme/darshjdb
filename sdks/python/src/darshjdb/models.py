"""Data models for the DarshJDB Python SDK."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any


class ConnectionState(Enum):
    """Connection lifecycle states."""

    DISCONNECTED = "disconnected"
    CONNECTING = "connecting"
    CONNECTED = "connected"
    CLOSING = "closing"


class LiveAction(Enum):
    """Actions emitted by live query subscriptions."""

    CREATE = "CREATE"
    UPDATE = "UPDATE"
    DELETE = "DELETE"


@dataclass(frozen=True, slots=True)
class QueryResult:
    """
    Result from a query or mutation.

    Attributes:
        data: List of result records.
        meta: Server metadata (count, duration, cached, etc.).
    """

    data: list[dict[str, Any]] = field(default_factory=list)
    meta: dict[str, Any] = field(default_factory=dict)

    @property
    def count(self) -> int:
        """Number of records in the result."""
        return self.meta.get("count", len(self.data))

    @property
    def duration_ms(self) -> float:
        """Server-side execution time in milliseconds."""
        return self.meta.get("duration_ms", 0.0)

    def first(self) -> dict[str, Any] | None:
        """Return the first record, or None."""
        return self.data[0] if self.data else None

    def __iter__(self):
        return iter(self.data)

    def __len__(self) -> int:
        return len(self.data)

    def __bool__(self) -> bool:
        return len(self.data) > 0


@dataclass(frozen=True, slots=True)
class LiveNotification:
    """
    A single change event from a live query subscription.

    Attributes:
        action: The type of change (CREATE, UPDATE, DELETE).
        result: The affected record data.
    """

    action: LiveAction
    result: dict[str, Any]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> LiveNotification:
        """Parse a live notification from server JSON."""
        action_str = data.get("action", data.get("event", "UPDATE")).upper()
        try:
            action = LiveAction(action_str)
        except ValueError:
            action = LiveAction.UPDATE

        result = data.get("result", data.get("data", data))
        return cls(action=action, result=result)


@dataclass(frozen=True, slots=True)
class AuthResponse:
    """
    Response from signin/signup operations.

    Attributes:
        token: The JWT access token.
        user: User profile data (if returned by server).
        refresh_token: Refresh token for token renewal.
    """

    token: str
    user: dict[str, Any] = field(default_factory=dict)
    refresh_token: str = ""
