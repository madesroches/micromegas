import argparse
import json
import sys

import pytest
import requests

from micromegas.cli import import_keys
from micromegas.web_client import WebClient


class FakeClient:
    """Records every import call and returns a canned response per name,
    raising `RuntimeError` for names mapped to an exception -- mirrors
    `WebClient`'s own `_check_response` contract, per `test_logout.py`'s
    lightweight-mocking style."""

    def __init__(self, responses):
        self.responses = responses
        self.calls = []

    def _handle(self, name, key, audience=None):
        self.calls.append((name, key, audience))
        result = self.responses[name]
        if isinstance(result, Exception):
            raise result
        return result

    def import_ingestion_api_key(self, name, key, audience=None):
        return self._handle(name, key, audience)

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
        "audience": None,
    }
    defaults.update(overrides)
    return argparse.Namespace(**defaults)


class FakeParser:
    """Stand-in for `argparse.ArgumentParser` -- `.error()` raises
    `SystemExit` the same way the real parser's does."""

    def error(self, message):
        raise SystemExit(f"error: {message}")


# ---------------------------------------------------------------------------
# make_client
# ---------------------------------------------------------------------------


def test_make_client_returns_web_client_for_ingestion_table(monkeypatch):
    """Regression test for #1458: `--table ingestion` used to return an
    `IngestionClient` (calling ingestion directly); it now always returns a
    `WebClient` pointed at `analytics-web-srv`, since ingestion exposes no
    key-management HTTP routes of its own."""
    monkeypatch.setattr(import_keys, "build_auth_provider", lambda args, parser: None)
    args = make_args(table="ingestion", url="http://analytics:3000")
    client = import_keys.make_client(args, FakeParser())
    assert isinstance(client, WebClient)
    assert client.base_url == "http://analytics:3000"


def test_make_client_returns_web_client_for_analytics_table(monkeypatch):
    monkeypatch.setattr(import_keys, "build_auth_provider", lambda args, parser: None)
    args = make_args(table="analytics", url="http://analytics:3000")
    client = import_keys.make_client(args, FakeParser())
    assert isinstance(client, WebClient)
    assert client.base_url == "http://analytics:3000"


# ---------------------------------------------------------------------------
# read_keyring
# ---------------------------------------------------------------------------


def test_read_keyring_from_env_var(monkeypatch):
    # No prefixed `MICROMEGAS_INGESTION_API_KEYS` set -- this exercises the
    # fallback-to-unprefixed path, which is exactly what a split deployment's
    # `telemetry-ingestion-srv` (built with `ProviderBuilder::new("")`) needs.
    monkeypatch.delenv("MICROMEGAS_INGESTION_API_KEYS", raising=False)
    monkeypatch.setenv(
        "MICROMEGAS_API_KEYS", json.dumps([{"name": "a", "key": "secret-a"}])
    )
    args = make_args()
    entries = import_keys.read_keyring(args, FakeParser())
    assert entries == [("a", "secret-a", None)]


def test_read_keyring_uses_analytics_default_var(monkeypatch):
    monkeypatch.setenv(
        "MICROMEGAS_ANALYTICS_API_KEYS", json.dumps([{"name": "b", "key": "secret-b"}])
    )
    args = make_args(table="analytics")
    entries = import_keys.read_keyring(args, FakeParser())
    assert entries == [("b", "secret-b", None)]


def test_read_keyring_uses_ingestion_default_var_when_prefixed_is_set(monkeypatch):
    """The prefixed var (as the monolith's ingestion-role `ProviderBuilder`
    would populate it) is used as-is when present, with no need to fall
    back."""
    monkeypatch.setenv(
        "MICROMEGAS_INGESTION_API_KEYS",
        json.dumps([{"name": "a", "key": "secret-a"}]),
    )
    monkeypatch.delenv("MICROMEGAS_API_KEYS", raising=False)
    args = make_args(table="ingestion")
    entries = import_keys.read_keyring(args, FakeParser())
    assert entries == [("a", "secret-a", None)]


