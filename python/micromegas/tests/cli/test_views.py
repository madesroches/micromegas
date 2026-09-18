"""Unit tests for micromegas-views: file parsing, canonicalization, and the plan/apply/pull/
list/show commands, all driven against a fake `.query(sql)` client -- no live DB or service,
matching `CONTRIBUTING.md`'s rule that a live-service test only pins a bug witnessed in the
wild.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import textwrap
from pathlib import Path

import pandas as pd
import pyarrow
import pytest

import micromegas.cli.views as views_module
from micromegas.cli.config import ProfileError
from micromegas.cli.state_sync import confirm_apply
from micromegas.cli.views import (
    canonical_ddl,
    cmd_apply,
    cmd_list,
    cmd_plan,
    cmd_pull,
    cmd_show,
    compute_plan,
    format_plan,
    list_local_definitions,
    parse_local_definition,
    with_or_replace,
)

NON_ASCII_CONTENT = "em dash —, accented café, CJK 日本語"

_DROP_NAME_RE = re.compile(
    r"DROP\s+MATERIALIZED\s+VIEW\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)", re.IGNORECASE
)


def _ddl_name(sql):
    match = views_module._HEADER_RE.search(sql)
    if match:
        return match.group("name")
    return _DROP_NAME_RE.search(sql).group("name")


def _is_select(sql):
    return sql.strip().upper().startswith("SELECT")


def _server_row(name, sql, update_group=1, updated_by="admin"):
    return {
        "view_set_name": name,
        "definition_sql": sql,
        "update_group": update_group,
        "updated_at": pd.Timestamp("2024-01-01T00:00:00Z"),
        "updated_by": updated_by,
    }


def _server_df(rows):
    columns = [
        "view_set_name",
        "definition_sql",
        "update_group",
        "updated_at",
        "updated_by",
    ]
    if not rows:
        return pd.DataFrame(columns=columns)
    return pd.DataFrame(list(rows))


class FakeClient:
    """`.query(sql)`: a SELECT returns the canned server-state DataFrame; anything else is
    DDL, recorded in `statements`, and answered with `ddl_results`/`ddl_errors` (by the
    view-set name parsed out of the statement).
    """

    def __init__(self, server_rows=(), ddl_results=None, ddl_errors=None):
        self.server_df = _server_df(server_rows)
        self.statements = []
        self.ddl_results = ddl_results or {}
        self.ddl_errors = ddl_errors or {}

    def query(self, sql):
        self.statements.append(sql)
        if _is_select(sql):
            return self.server_df
        name = _ddl_name(sql)
        if name in self.ddl_errors:
            raise self.ddl_errors[name]
        status = self.ddl_results.get(name, "created")
        return pd.DataFrame({"view_set_name": [name], "status": [status]})

    @property
    def ddl_statements(self):
        return [s for s in self.statements if not _is_select(s)]


class RaisingClient:
    """`.query` always raises -- for pinning that read_server_state's failure propagates
    out of every read-only cmd_* uncaught."""

    def query(self, sql):
        raise pyarrow.lib.ArrowException("boom")


def make_args(**overrides):
    defaults = {
        "profile": None,
        "dir": ".",
        "names": [],
        "prune": False,
        "auto_approve": False,
        "color": False,
        "format": "table",
    }
    defaults.update(overrides)
    return argparse.Namespace(**defaults)


# ---------------------------------------------------------------------------
# File parsing
# ---------------------------------------------------------------------------


class TestParseLocalDefinition:
    def test_leading_line_comment_does_not_defeat_name_scan(self, tmp_path):
        path = tmp_path / "foo.sql"
        path.write_text(
            "-- a leading comment\nCREATE MATERIALIZED VIEW foo WITH (x=1)\n",
            encoding="utf-8",
        )
        assert parse_local_definition(path).name == "foo"

    def test_leading_block_comment_does_not_defeat_name_scan(self, tmp_path):
        path = tmp_path / "foo.sql"
        path.write_text(
            "/* leading\nblock comment */\nCREATE MATERIALIZED VIEW foo WITH (x=1)\n",
            encoding="utf-8",
        )
        assert parse_local_definition(path).name == "foo"

    def test_stacked_mixed_leading_comments(self, tmp_path):
        path = tmp_path / "foo.sql"
        path.write_text(
            "-- one\n/* two */\n-- three\nCREATE OR REPLACE MATERIALIZED VIEW foo WITH (x=1)\n",
            encoding="utf-8",
        )
        assert parse_local_definition(path).name == "foo"

    def test_update_group_inside_dollar_body_does_not_disturb_name_scan(self, tmp_path):
        path = tmp_path / "foo.sql"
        path.write_text(
            "CREATE MATERIALIZED VIEW foo WITH (\n"
            "  extract_query = $$ SELECT 1 as update_group $$,\n"
            "  update_group = 1\n"
            ")\n",
            encoding="utf-8",
        )
        assert parse_local_definition(path).name == "foo"

    def test_round_trip_through_pull(self, tmp_path, monkeypatch):
        client = FakeClient(
            server_rows=[_server_row("foo", "CREATE MATERIALIZED VIEW foo WITH (x=1)")]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        # Named, since "foo" isn't local yet -- bare pull only refreshes names already
        # present in the directory (see TestCmdPull.test_bare_pull_refreshes_only_...).
        cmd_pull(make_args(dir=str(tmp_path), names=["foo"]))
        local_def = parse_local_definition(tmp_path / "foo.sql")
        assert local_def.name == "foo"
        assert local_def.text == "CREATE MATERIALIZED VIEW foo WITH (x=1)\n"


class TestListLocalDefinitions:
    def test_decode_failure_is_skipped_and_protects_stem(self, tmp_path):
        (tmp_path / "broken.sql").write_bytes(
            "CREATE MATERIALIZED VIEW broken WITH (x='caf\xe9')".encode("latin-1")
        )
        definitions, protected = list_local_definitions(tmp_path)
        assert "broken" not in definitions
        assert protected == {"broken"}

    def test_unparseable_header_is_skipped_and_protects_only_its_stem(self, tmp_path):
        (tmp_path / "not_ddl.sql").write_text("SELECT 1\n", encoding="utf-8")
        definitions, protected = list_local_definitions(tmp_path)
        assert "not_ddl" not in definitions
        assert protected == {"not_ddl"}

    def test_name_mismatch_protects_both_stem_and_declared_name(self, tmp_path):
        (tmp_path / "wrong_file.sql").write_text(
            "CREATE MATERIALIZED VIEW actual_name WITH (x=1)\n", encoding="utf-8"
        )
        definitions, protected = list_local_definitions(tmp_path)
        assert "wrong_file" not in definitions
        assert "actual_name" not in definitions
        assert protected == {"wrong_file", "actual_name"}

    def test_valid_files_are_collected(self, tmp_path):
        (tmp_path / "foo.sql").write_text(
            "CREATE MATERIALIZED VIEW foo WITH (x=1)\n", encoding="utf-8"
        )
        definitions, protected = list_local_definitions(tmp_path)
        assert set(definitions) == {"foo"}
        assert protected == set()


# ---------------------------------------------------------------------------
# Canonicalization
# ---------------------------------------------------------------------------


class TestCanonicalDdl:
    def test_or_replace_and_plain_create_canonicalize_equal(self):
        plain = "CREATE MATERIALIZED VIEW foo WITH (x=1)"
        or_replace = "CREATE OR REPLACE MATERIALIZED VIEW foo WITH (x=1)"
        assert canonical_ddl(plain) == canonical_ddl(or_replace)

    def test_trailing_semicolon_newline_and_crlf_canonicalize_equal(self):
        base = "CREATE MATERIALIZED VIEW foo WITH (x=1)"
        variants = [
            base,
            base + ";",
            base + "\n",
            base + ";\n",
            base.replace("\n", "\r\n") + ";\r\n",
        ]
        assert len({canonical_ddl(v) for v in variants}) == 1

    def test_differing_interior_whitespace_canonicalizes_unequal(self):
        a = "CREATE MATERIALIZED VIEW foo WITH (x = 1)"
        b = "CREATE MATERIALIZED VIEW foo WITH (x  =  1)"
        assert canonical_ddl(a) != canonical_ddl(b)

    def test_with_or_replace_is_idempotent_and_inverse_of_canonicalization(self):
        text = "CREATE MATERIALIZED VIEW foo WITH (x=1)"
        once = with_or_replace(text)
        twice = with_or_replace(once)
        assert once == twice
        assert once.startswith("CREATE OR REPLACE MATERIALIZED VIEW foo")
        assert canonical_ddl(once) == canonical_ddl(text)


# ---------------------------------------------------------------------------
# compute_plan / _compute_drops
# ---------------------------------------------------------------------------


class TestComputePlan:
    def test_classifies_each_case_of_the_local_x_server_table(self):
        definitions = {
            "create_me": views_module.LocalDefinition(
                "create_me",
                "CREATE MATERIALIZED VIEW create_me WITH (x=1)",
                Path("create_me.sql"),
            ),
            "update_me": views_module.LocalDefinition(
                "update_me",
                "CREATE MATERIALIZED VIEW update_me WITH (x=2)",
                Path("update_me.sql"),
            ),
            "same_me": views_module.LocalDefinition(
                "same_me",
                "CREATE MATERIALIZED VIEW same_me WITH (x=3)",
                Path("same_me.sql"),
            ),
        }
        local_scan = (definitions, set())
        server_state = _server_df(
            [
                _server_row(
                    "update_me", "CREATE MATERIALIZED VIEW update_me WITH (x=99)"
                ),
                _server_row("same_me", "CREATE MATERIALIZED VIEW same_me WITH (x=3)"),
                _server_row(
                    "only_on_server",
                    "CREATE MATERIALIZED VIEW only_on_server WITH (x=4)",
                ),
            ]
        )
        creates, updates, unchanged, server_only = compute_plan(
            server_state, local_scan
        )
        assert creates == ["create_me"]
        assert [u[0] for u in updates] == ["update_me"]
        assert unchanged == ["same_me"]
        assert server_only == ["only_on_server"]

    def test_server_only_ignores_prune_and_is_always_full(self):
        definitions = {}
        local_scan = (definitions, set())
        server_state = _server_df(
            [_server_row("orphan", "CREATE MATERIALIZED VIEW orphan WITH (x=1)")]
        )
        _creates, _updates, _unchanged, server_only = compute_plan(
            server_state, local_scan
        )
        assert server_only == ["orphan"]

    def test_names_narrows_classification_but_not_server_only(self):
        definitions = {
            "a": views_module.LocalDefinition(
                "a", "CREATE MATERIALIZED VIEW a WITH (x=1)", Path("a.sql")
            ),
            "b": views_module.LocalDefinition(
                "b", "CREATE MATERIALIZED VIEW b WITH (x=1)", Path("b.sql")
            ),
        }
        local_scan = (definitions, set())
        server_state = _server_df([])
        creates, _updates, _unchanged, server_only = compute_plan(
            server_state, local_scan, names=["a"]
        )
        assert creates == ["a"]
        assert server_only == []


class TestComputeDrops:
    def test_empty_when_prune_unset_regardless_of_names(self):
        assert views_module._compute_drops(["a", "b"], set(), None, False) == []
        assert views_module._compute_drops(["a", "b"], set(), ["a"], False) == []

    def test_equals_unprotected_server_only_when_prune_set_with_no_names(self):
        assert views_module._compute_drops(["a", "b", "c"], {"b"}, None, True) == [
            "a",
            "c",
        ]

    def test_limited_to_named_subset_when_prune_and_names_both_set(self):
        assert views_module._compute_drops(["a", "b", "c"], set(), ["a"], True) == ["a"]


# ---------------------------------------------------------------------------
# format_plan
# ---------------------------------------------------------------------------


class TestFormatPlan:
    def test_footer_mentions_prune_when_nothing_was_dropped(self):
        text = format_plan([], [], [], ["orphan"], [])
        assert "Server-only view sets (use 'pull' to adopt, '--prune' to drop):" in text
        assert "  ? orphan" in text

    def test_drops_render_as_actions_and_shrink_the_footer(self):
        text = format_plan(
            [], [], [], ["dropped_one", "protected_one"], ["dropped_one"]
        )
        assert "  - drop: dropped_one" in text
        assert "Server-only view sets (use 'pull' to adopt):" in text
        footer = text.split("Server-only view sets")[1]
        assert "protected_one" in footer
        assert "dropped_one" not in footer

    def test_no_changes_message(self):
        text = format_plan([], [], ["a", "b"], [], [])
        assert text == "No changes. 2 unchanged."


# ---------------------------------------------------------------------------
# plan
# ---------------------------------------------------------------------------


class TestCmdPlan:
    def test_diff_for_update_contains_no_or_replace_line(
        self, tmp_path, monkeypatch, capsys
    ):
        (tmp_path / "foo.sql").write_text(
            "CREATE OR REPLACE MATERIALIZED VIEW foo WITH (x=2)\n", encoding="utf-8"
        )
        client = FakeClient(
            server_rows=[_server_row("foo", "CREATE MATERIALIZED VIEW foo WITH (x=1)")]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        cmd_plan(make_args(dir=str(tmp_path)))
        out = capsys.readouterr().out
        assert "OR REPLACE" not in out.upper()

    def test_unknown_name_is_reported_and_exits_nonzero(
        self, tmp_path, monkeypatch, capsys
    ):
        client = FakeClient(server_rows=[])
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        with pytest.raises(SystemExit) as exc_info:
            cmd_plan(make_args(dir=str(tmp_path), names=["missing"]))
        assert exc_info.value.code == 1
        assert "missing" in capsys.readouterr().err

    def test_pending_changes_do_not_move_the_exit_code(self, tmp_path, monkeypatch):
        (tmp_path / "foo.sql").write_text(
            "CREATE MATERIALIZED VIEW foo WITH (x=1)\n", encoding="utf-8"
        )
        client = FakeClient(server_rows=[])
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        cmd_plan(
            make_args(dir=str(tmp_path))
        )  # a pending create must not raise SystemExit

    def test_skipped_local_file_moves_the_exit_code(self, tmp_path, monkeypatch):
        (tmp_path / "broken.sql").write_text("not ddl at all\n", encoding="utf-8")
        client = FakeClient(server_rows=[])
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        with pytest.raises(SystemExit) as exc_info:
            cmd_plan(make_args(dir=str(tmp_path)))
        assert exc_info.value.code == 1

    def test_unparseable_header_does_not_suppress_unrelated_prune_drop(
        self, tmp_path, monkeypatch, capsys
    ):
        (tmp_path / "not_ddl.sql").write_text("SELECT 1\n", encoding="utf-8")
        (tmp_path / "kept.sql").write_text(
            "CREATE MATERIALIZED VIEW kept WITH (x=1)\n", encoding="utf-8"
        )
        client = FakeClient(
            server_rows=[
                _server_row("kept", "CREATE MATERIALIZED VIEW kept WITH (x=1)"),
                _server_row(
                    "unrelated_server_only",
                    "CREATE MATERIALIZED VIEW unrelated_server_only WITH (x=1)",
                ),
            ]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        with pytest.raises(SystemExit) as exc_info:
            cmd_plan(make_args(dir=str(tmp_path), prune=True))
        assert (
            exc_info.value.code == 1
        )  # the skipped not_ddl.sql moves plan's exit code
        out = capsys.readouterr().out
        assert "  - drop: unrelated_server_only" in out

    def test_prune_against_empty_directory_errors_before_any_round_trip(
        self, tmp_path, monkeypatch, capsys
    ):
        client = FakeClient(
            server_rows=[
                _server_row("orphan", "CREATE MATERIALIZED VIEW orphan WITH (x=1)")
            ]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        with pytest.raises(SystemExit) as exc_info:
            cmd_plan(make_args(dir=str(tmp_path), prune=True))
        assert exc_info.value.code == 1
        assert "--prune refuses" in capsys.readouterr().err
        assert client.statements == []

    def test_arrow_exception_from_read_server_state_propagates_uncaught(
        self, tmp_path, monkeypatch
    ):
        monkeypatch.setattr(views_module, "make_client", lambda args: RaisingClient())
        with pytest.raises(pyarrow.lib.ArrowException):
            cmd_plan(make_args(dir=str(tmp_path)))


# ---------------------------------------------------------------------------
# apply
# ---------------------------------------------------------------------------


class TestCmdApply:
    def test_creates_and_updates_run_before_drops_in_name_order(
        self, tmp_path, monkeypatch
    ):
        (tmp_path / "b_create.sql").write_text(
            "CREATE MATERIALIZED VIEW b_create WITH (x=1)\n", encoding="utf-8"
        )
        (tmp_path / "a_create.sql").write_text(
            "CREATE MATERIALIZED VIEW a_create WITH (x=1)\n", encoding="utf-8"
        )
        (tmp_path / "z_update.sql").write_text(
            "CREATE MATERIALIZED VIEW z_update WITH (x=2)\n", encoding="utf-8"
        )
        client = FakeClient(
            server_rows=[
                _server_row("z_update", "CREATE MATERIALIZED VIEW z_update WITH (x=1)"),
                _server_row("drop_me", "CREATE MATERIALIZED VIEW drop_me WITH (x=1)"),
                _server_row(
                    "also_drop", "CREATE MATERIALIZED VIEW also_drop WITH (x=1)"
                ),
            ]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        cmd_apply(make_args(dir=str(tmp_path), prune=True, auto_approve=True))
        order = [_ddl_name(s) for s in client.ddl_statements]
        assert order == ["a_create", "b_create", "z_update", "also_drop", "drop_me"]

    def test_create_sends_plain_create_even_if_file_says_or_replace(
        self, tmp_path, monkeypatch
    ):
        (tmp_path / "foo.sql").write_text(
            "CREATE OR REPLACE MATERIALIZED VIEW foo WITH (x=1)\n", encoding="utf-8"
        )
        client = FakeClient(server_rows=[])
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        cmd_apply(make_args(dir=str(tmp_path), auto_approve=True))
        assert len(client.ddl_statements) == 1
        assert "OR REPLACE" not in client.ddl_statements[0].upper()

    def test_update_sends_or_replace(self, tmp_path, monkeypatch):
        (tmp_path / "foo.sql").write_text(
            "CREATE MATERIALIZED VIEW foo WITH (x=2)\n", encoding="utf-8"
        )
        client = FakeClient(
            server_rows=[_server_row("foo", "CREATE MATERIALIZED VIEW foo WITH (x=1)")]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        cmd_apply(make_args(dir=str(tmp_path), auto_approve=True))
        assert len(client.ddl_statements) == 1
        assert "OR REPLACE" in client.ddl_statements[0].upper()

    def test_failed_statements_are_counted_continue_and_exit_nonzero(
        self, tmp_path, monkeypatch, capsys
    ):
        (tmp_path / "will_404.sql").write_text(
            "CREATE MATERIALIZED VIEW will_404 WITH (x=1)\n", encoding="utf-8"
        )
        (tmp_path / "will_exist.sql").write_text(
            "CREATE MATERIALIZED VIEW will_exist WITH (x=1)\n", encoding="utf-8"
        )
        client = FakeClient(
            server_rows=[],
            ddl_errors={
                "will_404": pyarrow.lib.ArrowKeyError("not_found: will_404"),
                "will_exist": pyarrow.lib.ArrowException("already_exists: will_exist"),
            },
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        with pytest.raises(SystemExit) as exc_info:
            cmd_apply(make_args(dir=str(tmp_path), auto_approve=True))
        assert exc_info.value.code == 1
        assert len(client.ddl_statements) == 2  # neither error aborted the other
        err = capsys.readouterr().err
        assert "will_404" in err and "will_exist" in err

    def test_auto_approve_skips_prompt(self, tmp_path, monkeypatch):
        (tmp_path / "foo.sql").write_text(
            "CREATE MATERIALIZED VIEW foo WITH (x=1)\n", encoding="utf-8"
        )
        client = FakeClient(server_rows=[])
        monkeypatch.setattr(views_module, "make_client", lambda args: client)

        def fail_input(prompt=""):
            raise AssertionError("input() must not be called with --auto-approve")

        monkeypatch.setattr("builtins.input", fail_input)
        cmd_apply(make_args(dir=str(tmp_path), auto_approve=True))
        assert len(client.ddl_statements) == 1

    def test_declined_prompt_applies_nothing_and_exits_1(self, tmp_path, monkeypatch):
        (tmp_path / "foo.sql").write_text(
            "CREATE MATERIALIZED VIEW foo WITH (x=1)\n", encoding="utf-8"
        )
        client = FakeClient(server_rows=[])
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        monkeypatch.setattr("builtins.input", lambda prompt="": "n")
        with pytest.raises(SystemExit) as exc_info:
            cmd_apply(make_args(dir=str(tmp_path), auto_approve=False))
        assert exc_info.value.code == 1
        assert client.ddl_statements == []

    def test_no_changes_skips_confirmation(self, tmp_path, monkeypatch, capsys):
        (tmp_path / "foo.sql").write_text(
            "CREATE MATERIALIZED VIEW foo WITH (x=1)\n", encoding="utf-8"
        )
        client = FakeClient(
            server_rows=[_server_row("foo", "CREATE MATERIALIZED VIEW foo WITH (x=1)")]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)

        def fail_input(prompt=""):
            raise AssertionError("input() must not be called when there are no changes")

        monkeypatch.setattr("builtins.input", fail_input)
        cmd_apply(make_args(dir=str(tmp_path), auto_approve=False))
        assert "No changes. 1 unchanged." in capsys.readouterr().out

    def test_apply_with_skipped_file_still_applies_parseable_files_and_exits_nonzero(
        self, tmp_path, monkeypatch
    ):
        (tmp_path / "broken.sql").write_text("not ddl at all\n", encoding="utf-8")
        (tmp_path / "ok.sql").write_text(
            "CREATE MATERIALIZED VIEW ok WITH (x=1)\n", encoding="utf-8"
        )
        client = FakeClient(server_rows=[])
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        with pytest.raises(SystemExit) as exc_info:
            cmd_apply(make_args(dir=str(tmp_path), auto_approve=True))
        assert exc_info.value.code == 1
        assert len(client.ddl_statements) == 1
        assert _ddl_name(client.ddl_statements[0]) == "ok"

    def test_prune_against_empty_directory_errors(self, tmp_path, monkeypatch, capsys):
        client = FakeClient(
            server_rows=[
                _server_row("orphan", "CREATE MATERIALIZED VIEW orphan WITH (x=1)")
            ]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        with pytest.raises(SystemExit) as exc_info:
            cmd_apply(make_args(dir=str(tmp_path), prune=True, auto_approve=True))
        assert exc_info.value.code == 1
        assert "--prune refuses" in capsys.readouterr().err


class TestConfirmApply:
    def test_returns_false_on_declined_prompt_without_raising(self, monkeypatch):
        monkeypatch.setattr("builtins.input", lambda prompt="": "n")
        assert confirm_apply(auto_approve=False) is False

    def test_auto_approve_returns_true_without_prompting(self, monkeypatch):
        def fail_input(prompt=""):
            raise AssertionError("must not prompt")

        monkeypatch.setattr("builtins.input", fail_input)
        assert confirm_apply(auto_approve=True) is True


# ---------------------------------------------------------------------------
# pull
# ---------------------------------------------------------------------------


class TestCmdPull:
    def test_bare_pull_refreshes_only_parseable_local_names(
        self, tmp_path, monkeypatch
    ):
        (tmp_path / "good.sql").write_text(
            "CREATE MATERIALIZED VIEW good WITH (x=1)\n", encoding="utf-8"
        )
        (tmp_path / "broken.sql").write_text("not ddl\n", encoding="utf-8")
        client = FakeClient(
            server_rows=[
                _server_row("good", "CREATE MATERIALIZED VIEW good WITH (x=9)"),
                _server_row("broken", "CREATE MATERIALIZED VIEW broken WITH (x=9)"),
            ]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        cmd_pull(make_args(dir=str(tmp_path)))
        assert (tmp_path / "good.sql").read_text(encoding="utf-8") == (
            "CREATE MATERIALIZED VIEW good WITH (x=9)\n"
        )
        assert (tmp_path / "broken.sql").read_text(encoding="utf-8") == "not ddl\n"

    def test_pull_unchanged_leaves_file_byte_identical(
        self, tmp_path, monkeypatch, capsys
    ):
        rendered = "CREATE MATERIALIZED VIEW foo WITH (x=1)\n"
        target = tmp_path / "foo.sql"
        target.write_text(rendered, encoding="utf-8")
        before_mtime = target.stat().st_mtime_ns
        client = FakeClient(
            server_rows=[_server_row("foo", "CREATE MATERIALIZED VIEW foo WITH (x=1)")]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        cmd_pull(make_args(dir=str(tmp_path)))
        assert target.stat().st_mtime_ns == before_mtime
        assert target.read_text(encoding="utf-8") == rendered
        assert "Pull complete: 0 updated, 1 unchanged." in capsys.readouterr().out

    def test_pull_strips_or_replace_from_server_row(self, tmp_path, monkeypatch):
        client = FakeClient(
            server_rows=[
                _server_row("foo", "CREATE OR REPLACE MATERIALIZED VIEW foo WITH (x=1)")
            ]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        cmd_pull(make_args(dir=str(tmp_path), names=["foo"]))
        text = (tmp_path / "foo.sql").read_text(encoding="utf-8")
        assert text == "CREATE MATERIALIZED VIEW foo WITH (x=1)\n"

    def test_named_pull_adopts_server_only_name_into_new_file(
        self, tmp_path, monkeypatch
    ):
        client = FakeClient(
            server_rows=[
                _server_row("adopted", "CREATE MATERIALIZED VIEW adopted WITH (x=1)")
            ]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        cmd_pull(make_args(dir=str(tmp_path), names=["adopted"]))
        assert (tmp_path / "adopted.sql").exists()

    def test_named_pull_absent_from_both_sides_is_an_error(
        self, tmp_path, monkeypatch, capsys
    ):
        client = FakeClient(server_rows=[])
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        with pytest.raises(SystemExit) as exc_info:
            cmd_pull(make_args(dir=str(tmp_path), names=["missing"]))
        assert exc_info.value.code == 1
        assert "missing" in capsys.readouterr().err

    def test_local_only_name_is_warned_and_skipped_without_affecting_exit_code(
        self, tmp_path, monkeypatch, capsys
    ):
        (tmp_path / "local_only.sql").write_text(
            "CREATE MATERIALIZED VIEW local_only WITH (x=1)\n", encoding="utf-8"
        )
        client = FakeClient(server_rows=[])
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        cmd_pull(make_args(dir=str(tmp_path)))  # must not raise
        assert "local_only" in capsys.readouterr().err

    def test_named_pull_overwrites_undecodable_target(
        self, tmp_path, monkeypatch, capsys
    ):
        (tmp_path / "bad.sql").write_bytes(
            "CREATE MATERIALIZED VIEW bad WITH (x='caf\xe9')".encode("latin-1")
        )
        client = FakeClient(
            server_rows=[_server_row("bad", "CREATE MATERIALIZED VIEW bad WITH (x=1)")]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        cmd_pull(make_args(dir=str(tmp_path), names=["bad"]))
        assert (tmp_path / "bad.sql").read_text(encoding="utf-8") == (
            "CREATE MATERIALIZED VIEW bad WITH (x=1)\n"
        )
        assert "overwriting" in capsys.readouterr().err

    def test_named_pull_overwrites_target_with_unparseable_header(
        self, tmp_path, monkeypatch, capsys
    ):
        (tmp_path / "bad.sql").write_text("not ddl at all\n", encoding="utf-8")
        client = FakeClient(
            server_rows=[_server_row("bad", "CREATE MATERIALIZED VIEW bad WITH (x=1)")]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        cmd_pull(make_args(dir=str(tmp_path), names=["bad"]))
        assert (tmp_path / "bad.sql").read_text(encoding="utf-8") == (
            "CREATE MATERIALIZED VIEW bad WITH (x=1)\n"
        )
        assert "overwriting" in capsys.readouterr().err

    def test_arrow_exception_from_read_server_state_propagates_uncaught(
        self, tmp_path, monkeypatch
    ):
        monkeypatch.setattr(views_module, "make_client", lambda args: RaisingClient())
        with pytest.raises(pyarrow.lib.ArrowException):
            cmd_pull(make_args(dir=str(tmp_path)))


# ---------------------------------------------------------------------------
# list
# ---------------------------------------------------------------------------


class TestCmdList:
    def test_table_and_json_formats(self, tmp_path, monkeypatch, capsys):
        (tmp_path / "foo.sql").write_text(
            "CREATE MATERIALIZED VIEW foo WITH (x=1)\n", encoding="utf-8"
        )
        client = FakeClient(
            server_rows=[
                _server_row("foo", "CREATE MATERIALIZED VIEW foo WITH (x=1)"),
                _server_row("bar", "CREATE MATERIALIZED VIEW bar WITH (x=9)"),
            ]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)

        cmd_list(make_args(dir=str(tmp_path), format="table"))
        table_out = capsys.readouterr().out
        assert "foo" in table_out and "unchanged" in table_out
        assert "bar" in table_out and "server-only" in table_out

        cmd_list(make_args(dir=str(tmp_path), format="json"))
        rows = json.loads(capsys.readouterr().out)
        assert {row["name"]: row["status"] for row in rows} == {
            "foo": "unchanged",
            "bar": "server-only",
        }

    def test_create_row_does_not_upcast_update_group_to_float(
        self, tmp_path, monkeypatch, capsys
    ):
        # "new" is local-only (a create row): the left merge leaves its update_group null,
        # which must not upcast the whole int column to float64 and turn "foo"'s
        # update_group into e.g. 4000.0.
        (tmp_path / "foo.sql").write_text(
            "CREATE MATERIALIZED VIEW foo WITH (x=1)\n", encoding="utf-8"
        )
        (tmp_path / "new.sql").write_text(
            "CREATE MATERIALIZED VIEW new WITH (x=1)\n", encoding="utf-8"
        )
        client = FakeClient(
            server_rows=[
                _server_row(
                    "foo", "CREATE MATERIALIZED VIEW foo WITH (x=1)", update_group=4000
                )
            ]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)

        cmd_list(make_args(dir=str(tmp_path), format="table"))
        table_out = capsys.readouterr().out
        assert "4000" in table_out
        assert "4000.0" not in table_out

        cmd_list(make_args(dir=str(tmp_path), format="json"))
        rows = {row["name"]: row for row in json.loads(capsys.readouterr().out)}
        assert rows["foo"]["update_group"] == 4000
        assert isinstance(rows["foo"]["update_group"], int)
        assert rows["new"]["update_group"] is None

    def test_skipped_local_file_does_not_move_the_exit_code(
        self, tmp_path, monkeypatch, capsys
    ):
        (tmp_path / "broken.sql").write_text("not ddl at all\n", encoding="utf-8")
        client = FakeClient(server_rows=[])
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        cmd_list(make_args(dir=str(tmp_path)))  # must not raise
        assert "broken" in capsys.readouterr().err

    def test_arrow_exception_from_read_server_state_propagates_uncaught(
        self, tmp_path, monkeypatch
    ):
        monkeypatch.setattr(views_module, "make_client", lambda args: RaisingClient())
        with pytest.raises(pyarrow.lib.ArrowException):
            cmd_list(make_args(dir=str(tmp_path)))


# ---------------------------------------------------------------------------
# show
# ---------------------------------------------------------------------------


class TestCmdShow:
    def test_prints_stored_definition_verbatim(self, monkeypatch, capsys):
        client = FakeClient(
            server_rows=[
                _server_row("foo", "CREATE OR REPLACE MATERIALIZED VIEW foo WITH (x=1)")
            ]
        )
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        cmd_show(make_args(name="foo"))
        assert (
            "CREATE OR REPLACE MATERIALIZED VIEW foo WITH (x=1)"
            in capsys.readouterr().out
        )

    def test_unknown_name_exits_nonzero(self, monkeypatch, capsys):
        client = FakeClient(server_rows=[])
        monkeypatch.setattr(views_module, "make_client", lambda args: client)
        with pytest.raises(SystemExit) as exc_info:
            cmd_show(make_args(name="missing"))
        assert exc_info.value.code == 1
        assert "missing" in capsys.readouterr().err

    def test_arrow_exception_from_read_server_state_propagates_uncaught(
        self, monkeypatch
    ):
        monkeypatch.setattr(views_module, "make_client", lambda args: RaisingClient())
        with pytest.raises(pyarrow.lib.ArrowException):
            cmd_show(make_args(name="foo"))


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------


class TestMain:
    def test_rejects_missing_dir(self, monkeypatch, capsys, tmp_path):
        missing = tmp_path / "does-not-exist"
        monkeypatch.setattr(
            sys, "argv", ["micromegas-views", "list", "--dir", str(missing)]
        )
        with pytest.raises(SystemExit) as exc_info:
            views_module.main()
        assert exc_info.value.code == 1
        assert "not a directory" in capsys.readouterr().err

    def test_reports_admin_identity_hint_on_arrow_exception(self, monkeypatch, capsys):
        monkeypatch.setattr(views_module, "make_client", lambda args: RaisingClient())
        monkeypatch.setattr(sys, "argv", ["micromegas-views", "list"])
        with pytest.raises(SystemExit) as exc_info:
            views_module.main()
        assert exc_info.value.code == 1
        assert "admin identity" in capsys.readouterr().err

    def test_reports_profile_error_without_admin_hint(self, monkeypatch, capsys):
        def raise_profile_error(args):
            raise ProfileError("no profile selected")

        monkeypatch.setattr(views_module, "make_client", raise_profile_error)
        monkeypatch.setattr(sys, "argv", ["micromegas-views", "list"])
        with pytest.raises(SystemExit) as exc_info:
            views_module.main()
        assert exc_info.value.code == 1
        err = capsys.readouterr().err
        assert "no profile selected" in err
        assert "admin identity" not in err


# ---------------------------------------------------------------------------
# Non-ASCII / non-UTF-8-locale regression guard
# ---------------------------------------------------------------------------


class TestNonAsciiEncodingRegression:
    def test_pull_round_trip_survives_non_utf8_locale(self, tmp_path):
        # `-c <script>` puts the script text on the child's argv; under a forced C locale
        # the child decodes argv as ASCII, so any literal non-ASCII byte there is a Python
        # startup failure before our code even runs. ascii()-escaping keeps the whole
        # script text ASCII-only -- the same technique (and reason) as
        # test_screen_files.py's write-side non-UTF-8-locale regression test.
        server_sql = (
            "CREATE MATERIALIZED VIEW ascii_test WITH (\n"
            f"  extract_query = $$ SELECT '{NON_ASCII_CONTENT}' as x $$,\n"
            "  count_src_query = $$ SELECT 1 as count $$,\n"
            "  merge_partitions_query = $$ SELECT x FROM {source} $$,\n"
            "  update_group = 1,\n"
            "  time_column = 'time_bin'\n"
            ")"
        )
        script = textwrap.dedent("""
            import argparse
            import os
            import sys

            assert sys.flags.utf8_mode == 0, "test setup failed: utf8_mode should be 0"

            import pandas as pd
            import micromegas.cli.views as views_module

            server_sql = {server_sql_literal}
            server_df = pd.DataFrame([{{
                "view_set_name": "ascii_test",
                "definition_sql": server_sql,
                "update_group": 1,
                "updated_at": pd.Timestamp("2024-01-01T00:00:00Z"),
                "updated_by": "admin",
            }}])

            class FakeClient:
                def query(self, sql):
                    return server_df

            views_module.make_client = lambda args: FakeClient()
            args = argparse.Namespace(
                profile=None, dir=os.environ["TEST_DIR"], names=["ascii_test"]
            )
            views_module.cmd_pull(args)

            from pathlib import Path
            text = (Path(os.environ["TEST_DIR"]) / "ascii_test.sql").read_text(encoding="utf-8")
            sys.stdout.buffer.write(text.encode("utf-8"))
            """).format(server_sql_literal=ascii(server_sql))
        env = dict(os.environ)
        env["LC_ALL"] = "C"
        env["PYTHONUTF8"] = "0"
        env["TEST_DIR"] = str(tmp_path)

        proc = subprocess.run(
            [sys.executable, "-c", script], env=env, capture_output=True
        )
        assert proc.returncode == 0, proc.stderr.decode("utf-8", errors="replace")
        assert NON_ASCII_CONTENT in proc.stdout.decode("utf-8")

    def test_colorized_diff_prints_without_unicode_encode_error(self, tmp_path):
        local = f"CREATE MATERIALIZED VIEW foo WITH (x='{NON_ASCII_CONTENT}')"
        server = "CREATE MATERIALIZED VIEW foo WITH (x='old')"
        script = textwrap.dedent("""
            import sys

            assert sys.flags.utf8_mode == 0, "test setup failed: utf8_mode should be 0"
            sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")

            from micromegas.cli.views import format_plan

            updates = [("foo", {local_literal}, {server_literal})]
            print(format_plan([], updates, [], [], [], use_color=True))
            """).format(local_literal=ascii(local), server_literal=ascii(server))
        env = dict(os.environ)
        env["LC_ALL"] = "C"
        env["PYTHONUTF8"] = "0"

        proc = subprocess.run(
            [sys.executable, "-c", script], env=env, capture_output=True
        )
        assert proc.returncode == 0, proc.stderr.decode("utf-8", errors="replace")
