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

# Stands in for a macro sub-operator's parameters, whose real edited values are
# unreachable from a stored history entry (see _is_macro_subop_ref).
_MACRO_PARAM_PLACEHOLDER: str = "<sub-operator, values unavailable>"

# Set of op.as_pointer() values seen on the previous poll. None until the first
# poll. A pointer is the stable identity of a wm.operators history entry (the
# underlying wmOperator* node), so set membership across polls tells us exactly
# which entries are new — no positional/string ambiguity.
_prev_op_ptrs: "set[int] | None" = None

# Blender hard-caps wm.operators at 32 entries; no Python API to resize.
_ring_capacity: int = 32

# Identity + formatted message of the newest ring entry as of the last poll.
# Redo-panel edits re-execute an operator in place (same as_pointer(), new
# params) via undo_post rather than redo_post, so the pointer-set diff above
# misses them; check_redo_update() diffs against this baseline instead.
_last_op_ptr: "int | None" = None
_last_op_msg: "str | None" = None
# Whether _last_op_msg was built from *readable* parameters. A baseline taken
# while as_keywords() was failing carries no params section at all (or is None),
# so it would differ from any later readable message purely because the section
# appeared — indistinguishable from a real edit. check_redo_update() uses this
# to re-baseline instead of reporting that non-difference as an update.
_last_op_params_ok: bool = False

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


def _is_macro_subop_ref(value) -> bool:
    """True for a macro's sub-operator reference (e.g. the
    ``TRANSFORM_OT_translate`` entry inside
    ``OBJECT_OT_duplicate_move.as_keywords()``): a live bpy.types.<OT>
    instance, not an OperatorProperties. Its bl_rna reads back frozen schema
    defaults and its IDProperty group is empty — verified in the Python
    console — so a stored wm.operators entry has no reachable API for the
    macro sub-op's real edited values.
    """
    return hasattr(value, "bl_rna") and not hasattr(value, "as_keywords")


def _params_of(op) -> "tuple[dict | None, bool]":
    """``(params, is_macro)``: ``op.as_keywords()`` with macro sub-op refs
    replaced by a placeholder (real values unreachable, see
    ``_is_macro_subop_ref``) instead of misleading frozen-default scalars.

    ``params`` is None when the stored entry's values are unreadable at all —
    distinct from an operator that simply takes no parameters ({}). ``is_macro``
    reports whether any sub-op ref was found. Both let callers that cannot work
    with unreadable values (``check_redo_update``) bail out without walking the
    params a second time.
    """
    # The sub-op scan is inside the try as well: hasattr() only swallows
    # AttributeError, so touching a value's bl_rna on a stale RNA reference
    # raises ReferenceError straight through it.
    try:
        params = dict(op.as_keywords())
        is_macro = False
        for key, value in list(params.items()):
            if _is_macro_subop_ref(value):
                params[key] = _MACRO_PARAM_PLACEHOLDER
                is_macro = True
    except Exception:
        return None, False
    return params, is_macro


def _format_op(op, params) -> str:
    """`bl_idname (name) {params}` capped to _MAX_MSG_LEN.

    bl_idname is always present and bounded. name is best-effort. ``params`` is
    the result of ``_params_of(op)[0]`` (None when unreadable) — callers pass it
    explicitly since they've always already computed it, rather than this
    function walking the operator's RNA a second time.
    """
    msg = op.bl_idname  # always available, bounded cardinality
    try:
        name = op.name
        if name:
            msg = f"{msg} ({name})"
    except Exception:
        pass
    try:
        if params:
            msg = f"{msg} {params}"
    except Exception:
        pass  # omit params, keep bl_idname (+ name)
    return msg[:_MAX_MSG_LEN]