def test_read_keyring_falls_back_to_unprefixed_var_for_analytics(monkeypatch):
    """Regression test for the bug this fix addresses: on a split deployment,
    `flight-sql-srv` builds its provider with `ProviderBuilder::new("")`
    (`rust/public/src/servers/flight_sql_server.rs`), so the analytics
    keyring only ever lives in the unprefixed `MICROMEGAS_API_KEYS` --
    `MICROMEGAS_ANALYTICS_API_KEYS` is never populated outside the monolith.
    `--table analytics --source env` with no `--var` must still find it."""
    monkeypatch.delenv("MICROMEGAS_ANALYTICS_API_KEYS", raising=False)
    monkeypatch.setenv(
        "MICROMEGAS_API_KEYS", json.dumps([{"name": "b", "key": "secret-b"}])
    )
    args = make_args(table="analytics")
    entries = import_keys.read_keyring(args, FakeParser())
    assert entries == [("b", "secret-b", None)]


def test_read_keyring_explicit_var_overrides_default(monkeypatch):
    monkeypatch.setenv("CUSTOM_VAR", json.dumps([{"name": "c", "key": "secret-c"}]))
    args = make_args(var="CUSTOM_VAR")
    entries = import_keys.read_keyring(args, FakeParser())
    assert entries == [("c", "secret-c", None)]


def test_read_keyring_explicit_var_does_not_fall_back(monkeypatch):
    """An explicit `--var` is used as-is -- no fallback to
    `MICROMEGAS_API_KEYS` even when it's set, since the fallback only exists
    to cover the *default*, not an override the operator asked for
    specifically."""
    monkeypatch.delenv("CUSTOM_VAR", raising=False)
    monkeypatch.setenv(
        "MICROMEGAS_API_KEYS", json.dumps([{"name": "z", "key": "secret-z"}])
    )
    args = make_args(var="CUSTOM_VAR")
    with pytest.raises(SystemExit) as exc_info:
        import_keys.read_keyring(args, FakeParser())
    assert "CUSTOM_VAR" in str(exc_info.value)


def test_read_keyring_missing_env_var_errors(monkeypatch):
    monkeypatch.delenv("MICROMEGAS_API_KEYS", raising=False)
    args = make_args()
    with pytest.raises(SystemExit):
        import_keys.read_keyring(args, FakeParser())


def test_read_keyring_both_prefixed_and_unprefixed_absent_errors_mentions_both(
    monkeypatch,
):
    """Neither the table's prefixed default nor the unprefixed fallback is
    set -- the error must name both, since either one being set would have
    worked."""
    monkeypatch.delenv("MICROMEGAS_ANALYTICS_API_KEYS", raising=False)
    monkeypatch.delenv("MICROMEGAS_API_KEYS", raising=False)
    args = make_args(table="analytics")
    with pytest.raises(SystemExit) as exc_info:
        import_keys.read_keyring(args, FakeParser())
    message = str(exc_info.value)
    assert "MICROMEGAS_ANALYTICS_API_KEYS" in message
    assert "MICROMEGAS_API_KEYS" in message


def test_read_keyring_from_file(tmp_path):
    path = tmp_path / "keyring.json"
    path.write_text(json.dumps([{"name": "d", "key": "secret-d"}]))
    args = make_args(source="file", path=str(path))
    entries = import_keys.read_keyring(args, FakeParser())
    assert entries == [("d", "secret-d", None)]


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
# read_keyring: per-entry "audience" (#1372, AbAC Stage 4)
# ---------------------------------------------------------------------------


def test_read_keyring_carries_a_per_entry_audience(monkeypatch):
    monkeypatch.setenv(
        "MICROMEGAS_API_KEYS",
        json.dumps([{"name": "a", "key": "secret-a", "audience": "team-alpha"}]),
    )
    args = make_args()
    entries = import_keys.read_keyring(args, FakeParser())
    assert entries == [("a", "secret-a", "team-alpha")]


def test_read_keyring_entry_with_no_audience_field_is_none(monkeypatch):
    monkeypatch.setenv(
        "MICROMEGAS_API_KEYS", json.dumps([{"name": "a", "key": "secret-a"}])
    )
    args = make_args()
    entries = import_keys.read_keyring(args, FakeParser())
    assert entries == [("a", "secret-a", None)]


