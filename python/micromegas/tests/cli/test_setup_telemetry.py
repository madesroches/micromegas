import argparse
import sys

import pytest

from micromegas.cli import import_keys, setup_telemetry
from micromegas.web_client import WebClient


class FakeClient:
    """Records every call and returns canned responses, mirroring
    `test_import_keys.py`/`test_grants.py`'s `FakeClient` lightweight-mocking style."""

    def __init__(self, my_audiences=None, mint_result=None, mint_error=None):
        self.calls = []
        self.my_audiences_result = my_audiences or {
            "is_admin": False,
            "audiences": [],
            "mint_prefix": None,
            "email": None,
            "held_pairs": [],
        }
        self.mint_result = mint_result or {
            "key_id": "key-1",
            "name": "laptop",
            "audience": "team-alpha",
            "key": "mmk_secret",
            "claimed": False,
        }
        # When set, `mint_ingestion_api_key` raises this instead of returning --
        # exercises `run()`'s 403-hint enrichment / non-403 passthrough.
        self.mint_error = mint_error

    def my_audiences(self):
        self.calls.append(("my_audiences",))
        return self.my_audiences_result

    def mint_ingestion_api_key(self, name, audience=None):
        self.calls.append(("mint", name, audience))
        if self.mint_error is not None:
            raise self.mint_error
        result = dict(self.mint_result)
        if audience is not None:
            result["audience"] = audience
        return result

    def create_audience_grant(self, audience, axis, selector):
        self.calls.append(("create", audience, axis, selector))
        return {
            "audience": audience,
            "axis": axis,
            "selector": selector,
            "created_at": "2026-08-19T00:00:00Z",
            "created_by": selector,
        }


class FakeParser:
    """Stand-in for `argparse.ArgumentParser` -- `.error()` raises `SystemExit`
    the same way the real parser's does."""

    def error(self, message):
        raise SystemExit(f"error: {message}")


def make_args(**overrides):
    defaults = {
        "url": "http://analytics:3000",
        "profile": None,
        "name": "laptop",
        "audience": None,
        "user_audience": None,
        "otlp_endpoint": None,
        "env_file": None,
    }
    defaults.update(overrides)
    return argparse.Namespace(**defaults)


# ---------------------------------------------------------------------------
# build_parser / make_client
# ---------------------------------------------------------------------------


def test_build_parser_requires_url_and_name():
    parser = setup_telemetry.build_parser()
    with pytest.raises(SystemExit):
        parser.parse_args([])


def test_build_parser_accepts_the_minimal_required_args():
    parser = setup_telemetry.build_parser()
    args = parser.parse_args(["--url", "http://analytics:3000", "--name", "laptop"])
    assert args.url == "http://analytics:3000"
    assert args.name == "laptop"
    assert args.audience is None
    assert args.user_audience is None
    assert args.otlp_endpoint is None
    assert args.env_file is None


def test_make_client_returns_web_client(monkeypatch):
    # `make_client` is re-exported from `import_keys` and resolves
    # `build_auth_provider` in *that* module's namespace (see the comment on
    # the re-export in setup_telemetry.py), so it must be patched there --
    # patching `setup_telemetry.build_auth_provider` would only rebind an
    # unused name and let this test fall through to the real implementation.
    monkeypatch.setattr(import_keys, "build_auth_provider", lambda args, parser: None)
    args = make_args()
    client = setup_telemetry.make_client(args, FakeParser())
    assert isinstance(client, WebClient)
    assert client.base_url == "http://analytics:3000"


# ---------------------------------------------------------------------------
# resolve_otlp_endpoint
# ---------------------------------------------------------------------------


def test_resolve_otlp_endpoint_uses_the_explicit_flag():
    args = make_args(otlp_endpoint="http://ingest:9000/ingestion/otlp")
    endpoint = setup_telemetry.resolve_otlp_endpoint(args, FakeParser())
    assert endpoint == "http://ingest:9000/ingestion/otlp"


def test_resolve_otlp_endpoint_derives_from_micromegas_telemetry_url(monkeypatch):
    monkeypatch.setenv("MICROMEGAS_TELEMETRY_URL", "http://localhost:9000")
    args = make_args()
    endpoint = setup_telemetry.resolve_otlp_endpoint(args, FakeParser())
    assert endpoint == "http://localhost:9000/ingestion/otlp"


def test_resolve_otlp_endpoint_strips_a_trailing_slash(monkeypatch):
    monkeypatch.setenv("MICROMEGAS_TELEMETRY_URL", "http://localhost:9000/")
    args = make_args()
    endpoint = setup_telemetry.resolve_otlp_endpoint(args, FakeParser())
    assert endpoint == "http://localhost:9000/ingestion/otlp"


