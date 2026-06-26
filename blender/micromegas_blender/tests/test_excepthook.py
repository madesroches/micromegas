"""Tests for the sys.excepthook wrapper in __init__.py."""

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
