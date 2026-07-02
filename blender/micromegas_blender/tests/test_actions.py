"""Tests for actions.py: operator-history drain + diff logic."""

import pytest

from micromegas_blender import actions


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


@pytest.fixture(autouse=True)
def _wire(rec_lib, fake_bpy):
    actions.set_context(rec_lib, object())
    actions._prev_op_ptrs = None
    actions._last_mode = None
    actions._last_workspace = None
    actions._last_tool = None
    actions._last_addons = None
    yield
    actions.set_context(None, None)


def _action_msgs(rec_lib):
    return [msg for _lvl, target, msg in rec_lib.logs if target == "blender.action"]


def _set_ops(fake_bpy, ops):
    fake_bpy.context.window_manager.operators = ops


# --- _poll_operators (integration over the fake ring) ----------------------


def test_poll_emits_new_actions_with_params(rec_lib, fake_bpy):
    select_all = FakeOp("OBJECT_OT_select_all")
    _set_ops(fake_bpy, [select_all])
    actions._poll_operators()  # first poll: baseline only, nothing emitted
    _set_ops(
        fake_bpy,
        [
            select_all,
            FakeOp("MESH_OT_primitive_cube_add", name="Add Cube", kw={"size": 2.0}),
        ],
    )
    actions._poll_operators()

    msgs = _action_msgs(rec_lib)
    assert len(msgs) == 1  # only cube_add is new; select_all was in the baseline
    assert "MESH_OT_primitive_cube_add" in msgs[0]
    assert "Add Cube" in msgs[0]
    assert "size" in msgs[0]  # parameters captured when available


def test_poll_no_change_emits_nothing(rec_lib, fake_bpy):
    _set_ops(fake_bpy, [FakeOp("OBJECT_OT_delete")])
    actions._poll_operators()
    before = len(_action_msgs(rec_lib))
    actions._poll_operators()  # buffer unchanged
    assert len(_action_msgs(rec_lib)) == before


def test_poll_params_unavailable_keeps_bl_idname(rec_lib, fake_bpy):
    _set_ops(fake_bpy, [])
    actions._poll_operators()
    _set_ops(fake_bpy, [FakeOp("WM_OT_save_mainfile", name="Save", kw=None)])
    actions._poll_operators()

    msgs = _action_msgs(rec_lib)
    assert msgs[-1].startswith("WM_OT_save_mainfile")
    assert "Save" in msgs[-1]


def test_poll_overflow_logs_gap_marker(rec_lib, fake_bpy):
    # Full ring, entirely replaced by new instances between polls: the ONE
    # genuine loss condition.
    cap = actions._ring_capacity
    _set_ops(fake_bpy, [FakeOp("A") for _ in range(cap)])
    actions._poll_operators()
    new_ops = [FakeOp("X") for _ in range(cap)]
    _set_ops(fake_bpy, new_ops)
    actions._poll_operators()

    gap_logs = [m for _l, t, m in rec_lib.logs if t == "blender.action" and "gap" in m]
    assert gap_logs
    # All new entries are still emitted (never silently dropped).
    captured_msgs = [m for m in _action_msgs(rec_lib) if "gap" not in m]
    assert len(captured_msgs) == cap


def test_poll_non_full_turnover_is_not_a_gap(rec_lib, fake_bpy):
    # Small buffer (below ring capacity) fully replaced between polls — this is
    # a clear (or a very small session), not overflow, so no gap.
    _set_ops(fake_bpy, [FakeOp("A"), FakeOp("B"), FakeOp("C")])
    actions._poll_operators()
    _set_ops(fake_bpy, [FakeOp("X"), FakeOp("Y"), FakeOp("Z")])
    actions._poll_operators()

    gap_logs = [m for _l, t, m in rec_lib.logs if t == "blender.action" and "gap" in m]
    assert not gap_logs
    assert any("X" in m for m in _action_msgs(rec_lib))


def test_poll_stable_buffer_storm_regression(rec_lib, fake_bpy):
    # Same instances re-polled many times: this is the exact production bug
    # (periodic/stable operator history polled by the backstop timer while the
    # recorder modal is suspended). Must produce zero emissions and zero gaps
    # after the baseline poll.
    ops = [
        FakeOp("VIEW3D_OT_select"),
        FakeOp("TRANSFORM_OT_translate"),
        FakeOp("OBJECT_OT_editmode_toggle"),
    ]
    _set_ops(fake_bpy, ops)
    actions._poll_operators()  # baseline
    for _ in range(10):
        actions._poll_operators()

    assert _action_msgs(rec_lib) == []
    assert [
        m for _l, t, m in rec_lib.logs if t == "blender.action" and "gap" in m
    ] == []


