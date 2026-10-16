"""Tests for the ctypes binding that don't need the native library."""

import pytest

from micromegas_blender import binding


class StubFn:
    def __init__(self) -> None:
        self.calls: list[tuple] = []

    def __call__(self, *args) -> None:
        self.calls.append(args)


class StubLib:
    def __init__(self) -> None:
        for name in (
            "mm_log",
            "mm_log_tagged",
            "mm_metric_i",
            "mm_metric_i_tagged",
            "mm_metric_f",
            "mm_metric_f_tagged",
        ):
            setattr(self, name, StubFn())


@pytest.fixture
def lib():
    # __init__ loads the shared library, so build the object without it.
    instance = object.__new__(binding.MicromegasLib)
    instance._lib = StubLib()
    return instance


def read_arrays(keys, values, count):
    return (
        [keys[i] for i in range(count)],
        [values[i] for i in range(count)],
    )


def test_property_arrays_hold_keys_values_and_count():
    keys, values, count = binding._property_arrays({"a": "1", "b": "2"})
    assert count == 2
    assert read_arrays(keys, values, count) == ([b"a", b"b"], [b"1", b"2"])


@pytest.mark.parametrize("empty", [None, {}])
def test_property_arrays_of_nothing(empty):
    assert binding._property_arrays(empty) == (None, None, 0)


def test_log_with_properties_calls_the_tagged_symbol(lib):
    lib.log("h", 4, "t", "m", {"k": "v"})
    assert lib._lib.mm_log.calls == []
    [(handle, level, target, msg, keys, values, count)] = lib._lib.mm_log_tagged.calls
    assert (handle, level, target, msg, count) == ("h", 4, b"t", b"m", 1)
    assert read_arrays(keys, values, count) == ([b"k"], [b"v"])


@pytest.mark.parametrize("empty", [None, {}])
def test_log_without_properties_calls_the_untagged_symbol(lib, empty):
    lib.log("h", 4, "t", "m", empty)
    assert lib._lib.mm_log_tagged.calls == []
    assert lib._lib.mm_log.calls == [("h", 4, b"t", b"m")]


def test_metric_i_with_properties_calls_the_tagged_symbol(lib):
    lib.metric_i("h", "n", "u", 7, {"k": "v"})
    assert lib._lib.mm_metric_i.calls == []
    [(handle, name, unit, keys, values, count, value)] = lib._lib.mm_metric_i_tagged.calls
    assert (handle, name, unit, count, value) == ("h", b"n", b"u", 1, 7)
    assert read_arrays(keys, values, count) == ([b"k"], [b"v"])


def test_metric_f_with_properties_calls_the_tagged_symbol(lib):
    lib.metric_f("h", "n", "u", 1.5, {"k": "v"})
    assert lib._lib.mm_metric_f.calls == []
    [(handle, name, unit, keys, values, count, value)] = lib._lib.mm_metric_f_tagged.calls
    assert (handle, name, unit, count, value) == ("h", b"n", b"u", 1, 1.5)
    assert read_arrays(keys, values, count) == ([b"k"], [b"v"])


@pytest.mark.parametrize("empty", [None, {}])
def test_metrics_without_properties_call_the_untagged_symbols(lib, empty):
    lib.metric_i("h", "n", "u", 7, empty)
    lib.metric_f("h", "n", "u", 1.5, empty)
    assert lib._lib.mm_metric_i_tagged.calls == []
    assert lib._lib.mm_metric_f_tagged.calls == []
    assert lib._lib.mm_metric_i.calls == [("h", b"n", b"u", 7)]
    assert lib._lib.mm_metric_f.calls == [("h", b"n", b"u", 1.5)]

