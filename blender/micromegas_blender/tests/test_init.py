"""Tests for the keep-alive session-caching behavior in __init__.py.

Keep-alive exists because the VS Code Blender-Dev extension re-enables the
add-on on every launch (bpy.ops.preferences.addon_enable on an
already-enabled module -> Blender calls unregister() then register() again,
same process). The native lib can only mm_init() once per process, so
without keep-alive that second register() fails. See
__init__._is_keep_alive_enabled / _STATE_ATTR.
"""

import glob
import importlib
import sys

import pytest

import micromegas_blender as mm

_PKG = "micromegas_blender"


@pytest.fixture(autouse=True)
def _no_real_crash_harvest(monkeypatch):
    """Keep mm.register() from harvesting genuine crash files off this machine.

    register() -> _wire_up() -> crash_harvester.register_startup_harvest()
    unconditionally globs /tmp (and tempfile.gettempdir()) for *.crash.txt
    files and os.rename()s any match — real Blender crash reports on a
    developer machine included. Patched at the stdlib glob.glob level rather
    than on crash_harvester's own functions: the fresh-module-reload test
    below purges `micromegas_blender.crash_harvester` from sys.modules and
    re-imports it, which would silently drop a monkeypatch on that module's
    attributes, but `glob` is stdlib and never gets purged.
    """
    monkeypatch.setattr(glob, "glob", lambda *a, **k: [])


class CountingLib:
    """Stand-in for binding.MicromegasLib that counts init/shutdown calls."""

    def __init__(self) -> None:
        self.init_calls = 0
        self.shutdown_calls = 0
        self.logs: list[tuple] = []

    def init(self, sink_url=None, properties=None):
        self.init_calls += 1
        return object()

    def shutdown(self, handle) -> None:
        self.shutdown_calls += 1

    def log(self, handle, level, target, msg) -> None:
        self.logs.append((level, target, msg))

    def flush(self, handle) -> None:
        pass


def _set_keep_alive(fake_bpy, enabled: bool) -> None:
    addons = fake_bpy.context.preferences.addons
    prefs = type("Prefs", (), {"preferences": type("P", (), {"keep_alive": enabled})()})
    addons[mm.__package__] = prefs()


@pytest.fixture(autouse=True)
def _restore_excepthook():
    """Keep a register()'s sys.excepthook swap from outliving the test.

    register() installs the telemetry excepthook and not every test here pairs
    it with an unregister(); the fresh-namespace test additionally purges the
    module the installed hook belongs to, which would otherwise leave
    sys.excepthook pointing into a dead namespace for the rest of the process.
    """
    saved = sys.excepthook
    yield
    sys.excepthook = saved


def _clear_state():
    mm._lib = None
    mm._handle = None
    mm._session_id = ""
    mm._prev_excepthook = None
    for attr in (mm._STATE_ATTR, mm._ATEXIT_HOOK_ATTR):
        if hasattr(sys, attr):
            delattr(sys, attr)


def _register_with_lib(monkeypatch, lib):
    monkeypatch.setattr(mm, "_load_lib", lambda: lib)
    mm.register()


def test_keep_alive_off_unregister_shuts_down_and_clears_state(fake_bpy, monkeypatch):
    _clear_state()
    _set_keep_alive(fake_bpy, False)
    lib = CountingLib()
    try:
        _register_with_lib(monkeypatch, lib)
        assert lib.init_calls == 1

        mm.unregister()

        assert lib.shutdown_calls == 1
        assert mm._lib is None
        assert mm._handle is None
        assert not hasattr(sys, mm._STATE_ATTR)
    finally:
        _clear_state()


def test_keep_alive_on_unregister_parks_state_without_shutdown(fake_bpy, monkeypatch):
    _clear_state()
    _set_keep_alive(fake_bpy, True)
    lib = CountingLib()
    try:
        _register_with_lib(monkeypatch, lib)
        assert lib.init_calls == 1
        session_id = mm._session_id
        assert session_id

        mm.unregister()

        assert lib.shutdown_calls == 0
        assert mm._lib is None
        assert mm._handle is None
        cached = getattr(sys, mm._STATE_ATTR, None)
        assert cached is not None
        cached_lib, cached_handle, cached_session_id = cached
        assert cached_lib is lib
        assert cached_handle is not None
        assert cached_session_id == session_id
    finally:
        _clear_state()


