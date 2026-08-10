import importlib.metadata
import platform
import sys

import pytest

import micromegas
from micromegas.cli import import_keys, logout, query, screens
from micromegas.cli.version import package_version


def test_package_version_returns_installed_version():
    assert package_version() == importlib.metadata.version("micromegas")


def test_package_version_unknown_when_not_installed(monkeypatch):
    def raise_not_found(name):
        raise importlib.metadata.PackageNotFoundError(name)

    monkeypatch.setattr(importlib.metadata, "version", raise_not_found)
    assert package_version() == "unknown"


def test_micromegas_dunder_version():
    assert micromegas.__version__ == importlib.metadata.version("micromegas")


def test_query_version_flag(monkeypatch, capsys):
    # Narrow terminal width to catch reflowing/mid-token wrapping regressions
    # (e.g. reverting to argparse's built-in action="version").
    monkeypatch.setenv("COLUMNS", "30")
    monkeypatch.setattr(sys, "argv", ["micromegas-query", "--version"])
    with pytest.raises(SystemExit) as exc_info:
        query.main()
    assert exc_info.value.code == 0
    out = capsys.readouterr().out
    assert importlib.metadata.version("micromegas") in out
    assert platform.python_version() in out
    assert sys.executable in out


def test_logout_version_flag(monkeypatch, capsys):
    monkeypatch.setattr(sys, "argv", ["micromegas-logout", "--version"])
    with pytest.raises(SystemExit) as exc_info:
        logout.main()
    assert exc_info.value.code == 0
    out = capsys.readouterr().out
    assert importlib.metadata.version("micromegas") in out
    assert platform.python_version() in out
    assert sys.executable in out


def test_screens_version_flag(monkeypatch, capsys):
    monkeypatch.setattr(sys, "argv", ["micromegas-screens", "--version"])
    with pytest.raises(SystemExit) as exc_info:
        screens.main()
    assert exc_info.value.code == 0
    out = capsys.readouterr().out
    assert importlib.metadata.version("micromegas") in out
    assert platform.python_version() in out
    assert sys.executable in out


def test_import_keys_version_flag(monkeypatch, capsys):
    monkeypatch.setattr(sys, "argv", ["micromegas-import-keys", "--version"])
    with pytest.raises(SystemExit) as exc_info:
        import_keys.main()
    assert exc_info.value.code == 0
    out = capsys.readouterr().out
    assert importlib.metadata.version("micromegas") in out
    assert platform.python_version() in out
    assert sys.executable in out


def test_query_version_flag_percent_in_interpreter_path(monkeypatch, capsys):
    # Regression test: a %-style formatted version string (e.g. via "%(prog)s")
    # would mangle or crash on a literal "%" in sys.executable.
    weird_executable = "/weird/pa%th/py%(prog)s/python3.11"
    monkeypatch.setattr(sys, "executable", weird_executable)
    monkeypatch.setattr(sys, "argv", ["micromegas-query", "--version"])
    with pytest.raises(SystemExit) as exc_info:
        query.main()
    assert exc_info.value.code == 0
    out = capsys.readouterr().out
    assert weird_executable in out
