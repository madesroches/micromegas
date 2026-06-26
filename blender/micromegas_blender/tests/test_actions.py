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
    # Last-occurrence anchoring would miss the new "G"; suffix alignment does not.
    start, gap = actions._appended_start(
        ["G", "G", "G", "G", "G"], ["G", "G", "G", "G"]
    )
    assert gap is False
    assert ["G", "G", "G", "G", "G"][start:] == ["G"]


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