def test_keep_alive_on_reregister_reuses_cached_session(fake_bpy, monkeypatch):
    _clear_state()
    _set_keep_alive(fake_bpy, True)
    lib = CountingLib()
    try:
        _register_with_lib(monkeypatch, lib)
        first_session_id = mm._session_id
        mm.unregister()
        assert hasattr(sys, mm._STATE_ATTR)

        # Simulate a fresh module reload re-registering in the same process:
        # _load_lib must not be called again (mm_init can't run twice).
        monkeypatch.setattr(
            mm,
            "_load_lib",
            lambda: (_ for _ in ()).throw(
                AssertionError("_load_lib should not be called on cached re-register")
            ),
        )
        mm.register()

        assert lib.init_calls == 1  # unchanged: no second mm_init
        assert mm._lib is lib
        assert mm._session_id == first_session_id
        # The consumed cache must not linger once it's back in module globals.
        assert not hasattr(sys, mm._STATE_ATTR)
    finally:
        _clear_state()


def test_keep_alive_on_reregister_reuses_session_even_if_toggled_off(
    fake_bpy, monkeypatch
):
    """mm_init cannot be called twice in a process, so a parked session must
    be reused on the next register() even if keep_alive was turned off in
    between — there's no other way to keep telemetry working."""
    _clear_state()
    _set_keep_alive(fake_bpy, True)
    lib = CountingLib()
    try:
        _register_with_lib(monkeypatch, lib)
        mm.unregister()
        assert hasattr(sys, mm._STATE_ATTR)

        _set_keep_alive(fake_bpy, False)
        monkeypatch.setattr(
            mm,
            "_load_lib",
            lambda: (_ for _ in ()).throw(
                AssertionError("_load_lib should not be called on cached re-register")
            ),
        )
        mm.register()

        assert lib.init_calls == 1
        assert mm._lib is lib
    finally:
        _clear_state()


def test_shutdown_falls_back_to_sys_cache(fake_bpy, monkeypatch):
    """Simulates atexit firing after a keep-alive unregister parked the state."""
    _clear_state()
    _set_keep_alive(fake_bpy, True)
    lib = CountingLib()
    try:
        _register_with_lib(monkeypatch, lib)
        mm.unregister()
        assert hasattr(sys, mm._STATE_ATTR)

        mm._shutdown()

        assert lib.shutdown_calls == 1
        assert not hasattr(sys, mm._STATE_ATTR)
    finally:
        _clear_state()


def _boom_load_lib():
    raise AssertionError("_load_lib should not be called on cached re-register")


def test_register_failure_reparks_the_session_for_the_next_enable(
    fake_bpy, monkeypatch
):
    """A register() that raises after taking the session must not lose it.

    Blender never calls unregister() for a failed enable — addon_utils.enable()
    sets __addon_enabled__ only after register() returns, disable() gates on
    that flag, and the failed module is dropped from sys.modules. So if
    register() let go of the live session on its way out, mm_init could never
    run again and the add-on would be dead until Blender restarts: exactly what
    keep-alive exists to prevent. This is a live path in the dev-reload
    workflow, where _wire_up() freshly compiles every sub-module on each enable
    and one bad edit lands here.
    """
    _clear_state()
    _set_keep_alive(fake_bpy, True)
    lib = CountingLib()
    real_wire_up = mm._wire_up
    try:
        _register_with_lib(monkeypatch, lib)
        session_id = mm._session_id
        mm.unregister()  # keep-alive: session parked on sys

        def _boom(_lib, _handle):
            raise RuntimeError("bad edit in a sub-module")

        monkeypatch.setattr(mm, "_wire_up", _boom)
        monkeypatch.setattr(mm, "_load_lib", _boom_load_lib)
        with pytest.raises(RuntimeError):
            mm.register()

        # Session still live, re-parked, and the module globals left clean.
        assert lib.shutdown_calls == 0
        assert mm._lib is None
        assert mm._handle is None
        cached = getattr(sys, mm._STATE_ATTR, None)
        assert cached is not None
        assert cached[0] is lib
        assert cached[2] == session_id

        # The next enable picks it back up — no second mm_init.
        monkeypatch.setattr(mm, "_wire_up", real_wire_up)
        mm.register()
        assert lib.init_calls == 1
        assert mm._lib is lib
        assert mm._session_id == session_id
    finally:
        _clear_state()


