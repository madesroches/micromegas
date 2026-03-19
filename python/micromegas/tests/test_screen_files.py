"""Unit tests for screen file I/O and plan logic."""

import json
import os
import tempfile

import pytest

from micromegas.cli.screens import (
    compute_plan,
    list_local_screens,
    read_screen_file,
    server_screen_to_file,
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
        assert set(result.keys()) == {"name", "screen_type", "config"}


class TestListLocalScreens:
    def test_lists_screens(self, tmp_path):
        os.chdir(tmp_path)
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
        screens = list_local_screens()
        assert set(screens.keys()) == {"notebook-a", "notebook-b"}


class TestComputePlan:
    def _make_client(self, server_screens):
        class FakeClient:
            def list_screens(self):
                return server_screens

        return FakeClient()

    def test_create(self, tmp_path):
        os.chdir(tmp_path)
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

    def test_delete_tracked(self, tmp_path):
        os.chdir(tmp_path)
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

    def test_no_delete_different_owner(self, tmp_path):
        os.chdir(tmp_path)
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

    def test_update_modified(self, tmp_path):
        os.chdir(tmp_path)
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
        assert updates == ["my-screen"]

    def test_unchanged(self, tmp_path):
        os.chdir(tmp_path)
        screen_data = {
            "name": "stable-screen",
            "screen_type": "notebook",
            "config": {"cells": []},
        }
        write_screen_file("stable-screen.json", screen_data)
        config = {
            "managed_by": "https://github.com/org/repo/tree/main/screens",
            "server": "http://localhost",
        }
        server_screens = [
            {
                "name": "stable-screen",
                "screen_type": "notebook",
                "config": {"cells": []},
                "managed_by": "https://github.com/org/repo/tree/main/screens",
            }
        ]
        client = self._make_client(server_screens)
        creates, updates, deletes, unchanged, untracked = compute_plan(config, client)
        assert unchanged == ["stable-screen"]
