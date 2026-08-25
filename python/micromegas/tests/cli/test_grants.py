import argparse

import pytest

from micromegas.cli import grants
from micromegas.web_client import WebClient


class FakeClient:
    """Records every call and returns a canned response, mirroring
    `test_import_keys.py`'s `FakeClient` lightweight-mocking style."""

    def __init__(self):
        self.calls = []

    def create_audience_grant(self, audience, axis, selector):
        self.calls.append(("create", audience, axis, selector))
        return {
            "audience": audience,
            "axis": axis,
            "selector": selector,
            "created_at": "2026-08-19T00:00:00Z",
            "created_by": "admin@example.com",
        }

    def delete_audience_grant(self, audience, axis, selector):
        self.calls.append(("delete", audience, axis, selector))


def make_args(**overrides):
    defaults = {
        "url": "http://analytics:3000",
        "profile": None,
    }
    defaults.update(overrides)
    return argparse.Namespace(**defaults)


# ---------------------------------------------------------------------------
# make_client
# ---------------------------------------------------------------------------


def test_make_client_returns_web_client(monkeypatch):
    monkeypatch.setattr(grants, "build_auth_provider", lambda args: None)
    args = make_args()
    client = grants.make_client(args)
    assert isinstance(client, WebClient)
    assert client.base_url == "http://analytics:3000"


# ---------------------------------------------------------------------------
# cmd_create / cmd_list / cmd_delete
# ---------------------------------------------------------------------------


def test_cmd_create_calls_create_audience_grant(monkeypatch, capsys):
    fake = FakeClient()
    monkeypatch.setattr(grants, "make_client", lambda args: fake)
    args = argparse.Namespace(audience="team-alpha", axis="read", selector="group:eng")
    grants.cmd_create(args)
    assert fake.calls == [("create", "team-alpha", "read", "group:eng")]
    out = capsys.readouterr().out
    assert "team-alpha" in out
    assert "group:eng" in out


def test_cmd_delete_calls_delete_audience_grant(monkeypatch, capsys):
    fake = FakeClient()
    monkeypatch.setattr(grants, "make_client", lambda args: fake)
    args = argparse.Namespace(
        audience="team-alpha", axis="mint", selector="user:alice@example.com"
    )
    grants.cmd_delete(args)
    assert fake.calls == [("delete", "team-alpha", "mint", "user:alice@example.com")]
    out = capsys.readouterr().out
    assert "Deleted" in out


# ---------------------------------------------------------------------------
# main -- argument parsing and error handling
# ---------------------------------------------------------------------------


def test_main_create_subcommand_dispatches(monkeypatch):
    fake = FakeClient()
    monkeypatch.setattr(grants, "make_client", lambda args: fake)
    monkeypatch.setattr(
        "sys.argv",
        [
            "micromegas-grants",
            "--url",
            "http://analytics:3000",
            "create",
            "team-alpha",
            "read",
            "group:eng",
        ],
    )
    grants.main()
    assert fake.calls == [("create", "team-alpha", "read", "group:eng")]


def test_main_rejects_list_subcommand(monkeypatch, capsys):
    """The `list` subcommand is removed (#1510) -- listing now goes through
    `micromegas-query --all "SELECT * FROM list_audience_grants()"` instead."""
    monkeypatch.setattr(
        "sys.argv",
        ["micromegas-grants", "--url", "http://analytics:3000", "list"],
    )
    with pytest.raises(SystemExit) as exc_info:
        grants.main()
    assert exc_info.value.code == 2
    assert "invalid choice: 'list'" in capsys.readouterr().err


def test_main_reports_runtime_error_and_exits(monkeypatch, capsys):
    class FailingClient:
        def create_audience_grant(self, audience, axis, selector):
            raise RuntimeError("HTTP 403: forbidden")

    monkeypatch.setattr(grants, "make_client", lambda args: FailingClient())
    monkeypatch.setattr(
        "sys.argv",
        [
            "micromegas-grants",
            "--url",
            "http://analytics:3000",
            "create",
            "team-alpha",
            "read",
            "group:eng",
        ],
    )
    try:
        grants.main()
        assert False, "expected SystemExit"
    except SystemExit as e:
        assert e.code == 1
    err = capsys.readouterr().err
    assert "forbidden" in err
