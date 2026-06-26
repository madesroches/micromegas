"""Tests for handlers.py: RSS reader, periodic sampling, depsgraph interval."""

import sys

import pytest

from micromegas_blender import handlers


@pytest.fixture(autouse=True)
def _wire(rec_lib):
    """Wire handlers to a recording lib and reset module state per test."""
    handlers.set_context(rec_lib, object())
    handlers._last_depsgraph_time = 0.0
    yield
    handlers.set_context(None, None)


@pytest.mark.skipif(sys.platform != "linux", reason="Linux RSS reader")
def test_read_rss_linux_returns_plausible_value():
    rss = handlers._read_process_rss_mb()
    assert rss > 0
    # A live CPython process is comfortably between 1 MB and 100 GB.
    assert 1.0 < rss < 100_000.0


def test_on_periodic_emits_rss_and_object_count(rec_lib, fake_bpy):
    fake_bpy.context.scene.objects = [object(), object(), object()]
    handlers.on_periodic()
    names = {name for name, _unit, _value in rec_lib.metrics}
    if sys.platform == "linux":
        assert "blender.rss_mb" in names
    assert ("blender.object_count", "count", 3) in rec_lib.metrics


def test_depsgraph_interval_skips_first_sample(rec_lib, fake_bpy):
    # First update after a (re)set emits nothing — no prior timestamp.
    handlers._on_depsgraph_update_post(fake_bpy.context.scene, None)
    assert not _interval_metrics(rec_lib)
    # Second update now has a baseline → emits one interval sample.
    handlers._on_depsgraph_update_post(fake_bpy.context.scene, None)
    assert len(_interval_metrics(rec_lib)) == 1


def test_load_resets_depsgraph_baseline(rec_lib, fake_bpy):
    # Establish a baseline and emit one sample.
    handlers._on_depsgraph_update_post(fake_bpy.context.scene, None)
    handlers._on_depsgraph_update_post(fake_bpy.context.scene, None)
    assert len(_interval_metrics(rec_lib)) == 1

    # A file load must reset the baseline so the first post-load update does not
    # emit an inflated interval spanning the load boundary.
    handlers._on_load_post(fake_bpy.context.scene, None)
    handlers._on_depsgraph_update_post(fake_bpy.context.scene, None)
    assert len(_interval_metrics(rec_lib)) == 1  # unchanged — first post-load skipped

    handlers._on_depsgraph_update_post(fake_bpy.context.scene, None)
    assert len(_interval_metrics(rec_lib)) == 2  # second post-load emits


def _interval_metrics(rec_lib):
    return [
        m for m in rec_lib.metrics if m[0] == "blender.depsgraph_update_interval_ms"
    ]
