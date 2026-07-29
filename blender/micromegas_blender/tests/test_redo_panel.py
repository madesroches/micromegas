"""Tests for check_redo_update(): catching redo-panel edits that fire
undo_post (same wmOperator, new params) instead of redo_post."""

import pytest
from _op_helpers import FakeOp, MacroSubOp, set_ops as _set_ops

from micromegas_blender import actions


@pytest.fixture(autouse=True)
def _wire(wired_actions):
    pass


def _redo_msgs(rec_lib):
    return [
        msg for _lvl, target, msg in rec_lib.logs if target == "blender.action_redo"
    ]


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


def test_redo_update_detects_second_consecutive_edit(rec_lib, fake_bpy):
    # check_redo_update() advances the baseline itself, so a second edit on the
    # same operator must still be detected — and a following undo_post with
    # nothing changed must not re-log the edit it already reported.
    op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
    _set_ops(fake_bpy, [op])
    actions._poll_operators()  # baseline

    op._kw = {"value": (2.0, 2.0, 2.0)}
    assert actions.check_redo_update() is True
    op._kw = {"value": (3.0, 3.0, 3.0)}
    assert actions.check_redo_update() is True
    assert actions.check_redo_update() is False  # plain undo, nothing changed

    msgs = _redo_msgs(rec_lib)
    assert len(msgs) == 2
    assert "2.0" in msgs[0]
    assert "3.0" in msgs[1]


def test_poll_rebaselines_on_new_op_after_detected_edit(rec_lib, fake_bpy):
    # After an edit advanced the baseline, a poll that sees a genuinely new
    # newest entry must re-point the baseline at it, so an edit on the *new*
    # operator is diffed against the new operator's own message.
    first = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
    _set_ops(fake_bpy, [first])
    actions._poll_operators()  # baseline

    first._kw = {"value": (2.0, 2.0, 2.0)}
    assert actions.check_redo_update() is True

    second = FakeOp("TRANSFORM_OT_rotate", kw={"value": 0.5})
    _set_ops(fake_bpy, [first, second])
    actions._poll_operators()  # re-baseline onto `second`
    assert actions._last_op_ptr == second.as_pointer()
    assert actions.check_redo_update() is False  # nothing changed on `second`

    second._kw = {"value": 1.5}
    assert actions.check_redo_update() is True
    msgs = _redo_msgs(rec_lib)
    assert len(msgs) == 2
    assert "TRANSFORM_OT_rotate" in msgs[1]
    assert "1.5" in msgs[1]


def test_redo_update_rebaselines_when_baseline_had_unreadable_params(rec_lib, fake_bpy):
    # A baseline taken while as_keywords() was failing carries no params
    # section, so it differs from any later readable message purely because the
    # section appeared. That non-difference must not be reported as an edit —
    # the readable message becomes the new baseline instead, so a real edit
    # after it is still caught.
    op = FakeOp("TRANSFORM_OT_resize", kw=None)  # params unreadable
    _set_ops(fake_bpy, [op])
    actions._poll_operators()  # baseline, without params

    op._kw = {"value": (1.0, 1.0, 1.0)}  # params now readable
    assert actions.check_redo_update() is False
    assert _redo_msgs(rec_lib) == []

    op._kw = {"value": (2.0, 2.0, 2.0)}  # genuine edit against the new baseline
    assert actions.check_redo_update() is True
    assert len(_redo_msgs(rec_lib)) == 1


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


def test_redo_update_survives_stale_rna_reference(rec_lib, fake_bpy, wired_handlers):
    # check_redo_update() runs straight off undo_post, whose handler has no
    # try/except of its own, so a stale RNA reference (as_pointer() raising
    # ReferenceError) must be swallowed here rather than escaping into Blender.
    op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
    _set_ops(fake_bpy, [op])
    actions._poll_operators()  # baseline

    def _removed():
        raise ReferenceError("StructRNA of type Operator has been removed")

    op.as_pointer = _removed

    assert actions.check_redo_update() is False
    # The handler still logs the plain "undo" rather than blowing up.
    wired_handlers._on_undo_post(fake_bpy.context.scene)
    assert _lifecycle_msgs(rec_lib, "undo") == ["undo"]


