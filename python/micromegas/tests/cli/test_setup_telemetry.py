import argparse
import sys

import pytest

from micromegas.cli import setup_telemetry
from micromegas.web_client import WebClient


class FakeClient:
    """Records every call and returns canned responses, mirroring
    `test_import_keys.py`/`test_grants.py`'s `FakeClient` lightweight-mocking style."""

    def __init__(
        self, my_audiences=None, mint_result=None, list_result=None, keys_result=None
    ):
        self.calls = []
        self.my_audiences_result = my_audiences or {
            "is_admin": False,
            "audiences": [],
            "mint_prefix": None,
            "email": None,
        }
        self.mint_result = mint_result or {
            "key_id": "key-1",
            "name": "laptop",
            "audience": "team-alpha",
            "key": "mmk_secret",
        }
        self.list_result = list_result if list_result is not None else []
        self.keys_result = keys_result if keys_result is not None else []

    def my_audiences(self):
        self.calls.append(("my_audiences",))
        return self.my_audiences_result

    def mint_ingestion_api_key(self, name, audience=None):
        self.calls.append(("mint", name, audience))
        result = dict(self.mint_result)
        if audience is not None:
            result["audience"] = audience
        return result

    def list_audience_grants(self, audience=None, axis=None, limit=None, offset=None):
        self.calls.append(("list", audience, axis, limit, offset))
        return self.list_result

    def list_ingestion_api_keys(self, limit=None, offset=None, include_revoked=None):
        self.calls.append(("list_keys", limit, offset, include_revoked))
        return self.keys_result

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
    assert args.otlp_endpoint is None
    assert args.env_file is None


def test_make_client_returns_web_client(monkeypatch):
    monkeypatch.setattr(
        setup_telemetry, "build_auth_provider", lambda args, parser: None
    )
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
# resolve_audience -- the three-way --audience prefix rule (§6)
# ---------------------------------------------------------------------------


def test_omitted_audience_non_admin_exactly_one_match_is_used_silently(capsys):
    client = FakeClient()
    my_audiences = {
        "is_admin": False,
        "audiences": ["team-alpha"],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
    }
    args = make_args(audience=None)
    audience, brand_new = setup_telemetry.resolve_audience(
        client, args, FakeParser(), my_audiences
    )
    assert audience == "team-alpha"
    assert brand_new is False


def test_omitted_audience_non_admin_multiple_matches_is_an_error():
    client = FakeClient()
    my_audiences = {
        "is_admin": False,
        "audiences": ["team-alpha", "team-beta"],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
    }
    args = make_args(audience=None)
    with pytest.raises(SystemExit):
        setup_telemetry.resolve_audience(client, args, FakeParser(), my_audiences)


def test_omitted_audience_non_admin_no_matches_is_an_error():
    client = FakeClient()
    my_audiences = {
        "is_admin": False,
        "audiences": [],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
    }
    args = make_args(audience=None)
    with pytest.raises(SystemExit):
        setup_telemetry.resolve_audience(client, args, FakeParser(), my_audiences)


def test_omitted_audience_admin_is_always_an_error():
    client = FakeClient()
    my_audiences = {
        "is_admin": True,
        "audiences": [],
        "mint_prefix": None,
        "email": "admin@example.com",
    }
    args = make_args(audience=None)
    with pytest.raises(SystemExit):
        setup_telemetry.resolve_audience(client, args, FakeParser(), my_audiences)


def test_audience_already_granted_is_used_verbatim_never_prefixed(capsys):
    client = FakeClient()
    my_audiences = {
        "is_admin": False,
        "audiences": ["team-alpha"],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
    }
    args = make_args(audience="team-alpha")
    audience, brand_new = setup_telemetry.resolve_audience(
        client, args, FakeParser(), my_audiences
    )
    assert audience == "team-alpha"
    assert brand_new is False
    assert client.calls == []


def test_fresh_audience_non_admin_is_prefixed_and_announced_to_stderr(capsys):
    client = FakeClient()
    my_audiences = {
        "is_admin": False,
        "audiences": [],
        "mint_prefix": "alice-",
        "email": "alice@example.com",
    }
    args = make_args(audience="laptop")
    audience, brand_new = setup_telemetry.resolve_audience(
        client, args, FakeParser(), my_audiences
    )
    assert audience == "alice-laptop"
    assert brand_new is False
    err = capsys.readouterr().err
    assert "alice-laptop" in err


def test_fresh_audience_non_admin_with_no_email_is_an_error():
    client = FakeClient()
    my_audiences = {
        "is_admin": False,
        "audiences": [],
        "mint_prefix": None,
        "email": None,
    }
    args = make_args(audience="laptop")
    with pytest.raises(SystemExit):
        setup_telemetry.resolve_audience(client, args, FakeParser(), my_audiences)


def test_admin_audience_is_never_prefixed_even_when_not_in_my_audiences():
    client = FakeClient(list_result=[])
    my_audiences = {
        "is_admin": True,
        "audiences": [],
        "mint_prefix": None,
        "email": "admin@example.com",
    }
    args = make_args(audience="ci")
    audience, brand_new = setup_telemetry.resolve_audience(
        client, args, FakeParser(), my_audiences
    )
    assert audience == "ci"
    assert brand_new is True
    assert client.calls == [
        ("list", "ci", None, None, None),
        ("list_keys", 500, 0, True),
    ]