def test_register_failure_parks_a_freshly_created_session(fake_bpy, monkeypatch):
    """Same recovery when the failing enable is the one that created the session.

    There is no prior park here — the session was just mm_init'd — but it is
    still the only one this process can ever have, so it must be parked rather
    than dropped.
    """
    _clear_state()
    _set_keep_alive(fake_bpy, False)
    lib = CountingLib()
    real_wire_up = mm._wire_up
    try:

        def _boom(_lib, _handle):
            raise RuntimeError("bad edit in a sub-module")

        monkeypatch.setattr(mm, "_wire_up", _boom)
        monkeypatch.setattr(mm, "_load_lib", lambda: lib)
        with pytest.raises(RuntimeError):
            mm.register()

        assert lib.init_calls == 1
        assert lib.shutdown_calls == 0
        assert getattr(sys, mm._STATE_ATTR, None) is not None

        monkeypatch.setattr(mm, "_wire_up", real_wire_up)
        monkeypatch.setattr(mm, "_load_lib", _boom_load_lib)
        mm.register()
        assert lib.init_calls == 1  # reused, not re-initialized
        assert mm._lib is lib
    finally:
        _clear_state()


def _patch_atexit(monkeypatch) -> list:
    """Capture the live atexit hooks in a list instead of the real registry."""
    hooks: list = []
    monkeypatch.setattr(mm.atexit, "register", lambda fn: hooks.append(fn))
    monkeypatch.setattr(
        mm.atexit,
        "unregister",
        lambda fn: hooks.remove(fn) if fn in hooks else None,
    )
    return hooks


def test_atexit_registered_only_once_across_keep_alive_cycle(fake_bpy, monkeypatch):
    _clear_state()
    _set_keep_alive(fake_bpy, True)
    lib = CountingLib()
    hooks = _patch_atexit(monkeypatch)
    try:
        _register_with_lib(monkeypatch, lib)
        mm.unregister()
        mm.register()

        assert len(hooks) == 1
    finally:
        _clear_state()


def test_atexit_hook_follows_a_fresh_module_namespace(fake_bpy, monkeypatch):
    """The atexit hook must belong to the namespace holding the live session.

    The VS Code Blender-Dev extension purges the add-on's modules from
    sys.modules before re-enabling, so the second register() runs in a brand
    new namespace. If atexit still held the *previous* namespace's _shutdown,
    that hook would see its own `_lib`/`_handle` as None and find the sys cache
    already consumed by the new register() — the exit flush would silently do
    nothing and the tail of the session would be lost.
    """
    _clear_state()
    _set_keep_alive(fake_bpy, True)
    lib = CountingLib()
    hooks = _patch_atexit(monkeypatch)
    saved = {
        name: module
        for name, module in sys.modules.items()
        if name == _PKG or name.startswith(_PKG + ".")
    }
    try:
        _register_with_lib(monkeypatch, lib)
        mm.unregister()  # keep-alive: session parked on sys, not shut down

        for name in saved:
            del sys.modules[name]
        fresh = importlib.import_module(_PKG)
        assert fresh is not mm
        monkeypatch.setattr(
            fresh,
            "_load_lib",
            lambda: (_ for _ in ()).throw(
                AssertionError("_load_lib should not be called on cached re-register")
            ),
        )
        fresh.register()
        assert fresh._lib is lib  # the parked session was reused

        assert len(hooks) == 1
        hooks[0]()  # simulate interpreter exit

        assert lib.shutdown_calls == 1
    finally:
        for name in [
            name for name in sys.modules if name == _PKG or name.startswith(_PKG + ".")
        ]:
            del sys.modules[name]
        sys.modules.update(saved)
        _clear_state()
