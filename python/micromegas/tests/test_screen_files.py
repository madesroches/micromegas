"""Unit tests for screen file I/O and plan logic."""

import json
import os
import subprocess
import sys
import tempfile
import textwrap
from pathlib import Path

import pytest

from micromegas.cli import screens as screens_module
from micromegas.cli.screens import (
    cmd_pull,
    compute_plan,
    format_screen_diff,
    list_local_screens,
    read_screen_file,
    server_screen_to_file,
    strip_volatile_keys,
    write_screen_file,
)


@pytest.fixture
def screen_dict():
    return {
        "name": "test-notebook",
        "screen_type": "notebook",
        "config": {
            "timeRangeFrom": "now-5m",
            "timeRangeTo": "now",
            "cells": [{"type": "markdown", "content": "hello"}],
        },
    }


class TestWriteReadRoundTrip:
    def test_round_trip(self, screen_dict, tmp_path):
        path = tmp_path / "test-notebook.json"
        write_screen_file(path, screen_dict)
        result = read_screen_file(path)
        assert result == screen_dict

    def test_key_order(self, screen_dict, tmp_path):
        path = tmp_path / "test-notebook.json"
        write_screen_file(path, screen_dict)
        with open(path) as f:
            content = f.read()
        lines = content.strip().split("\n")
        # First key should be "name"
        assert '"name"' in lines[1]
        # Second key should be "screen_type"
        assert '"screen_type"' in lines[2]

    def test_trailing_newline(self, screen_dict, tmp_path):
        path = tmp_path / "test-notebook.json"
        write_screen_file(path, screen_dict)
        with open(path) as f:
            content = f.read()
        assert content.endswith("\n")

    def test_round_trip_with_folder_path(self, tmp_path):
        path = tmp_path / "test-notebook.json"
        data = {
            "name": "test-notebook",
            "screen_type": "notebook",
            "config": {"cells": []},
            "folder_path": "dashboards/team-a",
        }
        write_screen_file(path, data)
        result = read_screen_file(path)
        assert result == data

    def test_extra_keys_stripped(self, tmp_path):
        """Extra keys like created_by should not appear in output."""
        data = {
            "name": "foo",
            "screen_type": "notebook",
            "config": {},
            "created_by": "user@test.com",
            "updated_at": "2024-01-01",
        }
        path = tmp_path / "foo.json"
        write_screen_file(path, data)
        result = read_screen_file(path)
        assert "created_by" not in result
        assert "updated_at" not in result


class TestValidation:
    def test_missing_name(self, tmp_path):
        path = tmp_path / "bad.json"
        with open(path, "w") as f:
            json.dump({"screen_type": "notebook", "config": {}}, f)
        with pytest.raises(ValueError, match="missing required field 'name'"):
            read_screen_file(path)

    def test_missing_config(self, tmp_path):
        path = tmp_path / "bad.json"
        with open(path, "w") as f:
            json.dump({"name": "foo", "screen_type": "notebook"}, f)
        with pytest.raises(ValueError, match="missing required field 'config'"):
            read_screen_file(path)

    def test_missing_screen_type(self, tmp_path):
        path = tmp_path / "bad.json"
        with open(path, "w") as f:
            json.dump({"name": "foo", "config": {}}, f)
        with pytest.raises(ValueError, match="missing required field 'screen_type'"):
            read_screen_file(path)


class TestServerScreenToFile:
    def test_strips_metadata(self):
        server = {
            "name": "test",
            "screen_type": "notebook",
            "config": {"cells": []},
            "created_by": "user@test.com",
            "updated_by": "user@test.com",
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z",
            "managed_by": "https://github.com/org/repo/tree/main/screens",
        }
        result = server_screen_to_file(server)
        assert set(result.keys()) == {"name", "screen_type", "config", "managed_by"}

    def test_copies_non_empty_folder_path(self):
        server = {
            "name": "test",
            "screen_type": "notebook",
            "config": {"cells": []},
            "folder_path": "dashboards/team-a",
        }
        result = server_screen_to_file(server)
        assert result["folder_path"] == "dashboards/team-a"

    def test_omits_empty_folder_path(self):
        server = {
            "name": "test",
            "screen_type": "notebook",
            "config": {"cells": []},
            "folder_path": "",
        }
        result = server_screen_to_file(server)
        assert "folder_path" not in result

    def test_omits_missing_folder_path(self):
        server = {
            "name": "test",
            "screen_type": "notebook",
            "config": {"cells": []},
        }
        result = server_screen_to_file(server)
        assert "folder_path" not in result


