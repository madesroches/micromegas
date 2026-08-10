import argparse
import json
import sys

import pytest
import requests

from micromegas.cli import import_keys


class FakeClient:
    """Records every import call and returns a canned response per name,
    raising `RuntimeError` for names mapped to an exception -- mirrors
    `WebClient`/`IngestionClient`'s own `_check_response` contract, per
    `test_logout.py`'s lightweight-mocking style."""

    def __init__(self, responses):
        self.responses = responses
        self.calls = []

    def _handle(self, name, key):
        self.calls.append((name, key))
        result = self.responses[name]
        if isinstance(result, Exception):
            raise result
        return result

    def import_ingestion_api_key(self, name, key):
        return self._handle(name, key)

    def import_analytics_api_key(self, name, key):
        return self._handle(name, key)


def make_args(**overrides):
    defaults = {
        "table": "ingestion",
        "source": "env",
        "var": None,
        "path": None,
        "only": None,
        "exclude": None,
        "profile": None,
    }
    defaults.update(overrides)
    return argparse.Namespace(**defaults)


class FakeParser:
    """Stand-in for `argparse.ArgumentParser` -- `.error()` raises
    `SystemExit` the same way the real parser's does."""

    def error(self, message):
        raise SystemExit(f"error: {message}")


# ---------------------------------------------------------------------------
# read_keyring
# ---------------------------------------------------------------------------


def test_read_keyring_from_env_var(monkeypatch):
    monkeypatch.setenv(
        "MICROMEGAS_API_KEYS", json.dumps([{"name": "a", "key": "secret-a"}])
    )
    args = make_args()
    entries = import_keys.read_keyring(args, FakeParser())
    assert entries == [("a", "secret-a")]


def test_read_keyring_uses_analytics_default_var(monkeypatch):
    monkeypatch.setenv(
        "MICROMEGAS_ANALYTICS_API_KEYS", json.dumps([{"name": "b", "key": "secret-b"}])
    )
    args = make_args(table="analytics")
    entries = import_keys.read_keyring(args, FakeParser())
    assert entries == [("b", "secret-b")]


def test_read_keyring_explicit_var_overrides_default(monkeypatch):
    monkeypatch.setenv("CUSTOM_VAR", json.dumps([{"name": "c", "key": "secret-c"}]))
    args = make_args(var="CUSTOM_VAR")
    entries = import_keys.read_keyring(args, FakeParser())
    assert entries == [("c", "secret-c")]


def test_read_keyring_missing_env_var_errors(monkeypatch):
    monkeypatch.delenv("MICROMEGAS_API_KEYS", raising=False)
    args = make_args()
    with pytest.raises(SystemExit):
        import_keys.read_keyring(args, FakeParser())


def test_read_keyring_from_file(tmp_path):
    path = tmp_path / "keyring.json"
    path.write_text(json.dumps([{"name": "d", "key": "secret-d"}]))
    args = make_args(source="file", path=str(path))
    entries = import_keys.read_keyring(args, FakeParser())
    assert entries == [("d", "secret-d")]


def test_read_keyring_invalid_json_errors(monkeypatch):
    monkeypatch.setenv("MICROMEGAS_API_KEYS", "not json")
    args = make_args()
    with pytest.raises(SystemExit):
        import_keys.read_keyring(args, FakeParser())


def test_read_keyring_rejects_non_array(monkeypatch):
    monkeypatch.setenv("MICROMEGAS_API_KEYS", json.dumps({"name": "a", "key": "b"}))
    args = make_args()
    with pytest.raises(SystemExit):
        import_keys.read_keyring(args, FakeParser())


def test_read_keyring_rejects_entry_missing_key(monkeypatch):
    monkeypatch.setenv("MICROMEGAS_API_KEYS", json.dumps([{"name": "a"}]))
    args = make_args()
    with pytest.raises(SystemExit):
        import_keys.read_keyring(args, FakeParser())


# ---------------------------------------------------------------------------
# select_entries
# ---------------------------------------------------------------------------

ENTRIES = [("a", "ka"), ("b", "kb"), ("c", "kc")]


def test_select_entries_no_filter_returns_all():
    args = make_args()
    assert import_keys.select_entries(ENTRIES, args, FakeParser()) == ENTRIES


def test_select_entries_only():
    args = make_args(only=["a", "c"])
    assert import_keys.select_entries(ENTRIES, args, FakeParser()) == [
        ("a", "ka"),
        ("c", "kc"),
    ]


def test_select_entries_exclude():
    args = make_args(exclude=["b"])
    assert import_keys.select_entries(ENTRIES, args, FakeParser()) == [
        ("a", "ka"),
        ("c", "kc"),
    ]


def test_select_entries_only_unknown_name_errors():
    args = make_args(only=["nope"])
    with pytest.raises(SystemExit):
        import_keys.select_entries(ENTRIES, args, FakeParser())


def test_select_entries_exclude_unknown_name_errors():
    args = make_args(exclude=["nope"])
    with pytest.raises(SystemExit):
        import_keys.select_entries(ENTRIES, args, FakeParser())


def test_only_and_exclude_are_mutually_exclusive(monkeypatch):
    monkeypatch.setenv("MICROMEGAS_API_KEYS", json.dumps([{"name": "a", "key": "ka"}]))
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "micromegas-import-keys",
            "--table",
            "ingestion",
            "--url",
            "http://ingestion:8081",
            "--only",
            "a",
            "--exclude",
            "a",
        ],
    )
    with pytest.raises(SystemExit):
        import_keys.main()


