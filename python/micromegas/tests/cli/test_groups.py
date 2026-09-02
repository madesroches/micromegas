import argparse

import pytest

from micromegas.cli import groups
from micromegas.web_client import WebClient


class FakeClient:
    """Records every call and returns a canned response, mirroring
    `test_grants.py`'s `FakeClient` lightweight-mocking style."""

    def __init__(self):
        self.calls = []

    def list_groups(self):
        self.calls.append(("list",))
        return [
            {
                "name": "admins",
                "description": "Deployment administrators",
                "member_count": 1,
                "created_at": "2026-08-19T00:00:00Z",
                "created_by": "default",
            }
        ]

    def create_group(self, name, description=None):
        self.calls.append(("create", name, description))
        return {
            "name": name,
            "description": description,
            "member_count": 0,
            "created_at": "2026-08-19T00:00:00Z",
            "created_by": "admin@example.com",
        }

    def delete_group(self, name):
        self.calls.append(("delete", name))

    def list_group_members(self, name):
        self.calls.append(("members", name))
        return [
            {
                "group_name": name,
                "member": "user:alice@example.com",
                "created_at": "2026-08-19T00:00:00Z",
                "created_by": "admin@example.com",
            }
        ]

    def add_group_member(self, name, member):
        self.calls.append(("add", name, member))
        return {
            "group_name": name,
            "member": member,
            "created_at": "2026-08-19T00:00:00Z",
            "created_by": "admin@example.com",
        }

    def remove_group_member(self, name, member):
        self.calls.append(("remove", name, member))


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
    monkeypatch.setattr(groups, "build_auth_provider", lambda args: None)
    args = make_args()
    client = groups.make_client(args)
    assert isinstance(client, WebClient)
    assert client.base_url == "http://analytics:3000"


# ---------------------------------------------------------------------------
# cmd_* dispatch
# ---------------------------------------------------------------------------


def test_cmd_list_calls_list_groups(monkeypatch, capsys):
    fake = FakeClient()
    monkeypatch.setattr(groups, "make_client", lambda args: fake)
    groups.cmd_list(argparse.Namespace())
    assert fake.calls == [("list",)]
    out = capsys.readouterr().out
    assert "admins" in out


def test_cmd_list_reports_no_groups(monkeypatch, capsys):
    class EmptyClient:
        def list_groups(self):
            return []

    monkeypatch.setattr(groups, "make_client", lambda args: EmptyClient())
    groups.cmd_list(argparse.Namespace())
    assert "No groups." in capsys.readouterr().out


def test_cmd_create_calls_create_group(monkeypatch, capsys):
    fake = FakeClient()
    monkeypatch.setattr(groups, "make_client", lambda args: fake)
    args = argparse.Namespace(name="eng", description="Engineering")
    groups.cmd_create(args)
    assert fake.calls == [("create", "eng", "Engineering")]
    out = capsys.readouterr().out
    assert "eng" in out


def test_cmd_delete_calls_delete_group(monkeypatch, capsys):
    fake = FakeClient()
    monkeypatch.setattr(groups, "make_client", lambda args: fake)
    args = argparse.Namespace(name="eng")
    groups.cmd_delete(args)
    assert fake.calls == [("delete", "eng")]
    assert "Deleted" in capsys.readouterr().out


def test_cmd_members_calls_list_group_members(monkeypatch, capsys):
    fake = FakeClient()
    monkeypatch.setattr(groups, "make_client", lambda args: fake)
    args = argparse.Namespace(name="admins")
    groups.cmd_members(args)
    assert fake.calls == [("members", "admins")]
    assert "user:alice@example.com" in capsys.readouterr().out


def test_cmd_members_reports_no_members(monkeypatch, capsys):
    class EmptyClient:
        def list_group_members(self, name):
            return []

    monkeypatch.setattr(groups, "make_client", lambda args: EmptyClient())
    groups.cmd_members(argparse.Namespace(name="eng"))
    assert "No members." in capsys.readouterr().out


def test_cmd_add_calls_add_group_member(monkeypatch, capsys):
    fake = FakeClient()
    monkeypatch.setattr(groups, "make_client", lambda args: fake)
    args = argparse.Namespace(name="admins", member="user:alice@example.com")
    groups.cmd_add(args)
    assert fake.calls == [("add", "admins", "user:alice@example.com")]
    out = capsys.readouterr().out
    assert "admins" in out and "user:alice@example.com" in out


def test_cmd_remove_calls_remove_group_member(monkeypatch, capsys):
    fake = FakeClient()
    monkeypatch.setattr(groups, "make_client", lambda args: fake)
    args = argparse.Namespace(name="admins", member="*")
    groups.cmd_remove(args)
    assert fake.calls == [("remove", "admins", "*")]
    out = capsys.readouterr().out
    assert "admins" in out


# ---------------------------------------------------------------------------
# main -- argument parsing and error handling
# ---------------------------------------------------------------------------


def test_main_add_subcommand_dispatches(monkeypatch):
    fake = FakeClient()
    monkeypatch.setattr(groups, "make_client", lambda args: fake)
    monkeypatch.setattr(
        "sys.argv",
        [
            "micromegas-groups",
            "--url",
            "http://analytics:3000",
            "add",
            "admins",
            "user:alice@example.com",
        ],
    )
    groups.main()
    assert fake.calls == [("add", "admins", "user:alice@example.com")]


def test_main_rejects_bootstrap_subcommand(monkeypatch, capsys):
    """No `bootstrap` convenience command -- the two-command
    add-then-remove sequence stays documented as-is."""
    monkeypatch.setattr(
        "sys.argv",
        ["micromegas-groups", "--url", "http://analytics:3000", "bootstrap"],
    )
    with pytest.raises(SystemExit) as exc_info:
        groups.main()
    assert exc_info.value.code == 2
    assert "invalid choice: 'bootstrap'" in capsys.readouterr().err


def test_main_reports_runtime_error_and_exits(monkeypatch, capsys):
    class FailingClient:
        def add_group_member(self, name, member):
            raise RuntimeError("HTTP 409: would create a cycle")

    monkeypatch.setattr(groups, "make_client", lambda args: FailingClient())
    monkeypatch.setattr(
        "sys.argv",
        [
            "micromegas-groups",
            "--url",
            "http://analytics:3000",
            "add",
            "a",
            "group:b",
        ],
    )
    try:
        groups.main()
        assert False, "expected SystemExit"
    except SystemExit as e:
        assert e.code == 1
    err = capsys.readouterr().err
    assert "cycle" in err
