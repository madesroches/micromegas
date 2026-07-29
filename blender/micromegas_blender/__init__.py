"""
Micromegas Blender Add-on

Captures Blender session telemetry — user actions, lifecycle events, and
performance metrics — and ships them to a Micromegas ingestion server via
the micromegas-capi native library.

Configuration (environment variables):
    MICROMEGAS_TELEMETRY_URL        Ingestion server endpoint (required)
    MICROMEGAS_INGESTION_API_KEY    API key for authenticated ingestion (optional)
    MICROMEGAS_OIDC_*               OIDC client-credentials variables (alternative auth)

One dev-only setting lives in the add-on preferences instead of the
environment: "Keep Alive (Dev Only)" keeps the telemetry session alive across
an add-on disable/enable within the same Blender process (see
MicromegasAddonPreferences).

The native library (libmicromegas_capi.so / micromegas_capi.dll) must be
present in the add-on's lib/ subdirectory.  Pre-built binaries are bundled
in the distributed extension zip.
"""

import atexit
import bpy
import os
import re
import sys
import uuid

try:
    from . import _build_info

    _COMMIT = _build_info.COMMIT
except ImportError:
    # _build_info.py is generated at build time (see build/build_blender_plugin.py)
    # and is not tracked in git; running from source before a build is fine.
    _COMMIT = "unknown"


def _read_addon_version() -> str:
    """Read the add-on version from the bundled manifest so telemetry reports
    the version that is actually installed. The manifest is stamped from the
    workspace version at build time (see build/build_blender_plugin.py)."""
    manifest = os.path.join(os.path.dirname(__file__), "blender_manifest.toml")
    try:
        with open(manifest, encoding="utf-8") as f:
            for line in f:
                # Matches the top-level `version` key, not `schema_version`.
                m = re.match(r'\s*version\s*=\s*"([^"]+)"', line)
                if m:
                    return m.group(1)
    except OSError:
        pass
    return "unknown"


_ADDON_VERSION = _read_addon_version()

# Module-level state — populated in register(), cleared in unregister().
_lib = None
_handle = None
_session_id: str = ""


# sys attribute names used to smuggle live state across a module reload —
# sys is the one thing that survives disable/re-enable within the same
# Blender process (module globals get reset on reload).
_STATE_ATTR = "_micromegas_addon_state"  # (lib, handle, session_id) tuple
_ATEXIT_HOOK_ATTR = "_micromegas_addon_atexit_hook"  # the registered _shutdown


class MicromegasAddonPreferences(bpy.types.AddonPreferences):
    bl_idname = __package__

    keep_alive: bpy.props.BoolProperty(
        name="Keep Alive (Dev Only)",
        description=(
            "Keep the telemetry session alive across add-on disable/enable "
            "within the same Blender process. Needed when developing via the "
            "VS Code Blender-Dev extension, which re-enables the add-on on "
            "every launch — the native telemetry layer can only initialize "
            "once per process, so without this the add-on would go inactive "
            "on every debug launch. Leave off for normal use."
        ),
        default=False,
    )

    def draw(self, context):
        self.layout.prop(self, "keep_alive")


def _is_keep_alive_enabled() -> bool:
    try:
        return bool(bpy.context.preferences.addons[__package__].preferences.keep_alive)
    except Exception:
        return False


def _build_process_properties() -> dict:
    """Assemble the process fingerprint attached once at mm_init.

    These low-cardinality dimensions are what triage groups/filters by (GPU,
    enabled add-ons, etc.). Every source is wrapped individually so a single
    failure (e.g. GPU calls in --background mode) never drops the rest.
    """
    props: dict[str, str] = {
        "session_id": _session_id,
        "addon_version": _ADDON_VERSION,
    }
    try:
        props["blender_version"] = ".".join(str(v) for v in bpy.app.version)
        props["blender_version_hash"] = bpy.app.build_hash.decode(
            "utf-8", errors="replace"
        )
        props["platform"] = sys.platform
        props["os_version"] = _get_os_version()
    except Exception:
        pass

    # Each fingerprint dimension below is independent: guard individually.
    _set_prop(props, "python_version", lambda: sys.version.split()[0])
    _set_prop(props, "cpu_count", lambda: str(os.cpu_count() or 0))
    _set_prop(props, "total_ram_mb", _get_total_ram_mb)
    _set_prop(props, "background", _get_background)
    _set_prop(props, "render_engine", _get_render_engine)
    _set_prop(props, "enabled_addons", _get_enabled_addons)

    # GPU/driver — the #1 crash dimension for a DCC tool. Requires a GPU
    # context; in --background mode these may raise, so each is guarded.
    _set_prop(props, "gpu_renderer", lambda: _gpu_call("renderer_get"))
    _set_prop(props, "gpu_vendor", lambda: _gpu_call("vendor_get"))
    _set_prop(props, "gpu_backend", lambda: _gpu_call("backend_type_get"))
    _set_prop(props, "gpu_driver", lambda: _gpu_call("version_get"))

    return props


