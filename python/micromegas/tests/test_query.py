"""Regression tests for micromegas/cli/query.py's SQL source resolution.

These tests exercise the real `read_sql_source()` extracted from
`main()` under a forced non-UTF-8 locale (LC_ALL=C + PYTHONUTF8=0), the
same technique used in test_screen_files.py, to guard against issue
#1399: reading a SQL file (or stdin) with no explicit encoding falls
back to the platform's locale-preferred encoding and can mis-decode
non-ASCII content.
"""

import os
import subprocess
import sys
import textwrap

NON_ASCII_SQL = "-- em dash —, accented café, CJK 日本語\nSELECT 1"


def test_read_sql_source_file_survives_non_utf8_locale(tmp_path):
    """--file <path> must be read as UTF-8 regardless of process locale."""
    sql_path = tmp_path / "query.sql"
    script = textwrap.dedent(
        """
        import argparse
        import os
        import sys

        assert sys.flags.utf8_mode == 0, "test setup failed: utf8_mode should be 0"

        from micromegas.cli import query

        sql_path = os.environ["TEST_SQL_PATH"]
        args = argparse.Namespace(file=sql_path, sql=None)
        sql = query.read_sql_source(args)
        sys.stdout.buffer.write(sql.encode("utf-8"))
        """
    )
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
    script = textwrap.dedent(
        """
        import argparse
        import sys

        assert sys.flags.utf8_mode == 0, "test setup failed: utf8_mode should be 0"

        from micromegas.cli import query

        args = argparse.Namespace(file="-", sql=None)
        sql = query.read_sql_source(args)
        expected = {expected_literal}
        assert sql == expected, (sql, expected)
        sys.stdout.buffer.write(sql.encode("utf-8"))
        """
    ).format(expected_literal=ascii(NON_ASCII_SQL))

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
