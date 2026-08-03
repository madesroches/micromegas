import sys
from pathlib import Path

import pytest

from micromegas.cli.logout import main


@pytest.fixture
def fake_home(tmp_path, monkeypatch):
    """Point Path.home() (and thus every path logout.main() touches) at a
    scratch directory, so these tests never unlink a developer's real
    token files."""
    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    (tmp_path / ".micromegas").mkdir()
    return tmp_path


def test_bare_logout_clears_plain_and_all_profile_tokens(
    fake_home, monkeypatch, capsys
):
    token_dir = fake_home / ".micromegas"
    (token_dir / "tokens.json").write_text("{}")
    (token_dir / "tokens-prod.json").write_text("{}")
    (token_dir / "tokens-dev.json").write_text("{}")

    monkeypatch.setattr(sys, "argv", ["micromegas-logout"])
    main()

    assert not (token_dir / "tokens.json").exists()
    assert not (token_dir / "tokens-prod.json").exists()
    assert not (token_dir / "tokens-dev.json").exists()

    out = capsys.readouterr().out
    assert "tokens.json" in out
    assert "tokens-prod.json" in out
    assert "tokens-dev.json" in out


def test_profile_logout_clears_only_that_profile(fake_home, monkeypatch, capsys):
    token_dir = fake_home / ".micromegas"
    (token_dir / "tokens.json").write_text("{}")
    (token_dir / "tokens-prod.json").write_text("{}")
    (token_dir / "tokens-dev.json").write_text("{}")

    monkeypatch.setattr(sys, "argv", ["micromegas-logout", "--profile", "prod"])
    main()

    assert not (token_dir / "tokens-prod.json").exists()
    assert (token_dir / "tokens.json").exists()
    assert (token_dir / "tokens-dev.json").exists()

    out = capsys.readouterr().out
    assert "Tokens cleared from" in out
    assert "tokens-prod.json" in out


def test_logout_no_files_prints_no_saved_tokens(fake_home, monkeypatch, capsys):
    monkeypatch.setattr(sys, "argv", ["micromegas-logout"])
    main()

    out = capsys.readouterr().out
    assert "No saved tokens found" in out


def test_logout_ignores_micromegas_profile_env(fake_home, monkeypatch, capsys):
    """A bare invocation clears everything even with MICROMEGAS_PROFILE set."""
    token_dir = fake_home / ".micromegas"
    (token_dir / "tokens.json").write_text("{}")
    (token_dir / "tokens-dev.json").write_text("{}")

    monkeypatch.setenv("MICROMEGAS_PROFILE", "dev")
    monkeypatch.setattr(sys, "argv", ["micromegas-logout"])
    main()

    assert not (token_dir / "tokens.json").exists()
    assert not (token_dir / "tokens-dev.json").exists()
