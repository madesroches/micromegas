#!/usr/bin/env python3
"""Python package CI validation script.

Runs the hermetic pytest subset plus a `black --check` gate against the
Poetry-managed virtualenv in python/micromegas, mirroring build/rust_ci.py's
and build/analytics_web_ci.py's "delegate the actual checks to a script, not
inline CI YAML" pattern.
"""
import pathlib
import subprocess
import sys

repo_root = pathlib.Path(__file__).parent.parent.absolute()
python_pkg_dir = repo_root / "python" / "micromegas"

# Explicit hermetic file list: pytest over the whole tests/ directory would
# also collect the integration suite, which requires a live service.
HERMETIC_TEST_ARGS = [
    "--doctest-modules",
    "micromegas/time.py",
    "tests/test_time.py",
    "tests/test_flightsql_headers.py",
    "tests/cli",
    "tests/test_query.py",
    "tests/test_web_client.py",
    "tests/test_screen_files.py",
    "tests/auth/test_oidc_unit.py",
    "tests/auth/test_client_credentials_unit.py",
]


def run(cmd):
    print(f"\n$ {' '.join(cmd)}  (cwd={python_pkg_dir})")
    return subprocess.run(cmd, cwd=python_pkg_dir).returncode


def check_interpreter_version(expected_version):
    """Assert the Poetry venv's interpreter matches `expected_version`.

    This must observe the venv's own interpreter via a `poetry run`
    subprocess rather than this script's `sys.version_info`: this script is
    invoked by whatever interpreter is on PATH (the matrix leg's
    setup-python on CI), so its own version is the matrix version by
    construction and could never catch Poetry silently resolving `poetry
    run` to the runner's default interpreter instead of the matrix one.
    """
    result = subprocess.run(
        [
            "poetry",
            "run",
            "python",
            "-c",
            "import sys; print('%d.%d' % sys.version_info[:2])",
        ],
        cwd=python_pkg_dir,
        capture_output=True,
        text=True,
        check=True,
    )
    actual_version = result.stdout.strip()
    if actual_version != expected_version:
        print(
            f"ERROR: expected the Poetry venv to use Python {expected_version}, "
            f"but `poetry run python` resolved to {actual_version}"
        )
        sys.exit(1)
    print(f"Poetry venv interpreter: Python {actual_version} (matches expected)")


def main():
    # The expected Python version is optional so this script also works for
    # local, non-matrix runs (e.g. `python3 build/python_ci.py`); CI passes
    # it explicitly as `python build/python_ci.py ${{ matrix.python-version }}`.
    expected_version = sys.argv[1] if len(sys.argv) > 1 else None
    if expected_version is not None:
        check_interpreter_version(expected_version)

    exit_code = run(["poetry", "run", "pytest"] + HERMETIC_TEST_ARGS)
    if exit_code != 0:
        sys.exit(exit_code)

    exit_code = run(["poetry", "run", "black", "--check", "."])
    if exit_code != 0:
        sys.exit(exit_code)

    print("\nPython package CI checks passed!")


if __name__ == "__main__":
    main()