def test_resolve_otlp_endpoint_errors_when_neither_is_available():
    args = make_args()
    with pytest.raises(SystemExit):
        setup_telemetry.resolve_otlp_endpoint(args, FakeParser())


# ---------------------------------------------------------------------------
# resolve_audience -- --audience
# ---------------------------------------------------------------------------


def test_omitted_audience_non_admin_exactly_one_match_is_used_silently(capsys):
    my_audiences = {
        "is_admin": False,
        "audiences": ["public", "team-alpha"],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
        "held_pairs": ["team-alpha:mint"],
    }
    args = make_args(audience=None)
    audience = setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)
    assert audience == "team-alpha"


def test_omitted_audience_non_admin_multiple_matches_is_an_error():
    my_audiences = {
        "is_admin": False,
        "audiences": ["team-alpha", "team-beta"],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
        "held_pairs": ["team-alpha:mint", "team-beta:mint"],
    }
    args = make_args(audience=None)
    with pytest.raises(SystemExit):
        setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)


def test_omitted_audience_non_admin_no_matches_is_an_error():
    my_audiences = {
        "is_admin": False,
        "audiences": [],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
        "held_pairs": [],
    }
    args = make_args(audience=None)
    with pytest.raises(SystemExit):
        setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)


def test_omitted_audience_admin_is_always_an_error():
    my_audiences = {
        "is_admin": True,
        "audiences": [],
        "mint_prefix": "admin-",
        "email": "admin@example.com",
        "held_pairs": [],
    }
    args = make_args(audience=None)
    with pytest.raises(SystemExit) as exc_info:
        setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)
    message = str(exc_info.value)
    assert "--audience" in message
    assert "--user-audience" in message


def test_omitted_audience_non_admin_only_seeded_row_visible_is_a_zero_match_error():
    """A caller whose only mint authority is a seeded `"*"` row (e.g. the default
    `public` mint grant) holds no personal grant, so the `held_pairs` filter empties
    `personal` even though `public` is in `audiences` -- omitting both flags must not
    silently mint into that shared audience."""
    my_audiences = {
        "is_admin": False,
        "audiences": ["public"],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
        "held_pairs": [],
    }
    args = make_args(audience=None)
    with pytest.raises(SystemExit) as exc_info:
        setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)
    message = str(exc_info.value)
    assert "--audience" in message
    assert "public" in message
    assert "--user-audience" in message


def test_omitted_audience_non_admin_no_matches_no_email_asks_admin():
    """When the caller has no email at all, `_fresh_audience_suggestion` can't offer
    `--user-audience` (no prefix to derive) or `--audience <new-name>` (no way to claim
    anything without an email) -- the zero-match error must fall back to asking an
    admin, and must not do so via the (email-is-not-None) `visible` branch above it."""
    my_audiences = {
        "is_admin": False,
        "audiences": [],
        "mint_prefix": None,
        "email": None,
        "held_pairs": [],
    }
    args = make_args(audience=None)
    with pytest.raises(SystemExit) as exc_info:
        setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)
    message = str(exc_info.value)
    assert "ask an admin for a grant" in message
    assert "--audience" not in message
    assert "--user-audience" not in message


def test_omitted_audience_non_admin_only_seeded_row_visible_no_email_asks_admin():
    """Same no-email caller as above, but with a seeded `"*"` row (e.g. `public`)
    visible-but-not-held -- exercises the `visible` sub-branch of the email-is-None
    zero-match error, not just the empty-audiences one."""
    my_audiences = {
        "is_admin": False,
        "audiences": ["public"],
        "mint_prefix": None,
        "email": None,
        "held_pairs": [],
    }
    args = make_args(audience=None)
    with pytest.raises(SystemExit) as exc_info:
        setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)
    message = str(exc_info.value)
    assert "public" in message
    assert "ask an admin for a grant" in message
    assert "--user-audience" not in message


def test_audience_already_granted_is_used_verbatim(capsys):
    my_audiences = {
        "is_admin": False,
        "audiences": ["team-alpha"],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
        "held_pairs": ["team-alpha:mint"],
    }
    args = make_args(audience="team-alpha")
    audience = setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)
    assert audience == "team-alpha"


def test_audience_public_already_granted_via_seeded_row_is_used_verbatim(capsys):
    """The seeded `('public', 'mint', '*')` row puts `public` in every caller's
    `audiences`, so `--audience public` lands in the existing verbatim branch with no
    code change and nothing printed to stderr."""
    my_audiences = {
        "is_admin": False,
        "audiences": ["public"],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
        "held_pairs": [],
    }
    args = make_args(audience="public")
    audience = setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)
    assert audience == "public"
    assert capsys.readouterr().err == ""