def test_poll_clears_baseline_when_ring_empties(rec_lib, fake_bpy):
    # A cleared ring (e.g. on file load) must drop the baseline entirely, not
    # leave it pointing at a freed wmOperator whose address a later entry could
    # reuse — that would diff a new operator against a dead one's message.
    op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
    _set_ops(fake_bpy, [op])
    actions._poll_operators()  # baseline
    assert actions._last_op_ptr == op.as_pointer()
    assert actions._last_op_msg is not None
    assert actions._last_op_params_ok is True

    _set_ops(fake_bpy, [])
    actions._poll_operators()

    assert actions._last_op_ptr is None
    assert actions._last_op_msg is None
    assert actions._last_op_params_ok is False


def test_redo_update_skips_macros(rec_lib, fake_bpy):
    op = FakeOp("OBJECT_OT_duplicate_move", kw={"TRANSFORM_OT_translate": MacroSubOp()})
    _set_ops(fake_bpy, [op])
    actions._poll_operators()  # baseline

    op._kw = {"TRANSFORM_OT_translate": MacroSubOp()}  # would-be "change"
    assert actions.check_redo_update() is False
    assert _redo_msgs(rec_lib) == []


def test_redo_update_skips_ops_with_unreadable_params(rec_lib, fake_bpy):
    # kw=None -> as_keywords() raises, so there are no values to diff. Must not
    # be mistaken for a parameter change even though the formatted message can
    # differ from the baseline for unrelated reasons.
    op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
    _set_ops(fake_bpy, [op])
    actions._poll_operators()  # baseline, with params readable

    op._kw = None  # params became unreadable
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


def _lifecycle_msgs(rec_lib, msg):
    return [m for _l, t, m in rec_lib.logs if t == "blender.lifecycle" and m == msg]


def test_on_undo_post_skips_plain_undo_log_on_redo_update(
    rec_lib, fake_bpy, wired_handlers
):
    op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
    _set_ops(fake_bpy, [op])
    actions._poll_operators()  # baseline

    op._kw = {"value": (5.0, 5.0, 5.0)}
    wired_handlers._on_undo_post(fake_bpy.context.scene)

    assert _lifecycle_msgs(rec_lib, "undo") == []
    assert _redo_msgs(rec_lib)


def test_on_undo_post_logs_plain_undo_when_no_redo_update(
    rec_lib, fake_bpy, wired_handlers
):
    op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
    _set_ops(fake_bpy, [op])
    actions._poll_operators()  # baseline

    wired_handlers._on_undo_post(fake_bpy.context.scene)  # nothing changed

    assert _lifecycle_msgs(rec_lib, "undo") == ["undo"]


def test_on_redo_post_skips_plain_redo_log_on_redo_update(
    rec_lib, fake_bpy, wired_handlers
):
    op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
    _set_ops(fake_bpy, [op])
    actions._poll_operators()  # baseline

    op._kw = {"value": (5.0, 5.0, 5.0)}
    wired_handlers._on_redo_post(fake_bpy.context.scene)

    assert _lifecycle_msgs(rec_lib, "redo") == []
    assert _redo_msgs(rec_lib)


def test_on_redo_post_logs_plain_redo_when_no_redo_update(
    rec_lib, fake_bpy, wired_handlers
):
    op = FakeOp("TRANSFORM_OT_resize", kw={"value": (1.0, 1.0, 1.0)})
    _set_ops(fake_bpy, [op])
    actions._poll_operators()  # baseline

    wired_handlers._on_redo_post(fake_bpy.context.scene)  # nothing changed

    assert _lifecycle_msgs(rec_lib, "redo") == ["redo"]
