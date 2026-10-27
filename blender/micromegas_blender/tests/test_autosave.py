"""Tests for autosave.py: the fallback for Blender's modal-blocked autosave timer."""

import types

import pytest
from _op_helpers import FakeOp

from micromegas_blender import autosave, recorder
from micromegas_blender import binding as _b

RECORDER = "MICROMEGAS_OT_recorder"


@pytest.fixture(autouse=True)
def _wire(wired_autosave):
    return wired_autosave


def _autosave_logs(rec_lib, level=None):
    return [
        msg
        for lvl, target, msg in rec_lib.logs
        if target == "blender.autosave" and level in (None, lvl)
    ]


def test_recorder_idname_matches_what_modal_operators_reports():
    # Blender derives the C idname from bl_idname ("module.name" ->
    # "MODULE_OT_name"), not from the Python class name.
    module, name = recorder.MICROMEGAS_OT_recorder.bl_idname.split(".")
    assert autosave._RECORDER_OP_IDNAME == f"{module.upper()}_OT_{name}"


# --- the tick -----------------------------------------------------------------


def test_saves_when_only_the_recorder_blocks_the_native_timer(wired_autosave, rec_lib):
    wired_autosave.fake.window.modal_operators = [FakeOp(RECORDER)]
    assert wired_autosave._autosave_timer() == 120.0  # 2 minutes
    assert wired_autosave.fake.saves == 1
    assert _autosave_logs(rec_lib, _b.LEVEL_DEBUG) == ["fallback autosave written"]
    assert [m[:2] for m in rec_lib.metrics] == [("blender.autosave_fallback_ms", "ms")]


def test_does_not_save_when_the_recorder_is_not_running(wired_autosave):
    # Blender's own timer is not blocked, so saving too would write the file twice.
    wired_autosave.fake.window.modal_operators = []
    assert wired_autosave._autosave_timer() == 120.0
    assert wired_autosave.fake.saves == 0


def test_retries_soon_while_another_modal_operator_runs(wired_autosave):
    fake = wired_autosave.fake
    fake.window.modal_operators = [FakeOp(RECORDER), FakeOp("TRANSFORM_OT_translate")]
    assert wired_autosave._autosave_timer() == wired_autosave._BLOCKED_RETRY_S
    assert fake.saves == 0

    fake.window.modal_operators = [FakeOp(RECORDER)]
    assert wired_autosave._autosave_timer() == 120.0
    assert fake.saves == 1


def test_foreign_modal_in_any_window_blocks_the_save(wired_autosave, fake_bpy):
    other = types.SimpleNamespace(modal_operators=[FakeOp("SCULPT_OT_brush_stroke")])
    wired_autosave.fake.window.modal_operators = [FakeOp(RECORDER)]
    fake_bpy.context.window_manager.windows.append(other)
    assert wired_autosave._autosave_timer() == wired_autosave._BLOCKED_RETRY_S
    assert wired_autosave.fake.saves == 0


def test_does_nothing_when_the_preference_is_off(wired_autosave):
    wired_autosave.fake.filepaths.use_auto_save_temporary_files = False
    wired_autosave.fake.window.modal_operators = [FakeOp(RECORDER)]
    assert wired_autosave._autosave_timer() == 120.0
    assert wired_autosave.fake.saves == 0


@pytest.mark.parametrize(
    "minutes, seconds", [(1, 60.0), (2, 120.0), (10, 600.0), (0, 60.0)]
)
def test_interval_follows_the_preference_and_is_read_every_tick(
    wired_autosave, minutes, seconds
):
    wired_autosave.fake.filepaths.auto_save_time = minutes
    assert wired_autosave._autosave_timer() == seconds


def test_cancelled_save_is_logged_as_a_warning(wired_autosave, rec_lib):
    wired_autosave.fake.window.modal_operators = [FakeOp(RECORDER)]
    wired_autosave.fake.save_result = {"CANCELLED"}
    assert wired_autosave._autosave_timer() == 120.0
    assert not rec_lib.metrics
    (warning,) = _autosave_logs(rec_lib, _b.LEVEL_WARN)
    assert "CANCELLED" in warning


def test_exception_in_the_save_is_contained_and_logged(wired_autosave, rec_lib):
    wired_autosave.fake.window.modal_operators = [FakeOp(RECORDER)]
    wired_autosave.fake.save_error = RuntimeError("disk full")
    assert wired_autosave._autosave_timer() == 120.0  # keeps the timer alive
    (warning,) = _autosave_logs(rec_lib, _b.LEVEL_WARN)
    assert "disk full" in warning


def test_unreadable_preferences_keep_the_timer_alive(wired_autosave, fake_bpy):
    fake_bpy.context.preferences = None
    assert wired_autosave._autosave_timer() == wired_autosave._FALLBACK_INTERVAL_S


# --- registration -------------------------------------------------------------


def test_register_arms_a_persistent_timer_at_the_preference_interval(wired_autosave):
    wired_autosave.register()
    assert wired_autosave.fake.timers == {wired_autosave._autosave_timer: (120.0, True)}


def test_register_twice_keeps_a_single_timer(wired_autosave):
    wired_autosave.register()
    wired_autosave.fake.filepaths.auto_save_time = 5
    wired_autosave.register()
    assert wired_autosave.fake.timers == {wired_autosave._autosave_timer: (120.0, True)}


def test_register_is_inactive_without_the_save_operator(
    wired_autosave, fake_bpy, rec_lib, monkeypatch
):
    monkeypatch.setattr(fake_bpy.ops, "wm", types.SimpleNamespace())
    wired_autosave.register()
    assert not wired_autosave.fake.timers
    (note,) = _autosave_logs(rec_lib, _b.LEVEL_INFO)
    assert "5.2" in note


def test_register_failure_is_logged_as_a_warning(
    wired_autosave, fake_bpy, rec_lib, monkeypatch
):
    def _fail(*args, **kwargs):
        raise RuntimeError("timers unavailable")

    monkeypatch.setattr(fake_bpy.app.timers, "register", _fail)
    wired_autosave.register()
    assert not wired_autosave.fake.timers
    (warning,) = _autosave_logs(rec_lib, _b.LEVEL_WARN)
    assert "timers unavailable" in warning


def test_register_is_inactive_in_background_mode(wired_autosave, fake_bpy):
    fake_bpy.app.background = True
    wired_autosave.register()
    assert not wired_autosave.fake.timers


def test_unregister_removes_the_timer_and_is_idempotent(wired_autosave):
    wired_autosave.register()
    wired_autosave.unregister()
    assert not wired_autosave.fake.timers
    wired_autosave.unregister()