def test_audience_outside_mintable_set_is_used_verbatim_no_longer_an_error():
    """The point of the change: `--audience` for a name outside the caller's
    mintable set is no longer a client-side refusal -- it is passed straight
    through, and the server's mint route lazily claims or denies it. The
    denial-plus-hint case now lives in the `run()` 403 test below."""
    my_audiences = {
        "is_admin": False,
        "audiences": ["team-alpha"],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
        "held_pairs": ["team-alpha:mint"],
    }
    args = make_args(audience="prod", url="http://analytics:3000")
    audience = setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)
    assert audience == "prod"


def test_admin_audience_is_used_verbatim_even_when_not_in_my_audiences():
    """The admin branch does not decide (or report) whether the name is
    brand-new -- the server's mint route runs that check itself and
    claims the audience server-side when appropriate."""
    my_audiences = {
        "is_admin": True,
        "audiences": [],
        "mint_prefix": "admin-",
        "email": "admin@example.com",
        "held_pairs": [],
    }
    args = make_args(audience="ci")
    audience = setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)
    assert audience == "ci"


def test_audience_empty_string_is_an_error():
    my_audiences = {
        "is_admin": False,
        "audiences": [],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
        "held_pairs": [],
    }
    args = make_args(audience="")
    with pytest.raises(SystemExit):
        setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)


def test_audience_and_user_audience_together_is_an_error():
    my_audiences = {
        "is_admin": False,
        "audiences": [],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
        "held_pairs": [],
    }
    args = make_args(audience="team-alpha", user_audience="laptop")
    with pytest.raises(SystemExit):
        setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)


# ---------------------------------------------------------------------------
# resolve_audience -- --user-audience
# ---------------------------------------------------------------------------


def test_user_audience_composes_the_callers_mint_prefix():
    my_audiences = {
        "is_admin": False,
        "audiences": [],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
        "held_pairs": [],
    }
    args = make_args(user_audience="laptop")
    audience = setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)
    assert audience == "alice-laptop"


def test_user_audience_composes_the_same_way_for_an_admin_caller():
    """Pins the issue's central claim: `--user-audience` is role-independent --
    an admin's `mint_prefix` is derived from the same email-sanitizing function
    and composed exactly the same way as a non-admin's."""
    my_audiences = {
        "is_admin": True,
        "audiences": [],
        "mint_prefix": "admin-",
        "email": "admin@example.com",
        "held_pairs": [],
    }
    args = make_args(user_audience="laptop")
    audience = setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)
    assert audience == "admin-laptop"


def test_user_audience_with_email_sanitizing_to_empty_names_audience_flag():
    my_audiences = {
        "is_admin": False,
        "audiences": [],
        "mint_prefix": None,
        "email": "+++@example.com",
        "held_pairs": [],
    }
    args = make_args(user_audience="laptop")
    with pytest.raises(SystemExit) as exc_info:
        setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)
    assert "--audience" in str(exc_info.value)


def test_user_audience_with_no_email_asks_for_an_admin_grant_not_audience_flag():
    my_audiences = {
        "is_admin": False,
        "audiences": [],
        "mint_prefix": None,
        "email": None,
        "held_pairs": [],
    }
    args = make_args(user_audience="laptop")
    with pytest.raises(SystemExit) as exc_info:
        setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)
    message = str(exc_info.value)
    assert "admin" in message
    assert "--audience" not in message


def test_user_audience_empty_string_is_an_error():
    my_audiences = {
        "is_admin": False,
        "audiences": [],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
        "held_pairs": [],
    }
    args = make_args(user_audience="")
    with pytest.raises(SystemExit):
        setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)


def test_user_audience_never_normalizes_the_suffix():
    """Guards against a future `.lower()`/`.strip()` creeping back into the
    composition -- only the prefix is applied, the suffix passed straight
    through verbatim."""
    my_audiences = {
        "is_admin": False,
        "audiences": [],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
        "held_pairs": [],
    }
    args = make_args(user_audience="Ci_Runner")
    audience = setup_telemetry.resolve_audience(args, FakeParser(), my_audiences)
    assert audience == "alice-Ci_Runner"


# ---------------------------------------------------------------------------
# run() -- end-to-end wiring
# ---------------------------------------------------------------------------


