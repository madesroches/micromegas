"""Tests for check_redo_update(): catching redo-panel edits that fire
undo_post (same wmOperator, new params) instead of redo_post."""

import pytest

from micromegas_blender import actions, handlers


class FakeOp:
    def __init__(self, idname, name="", kw=None):
        self.bl_idname = idname
        self.name = name
        self._kw = kw

    def as_keywords(self):
        if self._kw is None:
            raise RuntimeError("params unavailable on stored history entry")
        return self._kw

    def as_pointer(self):
        return id(self)


class MacroSubOp:
    """Stand-in for a macro's sub-operator reference: has bl_rna but not
    as_keywords, per actions._is_macro_subop_ref."""

    bl_rna = object()


@pytest.fixture(autouse=True)
def _wire(rec_lib, fake_bpy):
    actions.set_context(rec_lib, object())
    actions._prev_op_ptrs = None
    actions._last_op_ptr = None
    actions._last_op_msg = None
    yield
    actions.set_context(None, None)


def _set_ops(fake_bpy, ops):
    fake_bpy.context.window_manager.operators = ops


def _redo_msgs(rec_lib):
    return [msg for _lvl, target, msg in rec_lib.logs if target == "blender.action_redo"]


def test_redo_update_detects_param_change_on_same_op(rec_lib, fake_bpy):
    op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
    _set_ops(fake_bpy, [op])
    actions._poll_operators()  # baseline

    op._kw = {"value": (2.0, 2.0, 2.0)}  # redo-panel edit: same op, new params
    assert actions.check_redo_update() is True

    msgs = _redo_msgs(rec_lib)
    assert len(msgs) == 1
    assert "TRANSFORM_OT_resize" in msgs[0]
    assert "2.0" in msgs[0]


def test_redo_update_survives_interleaved_poll(rec_lib, fake_bpy):
    # Regression: an unrelated _poll_operators() call (backstop timer or
    # another drain_operators() event) landing between the redo-panel edit
    # and check_redo_update() must not overwrite the baseline with the
    # already-edited value — the pointer hasn't changed, so the poll should
    # leave the baseline alone.
    op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
    _set_ops(fake_bpy, [op])
    actions._poll_operators()  # baseline

    op._kw = {"value": (2.0, 2.0, 2.0)}  # redo-panel edit: same op, new params
    actions._poll_operators()  # interleaved poll (same pointer, no-op expected)

    assert actions.check_redo_update() is True
    msgs = _redo_msgs(rec_lib)
    assert len(msgs) == 1
    assert "2.0" in msgs[0]


def test_redo_update_no_change_returns_false(rec_lib, fake_bpy):
    op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
    _set_ops(fake_bpy, [op])
    actions._poll_operators()  # baseline

    assert actions.check_redo_update() is False
    assert _redo_msgs(rec_lib) == []


def test_redo_update_returns_false_when_newest_op_differs(rec_lib, fake_bpy):
    op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
    _set_ops(fake_bpy, [op])
    actions._poll_operators()  # baseline

    # A genuinely new op appeared (real undo/new action), not a redo-panel edit.
    _set_ops(fake_bpy, [op, FakeOp("OBJECT_OT_delete")])
    assert actions.check_redo_update() is False


def test_redo_update_returns_false_with_no_ops(rec_lib, fake_bpy):
    _set_ops(fake_bpy, [])
    actions._poll_operators()  # baseline
    assert actions.check_redo_update() is False


def test_redo_update_skips_macros(rec_lib, fake_bpy):
    op = FakeOp("OBJECT_OT_duplicate_move", kw={"TRANSFORM_OT_translate": MacroSubOp()})
    _set_ops(fake_bpy, [op])
    actions._poll_operators()  # baseline

    op._kw = {"TRANSFORM_OT_translate": MacroSubOp()}  # would-be "change"
    assert actions.check_redo_update() is False
    assert _redo_msgs(rec_lib) == []


def test_redo_update_emits_action_captured_metric(rec_lib, fake_bpy):
    op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
    _set_ops(fake_bpy, [op])
    actions._poll_operators()  # baseline

    op._kw = {"value": (3.0, 3.0, 3.0)}
    actions.check_redo_update()

    captured = [
        (n, u, v) for n, u, v in rec_lib.metrics if n == "blender.action_captured"
    ]
    assert captured == [("blender.action_captured", "count", 1)]


def test_on_undo_post_skips_plain_undo_log_on_redo_update(rec_lib, fake_bpy):
    handlers.set_context(rec_lib, object())
    try:
        op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
        _set_ops(fake_bpy, [op])
        actions._poll_operators()  # baseline

        op._kw = {"value": (5.0, 5.0, 5.0)}
        handlers._on_undo_post(fake_bpy.context.scene)

        undo_logs = [m for _l, t, m in rec_lib.logs if t == "blender.lifecycle" and m == "undo"]
        assert undo_logs == []
        assert _redo_msgs(rec_lib)
    finally:
        handlers.set_context(None, None)


def test_on_undo_post_logs_plain_undo_when_no_redo_update(rec_lib, fake_bpy):
    handlers.set_context(rec_lib, object())
    try:
        op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
        _set_ops(fake_bpy, [op])
        actions._poll_operators()  # baseline

        handlers._on_undo_post(fake_bpy.context.scene)  # nothing changed

        undo_logs = [m for _l, t, m in rec_lib.logs if t == "blender.lifecycle" and m == "undo"]
        assert undo_logs == ["undo"]
    finally:
        handlers.set_context(None, None)


def test_on_redo_post_skips_plain_redo_log_on_redo_update(rec_lib, fake_bpy):
    handlers.set_context(rec_lib, object())
    try:
        op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
        _set_ops(fake_bpy, [op])
        actions._poll_operators()  # baseline

        op._kw = {"value": (5.0, 5.0, 5.0)}
        handlers._on_redo_post(fake_bpy.context.scene)

        redo_logs = [m for _l, t, m in rec_lib.logs if t == "blender.lifecycle" and m == "redo"]
        assert redo_logs == []
        assert _redo_msgs(rec_lib)
    finally:
        handlers.set_context(None, None)


def test_on_redo_post_logs_plain_redo_when_no_redo_update(rec_lib, fake_bpy):
    handlers.set_context(rec_lib, object())
    try:
        op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
        _set_ops(fake_bpy, [op])
        actions._poll_operators()  # baseline

        handlers._on_redo_post(fake_bpy.context.scene)  # nothing changed

        redo_logs = [m for _l, t, m in rec_lib.logs if t == "blender.lifecycle" and m == "redo"]
        assert redo_logs == ["redo"]
    finally:
        handlers.set_context(None, None)
