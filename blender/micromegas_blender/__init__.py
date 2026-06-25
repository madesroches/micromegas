"""
Micromegas Blender Add-on

Captures Blender session telemetry — user actions, lifecycle events, and
performance metrics — and ships them to a Micromegas ingestion server via
the micromegas-capi native library.

Configuration (all via environment variables):
    MICROMEGAS_TELEMETRY_URL        Ingestion server endpoint (required)
    MICROMEGAS_INGESTION_API_KEY    API key for authenticated ingestion (optional)
    MICROMEGAS_OIDC_*               OIDC client-credentials variables (alternative auth)

The native library (libmicromegas_capi.so / micromegas_capi.dll) must be
present in the add-on's lib/ subdirectory.  Pre-built binaries are bundled
in the distributed wheel.
"""

import atexit
import os
import sys
import uuid

bl_info = {
    "name": "Micromegas Telemetry",
    "author": "Micromegas",
    "version": (1, 0, 0),
    "blender": (4, 0, 0),
    "location": "Preferences > Add-ons",
    "description": (
        "Captures Blender session telemetry (logs, metrics, user actions) "
        "and ships it to a Micromegas observability server."
    ),
    "category": "Development",
}

# Module-level state — populated in register(), cleared in unregister().
_lib = None
_handle = None
_session_id: str = ""


def _build_process_properties() -> dict:
    props: dict[str, str] = {
        "session_id": _session_id,
        "addon_version": "{}.{}.{}".format(*bl_info["version"]),
    }
    try:
        import bpy

        props["blender_version"] = ".".join(str(v) for v in bpy.app.version)
        props["blender_version_hash"] = bpy.app.build_hash.decode(
            "utf-8", errors="replace"
        )
        props["platform"] = sys.platform
        props["os_version"] = _get_os_version()
    except Exception:
        pass
    return props


def _get_os_version() -> str:
    try:
        import platform

        return platform.version()
    except Exception:
        return "unknown"


def _load_lib():
    """Load the native cdylib.  Returns a MicromegasLib instance or None."""
    from . import binding as _b

    lib_path = _b._get_lib_path()
    if not os.path.exists(lib_path):
        print(
            f"[Micromegas] native library not found at {lib_path!r}; "
            "add-on will be inactive."
        )
        return None
    try:
        return _b.MicromegasLib(lib_path)
    except Exception as exc:
        print(f"[Micromegas] failed to load native library: {exc}")
        return None


def _periodic_flush():
    """Timer callback: flush telemetry buffers every 30 s."""
    if _lib and _handle:
        _lib.flush(_handle)
    return 30.0


def register():
    global _lib, _handle, _session_id

    _session_id = str(uuid.uuid4())

    lib = _load_lib()
    if lib is None:
        return

    props = _build_process_properties()
    sink_url = os.environ.get("MICROMEGAS_TELEMETRY_URL")
    handle = lib.init(sink_url=sink_url, properties=props)
    if handle is None:
        print("[Micromegas] telemetry init failed; add-on will be inactive.")
        return

    _lib = lib
    _handle = handle

    # Wire the sub-modules with the active lib + handle.
    from . import crash_harvester, handlers, recorder

    crash_harvester.set_context(lib, handle)
    handlers.set_context(lib, handle)
    recorder.set_context(lib, handle)

    crash_harvester.register_startup_harvest()
    handlers.register()
    recorder.register()

    # Flush on interpreter exit (belt-and-suspenders alongside mm_shutdown).
    atexit.register(_shutdown)

    # Periodic flush timer.
    try:
        import bpy

        bpy.app.timers.register(_periodic_flush, first_interval=30.0, persistent=True)
    except Exception:
        pass

    lib.log(handle, 4, "blender.addon", "Micromegas add-on registered")  # INFO=4
    # Use INFO level (4) for the startup log
    lib.log(handle, 4, "blender.addon", f"session_id={_session_id}")


def _shutdown():
    global _lib, _handle
    if _lib is None or _handle is None:
        return
    lib, handle = _lib, _handle
    _lib = None
    _handle = None
    lib.log(handle, 4, "blender.addon", "Micromegas add-on shutting down")
    lib.flush(handle)
    lib.shutdown(handle)


def unregister():
    try:
        import bpy

        bpy.app.timers.unregister(_periodic_flush)
    except Exception:
        pass

    try:
        from . import crash_harvester, handlers, recorder

        recorder.unregister()
        handlers.unregister()
        crash_harvester.unregister_startup_harvest()
    except Exception:
        pass

    _shutdown()
