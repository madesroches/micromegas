"""Tests for the dev-mode session-caching behavior in __init__.py.

Dev mode exists because the VS Code Blender-Dev extension re-enables the
add-on on every launch (bpy.ops.preferences.addon_enable on an
already-enabled module -> Blender calls unregister() then register() again,
same process). The native lib can only mm_init() once per process, so
without dev mode that second register() fails. See __init__._is_dev_mode /
_STATE_ATTR.
"""

import sys

import micromegas_blender as mm


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


def _set_dev_mode(fake_bpy, enabled: bool) -> None:
    addons = fake_bpy.context.preferences.addons
    prefs = type("Prefs", (), {"preferences": type("P", (), {"dev_mode": enabled})()})
    addons[mm.__package__] = prefs()


def _clear_state():
    mm._lib = None
    mm._handle = None
    mm._session_id = ""
    for attr in (mm._STATE_ATTR, mm._ATEXIT_REGISTERED_ATTR):
        if hasattr(sys, attr):
            delattr(sys, attr)


def _register_with_lib(monkeypatch, lib):
    monkeypatch.setattr(mm, "_load_lib", lambda: lib)
    mm.register()


def test_dev_mode_off_unregister_shuts_down_and_clears_state(
    fake_bpy, monkeypatch
):
    _clear_state()
    _set_dev_mode(fake_bpy, False)
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


def test_dev_mode_on_unregister_parks_state_without_shutdown(fake_bpy, monkeypatch):
    _clear_state()
    _set_dev_mode(fake_bpy, True)
    lib = CountingLib()
    try:
        _register_with_lib(monkeypatch, lib)
        assert lib.init_calls == 1

        mm.unregister()

        assert lib.shutdown_calls == 0
        assert mm._lib is None
        assert mm._handle is None
        cached = getattr(sys, mm._STATE_ATTR, None)
        assert cached is not None
        cached_lib, cached_handle, cached_session_id = cached
        assert cached_lib is lib
        assert cached_handle is not None
        assert cached_session_id == mm._session_id or cached_session_id
    finally:
        _clear_state()


def test_dev_mode_on_reregister_reuses_cached_session(fake_bpy, monkeypatch):
    _clear_state()
    _set_dev_mode(fake_bpy, True)
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
    finally:
        _clear_state()


def test_shutdown_falls_back_to_sys_cache(fake_bpy, monkeypatch):
    """Simulates atexit firing after a dev-mode unregister parked the state."""
    _clear_state()
    _set_dev_mode(fake_bpy, True)
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


def test_atexit_registered_only_once_across_dev_mode_cycle(fake_bpy, monkeypatch):
    _clear_state()
    _set_dev_mode(fake_bpy, True)
    lib = CountingLib()
    calls = []
    monkeypatch.setattr(
        mm.atexit, "register", lambda fn: calls.append(fn)
    )
    try:
        _register_with_lib(monkeypatch, lib)
        mm.unregister()
        mm.register()

        assert len(calls) == 1
    finally:
        _clear_state()