def test_read_keyring_rejects_a_non_string_audience(monkeypatch):
    monkeypatch.setenv(
        "MICROMEGAS_API_KEYS",
        json.dumps([{"name": "a", "key": "secret-a", "audience": 42}]),
    )
    args = make_args()
    with pytest.raises(SystemExit):
        import_keys.read_keyring(args, FakeParser())


def test_read_keyring_rejects_a_per_entry_audience_with_analytics_table(monkeypatch):
    """A keyring built for ingestion must not be silently reused against
    `--table analytics` with its audience dropped -- rejected up front,
    before any HTTP request, per §7's design."""
    monkeypatch.setenv(
        "MICROMEGAS_API_KEYS",
        json.dumps([{"name": "a", "key": "secret-a", "audience": "team-alpha"}]),
    )
    args = make_args(table="analytics")
    with pytest.raises(SystemExit) as exc_info:
        import_keys.read_keyring(args, FakeParser())
    assert "audience" in str(exc_info.value)


def test_read_keyring_per_entry_audience_guard_fires_even_when_only_would_drop_it(
    monkeypatch,
):
    """The guard runs in `read_keyring`, before `select_entries` -- an
    offending entry aborts the run even when `--only`/`--exclude` would have
    excluded it."""
    monkeypatch.setenv(
        "MICROMEGAS_API_KEYS",
        json.dumps(
            [
                {"name": "a", "key": "secret-a"},
                {"name": "b", "key": "secret-b", "audience": "team-alpha"},
            ]
        ),
    )
    args = make_args(table="analytics", only=["a"])
    with pytest.raises(SystemExit):
        import_keys.read_keyring(args, FakeParser())


# ---------------------------------------------------------------------------
# select_entries
# ---------------------------------------------------------------------------

ENTRIES = [("a", "ka", None), ("b", "kb", None), ("c", "kc", None)]


def test_select_entries_no_filter_returns_all():
    args = make_args()
    assert import_keys.select_entries(ENTRIES, args, FakeParser()) == ENTRIES


def test_select_entries_only():
    args = make_args(only=["a", "c"])
    assert import_keys.select_entries(ENTRIES, args, FakeParser()) == [
        ("a", "ka", None),
        ("c", "kc", None),
    ]


def test_select_entries_exclude():
    args = make_args(exclude=["b"])
    assert import_keys.select_entries(ENTRIES, args, FakeParser()) == [
        ("a", "ka", None),
        ("c", "kc", None),
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
            "http://analytics:3000",
            "--only",
            "a",
            "--exclude",
            "a",
        ],
    )
    with pytest.raises(SystemExit):
        import_keys.main()


# ---------------------------------------------------------------------------
# --audience / --table cross-flag guard
# ---------------------------------------------------------------------------


def test_audience_flag_rejected_with_analytics_table(monkeypatch, capsys):
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "micromegas-import-keys",
            "--table",
            "analytics",
            "--url",
            "http://analytics:3000",
            "--audience",
            "team-alpha",
        ],
    )
    with pytest.raises(SystemExit):
        import_keys.main()
    assert "--audience" in capsys.readouterr().err


def test_audience_flag_empty_string_is_rejected_not_silently_omitted(
    monkeypatch, capsys
):
    """The guard tests `args.audience is not None`, not truthiness -- an
    explicitly passed empty string is a transmitted value, not an absence,
    and must still be rejected against `--table analytics`."""
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "micromegas-import-keys",
            "--table",
            "analytics",
            "--url",
            "http://analytics:3000",
            "--audience",
            "",
        ],
    )
    with pytest.raises(SystemExit):
        import_keys.main()
    assert "--audience" in capsys.readouterr().err


def test_audience_flag_accepted_with_ingestion_table(monkeypatch):
    monkeypatch.setenv(
        "MICROMEGAS_API_KEYS", json.dumps([{"name": "a", "key": "ka"}])
    )
    fake_client = FakeClient(
        {"a": {"key_id": "id-a", "imported": True, "revoked_at": None}}
    )
    monkeypatch.setattr(import_keys, "make_client", lambda args, parser: fake_client)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "micromegas-import-keys",
            "--table",
            "ingestion",
            "--url",
            "http://analytics:3000",
            "--audience",
            "team-alpha",
        ],
    )
    import_keys.main()
    assert fake_client.calls == [("a", "ka", "team-alpha")]


