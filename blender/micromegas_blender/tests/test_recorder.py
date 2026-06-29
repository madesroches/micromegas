"""Tests for recorder.py: event callback injection."""

import types

import pytest

from micromegas_blender import recorder


class FakeEvent:
    def __init__(self, etype, value="PRESS"):
        self.type = etype
        self.value = value


_FAKE_CONTEXT = types.SimpleNamespace(area=None)


@pytest.fixture(autouse=True)
def _wire(rec_lib):
    recorder.set_context(rec_lib, object())
    recorder._registered = True
    yield
    recorder._registered = False
    recorder.set_context(None, None)
    recorder.set_event_callback(None)


def _make_modal_instance():
    """Return a MICROMEGAS_OT_recorder instance aligned to the current generation."""
    op = recorder.MICROMEGAS_OT_recorder()
    op._generation = recorder._generation
    return op


def test_event_callback_fires_for_discrete_event():
    calls = []
    recorder.set_event_callback(lambda: calls.append(1))
    op = _make_modal_instance()
    op.modal(_FAKE_CONTEXT, FakeEvent("A", "PRESS"))
    assert calls, "callback must fire for a discrete (non-SKIP_TYPES) event"


def test_event_callback_fires_on_release():
    calls = []
    recorder.set_event_callback(lambda: calls.append(1))
    op = _make_modal_instance()
    op.modal(_FAKE_CONTEXT, FakeEvent("A", "RELEASE"))
    assert calls, "callback must fire for RELEASE events (not only PRESS)"


def test_event_callback_not_fired_for_skip_types():
    calls = []
    recorder.set_event_callback(lambda: calls.append(1))
    op = _make_modal_instance()
    for etype in recorder._SKIP_TYPES:
        op.modal(_FAKE_CONTEXT, FakeEvent(etype))
    assert not calls, "callback must not fire for _SKIP_TYPES events"


def test_event_callback_not_fired_for_timer():
    calls = []
    recorder.set_event_callback(lambda: calls.append(1))
    op = _make_modal_instance()
    op.modal(_FAKE_CONTEXT, FakeEvent("TIMER"))
    assert not calls


def test_raising_callback_does_not_break_passthrough():
    def _bad():
        raise RuntimeError("drain exploded")

    recorder.set_event_callback(_bad)
    op = _make_modal_instance()
    result = op.modal(_FAKE_CONTEXT, FakeEvent("A", "PRESS"))
    assert result == {
        "PASS_THROUGH"
    }, "a raising callback must not prevent PASS_THROUGH"


def test_no_callback_is_safe():
    recorder.set_event_callback(None)
    op = _make_modal_instance()
    result = op.modal(_FAKE_CONTEXT, FakeEvent("A", "PRESS"))
    assert result == {"PASS_THROUGH"}
