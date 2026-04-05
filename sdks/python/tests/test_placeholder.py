"""Placeholder tests to ensure pytest collection succeeds."""


def test_sdk_importable():
    """Verify the darshandb SDK can be imported."""
    import darshandb  # noqa: F401

    assert True


def test_client_class_exists():
    """Verify DarshanDB client class is accessible."""
    from darshandb import DarshanDB

    assert DarshanDB is not None