# ---------------------------------------------------------------------------
# run_import: per-key result classification
# ---------------------------------------------------------------------------


def test_run_import_classifies_fresh_import(capsys):
    client = FakeClient({"a": {"key_id": "id-a", "imported": True, "revoked_at": None}})
    ok = import_keys.run_import(client, "ingestion", [("a", "ka", None)])
    assert ok is True
    out = capsys.readouterr().out
    assert "a: imported (key_id=id-a)" in out


def test_run_import_classifies_already_present_usable(capsys):
    client = FakeClient(
        {"a": {"key_id": "id-a", "imported": False, "revoked_at": None}}
    )
    ok = import_keys.run_import(client, "ingestion", [("a", "ka", None)])
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
    ok = import_keys.run_import(client, "ingestion", [("a", "ka", None)])
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
    ok = import_keys.run_import(
        client, "ingestion", [("a", "ka", None), ("b", "kb", None)]
    )
    assert ok is False
    # Both keys were attempted -- the batch didn't abort at the first failure.
    assert client.calls == [("a", "ka", None), ("b", "kb", None)]
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
    ok = import_keys.run_import(
        client, "ingestion", [("a", "ka", None), ("b", "kb", None)]
    )
    assert ok is False
    # Both keys were attempted -- the batch didn't abort at the first failure.
    assert client.calls == [("a", "ka", None), ("b", "kb", None)]
    captured = capsys.readouterr()
    assert "b: imported (key_id=id-b)" in captured.out
    assert "a: error:" in captured.err


def test_run_import_dispatches_to_the_right_client_method():
    calls = []

    class Client:
        def import_ingestion_api_key(self, name, key, audience=None):
            calls.append(("ingestion", name, key, audience))
            return {"key_id": "id", "imported": True, "revoked_at": None}

        def import_analytics_api_key(self, name, key):
            calls.append(("analytics", name, key))
            return {"key_id": "id", "imported": True, "revoked_at": None}

    client = Client()
    import_keys.run_import(client, "ingestion", [("a", "ka", None)])
    import_keys.run_import(client, "analytics", [("a", "ka", None)])
    assert calls == [("ingestion", "a", "ka", None), ("analytics", "a", "ka")]


def test_run_import_per_entry_audience_wins_over_cli_audience(capsys):
    client = FakeClient({"a": {"key_id": "id-a", "imported": True, "revoked_at": None}})
    import_keys.run_import(
        client, "ingestion", [("a", "ka", "entry-audience")], cli_audience="cli-audience"
    )
    assert client.calls == [("a", "ka", "entry-audience")]


def test_run_import_falls_back_to_cli_audience_when_entry_has_none(capsys):
    client = FakeClient({"a": {"key_id": "id-a", "imported": True, "revoked_at": None}})
    import_keys.run_import(
        client, "ingestion", [("a", "ka", None)], cli_audience="cli-audience"
    )
    assert client.calls == [("a", "ka", "cli-audience")]


def test_run_import_prints_the_server_reported_audience(capsys):
    client = FakeClient(
        {"a": {"key_id": "id-a", "imported": True, "revoked_at": None, "audience": "public"}}
    )
    import_keys.run_import(client, "ingestion", [("a", "ka", None)])
    out = capsys.readouterr().out
    assert "a: imported (key_id=id-a, audience=public)" in out


def test_run_import_analytics_response_prints_no_audience_suffix(capsys):
    """`analytics_api_keys` rows carry no `audience` at all -- the printed
    line must not grow a stray `audience=None`."""
    client = FakeClient({"a": {"key_id": "id-a", "imported": True, "revoked_at": None}})
    import_keys.run_import(client, "analytics", [("a", "ka", None)])
    out = capsys.readouterr().out
    assert "a: imported (key_id=id-a)" in out
    assert "audience" not in out


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
            "http://analytics:3000",
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
            "http://analytics:3000",
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
            "http://analytics:3000",
            "--exclude",
            "a",
        ],
    )
    import_keys.main()
    assert called == []
    out = capsys.readouterr().out
    assert "No keys selected" in out