def test_run_never_calls_create_audience_grant(monkeypatch):
    """The server claims a brand-new audience itself as part of the mint
    request, for admin and non-admin callers alike -- `run()`
    never writes a grant row client-side."""
    client = FakeClient(
        my_audiences={
            "is_admin": True,
            "audiences": [],
            "mint_prefix": "admin-",
            "email": "admin@example.com",
        },
        mint_result={
            "key_id": "key-1",
            "name": "laptop",
            "audience": "ci",
            "key": "mmk_secret",
            "claimed": True,
        },
    )
    monkeypatch.setattr(setup_telemetry, "make_client", lambda args, parser: client)
    args = make_args(audience="ci", otlp_endpoint="http://ingest:9000/ingestion/otlp")
    setup_telemetry.run(args, FakeParser())

    assert ("mint", "laptop", "ci") in client.calls
    assert not any(call[0] == "create" for call in client.calls)


def test_run_reports_claimed_audience_on_stderr_when_claimed_true(monkeypatch, capsys):
    client = FakeClient(
        my_audiences={
            "is_admin": True,
            "audiences": [],
            "mint_prefix": "admin-",
            "email": "admin@example.com",
        },
        mint_result={
            "key_id": "key-1",
            "name": "laptop",
            "audience": "ci",
            "key": "mmk_secret",
            "claimed": True,
        },
    )
    monkeypatch.setattr(setup_telemetry, "make_client", lambda args, parser: client)
    args = make_args(audience="ci", otlp_endpoint="http://ingest:9000/ingestion/otlp")
    setup_telemetry.run(args, FakeParser())

    err = capsys.readouterr().err
    assert "claimed audience ci" in err


def test_run_omits_claimed_line_when_claimed_false(monkeypatch, capsys):
    client = FakeClient(
        my_audiences={
            "is_admin": False,
            "audiences": ["team-alpha"],
            "mint_prefix": "alice-",
            "email": "alice@example.com",
        },
        mint_result={
            "key_id": "key-1",
            "name": "laptop",
            "audience": "team-alpha",
            "key": "mmk_secret",
            "claimed": False,
        },
    )
    monkeypatch.setattr(setup_telemetry, "make_client", lambda args, parser: client)
    args = make_args(
        audience="team-alpha", otlp_endpoint="http://ingest:9000/ingestion/otlp"
    )
    setup_telemetry.run(args, FakeParser())

    err = capsys.readouterr().err
    assert "claimed audience" not in err


def test_run_403_appends_mint_denied_hint(monkeypatch):
    my_audiences = {
        "is_admin": False,
        "audiences": ["team-alpha"],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
        "held_pairs": ["team-alpha:mint"],
    }
    client = FakeClient(
        my_audiences=my_audiences,
        mint_error=RuntimeError(
            "HTTP 403: audience 'prod' already exists and the caller has no grant "
            "for it"
        ),
    )
    monkeypatch.setattr(setup_telemetry, "make_client", lambda args, parser: client)
    args = make_args(audience="prod", otlp_endpoint="http://ingest:9000/ingestion/otlp")
    with pytest.raises(RuntimeError) as exc_info:
        setup_telemetry.run(args, FakeParser())
    message = str(exc_info.value)
    assert "HTTP 403: audience 'prod' already exists" in message
    assert "team-alpha" in message
    assert "--user-audience" in message
    assert (
        "micromegas-grants --url http://analytics:3000 create prod mint "
        "'user:alice@example.com'" in message
    )
    assert (
        "micromegas-grants --url http://analytics:3000 create prod mint '*'" in message
    )


def test_run_403_hint_degrades_to_audience_flag_when_prefix_unavailable(monkeypatch):
    """A caller with an email that sanitizes to an empty prefix (`mint_prefix`
    is `None` but `email` is not) can't be offered `--user-audience` -- the hint
    must degrade to a bare `--audience <new-name>` suggestion instead."""
    my_audiences = {
        "is_admin": False,
        "audiences": [],
        "mint_prefix": None,
        "email": "+++@example.com",
        "held_pairs": [],
    }
    client = FakeClient(
        my_audiences=my_audiences,
        mint_error=RuntimeError(
            "HTTP 403: audience 'prod' already exists and the caller has no grant "
            "for it"
        ),
    )
    monkeypatch.setattr(setup_telemetry, "make_client", lambda args, parser: client)
    args = make_args(audience="prod", otlp_endpoint="http://ingest:9000/ingestion/otlp")
    with pytest.raises(RuntimeError) as exc_info:
        setup_telemetry.run(args, FakeParser())
    message = str(exc_info.value)
    assert "--audience <new-name>" in message
    assert "--user-audience" not in message