def test_poll_periodic_tail_no_false_gap(rec_lib, fake_bpy):
    # Repeating idnames across distinct instances (what confused the old
    # positional string diff) must not trigger re-emission or a false gap once
    # stable.
    ops = [
        FakeOp("VIEW3D_OT_select"),
        FakeOp("TRANSFORM_OT_translate"),
        FakeOp("OBJECT_OT_editmode_toggle"),
        FakeOp("VIEW3D_OT_select"),
        FakeOp("TRANSFORM_OT_translate"),
        FakeOp("OBJECT_OT_editmode_toggle"),
    ]
    _set_ops(fake_bpy, ops)
    actions._poll_operators()  # baseline
    actions._poll_operators()  # unchanged

    assert _action_msgs(rec_lib) == []
    assert [
        m for _l, t, m in rec_lib.logs if t == "blender.action" and "gap" in m
    ] == []


def test_poll_repeated_identical_clicks_all_captured(rec_lib, fake_bpy):
    # Distinct instances sharing a bl_idname must each be captured — the old
    # string diff could not distinguish these.
    _set_ops(fake_bpy, [])
    actions._poll_operators()  # baseline

    clicks = [FakeOp("VIEW3D_OT_select") for _ in range(5)]
    accumulated = []
    for click in clicks:
        accumulated.append(click)
        _set_ops(fake_bpy, list(accumulated))
        actions._poll_operators()

    msgs = _action_msgs(rec_lib)
    assert len(msgs) == 5


def test_poll_append_and_fifo_drop_emits_only_new(rec_lib, fake_bpy):
    a, b, c = FakeOp("A"), FakeOp("B"), FakeOp("C")
    _set_ops(fake_bpy, [a, b, c])
    actions._poll_operators()  # baseline
    d = FakeOp("D")
    _set_ops(fake_bpy, [b, c, d])  # a dropped (FIFO), d appended
    actions._poll_operators()

    msgs = _action_msgs(rec_lib)
    assert len(msgs) == 1
    assert "D" in msgs[0]
    assert not [m for _l, t, m in rec_lib.logs if t == "blender.action" and "gap" in m]


# --- drain_operators (public delegation) -----------------------------------


def test_drain_operators_delegates_to_poll(rec_lib, fake_bpy):
    _set_ops(fake_bpy, [])
    actions._poll_operators()  # baseline
    _set_ops(fake_bpy, [FakeOp("OBJECT_OT_delete")])
    actions.drain_operators()
    assert any("OBJECT_OT_delete" in m for m in _action_msgs(rec_lib))


# --- metrics ---------------------------------------------------------------


def test_gap_emits_action_gap_metric(rec_lib, fake_bpy):
    cap = actions._ring_capacity
    _set_ops(fake_bpy, [FakeOp("A") for _ in range(cap)])
    actions._poll_operators()
    _set_ops(fake_bpy, [FakeOp("X") for _ in range(cap)])
    actions._poll_operators()

    gap_metrics = [
        (n, u, v) for n, u, v in rec_lib.metrics if n == "blender.action_gap"
    ]
    assert gap_metrics, "blender.action_gap metric not emitted on overflow"
    assert gap_metrics[0] == ("blender.action_gap", "count", 1)


def test_gap_warn_includes_ring_capacity(rec_lib, fake_bpy):
    cap = actions._ring_capacity
    _set_ops(fake_bpy, [FakeOp("A") for _ in range(cap)])
    actions._poll_operators()
    _set_ops(fake_bpy, [FakeOp("X") for _ in range(cap)])
    actions._poll_operators()

    gap_logs = [m for _l, t, m in rec_lib.logs if t == "blender.action" and "gap" in m]
    assert gap_logs
    assert "ring_capacity=32" in gap_logs[0]


def test_action_captured_metric_on_new_ops(rec_lib, fake_bpy):
    _set_ops(fake_bpy, [])
    actions._poll_operators()
    _set_ops(fake_bpy, [FakeOp("OBJECT_OT_delete"), FakeOp("MESH_OT_extrude_region")])
    actions._poll_operators()

    captured = [
        (n, u, v) for n, u, v in rec_lib.metrics if n == "blender.action_captured"
    ]
    assert captured, "blender.action_captured metric not emitted when ops were logged"
    assert captured[0] == ("blender.action_captured", "count", 2)


def test_action_captured_metric_not_emitted_when_no_new_ops(rec_lib, fake_bpy):
    _set_ops(fake_bpy, [FakeOp("OBJECT_OT_delete")])
    actions._poll_operators()
    before = len(rec_lib.metrics)
    actions._poll_operators()  # same state — no new ops
    captured_after = [
        m for m in rec_lib.metrics[before:] if m[0] == "blender.action_captured"
    ]
    assert not captured_after, "blender.action_captured must not fire when n == 0"
