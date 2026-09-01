"""Unit tests for StaticTokenAuthProvider."""

import pytest

from micromegas.auth import StaticTokenAuthProvider
from micromegas.flightsql.client import DynamicAuthMiddleware


def test_get_token_round_trips():
    auth = StaticTokenAuthProvider("mmk_abc123")
    assert auth.get_token() == "mmk_abc123"


def test_get_token_strips_surrounding_whitespace():
    auth = StaticTokenAuthProvider("  mmk_abc123  \n")
    assert auth.get_token() == "mmk_abc123"


def test_empty_token_raises_value_error():
    with pytest.raises(ValueError):
        StaticTokenAuthProvider("")


def test_whitespace_only_token_raises_value_error():
    with pytest.raises(ValueError):
        StaticTokenAuthProvider("   \n\t  ")


def test_non_string_token_raises_value_error():
    with pytest.raises(ValueError):
        StaticTokenAuthProvider(12345)


def test_token_with_internal_whitespace_raises_value_error():
    with pytest.raises(ValueError):
        StaticTokenAuthProvider("mmk_abc 123")


def test_token_with_internal_newline_raises_value_error():
    with pytest.raises(ValueError):
        StaticTokenAuthProvider("mmk_abc\n123")


def test_token_with_non_ascii_character_raises_value_error():
    with pytest.raises(ValueError):
        StaticTokenAuthProvider("mmk_abcé123")


def test_from_file_reads_and_strips_trailing_newline(tmp_path):
    key_file = tmp_path / "local.key"
    key_file.write_text("mmk_abc123\n", encoding="utf-8")

    auth = StaticTokenAuthProvider.from_file(key_file)
    assert auth.get_token() == "mmk_abc123"


def test_from_file_expands_user(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    key_file = tmp_path / "local.key"
    key_file.write_text("mmk_abc123\n", encoding="utf-8")

    auth = StaticTokenAuthProvider.from_file("~/local.key")
    assert auth.get_token() == "mmk_abc123"


def test_from_file_empty_file_raises_value_error_naming_path(tmp_path):
    key_file = tmp_path / "empty.key"
    key_file.write_text("", encoding="utf-8")

    with pytest.raises(ValueError) as e:
        StaticTokenAuthProvider.from_file(key_file)
    assert str(key_file) in str(e.value)


def test_from_file_missing_path_raises_os_error(tmp_path):
    missing = tmp_path / "nonexistent.key"
    with pytest.raises(OSError):
        StaticTokenAuthProvider.from_file(missing)


def test_repr_does_not_contain_token():
    auth = StaticTokenAuthProvider("mmk_super-secret-token")
    assert "mmk_super-secret-token" not in repr(auth)


def test_dynamic_auth_middleware_sends_bearer_header():
    auth = StaticTokenAuthProvider("mmk_abc123")
    middleware = DynamicAuthMiddleware(auth)
    assert middleware.sending_headers() == {"authorization": b"Bearer mmk_abc123"}