def test_run_non_403_runtime_error_propagates_unchanged(monkeypatch):
    client = FakeClient(
        my_audiences={
            "is_admin": False,
            "audiences": [],
            "mint_prefix": "alice-",
            "email": "alice@example.com",
            "held_pairs": [],
        },
        mint_error=RuntimeError("HTTP 500: internal error"),
    )
    monkeypatch.setattr(setup_telemetry, "make_client", lambda args, parser: client)
    args = make_args(audience="prod", otlp_endpoint="http://ingest:9000/ingestion/otlp")
    with pytest.raises(RuntimeError) as exc_info:
        setup_telemetry.run(args, FakeParser())
    assert str(exc_info.value) == "HTTP 500: internal error"


def test_run_with_user_audience_sends_the_composed_name_over_the_wire(monkeypatch):
    client = FakeClient(
        my_audiences={
            "is_admin": False,
            "audiences": [],
            "mint_prefix": "alice-",
            "email": "alice@example.com",
            "held_pairs": [],
        },
        mint_result={
            "key_id": "key-1",
            "name": "laptop",
            "audience": "alice-claude",
            "key": "mmk_secret",
            "claimed": True,
        },
    )
    monkeypatch.setattr(setup_telemetry, "make_client", lambda args, parser: client)
    args = make_args(
        user_audience="claude", otlp_endpoint="http://ingest:9000/ingestion/otlp"
    )
    setup_telemetry.run(args, FakeParser())

    assert ("mint", "laptop", "alice-claude") in client.calls


def test_run_writes_env_file_with_secure_permissions_and_prints_its_path(
    monkeypatch, tmp_path, capsys
):
    client = FakeClient()
    monkeypatch.setattr(setup_telemetry, "make_client", lambda args, parser: client)
    env_file = tmp_path / "sub" / "telemetry.env"
    args = make_args(
        audience="team-alpha",
        otlp_endpoint="http://ingest:9000/ingestion/otlp",
        env_file=str(env_file),
    )
    my_audiences = {
        "is_admin": False,
        "audiences": ["team-alpha"],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
    }
    client.my_audiences_result = my_audiences

    setup_telemetry.run(args, FakeParser())

    out = capsys.readouterr().out
    assert out.strip() == str(env_file)
    content = env_file.read_text()
    assert "OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf" in content
    assert "OTEL_EXPORTER_OTLP_ENDPOINT=http://ingest:9000/ingestion/otlp" in content
    assert "Authorization=Bearer mmk_secret" in content

    mode = env_file.stat().st_mode & 0o777
    assert mode == 0o600


def test_run_env_file_write_failure_prints_key_to_stdout_and_reraises(
    monkeypatch, capsys
):
    """A key is minted exactly once and is never retrievable again -- an
    `--env-file` write failure must never silently discard it."""
    client = FakeClient()
    monkeypatch.setattr(setup_telemetry, "make_client", lambda args, parser: client)

    def boom(path, content):
        raise OSError("Read-only file system")

    monkeypatch.setattr(setup_telemetry, "write_env_file", boom)

    args = make_args(
        audience="team-alpha",
        otlp_endpoint="http://ingest:9000/ingestion/otlp",
        env_file="/no/such/place.env",
    )
    my_audiences = {
        "is_admin": False,
        "audiences": ["team-alpha"],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
    }
    client.my_audiences_result = my_audiences

    with pytest.raises(OSError):
        setup_telemetry.run(args, FakeParser())

    captured = capsys.readouterr()
    assert "Authorization=Bearer mmk_secret" in captured.out
    assert "warning" in captured.err.lower()
    assert "/no/such/place.env" in captured.err


def test_main_exits_non_zero_on_env_file_os_error(monkeypatch, capsys):
    def raise_os_error(args, parser):
        raise OSError("Read-only file system")

    monkeypatch.setattr(setup_telemetry, "run", raise_os_error)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "micromegas-setup-telemetry",
            "--url",
            "http://analytics:3000",
            "--name",
            "laptop",
        ],
    )
    with pytest.raises(SystemExit) as exc_info:
        setup_telemetry.main()
    assert exc_info.value.code == 1
    assert "Error:" in capsys.readouterr().err


# ---------------------------------------------------------------------------
# format_env_exports
# ---------------------------------------------------------------------------


def test_format_env_exports_includes_protocol_endpoint_and_bearer_header():
    content = setup_telemetry.format_env_exports(
        "mmk_x", "http://ingest:9000/ingestion/otlp"
    )
    assert "export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf" in content
    assert (
        "export OTEL_EXPORTER_OTLP_ENDPOINT=http://ingest:9000/ingestion/otlp"
        in content
    )
    assert 'export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer mmk_x"' in content