# ---------------------------------------------------------------------------
# run_import: per-key result classification
# ---------------------------------------------------------------------------


def test_run_import_classifies_fresh_import(capsys):
    client = FakeClient({"a": {"key_id": "id-a", "imported": True, "revoked_at": None}})
    ok = import_keys.run_import(client, "ingestion", [("a", "ka")])
    assert ok is True
    out = capsys.readouterr().out
    assert "a: imported (key_id=id-a)" in out


def test_run_import_classifies_already_present_usable(capsys):
    client = FakeClient(
        {"a": {"key_id": "id-a", "imported": False, "revoked_at": None}}
    )
    ok = import_keys.run_import(client, "ingestion", [("a", "ka")])
    assert ok is True
    out = capsys.readouterr().out
    assert "a: already present (key_id=id-a)" in out


def test_run_import_classifies_already_present_revoked_as_failure(capsys):
    client = FakeClient(
        {
            "a": {
                "key_id": "id-a",
                "imported": False,
                "revoked_at": "2026-01-01T00:00:00Z",
            }
        }
    )
    ok = import_keys.run_import(client, "ingestion", [("a", "ka")])
    assert ok is False
    out = capsys.readouterr().out
    assert "a: already present (revoked) (key_id=id-a)" in out


def test_run_import_continues_past_individual_failures(capsys):
    client = FakeClient(
        {
            "a": RuntimeError("HTTP 400: name must not be empty"),
            "b": {"key_id": "id-b", "imported": True, "revoked_at": None},
        }
    )
    ok = import_keys.run_import(client, "ingestion", [("a", "ka"), ("b", "kb")])
    assert ok is False
    # Both keys were attempted -- the batch didn't abort at the first failure.
    assert client.calls == [("a", "ka"), ("b", "kb")]
    captured = capsys.readouterr()
    assert "b: imported (key_id=id-b)" in captured.out
    assert "a: error:" in captured.err


def test_run_import_continues_past_network_level_failures(capsys):
    """A `requests.exceptions.RequestException` (connection reset, DNS
    failure, timeout) escapes straight out of `session.post` -- unlike an
    HTTP 4xx/5xx, it's never wrapped into a `RuntimeError` by
    `_check_response`. `run_import` must catch it too, or one bad-network
    key aborts every key after it."""
    client = FakeClient(
        {
            "a": requests.exceptions.ConnectionError("connection reset by peer"),
            "b": {"key_id": "id-b", "imported": True, "revoked_at": None},
        }
    )
    ok = import_keys.run_import(client, "ingestion", [("a", "ka"), ("b", "kb")])
    assert ok is False
    # Both keys were attempted -- the batch didn't abort at the first failure.
    assert client.calls == [("a", "ka"), ("b", "kb")]
    captured = capsys.readouterr()
    assert "b: imported (key_id=id-b)" in captured.out
    assert "a: error:" in captured.err


def test_run_import_dispatches_to_the_right_client_method():
    calls = []

    class Client:
        def import_ingestion_api_key(self, name, key):
            calls.append(("ingestion", name, key))
            return {"key_id": "id", "imported": True, "revoked_at": None}

        def import_analytics_api_key(self, name, key):
            calls.append(("analytics", name, key))
            return {"key_id": "id", "imported": True, "revoked_at": None}

    client = Client()
    import_keys.run_import(client, "ingestion", [("a", "ka")])
    import_keys.run_import(client, "analytics", [("a", "ka")])
    assert calls == [("ingestion", "a", "ka"), ("analytics", "a", "ka")]


# ---------------------------------------------------------------------------
# main(): end to end with a fake client, no network/auth
# ---------------------------------------------------------------------------


def test_main_exits_nonzero_when_a_key_fails(monkeypatch):
    monkeypatch.setenv(
        "MICROMEGAS_API_KEYS",
        json.dumps([{"name": "a", "key": "ka"}]),
    )
    monkeypatch.setattr(
        import_keys,
        "make_client",
        lambda args, parser: FakeClient({"a": RuntimeError("boom")}),
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "micromegas-import-keys",
            "--table",
            "ingestion",
            "--url",
            "http://ingestion:8081",
        ],
    )
    with pytest.raises(SystemExit) as exc_info:
        import_keys.main()
    assert exc_info.value.code == 1


def test_main_succeeds_when_every_key_imports(monkeypatch, capsys):
    monkeypatch.setenv(
        "MICROMEGAS_API_KEYS",
        json.dumps([{"name": "a", "key": "ka"}]),
    )
    monkeypatch.setattr(
        import_keys,
        "make_client",
        lambda args, parser: FakeClient(
            {"a": {"key_id": "id-a", "imported": True, "revoked_at": None}}
        ),
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "micromegas-import-keys",
            "--table",
            "ingestion",
            "--url",
            "http://ingestion:8081",
        ],
    )
    import_keys.main()
    out = capsys.readouterr().out
    assert "a: imported (key_id=id-a)" in out


def test_main_with_no_selected_keys_prints_message_and_does_not_call_client(
    monkeypatch, capsys
):
    monkeypatch.setenv("MICROMEGAS_API_KEYS", json.dumps([{"name": "a", "key": "ka"}]))
    called = []
    monkeypatch.setattr(
        import_keys,
        "make_client",
        lambda args, parser: called.append(True),
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "micromegas-import-keys",
            "--table",
            "ingestion",
            "--url",
            "http://ingestion:8081",
            "--exclude",
            "a",
        ],
    )
    import_keys.main()
    assert called == []
    out = capsys.readouterr().out
    assert "No keys selected" in out
