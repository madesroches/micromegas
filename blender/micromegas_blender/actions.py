"""
Semantic action capture — the "what did the user click" log.

Blender records nearly every button/menu/shortcut action as a registered
operator in ``bpy.context.window_manager.operators`` — the same ring buffer the
Info editor shows and "Copy as Python" reads. Draining that buffer turns raw
input events (which only the recorder sees) into a semantic action stream
(``OBJECT_OT_delete``, ``MESH_OT_primitive_cube_add``, …) with bounded
cardinality: the operator-name set is fixed, and free-form parameters go only in
the log *message body*, never a metric dimension or log target.

Draining is event-driven: ``recorder.py`` calls ``drain_operators()`` on every
discrete input event via an injected callback, so the ring is drained at
per-keystroke cadence rather than on a fixed schedule. A 0.1 s timer remains as
a backstop for periods when the recorder modal is suspended (e.g. while a
full-screen sub-modal is running) or receiving only motion events.

New entries are identified by stable per-entry identity (``op.as_pointer()``),
not by position: an entry's pointer is the address of its underlying
``wmOperator`` node, which is allocated once and freed only when FIFO-dropped
from the ring, so pointer-set membership across polls exactly determines what
is new — including under a periodic or otherwise repeating operator history,
where naive positional/string diffing is ambiguous.

Alongside the action stream this module also logs mode / workspace / tool
transitions and runtime add-on enable/disable — the bounded "what state was the
user in" signals.

This module is wired with the active lib + handle by __init__.py (set_context)
and owns its own bpy.app.timers callback (register / unregister).
"""

import bpy

from . import binding as _b

# Populated by __init__.py before register().
_lib: "_b.MicromegasLib | None" = None
_handle = None

# Poll cadence for the operator-history ring buffer backstop. Event-driven
# draining (via the recorder modal) is the primary path; this timer fires during
# periods when the modal is suspended or receiving only motion events. Kept short
# (0.1 s) so script/macro bursts don't overflow the 32-slot ring between events.
_POLL_INTERVAL_S: float = 0.1

# Cap on a single action log message (bl_idname + name + params).
_MAX_MSG_LEN: int = 4096

# Set of op.as_pointer() values seen on the previous poll. None until the first
# poll. A pointer is the stable identity of a wm.operators history entry (the
# underlying wmOperator* node), so set membership across polls tells us exactly
# which entries are new — no positional/string ambiguity.
_prev_op_ptrs: "set[int] | None" = None

# Blender hard-caps wm.operators at 32 entries; no Python API to resize.
_ring_capacity: int = 32

# Last observed editor-state values; transitions are logged on change.
_last_mode: "str | None" = None
_last_workspace: "str | None" = None
_last_tool: "str | None" = None
_last_addons: "set[str] | None" = None


def set_context(lib: "_b.MicromegasLib", handle) -> None:
    global _lib, _handle
    _lib, _handle = lib, handle


def _log(level: int, target: str, msg: str) -> None:
    if _lib and _handle:
        _lib.log(_handle, level, target, msg)


def _metric_i(name: str, unit: str, value: int) -> None:
    if _lib and _handle:
        _lib.metric_i(_handle, name, unit, value)


# ---------------------------------------------------------------------------
# Operator-history drain
# ---------------------------------------------------------------------------


def _format_op(op) -> str:
    """`bl_idname (name) {params}` capped to _MAX_MSG_LEN.

    bl_idname is always present and bounded. name is best-effort. Parameter
    extraction on a *stored* history entry is not guaranteed (it is an
    OperatorProperties/macro instance, not a live operator), so it runs in its
    own try/except and is simply omitted when unavailable.
    """
    msg = op.bl_idname  # always available, bounded cardinality
    try:
        name = op.name
        if name:
            msg = f"{msg} ({name})"
    except Exception:
        pass
    try:
        params = dict(op.as_keywords())
        if params:
            msg = f"{msg} {params}"
    except Exception:
        pass  # omit params, keep bl_idname (+ name)
    return msg[:_MAX_MSG_LEN]


