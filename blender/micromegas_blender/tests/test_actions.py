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


@pytest.fixture(autouse=True)
def _wire(rec_lib, fake_bpy):
    actions.set_context(rec_lib, object())
    actions._prev_op_idnames = None
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


# --- _appended_start (pure diff logic) -------------------------------------


def test_first_poll_treats_all_as_new_no_gap():
    start, gap = actions._appended_start(["a", "b"], [])
    assert (start, gap) == (0, False)


def test_no_change_emits_nothing():
    start, gap = actions._appended_start(["a", "b", "c"], ["a", "b", "c"])
    assert (start, gap) == (3, False)


def test_appended_entries_detected():
    start, gap = actions._appended_start(["a", "b", "c", "d"], ["a", "b"])
    assert (start, gap) == (2, False)
    # new entries = current[start:] == ["c", "d"]


def test_repeated_identical_operators_not_dropped():
    # All-identical entries make every drop count align, so the alignment is
    # ambiguous. The conservative policy reports the MOST new entries (never
    # silently drops) and flags the ambiguity as a possible gap.
    current = ["G", "G", "G", "G", "G"]
    start, gap = actions._appended_start(current, ["G", "G", "G", "G"])
    assert gap is True
    assert current[start:] == ["G", "G", "G", "G"]


def test_repeated_boundary_pattern_not_dropped():
    # bl_idname pattern repeats across the rotation boundary: prev[1:] and prev[3:]
    # both prefix `current`, disagreeing on how many entries are new. The greedy
    # smallest-d choice (start=3) would report only ["X", "Y"], silently dropping
    # the other two. The conservative policy reports the most new entries and
    # flags the ambiguity.
    current = ["Y", "X", "Y", "X", "Y"]
    start, gap = actions._appended_start(current, ["X", "Y", "X", "Y"])
    assert gap is True
    assert current[start:] == ["X", "Y", "X", "Y"]


def test_ring_rotation_emits_only_new():
    # 'a' rotated out, 'e' appended.
    start, gap = actions._appended_start(["b", "c", "d", "e"], ["a", "b", "c", "d"])
    assert gap is False
    assert ["b", "c", "d", "e"][start:] == ["e"]


def test_full_turnover_flags_gap():
    start, gap = actions._appended_start(["x", "y", "z"], ["a", "b", "c"])
    assert gap is True
    assert start == 0


def test_cleared_buffer_is_not_a_gap():
    # Blender clears operator history on file load — empty current, no gap.
    start, gap = actions._appended_start([], ["a", "b", "c"])
    assert (start, gap) == (0, False)


# --- _poll_operators (integration over the fake ring) ----------------------


def test_poll_emits_new_actions_with_params(rec_lib, fake_bpy):
    _set_ops(fake_bpy, [FakeOp("OBJECT_OT_select_all")])
    actions._poll_operators()
    _set_ops(
        fake_bpy,
        [
            FakeOp("OBJECT_OT_select_all"),
            FakeOp("MESH_OT_primitive_cube_add", name="Add Cube", kw={"size": 2.0}),
        ],
    )
    actions._poll_operators()

    msgs = _action_msgs(rec_lib)
    assert len(msgs) == 2  # select_all on first poll, cube_add on second
    assert "MESH_OT_primitive_cube_add" in msgs[1]
    assert "Add Cube" in msgs[1]
    assert "size" in msgs[1]  # parameters captured when available


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
    _set_ops(fake_bpy, [FakeOp("A"), FakeOp("B"), FakeOp("C")])
    actions._poll_operators()
    # Entire buffer turned over between polls.
    _set_ops(fake_bpy, [FakeOp("X"), FakeOp("Y"), FakeOp("Z")])
    actions._poll_operators()

    gap_logs = [m for _l, t, m in rec_lib.logs if t == "blender.action" and "gap" in m]
    assert gap_logs
    # All three new entries are still emitted (never silently dropped).
    assert any("X" in m for m in _action_msgs(rec_lib))


# --- drain_operators (public delegation) -----------------------------------


def test_drain_operators_delegates_to_poll(rec_lib, fake_bpy):
    _set_ops(fake_bpy, [FakeOp("OBJECT_OT_delete")])
    actions.drain_operators()
    assert any("OBJECT_OT_delete" in m for m in _action_msgs(rec_lib))


# --- metrics ---------------------------------------------------------------


def test_gap_emits_action_gap_metric(rec_lib, fake_bpy):
    _set_ops(fake_bpy, [FakeOp("A"), FakeOp("B")])
    actions._poll_operators()
    _set_ops(fake_bpy, [FakeOp("X"), FakeOp("Y")])
    actions._poll_operators()

    gap_metrics = [
        (n, u, v) for n, u, v in rec_lib.metrics if n == "blender.action_gap"
    ]
    assert gap_metrics, "blender.action_gap metric not emitted on overflow"
    assert gap_metrics[0] == ("blender.action_gap", "count", 1)


def test_gap_warn_includes_ring_capacity(rec_lib, fake_bpy):
    _set_ops(fake_bpy, [FakeOp("A"), FakeOp("B")])
    actions._poll_operators()
    _set_ops(fake_bpy, [FakeOp("X"), FakeOp("Y")])
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