def _set_prop(props: dict, key: str, getter) -> None:
    """Set props[key] from getter(), skipping the key entirely on any failure
    or empty result. Keeps one failing source from dropping the whole dict."""
    try:
        value = getter()
        if value:
            props[key] = str(value)
    except Exception:
        pass


def _gpu_call(fn_name: str) -> str:
    import gpu

    return getattr(gpu.platform, fn_name)()


def _get_background() -> str:
    return "true" if bpy.app.background else "false"


def _get_render_engine() -> str:
    return bpy.context.scene.render.engine


def _get_total_ram_mb() -> str:
    """Total physical RAM in MB. Linux via sysconf, Windows via ctypes.

    Returns "" when unavailable; the Rust total_memory system metric already
    covers the machine, so this is a best-effort convenience dimension.
    """
    if sys.platform == "linux":
        pages = os.sysconf("SC_PHYS_PAGES")
        page_size = os.sysconf("SC_PAGE_SIZE")
        return str(int(pages * page_size / (1024 * 1024)))
    if sys.platform == "win32":
        import ctypes
        from ctypes import wintypes

        class _MEMORYSTATUSEX(ctypes.Structure):
            _fields_ = [
                ("dwLength", wintypes.DWORD),
                ("dwMemoryLoad", wintypes.DWORD),
                ("ullTotalPhys", ctypes.c_uint64),
                ("ullAvailPhys", ctypes.c_uint64),
                ("ullTotalPageFile", ctypes.c_uint64),
                ("ullAvailPageFile", ctypes.c_uint64),
                ("ullTotalVirtual", ctypes.c_uint64),
                ("ullAvailVirtual", ctypes.c_uint64),
                ("ullAvailExtendedVirtual", ctypes.c_uint64),
            ]

        stat = _MEMORYSTATUSEX()
        stat.dwLength = ctypes.sizeof(_MEMORYSTATUSEX)
        if ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(stat)):
            return str(int(stat.ullTotalPhys / (1024 * 1024)))
    return ""


def _get_enabled_addons() -> str:
    """Sorted, comma-joined `name@version` of enabled add-ons.

    Third-party add-ons are a leading cause of Blender instability. Emitted as
    one bounded process property (capped ~2 KB) rather than per-add-on
    dimensions, so cardinality stays controlled.
    """
    import addon_utils

    enabled = set(bpy.context.preferences.addons.keys())
    entries: list[str] = []
    for mod in addon_utils.modules():
        name = getattr(mod, "__name__", None)
        if name not in enabled:
            continue
        info = getattr(mod, "bl_info", {}) or {}
        version = info.get("version")
        if isinstance(version, (tuple, list)):
            version = ".".join(str(v) for v in version)
        entries.append(f"{name}@{version}" if version else str(name))
    joined = ",".join(sorted(entries))
    return joined[:2048]


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
    """Timer callback: sample per-process metrics, then flush every 30 s."""
    if _lib and _handle:
        try:
            from . import handlers

            handlers.on_periodic()
        except Exception:
            pass
        _lib.flush(_handle)
    return 30.0


# Saved on register(), restored on unregister(). Catches the (rare) exceptions
# that reach the interpreter top level; the add-on's own callbacks are wrapped
# separately because Blender swallows operator/timer/handler exceptions before
# they get here.
_prev_excepthook = None


