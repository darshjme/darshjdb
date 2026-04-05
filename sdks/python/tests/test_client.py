"""Basic tests for the DarshanDB Python SDK."""

from darshandb import DarshanDB
from darshandb.exceptions import DarshanError, DarshanAPIError


def test_client_init():
    """Client initializes with server URL and API key."""
    db = DarshanDB("http://localhost:7700", api_key="test-key")
    assert db._server_url == "http://localhost:7700"
    assert db._api_key == "test-key"


def test_client_has_auth():
    """Client exposes auth sub-client."""
    db = DarshanDB("http://localhost:7700", api_key="test-key")
    assert db.auth is not None


def test_client_has_storage():
    """Client exposes storage sub-client."""
    db = DarshanDB("http://localhost:7700", api_key="test-key")
    assert db.storage is not None


def test_darshan_error_hierarchy():
    """DarshanAPIError is a subclass of DarshanError."""
    assert issubclass(DarshanAPIError, DarshanError)


def test_api_error_attributes():
    """DarshanAPIError carries status code and body."""
    err = DarshanAPIError("test", status_code=403, error_body={"code": "FORBIDDEN"})
    assert err.status_code == 403
    assert err.error_body["code"] == "FORBIDDEN"
