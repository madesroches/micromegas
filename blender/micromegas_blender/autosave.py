"""
Autosave fallback — keeps Blender's "Auto Save Temporary Files" working.

Blender's autosave timer (``wm_autosave_timer``) does not save while any window
has a running modal operator: it postpones the save and retries every 10 ms. The
recorder is a persistent modal operator, so with this add-on enabled the native
timer never gets to save and autosave silently stops, although the preferences
still say it is on.

This module re-creates the trigger on a ``bpy.app.timers`` callback and calls
``bpy.ops.wm.save_auto_save()``, which writes the same autosave file without the
modal check. It needs Blender 5.2+, where that operator was added; on older
versions it stays inactive.

Behaviour, all read from the user preferences on every tick:

- ``use_auto_save_temporary_files`` off: do nothing.
- ``auto_save_time`` (minutes) is the interval between saves.
- The recorder is not running: do nothing, Blender's own timer is not blocked
  and saving too would write a possibly very large file twice.
- Any other modal operator is running: retry shortly instead of saving, as
  Blender's own timer does, because the file may not be in a valid state.
"""

import time

import bpy

from . import binding as _b

# Populated by __init__.py before register().
_lib: "_b.MicromegasLib | None" = None
_handle = None

# Window.modal_operators reports operators by their C idname (CLASS_OT_name),
# not by the dotted bl_idname ("micromegas.recorder").
_RECORDER_OP_IDNAME: str = "MICROMEGAS_OT_recorder"

# How soon to look again while another add-on's modal operator is running.
_BLOCKED_RETRY_S: float = 0.5

# Interval used only if the preferences cannot be read.
_FALLBACK_INTERVAL_S: float = 120.0


def set_context(lib: "_b.MicromegasLib", handle) -> None:
    global _lib, _handle
    _lib, _handle = lib, handle


def _log(level: int, target: str, msg: str) -> None:
    if _lib and _handle:
        _lib.log(_handle, level, target, msg)


def _metric_f(name: str, unit: str, value: float) -> None:
    if _lib and _handle:
        _lib.metric_f(_handle, name, unit, value)


def _interval_s() -> float:
    # The preference is an integer number of minutes; Blender clamps it to >= 1.
    return max(bpy.context.preferences.filepaths.auto_save_time, 1) * 60.0


def _running_modal_idnames() -> "set[str]":
    windows = bpy.context.window_manager.windows
    return {op.bl_idname for win in windows for op in win.modal_operators}


def _save_operator_available() -> bool:
    # Probe the operator rather than the version: pre-release 5.2 builds report
    # (5, 2, 0) before it landed. `bpy.ops.wm.<name>` resolves lazily for any
    # name, but dir() only lists registered operators.
    return "save_auto_save" in dir(bpy.ops.wm)


def _save() -> None:
    start = time.perf_counter()
    result = bpy.ops.wm.save_auto_save()
    elapsed_ms = (time.perf_counter() - start) * 1000.0
    if "FINISHED" in result:
        _log(_b.LEVEL_DEBUG, "blender.autosave", "fallback autosave written")
        _metric_f("blender.autosave_fallback_ms", "ms", elapsed_ms)
    else:
        _log(_b.LEVEL_WARN, "blender.autosave", f"fallback autosave failed: {result}")


def _autosave_timer() -> float:
    interval = _FALLBACK_INTERVAL_S
    try:
        interval = _interval_s()
        if not bpy.context.preferences.filepaths.use_auto_save_temporary_files:
            return interval

        running = _running_modal_idnames()
        if _RECORDER_OP_IDNAME not in running:
            return interval  # Blender's own timer is not blocked by us
        if running - {_RECORDER_OP_IDNAME}:
            return _BLOCKED_RETRY_S

        _save()
    except Exception as exc:
        _log(_b.LEVEL_WARN, "blender.autosave", f"fallback autosave error: {exc!r}")
    return interval


def register() -> None:
    if bpy.app.background:
        return  # no window manager timer to replace
    try:
        if not _save_operator_available():
            _log(
                _b.LEVEL_INFO,
                "blender.autosave",
                "fallback autosave unavailable: wm.save_auto_save needs Blender "
                "5.2+, native autosave stays blocked by the recorder",
            )
            return
        if not bpy.app.timers.is_registered(_autosave_timer):
            bpy.app.timers.register(
                _autosave_timer, first_interval=_interval_s(), persistent=True
            )
    except Exception as exc:
        _log(
            _b.LEVEL_WARN,
            "blender.autosave",
            f"fallback autosave not armed: {exc!r}",
        )


def unregister() -> None:
    try:
        if bpy.app.timers.is_registered(_autosave_timer):
            bpy.app.timers.unregister(_autosave_timer)
    except Exception:
        pass
