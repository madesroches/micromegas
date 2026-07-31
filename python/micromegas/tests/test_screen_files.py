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
    cmd_apply,
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
        screens, unreadable, invalid_names = list_local_screens()
        assert set(screens.keys()) == {"notebook-a", "notebook-b"}
        assert unreadable == set()
        assert invalid_names == set()

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

        screens, unreadable, invalid_names = list_local_screens()
        assert set(screens.keys()) == {"ok"}
        assert unreadable == {"bad", "broken"}
        assert invalid_names == set()

    def test_invalid_but_parsed_file_reports_known_name(self, tmp_path, monkeypatch):
        """A file that parses as JSON but fails schema validation (missing a
        required field) has a known identity -- its `name`, if present,
        surfaces in `invalid_names`, distinct from `unreadable` (reserved for
        files whose identity can't be determined at all). A file with no
        `name` field contributes to neither -- its identity truly is
        unknown, but that's a local-authoring mistake unrelated to any other
        screen."""
        monkeypatch.chdir(tmp_path)
        with open("micromegas-screens.json", "w") as f:
            json.dump({"managed_by": "test", "server": "http://localhost"}, f)
        with open("half-written.json", "w") as f:
            json.dump({"name": "half-written", "screen_type": "notebook"}, f)
        with open("schema.json", "w") as f:
            json.dump({"$schema": "x"}, f)

        screens, unreadable, invalid_names = list_local_screens()
        assert screens == {}
        assert unreadable == set()
        assert invalid_names == {"half-written"}

    def test_non_string_name_does_not_crash(self, tmp_path, monkeypatch):
        """A `name` field that parses as JSON but isn't a string (e.g. an
        object) must not crash the scan with `TypeError: unhashable type`.
        Such a file fails schema validation and, since its `name` isn't a
        usable string, contributes to neither `invalid_names` nor
        `screens`."""
        monkeypatch.chdir(tmp_path)
        with open("micromegas-screens.json", "w") as f:
            json.dump({"managed_by": "test", "server": "http://localhost"}, f)
        with open("bad-name.json", "w") as f:
            json.dump({"name": {"a": 1}, "screen_type": "notebook", "config": {}}, f)

        screens, unreadable, invalid_names = list_local_screens()
        assert screens == {}
        assert unreadable == set()
        assert invalid_names == set()

    def test_scalar_top_level_json_does_not_crash(self, tmp_path, monkeypatch):
        """A file whose top-level JSON value is a scalar (e.g. `null`) parses
        fine as JSON but isn't a dict, so the required-field membership check
        must not crash with `TypeError: argument of type 'NoneType' is not
        iterable`. It fails schema validation like any other malformed file
        and, having no `name` field to extract, contributes to neither
        `invalid_names` nor `screens`."""
        monkeypatch.chdir(tmp_path)
        with open("micromegas-screens.json", "w") as f:
            json.dump({"managed_by": "test", "server": "http://localhost"}, f)
        with open("null.json", "w") as f:
            f.write("null")

        screens, unreadable, invalid_names = list_local_screens()
        assert screens == {}
        assert unreadable == set()
        assert invalid_names == set()


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

    def test_unreadable_file_with_mismatched_stem_still_skips_deletes(
        self, tmp_path, monkeypatch
    ):
        """Regression test: `unreadable` is keyed by file *stem*, but nothing
        enforces that a file's stem matches its internal `name` field. A file
        named dashboard.json whose real (unreadable) `name` is
        "prod-dashboard" must not let "prod-dashboard" slip through as a
        delete just because "prod-dashboard" != "dashboard" (the stem).
        compute_plan must skip delete computation entirely whenever any local
        file is unreadable, regardless of stem/name matching."""
        monkeypatch.chdir(tmp_path)
        managed_by = "https://github.com/org/repo/tree/main/screens"
        # File stem is "dashboard", but its (unreadable) internal name field
        # is "prod-dashboard" -- nothing enforces the two must match.
        with open("dashboard.json", "wb") as f:
            f.write(
                '{"name": "prod-dashboard", "screen_type": "notebook", '
                '"config": {"x": "caf\xe9"}}'.encode("latin-1")
            )

        config = {"managed_by": managed_by, "server": "http://localhost"}
        server_screens = [
            {
                "name": "prod-dashboard",
                "screen_type": "notebook",
                "config": {},
                "managed_by": managed_by,
            }
        ]
        client = self._make_client(server_screens)
        creates, updates, deletes, unchanged, untracked = compute_plan(config, client)
        assert deletes == []

    def test_unrelated_invalid_json_does_not_block_delete(self, tmp_path, monkeypatch):
        """A stray local JSON file that isn't a screen at all -- it parses
        fine but fails schema validation (no `name`/`screen_type`/`config`)
        -- must not suppress deletion of an unrelated, cleanly-removed
        screen. Only files whose identity is genuinely undeterminable (can't
        even be parsed) warrant the conservative repo-wide skip."""
        monkeypatch.chdir(tmp_path)
        managed_by = "https://github.com/org/repo/tree/main/screens"
        with open("schema.json", "w") as f:
            json.dump({"$schema": "x"}, f)

        config = {"managed_by": managed_by, "server": "http://localhost"}
        server_screens = [
            {
                "name": "removed-locally",
                "screen_type": "notebook",
                "config": {},
                "managed_by": managed_by,
            }
        ]
        client = self._make_client(server_screens)
        creates, updates, deletes, unchanged, untracked = compute_plan(config, client)
        assert deletes == ["removed-locally"]

    def test_invalid_file_with_known_name_protects_only_itself(
        self, tmp_path, monkeypatch
    ):
        """A local file that parses as JSON but fails schema validation
        (e.g. missing `config`) still has a known `name` field -- that
        specific name should be protected from deletion, without blocking
        deletes for unrelated, cleanly-removed screens."""
        monkeypatch.chdir(tmp_path)
        managed_by = "https://github.com/org/repo/tree/main/screens"
        with open("half-written.json", "w") as f:
            json.dump({"name": "half-written", "screen_type": "notebook"}, f)

        config = {"managed_by": managed_by, "server": "http://localhost"}
        server_screens = [
            {
                "name": "half-written",
                "screen_type": "notebook",
                "config": {},
                "managed_by": managed_by,
            },
            {
                "name": "removed-locally",
                "screen_type": "notebook",
                "config": {},
                "managed_by": managed_by,
            },
        ]
        client = self._make_client(server_screens)
        creates, updates, deletes, unchanged, untracked = compute_plan(config, client)
        assert deletes == ["removed-locally"]
        assert "half-written" not in deletes

    def test_named_subset_mode_no_skip_warning(self, tmp_path, monkeypatch, capsys):
        """`--names` mode never computes deletes (see the `if not names`
        guard), so the "skipping delete computation" warning must not fire
        even when genuinely-unreadable files are present."""
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
        with open("broken.json", "w") as f:
            f.write("{ not json")

        config = {"managed_by": managed_by, "server": "http://localhost"}
        client = self._make_client([])
        compute_plan(config, client, names=["ok"])

        captured = capsys.readouterr()
        assert "skipping delete computation" not in captured.err


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

    def test_invalid_json_local_file_not_overwritten(self, tmp_path, monkeypatch):
        """cmd_pull must not silently clobber a local file whose JSON fails to
        parse, even when the server returns fresh content for that name --
        it should skip/warn instead, same as the undecodable-bytes case."""
        monkeypatch.chdir(tmp_path)
        with open("micromegas-screens.json", "w") as f:
            json.dump({"managed_by": "test", "server": "http://localhost"}, f)
        original_bytes = b"{ not json -- my precious local edits"
        with open("broken.json", "wb") as f:
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
            names = ["broken"]

        cmd_pull(Args())

        assert Path("broken.json").read_bytes() == original_bytes