def _poll_operators() -> None:
    global _prev_op_ptrs
    try:
        ops = list(bpy.context.window_manager.operators)  # oldest -> newest
    except Exception:
        return
    cur_ptrs = [op.as_pointer() for op in ops]
    prev = _prev_op_ptrs

    # New entries = those whose pointer was not present last poll, in buffer
    # order. On the first poll (prev is None) nothing was missed, so we only
    # establish the baseline and emit nothing.
    if prev is None:
        new_ops = []
    else:
        new_ops = [op for op, p in zip(ops, cur_ptrs) if p not in prev]

    # Genuine loss (gap) — the ONLY real overflow condition: ring is full AND
    # none of last poll's entries survive, meaning entries were FIFO-dropped
    # before we ever saw them. Partial overlap proves we saw everything
    # appended since the newest retained entry, so it is NOT a gap.
    gap = (
        prev is not None
        and len(ops) >= _ring_capacity
        and bool(prev)
        and prev.isdisjoint(cur_ptrs)
    )
    if gap:
        cap = _ring_capacity
        _log(
            _b.LEVEL_WARN,
            "blender.action",
            f"possible gap: operator history overflowed between polls (ring_capacity={cap})",
        )
        _metric_i("blender.action_gap", "count", 1)
    n = 0
    for op in new_ops:
        try:
            _log(_b.LEVEL_TRACE, "blender.action", _format_op(op))
            n += 1
        except Exception:
            pass
    if n > 0:
        _metric_i("blender.action_captured", "count", n)
    _prev_op_ptrs = set(cur_ptrs)


def drain_operators() -> None:
    """Drain the operator-history ring; called by the recorder modal on each discrete event."""
    _poll_operators()


# ---------------------------------------------------------------------------
# Editor-state transitions (bounded "what state was the user in")
# ---------------------------------------------------------------------------


def _poll_transitions() -> None:
    global _last_mode, _last_workspace, _last_tool, _last_addons

    try:
        mode = bpy.context.mode
        if mode != _last_mode:
            if _last_mode is not None:
                _log(_b.LEVEL_TRACE, "blender.mode", f"{_last_mode} -> {mode}")
            _last_mode = mode
    except Exception:
        pass

    try:
        ws = bpy.context.workspace.name
        if ws != _last_workspace:
            if _last_workspace is not None:
                _log(_b.LEVEL_TRACE, "blender.workspace", f"{_last_workspace} -> {ws}")
            _last_workspace = ws
    except Exception:
        pass

    try:
        tool = bpy.context.workspace.tools.from_space_view3d_mode(
            bpy.context.mode, create=False
        )
        tool_id = tool.idname if tool else ""
        if tool_id != _last_tool:
            if _last_tool is not None:
                _log(_b.LEVEL_TRACE, "blender.tool", f"{_last_tool} -> {tool_id}")
            _last_tool = tool_id
    except Exception:
        pass

    try:
        addons = set(bpy.context.preferences.addons.keys())
        if _last_addons is not None and addons != _last_addons:
            for added in sorted(addons - _last_addons):
                _log(_b.LEVEL_INFO, "blender.addon_state", f"enabled {added}")
            for removed in sorted(_last_addons - addons):
                _log(_b.LEVEL_INFO, "blender.addon_state", f"disabled {removed}")
        _last_addons = addons
    except Exception:
        pass


def on_poll() -> None:
    """Single poll pass: drain operator history, then check state transitions."""
    _poll_operators()
    _poll_transitions()


# ---------------------------------------------------------------------------
# Timer registration
# ---------------------------------------------------------------------------


def _poll_timer() -> float:
    if _lib and _handle:
        try:
            on_poll()
        except Exception:
            pass
    return _POLL_INTERVAL_S


def register() -> None:
    try:
        if not bpy.app.timers.is_registered(_poll_timer):
            bpy.app.timers.register(
                _poll_timer, first_interval=_POLL_INTERVAL_S, persistent=True
            )
    except Exception:
        pass


def unregister() -> None:
    global _prev_op_ptrs, _last_mode, _last_workspace, _last_tool, _last_addons
    try:
        if bpy.app.timers.is_registered(_poll_timer):
            bpy.app.timers.unregister(_poll_timer)
    except Exception:
        pass
    # Reset state so a re-register starts clean (no stale anchor/transitions).
    _prev_op_ptrs = None
    _last_mode = None
    _last_workspace = None
    _last_tool = None
    _last_addons = None
