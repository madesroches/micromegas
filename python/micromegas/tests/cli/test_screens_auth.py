import argparse

import pytest

from micromegas.cli import screens as screens_module
from micromegas.cli.config import ProfileError
from micromegas.web_client import WebClient


def make_args(**overrides):
    defaults = {"profile": None, "no_auth": False}
    defaults.update(overrides)
    return argparse.Namespace(**defaults)


# ---------------------------------------------------------------------------
# make_client -- these monkeypatch screens.web_auth.resolve_web_auth to
# return a canned result rather than exercising real config resolution;
# test_web_auth.py already covers that via config_path=tmp_path, so these
# never touch a developer's actual ~/.micromegas/config.json.
# ---------------------------------------------------------------------------


def test_make_client_no_provider_raises_profile_error_with_hint(monkeypatch):
    monkeypatch.setattr(
        screens_module.web_auth,
        "resolve_web_auth",
        lambda profile=None: (None, "profile 'prod' resolves no auth mechanism"),
    )
    config = {"server": "http://analytics:3000", "managed_by": "test"}
    with pytest.raises(ProfileError) as exc_info:
        screens_module.make_client(config, make_args(profile="prod"))
    message = str(exc_info.value)
    assert "profile 'prod' resolves no auth mechanism" in message
    assert "--no-auth" in message


def test_make_client_no_auth_flag_short_circuits_resolve_web_auth(monkeypatch):
    called = []
    monkeypatch.setattr(
        screens_module.web_auth,
        "resolve_web_auth",
        lambda profile=None: called.append(profile) or (None, "unused"),
    )
    config = {"server": "http://analytics:3000", "managed_by": "test"}
    client = screens_module.make_client(config, make_args(no_auth=True))
    assert isinstance(client, WebClient)
    assert client.auth_provider is None
    assert client.base_url == "http://analytics:3000"
    assert called == []


def test_make_client_passes_through_resolved_provider(monkeypatch):
    sentinel_provider = object()
    monkeypatch.setattr(
        screens_module.web_auth,
        "resolve_web_auth",
        lambda profile=None: (sentinel_provider, None),
    )
    config = {"server": "http://analytics:3000", "managed_by": "test"}
    client = screens_module.make_client(config, make_args(profile="prod"))
    assert isinstance(client, WebClient)
    assert client.auth_provider is sentinel_provider
    assert client.base_url == "http://analytics:3000"


def test_make_client_reraises_profile_error_with_hint(monkeypatch):
    def raise_profile_error(profile=None):
        raise ProfileError("no profile selected")

    monkeypatch.setattr(
        screens_module.web_auth, "resolve_web_auth", raise_profile_error
    )
    config = {"server": "http://analytics:3000", "managed_by": "test"}
    with pytest.raises(ProfileError) as exc_info:
        screens_module.make_client(config, make_args())
    message = str(exc_info.value)
    assert "no profile selected" in message
    assert "--no-auth" in message


# ---------------------------------------------------------------------------
# Parser wiring
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "subcommand,extra_argv",
    [
        ("import", ["some-screen"]),
        ("pull", []),
        ("plan", []),
        ("apply", []),
        ("list", []),
    ],
)
def test_client_subcommands_get_profile_and_no_auth_flags(
    monkeypatch, subcommand, extra_argv
):
    captured = {}

    def fake_cmd(args):
        captured["args"] = args

    monkeypatch.setattr(screens_module, f"cmd_{subcommand}", fake_cmd)
    monkeypatch.setattr(
        "sys.argv",
        ["micromegas-screens", subcommand, *extra_argv, "--profile", "prod"],
    )
    screens_module.main()

    args = captured["args"]
    assert args.profile == "prod"
    assert args.no_auth is False


@pytest.mark.parametrize(
    "subcommand,extra_argv",
    [
        ("import", ["some-screen"]),
        ("pull", []),
        ("plan", []),
        ("apply", []),
        ("list", []),
    ],
)
def test_client_subcommands_accept_no_auth_flag(monkeypatch, subcommand, extra_argv):
    captured = {}

    def fake_cmd(args):
        captured["args"] = args

    monkeypatch.setattr(screens_module, f"cmd_{subcommand}", fake_cmd)
    monkeypatch.setattr(
        "sys.argv",
        ["micromegas-screens", subcommand, *extra_argv, "--no-auth"],
    )
    screens_module.main()

    args = captured["args"]
    assert args.no_auth is True


def test_init_rejects_profile_flag(monkeypatch, capsys):
    monkeypatch.setattr(
        "sys.argv",
        ["micromegas-screens", "init", "https://example.com", "--profile", "prod"],
    )
    with pytest.raises(SystemExit) as exc_info:
        screens_module.main()
    assert exc_info.value.code == 2