class TestListLocalScreens:
    def test_lists_screens(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        # Write config file (should be excluded)
        with open("micromegas-screens.json", "w") as f:
            json.dump({"managed_by": "test", "server": "http://localhost"}, f)
        # Write screen files
        write_screen_file(
            "notebook-a.json",
            {
                "name": "notebook-a",
                "screen_type": "notebook",
                "config": {},
            },
        )
        write_screen_file(
            "notebook-b.json",
            {
                "name": "notebook-b",
                "screen_type": "notebook",
                "config": {},
            },
        )
        screens, unreadable = list_local_screens()
        assert set(screens.keys()) == {"notebook-a", "notebook-b"}
        assert unreadable == set()

    def test_unreadable_files_not_silently_dropped(self, tmp_path, monkeypatch):
        """Files that exist locally but fail to decode/parse must show up in
        `unreadable`, not just disappear as if the file were absent -- for
        both a non-UTF-8-encoded file (UnicodeDecodeError) and a syntactically
        invalid JSON file (JSONDecodeError)."""
        monkeypatch.chdir(tmp_path)
        with open("micromegas-screens.json", "w") as f:
            json.dump({"managed_by": "test", "server": "http://localhost"}, f)
        write_screen_file(
            "ok.json",
            {"name": "ok", "screen_type": "notebook", "config": {}},
        )
        with open("bad.json", "wb") as f:
            f.write(
                '{"name": "bad", "screen_type": "notebook", '
                '"config": {"x": "caf\xe9"}}'.encode("latin-1")
            )
        with open("broken.json", "w") as f:
            f.write("{ not json")

        screens, unreadable = list_local_screens()
        assert set(screens.keys()) == {"ok"}
        assert unreadable == {"bad", "broken"}


class TestComputePlan:
    def _make_client(self, server_screens):
        class FakeClient:
            def list_screens(self):
                return server_screens

        return FakeClient()

    def test_create(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        write_screen_file(
            "new-screen.json",
            {
                "name": "new-screen",
                "screen_type": "notebook",
                "config": {"cells": []},
            },
        )
        config = {
            "managed_by": "https://github.com/org/repo/tree/main/screens",
            "server": "http://localhost",
        }
        client = self._make_client([])
        creates, updates, deletes, unchanged, untracked = compute_plan(config, client)
        assert creates == ["new-screen"]
        assert updates == []
        assert deletes == []

    def test_delete_tracked(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        managed_by = "https://github.com/org/repo/tree/main/screens"
        config = {"managed_by": managed_by, "server": "http://localhost"}
        server_screens = [
            {
                "name": "old-screen",
                "screen_type": "notebook",
                "config": {},
                "managed_by": managed_by,
            }
        ]
        client = self._make_client(server_screens)
        creates, updates, deletes, unchanged, untracked = compute_plan(config, client)
        assert deletes == ["old-screen"]

    def test_no_delete_different_owner(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        config = {
            "managed_by": "https://github.com/org/repo/tree/main/screens",
            "server": "http://localhost",
        }
        server_screens = [
            {
                "name": "other-screen",
                "screen_type": "notebook",
                "config": {},
                "managed_by": "https://github.com/other/repo/tree/main/screens",
            }
        ]
        client = self._make_client(server_screens)
        creates, updates, deletes, unchanged, untracked = compute_plan(config, client)
        assert deletes == []
        assert "other-screen" in untracked

    def test_update_modified(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        write_screen_file(
            "my-screen.json",
            {
                "name": "my-screen",
                "screen_type": "notebook",
                "config": {"cells": [{"type": "markdown", "content": "updated"}]},
            },
        )
        config = {
            "managed_by": "https://github.com/org/repo/tree/main/screens",
            "server": "http://localhost",
        }
        server_screens = [
            {
                "name": "my-screen",
                "screen_type": "notebook",
                "config": {"cells": [{"type": "markdown", "content": "old"}]},
                "managed_by": "https://github.com/org/repo/tree/main/screens",
            }
        ]
        client = self._make_client(server_screens)
        creates, updates, deletes, unchanged, untracked = compute_plan(config, client)
        assert len(updates) == 1
        name, local_dict, server_dict = updates[0]
        assert name == "my-screen"
        assert local_dict["config"]["cells"][0]["content"] == "updated"
        assert server_dict["config"]["cells"][0]["content"] == "old"

    def test_unchanged(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        managed_by = "https://github.com/org/repo/tree/main/screens"
        screen_data = {
            "name": "stable-screen",
            "screen_type": "notebook",
            "config": {"cells": []},
            "managed_by": managed_by,
        }
        write_screen_file("stable-screen.json", screen_data)
        config = {
            "managed_by": managed_by,
            "server": "http://localhost",
        }
        server_screens = [
            {
                "name": "stable-screen",
                "screen_type": "notebook",
                "config": {"cells": []},
                "managed_by": managed_by,
            }
        ]
        client = self._make_client(server_screens)
        creates, updates, deletes, unchanged, untracked = compute_plan(config, client)
        assert unchanged == ["stable-screen"]

    def test_folder_path_diff_surfaces_as_update(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        managed_by = "https://github.com/org/repo/tree/main/screens"
        write_screen_file(
            "my-screen.json",
            {
                "name": "my-screen",
                "screen_type": "notebook",
                "config": {"cells": []},
                "folder_path": "dashboards/team-a",
                "managed_by": managed_by,
            },
        )
        config = {"managed_by": managed_by, "server": "http://localhost"}
        server_screens = [
            {
                "name": "my-screen",
                "screen_type": "notebook",
                "config": {"cells": []},
                "folder_path": "dashboards/team-b",
                "managed_by": managed_by,
            }
        ]
        client = self._make_client(server_screens)
        creates, updates, deletes, unchanged, untracked = compute_plan(config, client)
        assert len(updates) == 1
        name, local_dict, server_dict = updates[0]
        assert name == "my-screen"
        assert local_dict["folder_path"] == "dashboards/team-a"
        assert server_dict["folder_path"] == "dashboards/team-b"

    def test_unmodified_root_screen_no_folder_path_key(self, tmp_path, monkeypatch):
        """Local file omits folder_path; server returns folder_path/managed_by as
        empty/null for a root, unmanaged screen. Should be unchanged, not modified."""
        monkeypatch.chdir(tmp_path)
        managed_by = "https://github.com/org/repo/tree/main/screens"
        write_screen_file(
            "root-screen.json",
            {
                "name": "root-screen",
                "screen_type": "notebook",
                "config": {"cells": []},
            },
        )
        config = {"managed_by": managed_by, "server": "http://localhost"}
        server_screens = [
            {
                "name": "root-screen",
                "screen_type": "notebook",
                "config": {"cells": []},
                "folder_path": "",
                "managed_by": None,
            }
        ]
        client = self._make_client(server_screens)
        creates, updates, deletes, unchanged, untracked = compute_plan(config, client)
        assert unchanged == ["root-screen"]
        assert updates == []

    def test_explicit_empty_folder_path_matches_server_root(
        self, tmp_path, monkeypatch
    ):
        """Local file explicitly sets folder_path="" (move-to-root intent); server
        also reports root. Should compare equal, not spuriously diff."""
        monkeypatch.chdir(tmp_path)
        managed_by = "https://github.com/org/repo/tree/main/screens"
        write_screen_file(
            "root-screen.json",
            {
                "name": "root-screen",
                "screen_type": "notebook",
                "config": {"cells": []},
                "folder_path": "",
                "managed_by": managed_by,
            },
        )
        config = {"managed_by": managed_by, "server": "http://localhost"}
        server_screens = [
            {
                "name": "root-screen",
                "screen_type": "notebook",
                "config": {"cells": []},
                "folder_path": "",
                "managed_by": managed_by,
            }
        ]
        client = self._make_client(server_screens)
        creates, updates, deletes, unchanged, untracked = compute_plan(config, client)
        assert unchanged == ["root-screen"]
        assert updates == []

    def test_unreadable_local_file_not_treated_as_delete(self, tmp_path, monkeypatch):
        """A locally corrupt/unparseable file that is tracked (managed_by ==
        this repo) on the server must never show up in `deletes` -- that
        would make `apply` delete the server-side screen just because the
        local file happens to be malformed."""
        monkeypatch.chdir(tmp_path)
        managed_by = "https://github.com/org/repo/tree/main/screens"
        write_screen_file(
            "ok.json",
            {
                "name": "ok",
                "screen_type": "notebook",
                "config": {},
                "managed_by": managed_by,
            },
        )
        with open("bad.json", "wb") as f:
            f.write(
                '{"name": "bad", "screen_type": "notebook", '
                '"config": {"x": "caf\xe9"}}'.encode("latin-1")
            )
        with open("broken.json", "w") as f:
            f.write("{ not json")

        config = {"managed_by": managed_by, "server": "http://localhost"}
        server_screens = [
            {
                "name": "ok",
                "screen_type": "notebook",
                "config": {},
                "managed_by": managed_by,
            },
            {
                "name": "bad",
                "screen_type": "notebook",
                "config": {},
                "managed_by": managed_by,
            },
            {
                "name": "broken",
                "screen_type": "notebook",
                "config": {},
                "managed_by": managed_by,
            },
        ]
        client = self._make_client(server_screens)
        creates, updates, deletes, unchanged, untracked = compute_plan(config, client)
        assert deletes == []


class TestCmdPull:
    def test_unreadable_local_file_not_overwritten(self, tmp_path, monkeypatch):
        """cmd_pull must not silently clobber a local file it can't decode,
        even when the server returns fresh content for that name -- it
        should skip/warn instead."""
        monkeypatch.chdir(tmp_path)
        with open("micromegas-screens.json", "w") as f:
            json.dump({"managed_by": "test", "server": "http://localhost"}, f)
        original_bytes = (
            '{"name": "bad", "screen_type": "notebook", '
            '"config": {"x": "caf\xe9"}}'.encode("latin-1")
        )
        with open("bad.json", "wb") as f:
            f.write(original_bytes)

        class FakeClient:
            def get_screen(self, name):
                return {
                    "name": name,
                    "screen_type": "notebook",
                    "config": {"cells": []},
                    "managed_by": "test",
                }

        monkeypatch.setattr(screens_module, "make_client", lambda config: FakeClient())

        class Args:
            names = ["bad"]

        cmd_pull(Args())

        assert Path("bad.json").read_bytes() == original_bytes


class TestFormatScreenDiff:
    def test_produces_unified_diff(self):
        server = {
            "name": "test",
            "screen_type": "notebook",
            "config": {"cells": [{"content": "old"}]},
        }
        local = {
            "name": "test",
            "screen_type": "notebook",
            "config": {"cells": [{"content": "new"}]},
        }
        result = format_screen_diff(local, server, use_color=False)
        assert "--- server" in result
        assert "+++ local" in result
        assert '-        "content": "old"' in result
        assert '+        "content": "new"' in result

    def test_no_color(self):
        server = {"name": "a", "screen_type": "notebook", "config": {"x": 1}}
        local = {"name": "a", "screen_type": "notebook", "config": {"x": 2}}
        result = format_screen_diff(local, server, use_color=False)
        assert "\033[" not in result

    def test_with_color(self):
        server = {"name": "a", "screen_type": "notebook", "config": {"x": 1}}
        local = {"name": "a", "screen_type": "notebook", "config": {"x": 2}}
        result = format_screen_diff(local, server, use_color=True)
        assert "\033[31m" in result  # red for removals
        assert "\033[32m" in result  # green for additions
        assert "\033[36m" in result  # cyan for @@ headers

    def test_identical_returns_empty(self):
        data = {"name": "a", "screen_type": "notebook", "config": {}}
        result = format_screen_diff(data, data, use_color=False)
        assert result == ""


NON_ASCII_CONTENT = "em dash —, accented café, CJK 日本語"


class TestNonAsciiEncodingRegression:
    def test_read_survives_non_utf8_locale(self, tmp_path):
        """Regression test for issue #1399: read_screen_file must decode a
        UTF-8-encoded screen file correctly even when the process's
        locale-preferred encoding is not UTF-8 (e.g. LC_ALL=C on Linux, or the
        default console codepage on Windows). Forcing LC_ALL=C alone is not
        enough on Python 3.10+ (PEP 538/540 auto-coerce it back to a UTF-8
        mode); PYTHONUTF8=0 must also be set to actually disable UTF-8 mode.
        A real subprocess is required because CPython's TextIOWrapper resolves
        its default encoding via OS locale APIs, not the Python-level
        locale.getpreferredencoding() function, so in-process monkeypatching
        has no effect on open()'s behavior.

        The fixture file is written as raw UTF-8 bytes directly (bypassing
        write_screen_file/json.dump, whose ensure_ascii=True default would
        escape non-ASCII content to plain-ASCII \\uXXXX sequences and mask
        the read-side decoding bug this test targets). This isolates the
        test to the encoding="utf-8-sig" pin in read_screen_file.
        """
        screen_path = tmp_path / "non-ascii.json"
        screen_json = json.dumps(
            {
                "name": "non-ascii-screen",
                "screen_type": "notebook",
                "config": {
                    "cells": [{"type": "markdown", "content": NON_ASCII_CONTENT}]
                },
            },
            indent=2,
            ensure_ascii=False,
        )
        screen_path.write_bytes(screen_json.encode("utf-8"))

        script = textwrap.dedent(
            """
            import os
            import sys

            assert sys.flags.utf8_mode == 0, "test setup failed: utf8_mode should be 0"

            from micromegas.cli.screens import read_screen_file

            screen_path = os.environ["TEST_SCREEN_PATH"]
            result = read_screen_file(screen_path)
            loaded_content = result["config"]["cells"][0]["content"]
            sys.stdout.buffer.write(loaded_content.encode("utf-8"))
            """
        )
        env = dict(os.environ)
        env["LC_ALL"] = "C"
        env["PYTHONUTF8"] = "0"
        env["TEST_SCREEN_PATH"] = str(screen_path)

        proc = subprocess.run(
            [sys.executable, "-c", script],
            env=env,
            capture_output=True,
        )
        assert proc.returncode == 0, proc.stderr.decode("utf-8", errors="replace")
        result = proc.stdout.decode("utf-8")
        assert result == NON_ASCII_CONTENT
