"""Pytest fixtures + a minimal fake Blender runtime.

The add-on imports ``bpy`` / ``gpu`` / ``addon_utils`` at module load, none of
which exist outside Blender. We install lightweight fakes into ``sys.modules``
before any add-on module is imported so the binding and handlers can be
exercised out-of-Blender — no Blender runtime required. ``binding.py`` stays
pure-ctypes and bpy-free, so it imports unchanged.
"""

import os
import sys
import types

import pytest

# tests/ -> micromegas_blender/ -> blender/  (so `import micromegas_blender` works)
_BLENDER_DIR = os.path.dirname(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
)
if _BLENDER_DIR not in sys.path:
    sys.path.insert(0, _BLENDER_DIR)


def _make_fake_handlers() -> types.SimpleNamespace:
    ns = types.SimpleNamespace()
    for name in (
        "load_post",
        "save_post",
        "undo_post",
        "redo_post",
        "render_pre",
        "render_post",
        "render_cancel",
        "frame_change_post",
        "depsgraph_update_post",
        "load_factory_startup_post",
    ):
        setattr(ns, name, [])
    ns.persistent = lambda fn: fn  # identity decorator
    return ns


class _FakeAddonsCollection(dict):
    """Dict-like stand-in for bpy.context.preferences.addons: supports both
    `.keys()` (used for the enabled-addons fingerprint) and `[module_name]`
    lookup (used by `_is_keep_alive_enabled()`)."""


def _make_fake_context() -> types.SimpleNamespace:
    return types.SimpleNamespace(
        scene=types.SimpleNamespace(
            render=types.SimpleNamespace(engine="CYCLES"),
            objects=[],
            frame_current=1,
        ),
        preferences=types.SimpleNamespace(addons=_FakeAddonsCollection()),
        mode="OBJECT",
        workspace=types.SimpleNamespace(
            name="Layout",
            tools=types.SimpleNamespace(
                from_space_view3d_mode=lambda mode, create=False: None
            ),
        ),
        window_manager=types.SimpleNamespace(operators=[]),
        active_object=None,
        area=None,
    )


def _install_fake_modules() -> None:
    bpy = types.ModuleType("bpy")
    bpy.app = types.SimpleNamespace(
        handlers=_make_fake_handlers(),
        timers=types.SimpleNamespace(
            is_registered=lambda fn: False,
            register=lambda *a, **k: None,
            unregister=lambda fn: None,
        ),
        version=(4, 2, 0),
        build_hash=b"deadbeef",
        background=False,
    )
    bpy.types = types.SimpleNamespace(
        LayerObjects=object, Operator=object, AddonPreferences=object
    )
    bpy.props = types.SimpleNamespace(BoolProperty=lambda **kwargs: None)
    bpy.context = _make_fake_context()
    bpy.data = types.SimpleNamespace(filepath="")
    bpy.msgbus = types.SimpleNamespace(
        subscribe_rna=lambda **k: None, clear_by_owner=lambda owner: None
    )
    bpy.ops = types.SimpleNamespace()
    bpy.utils = types.SimpleNamespace(
        register_class=lambda c: None, unregister_class=lambda c: None
    )
    sys.modules["bpy"] = bpy

    gpu = types.ModuleType("gpu")
    gpu.platform = types.SimpleNamespace(
        renderer_get=lambda: "FakeGPU 9000",
        vendor_get=lambda: "FakeVendor",
        backend_type_get=lambda: "OPENGL",
        version_get=lambda: "4.6 (Core Profile)",
    )
    sys.modules["gpu"] = gpu

    addon_utils = types.ModuleType("addon_utils")
    addon_utils.modules = lambda: []
    sys.modules["addon_utils"] = addon_utils


_install_fake_modules()


class RecordingLib:
    """Stand-in for binding.MicromegasLib that records every call."""

    def __init__(self) -> None:
        self.logs: list[tuple] = []
        self.metrics: list[tuple] = []

    def log(self, handle, level, target, msg) -> None:
        self.logs.append((level, target, msg))

    def metric_f(self, handle, name, unit, value) -> None:
        self.metrics.append((name, unit, float(value)))

    def metric_i(self, handle, name, unit, value) -> None:
        self.metrics.append((name, unit, int(value)))

    def flush(self, handle) -> None:
        pass


@pytest.fixture
def rec_lib():
    return RecordingLib()


@pytest.fixture
def fake_bpy():
    """The installed fake bpy module, reset to a clean context per test."""
    bpy = sys.modules["bpy"]
    bpy.context = _make_fake_context()
    bpy.data = types.SimpleNamespace(filepath="")
    return bpy


# --- operator-history helpers (shared by test_actions / test_redo_panel) ----


class FakeOp:
    """Stand-in for a stored ``wm.operators`` history entry.

    ``kw=None`` models the real case where a stored entry's parameters are
    unreadable: ``as_keywords()`` raises rather than returning a dict.
    """

    def __init__(self, idname, name="", kw=None):
        self.bl_idname = idname
        self.name = name
        self._kw = kw

    def as_keywords(self):
        if self._kw is None:
            raise RuntimeError("params unavailable on stored history entry")
        return self._kw

    def as_pointer(self):
        return id(self)


class MacroSubOp:
    """Stand-in for a macro's sub-operator reference: has bl_rna but not
    as_keywords, per actions._is_macro_subop_ref."""

    bl_rna = object()


def set_ops(fake_bpy, ops) -> None:
    """Replace the fake operator-history ring, oldest -> newest."""
    fake_bpy.context.window_manager.operators = ops


# Every cross-poll global in `actions`. Reset as a set so a test module can
# never silently depend on state a sibling module left behind.
_ACTIONS_POLL_STATE = (
    "_prev_op_ptrs",
    "_last_op_ptr",
    "_last_op_msg",
    "_last_mode",
    "_last_workspace",
    "_last_tool",
    "_last_addons",
)


@pytest.fixture
def wired_actions(rec_lib, fake_bpy):
    """`actions` wired to the recording lib with all poll state reset.

    Not autouse — modules that exercise `actions` opt in with a one-line autouse
    fixture of their own, so unrelated test modules are left untouched.
    """
    from micromegas_blender import actions

    actions.set_context(rec_lib, object())
    for name in _ACTIONS_POLL_STATE:
        setattr(actions, name, None)
    yield actions
    actions.set_context(None, None)
