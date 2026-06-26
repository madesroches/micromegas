"""Tests for the sys.excepthook wrapper in __init__.py."""

import sys

import micromegas_blender as mm


def test_excepthook_logs_error_and_chains(rec_lib):
    mm._lib = rec_lib
    mm._handle = object()
    chained = []
    mm._prev_excepthook = lambda *args: chained.append(args)
    try:
        try:
            raise ValueError("boom")
        except ValueError as exc:
            mm._telemetry_excepthook(type(exc), exc, exc.__traceback__)

        error_logs = [
            (lvl, target, msg)
            for lvl, target, msg in rec_lib.logs
            if target == "blender.exception"
        ]
        assert len(error_logs) == 1
        lvl, _target, msg = error_logs[0]
        assert lvl == 2  # ERROR
        assert "ValueError" in msg and "boom" in msg
        # Previous hook was chained.
        assert len(chained) == 1
    finally:
        mm._lib = None
        mm._handle = None
        mm._prev_excepthook = None


def test_excepthook_message_capped(rec_lib):
    mm._lib = rec_lib
    mm._handle = object()
    mm._prev_excepthook = None
    try:
        exc = ValueError("x" * 10_000)
        mm._telemetry_excepthook(type(exc), exc, None)
        msg = rec_lib.logs[-1][2]
        assert len(msg) <= 4096
    finally:
        mm._lib = None
        mm._handle = None


def test_excepthook_install_is_idempotent():
    """A second register() must not capture the telemetry hook as its own
    previous hook, which would cause infinite recursion on the next exception."""
    saved_hook = sys.excepthook
    saved_prev = mm._prev_excepthook
    try:
        original = object()
        sys.excepthook = original  # type: ignore[assignment]
        mm._prev_excepthook = None

        # First install: capture the original and swap in the telemetry hook.
        if sys.excepthook is not mm._telemetry_excepthook:
            mm._prev_excepthook = sys.excepthook
            sys.excepthook = mm._telemetry_excepthook
        assert mm._prev_excepthook is original

        # Second install (re-register without unregister): must be a no-op so
        # _prev_excepthook does not become _telemetry_excepthook itself.
        if sys.excepthook is not mm._telemetry_excepthook:
            mm._prev_excepthook = sys.excepthook
            sys.excepthook = mm._telemetry_excepthook
        assert mm._prev_excepthook is original
        assert mm._prev_excepthook is not mm._telemetry_excepthook
    finally:
        sys.excepthook = saved_hook
        mm._prev_excepthook = saved_prev
