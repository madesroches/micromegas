"""
Lifecycle handlers and performance-metric emitters for the Micromegas Blender add-on.

Wires into bpy.app.handlers for session lifecycle events (load, save, undo, render,
frame change, depsgraph update) and emits performance metrics at each hook site.

All handlers are idempotent — safe to register/unregister multiple times.
"""

import time

import bpy

from . import binding as _b

# Populated by __init__.py before handlers are registered.
_lib: "_b.MicromegasLib | None" = None
_handle = None

# Running-average helpers (lightweight, no extra deps)
_last_depsgraph_time: float = 0.0
_render_start_time: float = 0.0


def set_context(lib: "_b.MicromegasLib", handle) -> None:
    global _lib, _handle
    _lib, _handle = lib, handle


def _log(level: int, target: str, msg: str) -> None:
    if _lib and _handle:
        _lib.log(_handle, level, target, msg)


def _metric_f(name: str, unit: str, value: float) -> None:
    if _lib and _handle:
        _lib.metric_f(_handle, name, unit, value)


def _metric_i(name: str, unit: str, value: int) -> None:
    if _lib and _handle:
        _lib.metric_i(_handle, name, unit, value)


# ---------------------------------------------------------------------------
# Lifecycle handlers
# ---------------------------------------------------------------------------

def _on_load_post(scene, depsgraph=None):
    _log(_b.LEVEL_INFO, "blender.lifecycle", "blend file loaded")
    _emit_memory_metric()


def _on_save_post(scene, depsgraph=None):
    blend_path = bpy.data.filepath
    size_bytes = 0
    if blend_path:
        try:
            import os
            size_bytes = os.path.getsize(blend_path)
        except OSError:
            pass
    _log(_b.LEVEL_INFO, "blender.lifecycle", "blend file saved")
    if size_bytes > 0:
        _metric_f("blender.blend_size_mb", "mb", size_bytes / (1024 * 1024))


def _on_undo_post(scene, depsgraph=None):
    _log(_b.LEVEL_DEBUG, "blender.lifecycle", "undo")
    stack_depth = len(bpy.context.blend_data.scene.tool_settings.use_mesh_automerge.__class__.__mro__)
    # Undo stack depth is not directly queryable; log the action only.
    _ = stack_depth  # suppress unused warning


def _on_redo_post(scene, depsgraph=None):
    _log(_b.LEVEL_DEBUG, "blender.lifecycle", "redo")


def _on_render_pre(scene):
    global _render_start_time
    _render_start_time = time.monotonic()
    _log(_b.LEVEL_INFO, "blender.render", f"render start frame={scene.frame_current}")


def _on_render_post(scene):
    elapsed = time.monotonic() - _render_start_time if _render_start_time else 0.0
    _log(_b.LEVEL_INFO, "blender.render", f"render complete frame={scene.frame_current}")
    _metric_f("blender.render_duration_s", "s", elapsed)
    _render_start_time = 0.0


def _on_render_cancel(scene):
    _log(_b.LEVEL_WARN, "blender.render", f"render cancelled frame={scene.frame_current}")
    _render_start_time = 0.0


def _on_frame_change_post(scene, depsgraph=None):
    _metric_i("blender.frame", "frame", int(scene.frame_current))


def _on_depsgraph_update_post(scene, depsgraph):
    now = time.monotonic()
    global _last_depsgraph_time
    if _last_depsgraph_time > 0.0:
        elapsed_ms = (now - _last_depsgraph_time) * 1000.0
        _metric_f("blender.eval_ms", "ms", elapsed_ms)
    _last_depsgraph_time = now


# ---------------------------------------------------------------------------
# Memory helper
# ---------------------------------------------------------------------------

def _emit_memory_metric() -> None:
    try:
        import sys
        memory_mb = 0.0
        try:
            # sysinfo not available in Blender Python; use /proc/self/status on Linux
            with open("/proc/self/status") as f:
                for line in f:
                    if line.startswith("VmRSS:"):
                        memory_mb = int(line.split()[1]) / 1024.0
                        break
        except Exception:
            pass
        if memory_mb > 0:
            _metric_f("blender.rss_mb", "mb", memory_mb)
    except Exception:
        pass


# ---------------------------------------------------------------------------
# msgbus subscription for property edits
# ---------------------------------------------------------------------------

_msgbus_owner = object()


def _on_active_object_change():
    obj = bpy.context.active_object
    if obj is None:
        return
    _log(_b.LEVEL_TRACE, "blender.scene", f"active_object_type={obj.type}")


def _subscribe_msgbus() -> None:
    try:
        bpy.msgbus.subscribe_rna(
            key=bpy.types.LayerObjects,
            owner=_msgbus_owner,
            args=(),
            notify=_on_active_object_change,
        )
    except Exception:
        pass


def _unsubscribe_msgbus() -> None:
    try:
        bpy.msgbus.clear_by_owner(_msgbus_owner)
    except Exception:
        pass


# ---------------------------------------------------------------------------
# Register / Unregister
# ---------------------------------------------------------------------------

_HANDLER_MAP = [
    (bpy.app.handlers.load_post, _on_load_post),
    (bpy.app.handlers.save_post, _on_save_post),
    (bpy.app.handlers.undo_post, _on_undo_post),
    (bpy.app.handlers.redo_post, _on_redo_post),
    (bpy.app.handlers.render_pre, _on_render_pre),
    (bpy.app.handlers.render_post, _on_render_post),
    (bpy.app.handlers.render_cancel, _on_render_cancel),
    (bpy.app.handlers.frame_change_post, _on_frame_change_post),
    (bpy.app.handlers.depsgraph_update_post, _on_depsgraph_update_post),
]


def register() -> None:
    for handler_list, fn in _HANDLER_MAP:
        if fn not in handler_list:
            handler_list.append(fn)
    _subscribe_msgbus()


def unregister() -> None:
    for handler_list, fn in _HANDLER_MAP:
        if fn in handler_list:
            handler_list.remove(fn)
    _unsubscribe_msgbus()