def _telemetry_excepthook(exc_type, exc_value, exc_tb):
    try:
        import traceback

        text = "".join(traceback.format_exception(exc_type, exc_value, exc_tb))[:4096]
        if _lib and _handle:
            _lib.log(_handle, 2, "blender.exception", text)  # ERROR=2
    except Exception:
        pass
    finally:
        if _prev_excepthook is not None:
            _prev_excepthook(exc_type, exc_value, exc_tb)


def register():
    global _lib, _handle, _session_id

    try:
        bpy.utils.register_class(MicromegasAddonPreferences)
    except Exception as exc:
        # A registration leaked by an earlier register() that raised past this
        # point must not take the telemetry session down with it — without the
        # guard every later enable dies here until Blender is restarted.
        print(f"[Micromegas] add-on preferences registration failed: {exc}")

    cached = getattr(sys, _STATE_ATTR, None)
    if cached is not None:
        # A prior unregister() in this same process parked a live session
        # here (keep-alive) instead of shutting it down. Reuse it regardless
        # of the *current* keep_alive setting — mm_init cannot be called
        # twice in one process, so a parked session is the only one this
        # process can ever have and reusing it always beats trying (and
        # failing) to initialize a second. This does not make keep-alive
        # sticky: turning the preference off still takes effect at the next
        # unregister(), which shuts the session down for good.
        lib, handle, _session_id = cached
        delattr(sys, _STATE_ATTR)
    else:
        _session_id = str(uuid.uuid4())

        lib = _load_lib()
        if lib is None:
            return

        props = _build_process_properties()
        sink_url = os.environ.get("MICROMEGAS_TELEMETRY_URL")
        handle = lib.init(sink_url=sink_url, properties=props)
        if handle is None:
            print(
                "[Micromegas] telemetry init failed; add-on will be inactive. "
                "If you just disabled and re-enabled the add-on, restart Blender — "
                "the native telemetry layer initializes once per process and "
                "cannot be reinitialized within the same session. Turning on "
                "Keep Alive in the add-on preferences *before* disabling "
                "avoids this; enabling it now will not bring the session back."
            )
            return

    _lib = lib
    _handle = handle

    try:
        _wire_up(lib, handle)
    except Exception:
        # The session above is live but this enable failed, and Blender will
        # not call unregister() to clean it up: addon_utils.enable() sets
        # __addon_enabled__ only after register() returns, disable() gates on
        # that flag, and the failed module is dropped from sys.modules. Park
        # the session so the *next* enable reuses it — mm_init cannot be
        # called twice per process, so dropping it here would leave the add-on
        # inactive until Blender restarts. That matters most in the dev-reload
        # workflow keep-alive exists for, where _wire_up() freshly compiles
        # every sub-module on each enable and one bad edit lands here.
        #
        # Undo whatever _wire_up() managed to complete before failing: without
        # this, the next (successful) enable wires a *second* copy of every
        # handler/msgbus subscription from a fresh module instance, and the
        # dead instance's copies are unreachable by any future unregister().
        _teardown_wiring()
        setattr(sys, _STATE_ATTR, (lib, handle, _session_id))
        _lib = None
        _handle = None
        raise