class TestCmdApply:
    def test_unreadable_warning_printed_once(self, tmp_path, monkeypatch, capsys):
        """cmd_apply must scan the directory once per invocation: compute_plan
        used to re-scan (via list_local_screens) and cmd_apply scanned again
        afterwards, so each unreadable-file warning printed twice in a single
        `apply` run. It must now print exactly once.

        A valid `ok.json` screen (absent from the server) is included so that
        `creates` is non-empty and `cmd_apply` proceeds past its "No changes"
        early return into the create/update/delete code path that previously
        re-scanned the directory -- without it, the double-scan bug this test
        guards against is never reached and the test would pass regardless of
        whether the bug is present."""
        monkeypatch.chdir(tmp_path)
        managed_by = "https://github.com/org/repo/tree/main/screens"
        with open("micromegas-screens.json", "w") as f:
            json.dump({"managed_by": managed_by, "server": "http://localhost"}, f)
        with open("broken.json", "w") as f:
            f.write("{ not json")
        with open("ok.json", "w") as f:
            json.dump(
                {"name": "ok", "screen_type": "notebook", "config": {"cells": []}}, f
            )

        class FakeClient:
            def list_screens(self):
                return []

            def create_screen(self, name, screen_type, config, managed_by, folder_path):
                pass

        monkeypatch.setattr(screens_module, "make_client", lambda config: FakeClient())

        class Args:
            names = []
            auto_approve = True
            color = False

        cmd_apply(Args())

        captured = capsys.readouterr()
        assert captured.err.count("Warning: skipping broken.json") == 1


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

    def test_non_ascii_content_rendered_literally(self):
        """format_screen_diff must render non-ASCII content (em dash,
        accented characters, CJK) literally, not as \\uXXXX escapes -- covers
        the ensure_ascii=False fix for the diff's json.dumps calls. In-process
        is sufficient here (unlike the locale-forcing tests below) since
        format_screen_diff only does in-memory json.dumps, no file I/O whose
        encoding could depend on the process locale."""
        server = {"name": "a", "screen_type": "notebook", "config": {"x": "old"}}
        local = {
            "name": "a",
            "screen_type": "notebook",
            "config": {"x": NON_ASCII_CONTENT},
        }
        result = format_screen_diff(local, server, use_color=False)
        assert NON_ASCII_CONTENT in result
        assert "\\u" not in result


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

    def test_write_survives_non_utf8_locale(self, tmp_path):
        """Regression test for issue #1399: write_screen_file must be able to
        write non-ASCII content without raising, and must write it as literal
        UTF-8 bytes (not ensure_ascii-style \\uXXXX escapes), even when the
        process's locale-preferred encoding is not UTF-8.

        Complements test_read_survives_non_utf8_locale above, whose own
        docstring explains it deliberately bypasses write_screen_file (writing
        raw UTF-8 bytes directly) to isolate coverage to the read-side
        encoding="utf-8-sig" pin. This test exercises the write side instead:
        write_screen_file's encoding="utf-8" + ensure_ascii=False. Content is
        embedded as an ascii()-escaped literal so no non-ASCII bytes need to
        travel through argv or os.environ, matching the technique used for
        the stdin case in test_query.py.

        Verified empirically: under LC_ALL=C PYTHONUTF8=0, the pre-fix
        write_screen_file (plain open(path, "w"), no encoding=) raises
        UnicodeEncodeError on this content, since ensure_ascii=False content
        containing e.g. an em dash cannot be encoded by the locale's ASCII
        default.
        """
        screen_path = tmp_path / "non-ascii-write.json"
        script = textwrap.dedent(
            """
            import os
            import sys

            assert sys.flags.utf8_mode == 0, "test setup failed: utf8_mode should be 0"

            from micromegas.cli.screens import write_screen_file

            screen_path = os.environ["TEST_SCREEN_PATH"]
            content = {content_literal}
            write_screen_file(
                screen_path,
                {{
                    "name": "non-ascii-screen",
                    "screen_type": "notebook",
                    "config": {{"cells": [{{"type": "markdown", "content": content}}]}},
                }},
            )
            """
        ).format(content_literal=ascii(NON_ASCII_CONTENT))
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

        raw_bytes = screen_path.read_bytes()
        # Content must be present as literal UTF-8 bytes, not \uXXXX escapes.
        assert NON_ASCII_CONTENT.encode("utf-8") in raw_bytes
        assert b"\\u" not in raw_bytes
        decoded = json.loads(raw_bytes.decode("utf-8"))
        assert decoded["config"]["cells"][0]["content"] == NON_ASCII_CONTENT
