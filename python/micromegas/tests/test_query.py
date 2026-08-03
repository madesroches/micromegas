"""Regression tests for micromegas/cli/query.py's SQL source resolution.

These tests exercise the real `read_sql_source()` extracted from
`main()` under a forced non-UTF-8 locale (LC_ALL=C + PYTHONUTF8=0), the
same technique used in test_screen_files.py, to guard against issue
#1399: reading a SQL file (or stdin) with no explicit encoding falls
back to the platform's locale-preferred encoding and can mis-decode
non-ASCII content.
"""

import datetime
import json
import os
import subprocess
import sys
import textwrap

import pytest

from micromegas.cli import config
from micromegas.cli.query import main, parse_timestamp

NON_ASCII_SQL = "-- em dash —, accented café, CJK 日本語\nSELECT 1"


def test_read_sql_source_file_survives_non_utf8_locale(tmp_path):
    """--file <path> must be read as UTF-8 regardless of process locale."""
    sql_path = tmp_path / "query.sql"
    script = textwrap.dedent("""
        import argparse
        import os
        import sys

        assert sys.flags.utf8_mode == 0, "test setup failed: utf8_mode should be 0"

        from micromegas.cli import query

        sql_path = os.environ["TEST_SQL_PATH"]
        args = argparse.Namespace(file=sql_path, sql=None)
        sql = query.read_sql_source(args)
        sys.stdout.buffer.write(sql.encode("utf-8"))
        """)
    # Write the fixture file directly as UTF-8 bytes so its on-disk
    # encoding doesn't depend on this (parent) process's locale.
    sql_path.write_bytes(NON_ASCII_SQL.encode("utf-8"))

    env = dict(os.environ)
    env["LC_ALL"] = "C"
    env["PYTHONUTF8"] = "0"
    env["TEST_SQL_PATH"] = str(sql_path)

    proc = subprocess.run(
        [sys.executable, "-c", script],
        env=env,
        capture_output=True,
    )
    assert proc.returncode == 0, proc.stderr.decode("utf-8", errors="replace")
    assert proc.stdout.decode("utf-8") == NON_ASCII_SQL


def test_read_sql_source_stdin_survives_non_utf8_locale():
    """--file - (stdin) must be read as UTF-8 regardless of process locale."""
    # Embed the content as an ASCII-only \\uXXXX-escaped literal (via
    # ascii()) so no non-ASCII bytes travel through argv or os.environ,
    # only through the piped stdin bytes under test.
    script = textwrap.dedent("""
        import argparse
        import sys

        assert sys.flags.utf8_mode == 0, "test setup failed: utf8_mode should be 0"

        from micromegas.cli import query

        args = argparse.Namespace(file="-", sql=None)
        sql = query.read_sql_source(args)
        expected = {expected_literal}
        assert sql == expected, (sql, expected)
        sys.stdout.buffer.write(sql.encode("utf-8"))
        """).format(expected_literal=ascii(NON_ASCII_SQL))

    env = dict(os.environ)
    env["LC_ALL"] = "C"
    env["PYTHONUTF8"] = "0"

    proc = subprocess.run(
        [sys.executable, "-c", script],
        input=NON_ASCII_SQL.encode("utf-8"),
        env=env,
        capture_output=True,
    )
    assert proc.returncode == 0, proc.stderr.decode("utf-8", errors="replace")
    assert proc.stdout.decode("utf-8") == NON_ASCII_SQL


def test_parse_timestamp_none():
    assert parse_timestamp(None) is None


@pytest.mark.parametrize(
    "value,delta",
    [
        ("1h", datetime.timedelta(hours=1)),
        ("30m", datetime.timedelta(minutes=30)),
        ("7d", datetime.timedelta(days=7)),
    ],
)
def test_parse_timestamp_relative_delta(value, delta):
    before = datetime.datetime.now(datetime.timezone.utc)
    dt = parse_timestamp(value)
    after = datetime.datetime.now(datetime.timezone.utc)
    assert dt.tzinfo is not None
    # `dt` should land roughly `delta` before "now" -- tolerant window
    # rather than an exact instant, since parse_timestamp calls
    # datetime.now() internally.
    assert before - delta <= dt <= after - delta + datetime.timedelta(seconds=1)


def test_parse_timestamp_z_suffix():
    dt = parse_timestamp("2026-07-31T00:00:00Z")
    assert dt.tzinfo is not None
    assert dt.utcoffset() == datetime.timedelta(0)


def test_parse_timestamp_lowercase_z_suffix():
    dt = parse_timestamp("2026-07-31T00:00:00z")
    assert dt.tzinfo is not None
    assert dt.utcoffset() == datetime.timedelta(0)


def test_parse_timestamp_numeric_offset():
    dt = parse_timestamp("2026-07-31T00:00:00+00:00")
    assert dt.tzinfo is not None
    assert dt.utcoffset() == datetime.timedelta(0)


def test_parse_timestamp_non_utc_offset_preserved():
    dt = parse_timestamp("2026-07-31T00:00:00-04:00")
    assert dt.utcoffset() == datetime.timedelta(hours=-4)


def test_parse_timestamp_naive_defaults_to_utc():
    dt = parse_timestamp("2026-07-31T00:00:00")
    assert dt.tzinfo is not None
    assert dt.utcoffset() == datetime.timedelta(0)


def test_parse_timestamp_garbage_raises_value_error():
    with pytest.raises(ValueError):
        parse_timestamp("garbage")


def test_parse_timestamp_overflowing_delta_raises_overflow_error():
    # A number of days too large for datetime.timedelta overflows rather
    # than hitting the "invalid format" RuntimeError path.
    with pytest.raises(OverflowError):
        parse_timestamp("9999999999d")


def test_main_overflowing_begin_reports_usage_error(monkeypatch, capsys):
    """An overflowing --begin delta must be a clean argparse usage error,
    not an uncaught OverflowError traceback (issue regression)."""
    monkeypatch.setattr(sys, "argv", ["query", "--begin", "9999999999d", "SELECT 1"])
    with pytest.raises(SystemExit) as e_info:
        main()
    assert e_info.value.code == 2
    err = capsys.readouterr().err
    assert "invalid --begin timestamp" in err
    assert "Traceback" not in err


def test_main_unknown_profile_reports_usage_error(tmp_path, monkeypatch, capsys):
    """An unresolvable --profile must be a clean argparse usage error (via
    ProfileError), not an uncaught traceback (issue #1403)."""
    cfg_file = tmp_path / "config.json"
    cfg_file.write_text(
        json.dumps(
            {
                "default_profile": "prod",
                "profiles": {"prod": {"uri": "grpc://prod-host:50051"}},
            }
        )
    )
    monkeypatch.setattr(config, "CONFIG_PATH", cfg_file)
    monkeypatch.setattr(
        sys,
        "argv",
        ["query", "--profile", "nope", "--all", "SELECT 1"],
    )
    with pytest.raises(SystemExit) as e_info:
        main()
    assert e_info.value.code == 2
    err = capsys.readouterr().err
    assert "unknown profile" in err
    assert "nope" in err
    assert "Traceback" not in err
