"""Tests for the DarshanDB exception hierarchy."""

from __future__ import annotations

import pytest

from darshandb.exceptions import DarshanError, DarshanAPIError


# ---------------------------------------------------------------------------
#  Exception hierarchy
# ---------------------------------------------------------------------------


class TestExceptionHierarchy:
    def test_darshan_error_is_exception(self):
        assert issubclass(DarshanError, Exception)

    def test_api_error_is_darshan_error(self):
        assert issubclass(DarshanAPIError, DarshanError)

    def test_api_error_is_exception(self):
        assert issubclass(DarshanAPIError, Exception)

    def test_darshan_error_catchable_as_exception(self):
        with pytest.raises(Exception):
            raise DarshanError("boom")

    def test_api_error_catchable_as_darshan_error(self):
        with pytest.raises(DarshanError):
            raise DarshanAPIError("boom", status_code=500)

    def test_api_error_catchable_as_exception(self):
        with pytest.raises(Exception):
            raise DarshanAPIError("boom", status_code=500)


# ---------------------------------------------------------------------------
#  DarshanError
# ---------------------------------------------------------------------------


class TestDarshanError:
    def test_message(self):
        err = DarshanError("something broke")
        assert str(err) == "something broke"

    def test_args(self):
        err = DarshanError("msg")
        assert err.args == ("msg",)

    def test_empty_message(self):
        err = DarshanError("")
        assert str(err) == ""


# ---------------------------------------------------------------------------
#  DarshanAPIError attributes
# ---------------------------------------------------------------------------


class TestDarshanAPIErrorAttributes:
    def test_status_code(self):
        err = DarshanAPIError("Not found", status_code=404)
        assert err.status_code == 404

    def test_status_code_default_none(self):
        err = DarshanAPIError("error")
        assert err.status_code is None

    def test_error_body(self):
        body = {"code": "VALIDATION", "details": ["field required"]}
        err = DarshanAPIError("Validation error", status_code=422, error_body=body)
        assert err.error_body == body
        assert err.error_body["code"] == "VALIDATION"

    def test_error_body_default_empty_dict(self):
        err = DarshanAPIError("error")
        assert err.error_body == {}

    def test_error_body_none_becomes_empty(self):
        err = DarshanAPIError("error", error_body=None)
        assert err.error_body == {}

    def test_message_in_str(self):
        err = DarshanAPIError("Bad request", status_code=400)
        assert "Bad request" in str(err)

    def test_message_in_args(self):
        err = DarshanAPIError("Forbidden", status_code=403)
        assert err.args == ("Forbidden",)


# ---------------------------------------------------------------------------
#  DarshanAPIError repr
# ---------------------------------------------------------------------------


class TestDarshanAPIErrorRepr:
    def test_repr_format(self):
        err = DarshanAPIError("test", status_code=500, error_body={"k": "v"})
        r = repr(err)
        assert "DarshanAPIError" in r
        assert "500" in r
        assert "'test'" in r
        assert "{'k': 'v'}" in r

    def test_repr_with_none_status(self):
        err = DarshanAPIError("err")
        r = repr(err)
        assert "status_code=None" in r

    def test_repr_with_empty_body(self):
        err = DarshanAPIError("err", status_code=400)
        r = repr(err)
        assert "error_body={}" in r


# ---------------------------------------------------------------------------
#  Common HTTP status codes
# ---------------------------------------------------------------------------


class TestCommonStatusCodes:
    """Verify DarshanAPIError works with various status codes."""

    @pytest.mark.parametrize("code,label", [
        (400, "Bad Request"),
        (401, "Unauthorized"),
        (403, "Forbidden"),
        (404, "Not Found"),
        (409, "Conflict"),
        (422, "Unprocessable Entity"),
        (429, "Too Many Requests"),
        (500, "Internal Server Error"),
        (502, "Bad Gateway"),
        (503, "Service Unavailable"),
    ])
    def test_status_code_roundtrip(self, code: int, label: str):
        err = DarshanAPIError(label, status_code=code)
        assert err.status_code == code
        assert str(err) == label
