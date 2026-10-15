#!/usr/bin/env python3
"""Tests for check_client_deps.find_forbidden on canned `cargo tree --prefix none` output."""

from check_client_deps import find_forbidden


def test_clean_output():
    assert find_forbidden("micromegas v0.32.0\ntonic v0.14.0\narrow v59.2.0\n") == []


def test_forbidden_found():
    out = "micromegas v0.32.0\ndatafusion v55.2.0\nsqlx v0.8.6\ntonic v0.14.0\n"
    assert find_forbidden(out) == ["datafusion", "sqlx"]


def test_exact_name_match_only():
    out = "datafusion-wasm-bindgen v0.1.0\nsqlx-core v0.8.6\nobject_store_ext v1.0.0\n"
    assert find_forbidden(out) == []


def test_duplicate_markers_and_features():
    out = "sqlx v0.8.6 (*)\nfoyer v0.1.0 (/path/to/foyer) (*)\nmicromegas-auth v0.32.0 (proc-macro)\n"
    assert find_forbidden(out) == ["foyer", "micromegas-auth", "sqlx"]