def test_admin_audience_with_existing_grant_rows_is_not_brand_new():
    client = FakeClient(
        list_result=[
            {
                "audience": "ci",
                "axis": "read",
                "selector": "group:eng",
                "created_at": "2026-08-19T00:00:00Z",
                "created_by": "someone@example.com",
            }
        ]
    )
    my_audiences = {
        "is_admin": True,
        "audiences": [],
        "mint_prefix": None,
        "email": "admin@example.com",
    }
    args = make_args(audience="ci")
    audience, brand_new = setup_telemetry.resolve_audience(
        client, args, FakeParser(), my_audiences
    )
    assert audience == "ci"
    assert brand_new is False


def test_admin_audience_with_no_grants_but_existing_key_is_not_brand_new():
    """An audience an admin minted into before any grant row existed has no
    `audience_grants` row at all, but does have an `ingestion_api_keys` row --
    the CLI's brand-new check must catch this the same way the server's own
    broader ownership predicate does (`try_claim_and_mint`), or the admin
    would silently self-grant `read` on pre-existing data."""
    client = FakeClient(
        list_result=[],
        keys_result=[
            {
                "key_id": "key-0",
                "name": "old-key",
                "audience": "ci",
                "created_by": "someone@example.com",
            }
        ],
    )
    my_audiences = {
        "is_admin": True,
        "audiences": [],
        "mint_prefix": None,
        "email": "admin@example.com",
    }
    args = make_args(audience="ci")
    audience, brand_new = setup_telemetry.resolve_audience(
        client, args, FakeParser(), my_audiences
    )
    assert audience == "ci"
    assert brand_new is False


def test_admin_audience_check_pages_through_ingestion_keys():
    """A match on a later page (not just the first) must still count."""
    first_page = [
        {"key_id": f"k{i}", "name": "n", "audience": "other", "created_by": "x"}
        for i in range(setup_telemetry._KEY_PAGE_SIZE)
    ]
    second_page = [
        {"key_id": "k-last", "name": "n", "audience": "ci", "created_by": "x"}
    ]

    class PagingClient(FakeClient):
        def list_ingestion_api_keys(
            self, limit=None, offset=None, include_revoked=None
        ):
            self.calls.append(("list_keys", limit, offset, include_revoked))
            return first_page if offset == 0 else second_page

    client = PagingClient(list_result=[])
    my_audiences = {
        "is_admin": True,
        "audiences": [],
        "mint_prefix": None,
        "email": "admin@example.com",
    }
    args = make_args(audience="ci")
    audience, brand_new = setup_telemetry.resolve_audience(
        client, args, FakeParser(), my_audiences
    )
    assert audience == "ci"
    assert brand_new is False
    assert client.calls == [
        ("list", "ci", None, None, None),
        ("list_keys", 500, 0, True),
        ("list_keys", 500, 500, True),
    ]


# ---------------------------------------------------------------------------
# run() -- end-to-end wiring
# ---------------------------------------------------------------------------


def test_run_non_admin_claim_does_not_call_create_audience_grant(monkeypatch, capsys):
    client = FakeClient(
        my_audiences={
            "is_admin": False,
            "audiences": [],
            "mint_prefix": "alice-",
            "email": "alice@example.com",
        },
        mint_result={
            "key_id": "key-1",
            "name": "laptop",
            "audience": "alice-laptop",
            "key": "mmk_secret",
        },
    )
    monkeypatch.setattr(setup_telemetry, "make_client", lambda args, parser: client)
    args = make_args(
        audience="laptop", otlp_endpoint="http://ingest:9000/ingestion/otlp"
    )
    setup_telemetry.run(args, FakeParser())

    assert ("mint", "laptop", "alice-laptop") in client.calls
    assert not any(call[0] == "create" for call in client.calls)

    out = capsys.readouterr().out
    assert "OTEL_EXPORTER_OTLP_ENDPOINT=http://ingest:9000/ingestion/otlp" in out
    assert "Authorization=Bearer mmk_secret" in out


def test_run_admin_brand_new_claim_writes_mint_and_read_grants(monkeypatch, capsys):
    client = FakeClient(
        my_audiences={
            "is_admin": True,
            "audiences": [],
            "mint_prefix": None,
            "email": "admin@example.com",
        },
        mint_result={
            "key_id": "key-1",
            "name": "laptop",
            "audience": "ci",
            "key": "mmk_secret",
        },
        list_result=[],
    )
    monkeypatch.setattr(setup_telemetry, "make_client", lambda args, parser: client)
    args = make_args(audience="ci", otlp_endpoint="http://ingest:9000/ingestion/otlp")
    setup_telemetry.run(args, FakeParser())

    assert ("mint", "laptop", "ci") in client.calls
    assert ("create", "ci", "mint", "user:admin@example.com") in client.calls
    assert ("create", "ci", "read", "user:admin@example.com") in client.calls


def test_run_admin_non_brand_new_audience_skips_grant_calls(monkeypatch):
    client = FakeClient(
        my_audiences={
            "is_admin": True,
            "audiences": [],
            "mint_prefix": None,
            "email": "admin@example.com",
        },
        mint_result={
            "key_id": "key-1",
            "name": "laptop",
            "audience": "team-alpha",
            "key": "mmk_secret",
        },
        list_result=[
            {
                "audience": "team-alpha",
                "axis": "mint",
                "selector": "*",
                "created_at": "2026-08-19T00:00:00Z",
                "created_by": "admin@example.com",
            }
        ],
    )
    monkeypatch.setattr(setup_telemetry, "make_client", lambda args, parser: client)
    args = make_args(
        audience="team-alpha", otlp_endpoint="http://ingest:9000/ingestion/otlp"
    )
    setup_telemetry.run(args, FakeParser())

    assert not any(call[0] == "create" for call in client.calls)


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
