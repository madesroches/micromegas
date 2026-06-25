"""
Modal operator for discrete user-action capture.

Installs a persistent modal operator that observes Blender events and logs
non-trivial discrete input (key presses, mouse buttons, scroll, area type).
Continuous motion events (MOUSEMOVE, INBETWEEN_MOUSEMOVE) are throttled to
avoid flooding the telemetry stream.

The operator passes all events through (PASS_THROUGH) so it does not
interfere with normal Blender operation.

Limitations:
- Coverage is high but not 100%: the modal operator can be suspended in
  some states (e.g., while a sub-modal is running full-screen).
- Does not capture operator parameter values by default; only operator
  bl_idname is logged (see VERBOSE_PARAMS preference).
"""

import time

import bpy

from . import binding as _b

_lib: "_b.MicromegasLib | None" = None
_handle = None

# Throttle for continuous motion events (emit at most once per interval).
_MOTION_THROTTLE_S: float = 1.0
_last_motion_log: float = 0.0

# True while the add-on is registered; running modals self-cancel when False.
_registered: bool = False
# True while a modal recorder instance is live (single-instance guard).
_running: bool = False

# Event types that are not interesting for session analysis.
_SKIP_TYPES = {
    "MOUSEMOVE",
    "INBETWEEN_MOUSEMOVE",
    "TRACKPADPAN",
    "TIMER",
    "TIMER0",
    "TIMER1",
    "TIMER2",
    "TIMERREGION",
    "NONE",
}

# Map event types to terse category strings (bounded cardinality).
_CATEGORY_MAP = {
    "LEFTMOUSE": "mouse",
    "RIGHTMOUSE": "mouse",
    "MIDDLEMOUSE": "mouse",
    "BUTTON4MOUSE": "mouse",
    "BUTTON5MOUSE": "mouse",
    "WHEELUPMOUSE": "scroll",
    "WHEELDOWNMOUSE": "scroll",
    "WHEELINMOUSE": "scroll",
    "WHEELOUTMOUSE": "scroll",
}


def set_context(lib: "_b.MicromegasLib", handle) -> None:
    global _lib, _handle
    _lib, _handle = lib, handle


class MICROMEGAS_OT_recorder(bpy.types.Operator):
    """Micromegas persistent event recorder (runs invisibly in the background)."""

    bl_idname = "micromegas.recorder"
    bl_label = "Micromegas Event Recorder"
    bl_options = {"INTERNAL"}

    def modal(self, context, event):
        global _last_motion_log, _running

        # Add-on was unregistered: stop this modal and clear the guard.
        if not _registered:
            _running = False
            return {"CANCELLED"}

        if not _lib or not _handle:
            return {"PASS_THROUGH"}

        etype = event.type

        if etype in _SKIP_TYPES:
            # Throttled motion log — records overall mouse-activity frequency.
            if etype in {"MOUSEMOVE", "INBETWEEN_MOUSEMOVE"}:
                now = time.monotonic()
                if now - _last_motion_log >= _MOTION_THROTTLE_S:
                    _last_motion_log = now
                    _lib.log(
                        _handle,
                        _b.LEVEL_TRACE,
                        "blender.input",
                        "mouse_move",
                    )
            return {"PASS_THROUGH"}

        category = _CATEGORY_MAP.get(etype, "key")
        value = event.value  # PRESS / RELEASE / CLICK / DOUBLE_CLICK / NOTHING

        if value in {"PRESS", "CLICK", "DOUBLE_CLICK"}:
            area_type = context.area.type if context.area else "NONE"
            _lib.log(
                _handle,
                _b.LEVEL_TRACE,
                "blender.input",
                f"type={etype} category={category} value={value} area={area_type}",
            )

        return {"PASS_THROUGH"}

    def invoke(self, context, event):
        global _running
        # Single-instance guard: refuse to start a second concurrent modal.
        if _running:
            return {"CANCELLED"}
        _running = True
        context.window_manager.modal_handler_add(self)
        return {"RUNNING_MODAL"}


@bpy.app.handlers.persistent
def _start_recorder(scene=None, depsgraph=None) -> None:
    """Launch the modal recorder from a load_post handler (has valid context)."""
    global _running
    # A file load silently cancels any in-flight modal without invoking its
    # modal() callback, so clear the single-instance guard before relaunching.
    _running = False
    try:
        bpy.ops.micromegas.recorder("INVOKE_DEFAULT")
    except Exception:
        pass


def register() -> None:
    global _registered
    _registered = True
    bpy.utils.register_class(MICROMEGAS_OT_recorder)
    # Defer launch to load_post so a window context is available.
    if _start_recorder not in bpy.app.handlers.load_post:
        bpy.app.handlers.load_post.append(_start_recorder)
    # Also attempt an immediate launch in case Blender is already running.
    try:
        bpy.ops.micromegas.recorder("INVOKE_DEFAULT")
    except Exception:
        pass


def unregister() -> None:
    global _registered
    # Signal any in-flight modal to self-cancel on its next event.
    _registered = False
    if _start_recorder in bpy.app.handlers.load_post:
        bpy.app.handlers.load_post.remove(_start_recorder)
    try:
        bpy.utils.unregister_class(MICROMEGAS_OT_recorder)
    except Exception:
        pass