def _poll_operators() -> None:
    global _prev_op_ptrs, _last_op_ptr, _last_op_msg, _last_op_params_ok
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
        new_ops = [(op, p) for op, p in zip(ops, cur_ptrs) if p not in prev]

    # Genuine loss (gap) — the ONLY real overflow condition: ring is full AND
    # none of last poll's entries survive, meaning entries were FIFO-dropped
    # before we ever saw them. Partial overlap proves we saw everything
    # appended since the newest retained entry, so it is NOT a gap.
    # Note: an exactly-full turnover (precisely _ring_capacity new ops, so the
    # old set is fully replaced with nothing lost) also trips this condition.
    # The ring alone cannot distinguish it from a true overflow (>capacity),
    # hence the WARN is hedged as a "possible" gap rather than a certain one.
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
    newest_msg = None  # reused for the baseline below, so we format at most once
    newest_params_ok = False
    for op, ptr in new_ops:
        try:
            params = _params_of(op)[0]
            msg = _format_op(op, params)
            _log(_b.LEVEL_TRACE, "blender.action", msg)
            n += 1
            if ptr == cur_ptrs[-1]:
                newest_msg = msg
                newest_params_ok = params is not None
        except Exception:
            pass
    if n > 0:
        _metric_i("blender.action_captured", "count", n)
    _prev_op_ptrs = set(cur_ptrs)

    # Baseline for check_redo_update() to diff against. Only (re)established
    # when the newest pointer changes: refreshing it on every poll — even
    # while the newest op is unchanged — would let a poll that lands between
    # a redo-panel edit and the undo_post handler capture the *edited* value
    # as the baseline, so check_redo_update() would see no diff and miss the
    # edit. Leaving it untouched here means check_redo_update() is the only
    # thing that advances it once an op is baselined, regardless of how many
    # extra polls run before undo_post fires.
    if not ops:
        _last_op_ptr = None
        _last_op_msg = None
        _last_op_params_ok = False
    elif cur_ptrs[-1] != _last_op_ptr:
        _last_op_ptr = cur_ptrs[-1]
        if newest_msg is not None:
            # already formatted in the emit loop above
            _last_op_msg = newest_msg
            _last_op_params_ok = newest_params_ok
        else:
            try:
                params = _params_of(ops[-1])[0]
                _last_op_msg = _format_op(ops[-1], params)
                _last_op_params_ok = params is not None
            except Exception:
                _last_op_msg = None
                _last_op_params_ok = False


def drain_operators() -> None:
    """Drain the operator-history ring; called by the recorder modal on each discrete event."""
    _poll_operators()


def check_redo_update() -> bool:
    """Catch redo-panel edits: Blender re-runs exec() on the same wmOperator
    (same as_pointer(), new params) via undo_post rather than redo_post, so
    _poll_operators' pointer-set diff treats it as already-seen. This diffs
    the newest entry's formatted message against the baseline _poll_operators
    recorded, and logs an update on mismatch.

    Macros (e.g. OBJECT_OT_duplicate_move) are skipped: their sub-op values
    are unreachable (see _is_macro_subop_ref), so there's nothing to diff, and
    a plain Ctrl+Z undo of a macro would otherwise look identical to a redo
    edit (same ptr, no readable param change). They fall through to a normal
    "undo" log, as before this function existed.

    Call from undo_post (and redo_post, in case some Blender version fires it
    too). No preceding _poll_operators() call is required: the baseline only
    ever advances when the newest pointer changes, so any number of interleaved
    polls is safe. Returns True if a redo-panel update was logged, so the caller
    skips the plain "undo" log for the same event.
    """
    global _last_op_msg, _last_op_params_ok
    # Guarded together: unlike _poll_operators (whose only callers wrap it in
    # try/except), this runs straight off an undo_post/redo_post handler, so a
    # stale RNA reference here would escape into Blender instead of just costing
    # us the "undo" log.
    try:
        ops = list(bpy.context.window_manager.operators)
        if not ops or ops[-1].as_pointer() != _last_op_ptr:
            return False  # no entries, or newest isn't the one we last saw
    except Exception:
        return False
    newest = ops[-1]
    try:
        params, is_macro = _params_of(newest)
        if is_macro or params is None:
            # macro sub-op values, or params unreadable at all: nothing to diff
            return False
        msg = _format_op(newest, params)
    except Exception:
        return False
    if not _last_op_params_ok:
        # The baseline was taken while the params were unreadable, so it carries
        # no params section: any mismatch below would just be that section
        # appearing, not a parameter change. Adopt the readable message as the
        # baseline so subsequent edits *are* diffable, and report no update.
        _last_op_msg = msg
        _last_op_params_ok = True
        return False
    if msg == _last_op_msg:
        return False
    _log(_b.LEVEL_TRACE, "blender.action_redo", msg)
    _metric_i("blender.action_captured", "count", 1)
    _last_op_msg = msg
    return True


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
    global _last_op_ptr, _last_op_msg, _last_op_params_ok
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
    _last_op_ptr = None
    _last_op_msg = None
    _last_op_params_ok = False
