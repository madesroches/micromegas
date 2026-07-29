"""Tests for the public log() API in __init__.py."""

import micromegas_blender as mm


def test_log_forwards_to_active_session(rec_lib):
    mm._lib = rec_lib
    mm._handle = object()
    try:
        mm.log(4, "other_addon.target", "hello")
        assert rec_lib.logs == [(4, "other_addon.target", "hello")]
    finally:
        mm._lib = None
        mm._handle = None


def test_log_stringifies_non_string_msg(rec_lib):
    mm._lib = rec_lib
    mm._handle = object()
    try:
        mm.log(4, "other_addon.target", 12345)
        assert rec_lib.logs[-1][2] == "12345"
    finally:
        mm._lib = None
        mm._handle = None


def test_log_is_noop_without_active_session(rec_lib):
    mm._lib = None
    mm._handle = None
    mm.log(4, "other_addon.target", "should not raise or log")
    assert rec_lib.logs == []


def test_log_swallows_exceptions_from_lib():
    class RaisingLib:
        def log(self, handle, level, target, msg):
            raise RuntimeError("native call failed")

    mm._lib = RaisingLib()
    mm._handle = object()
    try:
        mm.log(2, "other_addon.target", "boom")  # must not raise
    finally:
        mm._lib = None
        mm._handle = None
