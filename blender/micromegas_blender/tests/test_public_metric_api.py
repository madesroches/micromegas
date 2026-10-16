"""Tests for the public metric_i() / metric_f() API in __init__.py."""

import pytest

import micromegas_blender as mm


@pytest.fixture
def session(rec_lib):
    mm._lib = rec_lib
    mm._handle = object()
    try:
        yield rec_lib
    finally:
        mm._lib = None
        mm._handle = None


def test_metric_i_forwards_to_active_session(session):
    mm.metric_i("queue.depth", "count", 12)
    assert session.metrics == [("queue.depth", "count", 12)]
    assert session.metric_props == [None]


def test_metric_f_forwards_to_active_session(session):
    mm.metric_f("frame_time", "ms", 16.7)
    assert session.metrics == [("frame_time", "ms", 16.7)]
    assert session.metric_props == [None]


def test_metric_forwards_properties_as_strings(session):
    mm.metric_i("queue.depth", "count", 1, properties={"queue": "render", "n": 2})
    mm.metric_f("frame_time", "ms", 1.5, properties={"scene": "main"})
    assert session.metric_props == [{"queue": "render", "n": "2"}, {"scene": "main"}]


def test_metrics_are_noop_without_active_session(rec_lib):
    mm._lib = None
    mm._handle = None
    mm.metric_i("queue.depth", "count", 1)
    mm.metric_f("frame_time", "ms", 1.0)
    assert rec_lib.metrics == []


def test_metrics_swallow_exceptions_from_lib():
    class RaisingLib:
        def metric_i(self, handle, name, unit, value, properties=None):
            raise RuntimeError("native call failed")

        def metric_f(self, handle, name, unit, value, properties=None):
            raise RuntimeError("native call failed")

    mm._lib = RaisingLib()
    mm._handle = object()
    try:
        mm.metric_i("queue.depth", "count", 1)  # must not raise
        mm.metric_f("frame_time", "ms", 1.0)  # must not raise
    finally:
        mm._lib = None
        mm._handle = None