def _wire_up(lib, handle) -> None:
    """Point the sub-modules at the live session, install the hooks, arm the timer.

    Split out of register() so a failure part-way through can re-park the live
    session instead of losing it — see register()'s except branch.
    """
    # Wire the sub-modules with the active lib + handle.
    from . import actions, crash_harvester, handlers, recorder

    crash_harvester.set_context(lib, handle)
    handlers.set_context(lib, handle)
    recorder.set_context(lib, handle)
    actions.set_context(lib, handle)

    crash_harvester.register_startup_harvest()
    handlers.register()
    recorder.set_event_callback(actions.drain_operators)
    recorder.register()
    actions.register()

    # Backstop for exceptions that reach the interpreter top level. Blender
    # swallows most operator/timer/handler exceptions before they get here, so
    # this is a backstop, not the primary capture (the add-on's own callbacks
    # guard themselves).
    global _prev_excepthook
    if sys.excepthook is not _telemetry_excepthook:
        _prev_excepthook = sys.excepthook
        sys.excepthook = _telemetry_excepthook

    # Flush on interpreter exit (belt-and-suspenders alongside mm_shutdown).
    # Re-pointed at the current module instance on every register(), so a
    # keep-alive reload cycle keeps exactly one hook (no stacking) and that
    # hook always reads the namespace holding the live session. A reload that
    # builds a *fresh* module namespace — what the VS Code Blender-Dev
    # extension does by purging sys.modules before re-enabling — would
    # otherwise leave atexit holding a _shutdown whose globals are dead and
    # whose sys cache the new register() already consumed, so the exit flush
    # would silently find nothing.
    prev_hook = getattr(sys, _ATEXIT_HOOK_ATTR, None)
    if prev_hook is not _shutdown:
        if prev_hook is not None:
            atexit.unregister(prev_hook)
        atexit.register(_shutdown)
        setattr(sys, _ATEXIT_HOOK_ATTR, _shutdown)

    # Periodic flush timer.
    try:
        if not bpy.app.timers.is_registered(_periodic_flush):
            bpy.app.timers.register(
                _periodic_flush, first_interval=30.0, persistent=True
            )
    except Exception:
        pass

    lib.log(
        handle,
        4,
        "blender.addon",
        f"Micromegas add-on registered version={_ADDON_VERSION} commit={_COMMIT}",
    )  # INFO=4
    # Use INFO level (4) for the startup log
    lib.log(handle, 4, "blender.addon", f"session_id={_session_id}")


def _teardown_wiring() -> None:
    """Undo whatever _wire_up() installed: hooks, sub-module registration.

    Used both by unregister() (the expected, full teardown) and by
    register()'s except branch (rollback after a partial _wire_up() failure).
    In the rollback case some steps below may never have run — each step is
    therefore independent and individually guarded, so one missing/failing
    step neither raises nor prevents the others from running, and this helper
    itself can never mask the caller's original exception.
    """
    global _prev_excepthook

    if _prev_excepthook is not None:
        try:
            sys.excepthook = _prev_excepthook
        except Exception:
            pass
        _prev_excepthook = None

    try:
        bpy.app.timers.unregister(_periodic_flush)
    except Exception:
        pass

    try:
        from . import actions

        actions.unregister()
    except Exception:
        pass

    try:
        from . import recorder

        recorder.set_event_callback(None)
        recorder.unregister()
    except Exception:
        pass

    try:
        from . import handlers

        handlers.unregister()
    except Exception:
        pass

    try:
        from . import crash_harvester

        crash_harvester.unregister_startup_harvest()
    except Exception:
        pass


def _shutdown():
    """Really shut down the native telemetry session (mm_shutdown).

    Looks at the currently active module globals first; if those are empty
    (a keep-alive unregister moved the live session to the sys cache instead
    of tearing it down), falls back to that cache. Either way, this is the
    one place that finds "whatever session is still alive" and kills it for
    real — used both at true process exit (atexit) and by an unregister()
    with keep-alive off.
    """
    global _lib, _handle
    lib, handle = _lib, _handle
    if lib is None or handle is None:
        cached = getattr(sys, _STATE_ATTR, None)
        if cached is not None:
            lib, handle = cached[0], cached[1]
    _lib = None
    _handle = None
    if hasattr(sys, _STATE_ATTR):
        delattr(sys, _STATE_ATTR)
    if lib is None or handle is None:
        return
    lib.log(handle, 4, "blender.addon", "Micromegas add-on shutting down")
    lib.flush(handle)
    lib.shutdown(handle)


def unregister():
    global _lib, _handle

    keep_alive = _is_keep_alive_enabled()

    _teardown_wiring()

    if keep_alive and _lib is not None and _handle is not None:
        # Park the live session on sys instead of shutting it down, so a
        # same-process re-register (e.g. VS Code Blender-Dev's re-enable on
        # every launch) can reuse it — mm_init can't be called twice in one
        # process.
        setattr(sys, _STATE_ATTR, (_lib, _handle, _session_id))
        _lib = None
        _handle = None
    else:
        _shutdown()

    try:
        bpy.utils.unregister_class(MicromegasAddonPreferences)
    except Exception:
        pass
