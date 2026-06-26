"""Tests for the process fingerprint in __init__._build_process_properties."""

import sys

import micromegas_blender as mm


def test_fingerprint_includes_new_dimensions(fake_bpy):
    props = mm._build_process_properties()

    # Existing dimensions still present.
    assert props["blender_version"] == "4.2.0"
    assert props["platform"] == sys.platform

    # GPU dimensions.
    assert props["gpu_renderer"] == "FakeGPU 9000"
    assert props["gpu_vendor"] == "FakeVendor"
    assert props["gpu_backend"] == "OPENGL"
    assert props["gpu_driver"] == "4.6 (Core Profile)"

    # System dimensions.
    assert int(props["cpu_count"]) >= 1
    assert props["python_version"] == sys.version.split()[0]
    assert props["background"] == "false"
    assert props["render_engine"] == "CYCLES"
    if sys.platform == "linux":
        assert int(props["total_ram_mb"]) > 0


def test_one_failing_source_does_not_drop_the_rest(fake_bpy, monkeypatch):
    # GPU renderer raises — its key is skipped, everything else survives.
    def boom():
        raise RuntimeError("no GPU context (background mode)")

    monkeypatch.setattr(sys.modules["gpu"].platform, "renderer_get", boom)

    props = mm._build_process_properties()
    assert "gpu_renderer" not in props
    assert props["gpu_vendor"] == "FakeVendor"  # sibling GPU call still works
    assert props["blender_version"] == "4.2.0"  # core block untouched


def test_enabled_addons_serialized_as_name_at_version(fake_bpy, monkeypatch):
    class FakeMod:
        pass

    mod = FakeMod()
    mod.__name__ = "cool_addon"
    mod.bl_info = {"version": (1, 2, 3)}

    monkeypatch.setattr(sys.modules["addon_utils"], "modules", lambda: [mod])
    monkeypatch.setattr(
        fake_bpy.context.preferences.addons, "keys", lambda: ["cool_addon"]
    )

    props = mm._build_process_properties()
    assert props["enabled_addons"] == "cool_addon@1.2.3"
