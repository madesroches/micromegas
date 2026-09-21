#!/usr/bin/env python3
"""Unit tests for rust_ci.py's `_run_steps` orchestration logic.

These use trivial constructed commands (`sys.executable -c ...`) instead of real
cargo/audit/deny invocations, so they run fast and without a Rust toolchain.
"""

import subprocess
import sys

import pytest

from rust_ci import _run_steps

PASS_CMD = f'{sys.executable} -c "import sys; sys.exit(0)"'
FAIL_CMD = f'{sys.executable} -c "import sys; sys.exit(1)"'


def append_cmd(path, marker):
    return f'{sys.executable} -c "open(r\'{path}\', \'a\').write(\'{marker}\\n\')"'


def read_order(path):
    if not path.exists():
        return []
    return path.read_text().splitlines()


def test_parallel_failure_causes_nonzero_exit_even_when_sequential_pass():
    sequential_steps = [
        ("Step A", PASS_CMD, None),
        ("Step B", PASS_CMD, None),
    ]
    parallel_steps = [
        ("Good Parallel", PASS_CMD, None),
        ("Bad Parallel", FAIL_CMD, None),
    ]
    with pytest.raises(SystemExit) as exc_info:
        _run_steps("Test", sequential_steps, parallel_steps)
    assert exc_info.value.code == 1


def test_sequential_steps_run_in_order_when_parallel_steps_all_pass(tmp_path):
    order_file = tmp_path / "order.txt"
    sequential_steps = [
        ("Step A", append_cmd(order_file, "A"), None),
        ("Step B", append_cmd(order_file, "B"), None),
        ("Step C", append_cmd(order_file, "C"), None),
    ]
    parallel_steps = [
        ("Good Parallel 1", PASS_CMD, None),
        ("Good Parallel 2", PASS_CMD, None),
    ]
    _run_steps("Test", sequential_steps, parallel_steps)
    assert read_order(order_file) == ["A", "B", "C"]


def test_sequential_failure_propagates():
    sequential_steps = [
        ("Step A", PASS_CMD, None),
        ("Step B", FAIL_CMD, None),
        ("Step C", PASS_CMD, None),
    ]
    parallel_steps = [
        ("Good Parallel", PASS_CMD, None),
    ]
    with pytest.raises(subprocess.CalledProcessError):
        _run_steps("Test", sequential_steps, parallel_steps)


def test_single_list_call_shape_runs_everything_sequentially(tmp_path):
    order_file = tmp_path / "order.txt"
    steps = [
        ("Step A", append_cmd(order_file, "A"), None),
        ("Step B", append_cmd(order_file, "B"), None),
        ("Step C", append_cmd(order_file, "C"), None),
    ]
    _run_steps("Test", steps)
    assert read_order(order_file) == ["A", "B", "C"]


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-v"]))
