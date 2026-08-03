# Add `--version` to the CLI Entry Points Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1416

## Overview

None of the three `micromegas` console scripts (`micromegas-query`, `micromegas-screens`,
`micromegas-logout`) accept `--version`, and the package exposes no `micromegas.__version__`.
This makes it hard to tell which wheel version backs an installed console script, especially
across interpreter/shebang mismatches. This plan adds a `--version` action to each entry
point's argument parser and exposes `micromegas.__version__` for library users.

## Current State

- Package version lives in `python/micromegas/pyproject.toml:3` (`version = "0.29.0"`),
  read by Poetry/the build backend. It is not otherwise exposed at runtime today.
- Three console scripts are declared in `python/micromegas/pyproject.toml:37-39`:
  - `micromegas-logout = "micromegas.cli.logout:main"`
  - `micromegas-query = "micromegas.cli.query:main"`
  - `micromegas-screens = "micromegas.cli.screens:main"`
- Each entry point builds its own `argparse.ArgumentParser` in its own `main()`:
  - `python/micromegas/micromegas/cli/query.py:60` — flat parser, no subcommands.
  - `python/micromegas/micromegas/cli/logout.py:8` — flat parser, no arguments at all today.
  - `python/micromegas/micromegas/cli/screens.py:649` — parser with `add_subparsers(dest="command", required=True)`.
- `python/micromegas/micromegas/__init__.py` (23 lines) currently only re-exports submodules
  (`admin`, `auth`, `flightsql`, `oidc_connection`, `perfetto`, `time`) and defines `connect()`.
  It does not read or expose a package version.
- At runtime, `importlib.metadata.version("micromegas")` looks up whichever distribution is
  installed for the *running* interpreter, which is exactly the value we want to surface.

## Design

Use argparse's built-in `action="version"`, resolving the version via
`importlib.metadata.version("micromegas")` at call time (not import time), so a
`PackageNotFoundError` in an unusual/dev environment doesn't crash the CLI on unrelated
invocations before the flag is even used.

To avoid duplicating the same three lines across three files, add one small shared helper:

```python
# python/micromegas/micromegas/cli/version.py
import importlib.metadata


def package_version():
    """Return the installed micromegas package version, or 'unknown' if not installed."""
    try:
        return importlib.metadata.version("micromegas")
    except importlib.metadata.PackageNotFoundError:
        return "unknown"


def add_version_argument(parser):
    """Add a --version flag reporting the micromegas package version."""
    parser.add_argument(
        "--version",
        action="version",
        version=f"%(prog)s {package_version()}",
    )
```

Each CLI's `main()` calls `add_version_argument(parser)` right after constructing its
top-level `ArgumentParser`, before `parse_args()`. `%(prog)s` reuses each script's existing
`prog=` value, so output stays consistent with existing help text, e.g.:

```
$ micromegas-query --version
query 0.29.0
$ micromegas-screens --version
micromegas-screens 0.29.0
$ micromegas_logout --version
micromegas_logout 0.29.0
```

The issue's proposal additionally suggests reporting the interpreter (`... (Python 3.11.0 at
/usr/bin/python3.11)`). That's a reasonable companion but not required to solve the core
problem (which interpreter/venv the console script's wheel came from — `--version` alone
already answers that unambiguously since it's the same process). Adding it is a small
extension; see Open Questions.

For `screens.py`, `--version` is added to the **top-level** parser (before
`add_subparsers(...)`), not to each subcommand — this matches common CLI convention (git,
pip, poetry all support `tool --version` at the top level) and works correctly even though
the subparser is `required=True`: argparse's version action fires and calls `parser.exit()`
as soon as it's encountered, before the required-subcommand check runs, so
`micromegas-screens --version` (no subcommand) exits cleanly with the version string, exactly
like `--help` already does.

### `micromegas.__version__`

Add to `python/micromegas/micromegas/__init__.py`:

```python
from .cli.version import package_version

__version__ = package_version()
```

This reuses the same helper (single source of truth for the lookup/fallback logic) rather
than duplicating the `importlib.metadata.version` call.

## Implementation Steps

1. Create `python/micromegas/micromegas/cli/version.py` with `package_version()` and
   `add_version_argument()` as shown above.
2. In `python/micromegas/micromegas/cli/query.py`: import `add_version_argument` from
   `.version` and call `add_version_argument(parser)` after the `ArgumentParser(...)`
   construction in `main()` (around `python/micromegas/micromegas/cli/query.py:60`), before
   the other `add_argument` calls (order among flags doesn't matter to argparse, but keep it
   near the top for readability).
3. In `python/micromegas/micromegas/cli/logout.py`: same pattern — call
   `add_version_argument(parser)` after the `ArgumentParser(...)` construction in `main()`
   (`python/micromegas/micromegas/cli/logout.py:8`).
4. In `python/micromegas/micromegas/cli/screens.py`: call `add_version_argument(parser)`
   right after the top-level `ArgumentParser(...)` construction (`python/micromegas/micromegas/cli/screens.py:649`)
   and before `add_subparsers(...)`.
5. In `python/micromegas/micromegas/__init__.py`: import `package_version` from
   `.cli.version` and set `__version__ = package_version()`.
6. Add tests (see Testing Strategy).
7. Run `poetry run black` on all changed files.

## Files to Modify

- `python/micromegas/micromegas/cli/version.py` (new)
- `python/micromegas/micromegas/cli/query.py`
- `python/micromegas/micromegas/cli/logout.py`
- `python/micromegas/micromegas/cli/screens.py`
- `python/micromegas/micromegas/__init__.py`
- `python/micromegas/tests/cli/test_version.py` (new)

## Trade-offs

- **Shared helper vs. inlining `importlib.metadata.version(...)` three times**: a tiny shared
  module is preferred here since the fallback-to-"unknown" behavior and the string used to
  look up the distribution name must stay identical across all three entry points and
  `__version__`; duplicating it three-plus times would risk drift (e.g. someone updating one
  call site's error handling but not the others).
- **`importlib.metadata.version()` vs. a hardcoded `__version__` string kept in sync with
  `pyproject.toml`**: reading from installed package metadata is the standard approach for
  distributions built with Poetry/setuptools and requires no build-time codegen step; it also
  means the reported version always matches what `pip`/`poetry` actually installed, which is
  the exact ambiguity the issue is about. This does mean `__version__` and `--version` return
  "unknown" when running from a source checkout without the package installed (e.g. via
  `PYTHONPATH` instead of `pip install -e .`) — acceptable since that's a dev-only edge case
  and "unknown" is still a clear, non-crashing signal.
- **Top-level `--version` on `screens.py` vs. per-subcommand**: top-level matches the
  conventions of comparable multi-command CLIs (git, poetry, pip) and requires no change to
  each subparser.

## Testing Strategy

Add `python/micromegas/tests/cli/test_version.py` covering:
- `package_version()` returns the string from `importlib.metadata.version("micromegas")`
  when the package is installed (it will be, under the poetry test venv).
- `package_version()` returns `"unknown"` when `importlib.metadata.version` raises
  `PackageNotFoundError` (monkeypatch `importlib.metadata.version` to raise).
- Each of the three `main()` entry points exits with code `0` and prints a version string
  containing the package version when invoked with `--version`, using
  `monkeypatch.setattr(sys, "argv", [...])` + `pytest.raises(SystemExit)` (the standard
  pattern for testing argparse's `action="version"`, since it calls `parser.exit()`).
  Capture stdout via `capsys` and assert the printed version matches
  `importlib.metadata.version("micromegas")`.
- `micromegas-screens --version` (top-level, no subcommand) also exits `0` and prints the
  version, confirming the `required=True` subparser check doesn't preempt it.
- `import micromegas; micromegas.__version__` equals
  `importlib.metadata.version("micromegas")`.

Run via `poetry run pytest tests/cli/test_version.py` from `python/micromegas/`.

## Open Questions

- Whether to also report the interpreter path/version in the `--version` output (the issue's
  stretch goal, e.g. `micromegas 0.29.0 (Python 3.11.0 at /usr/bin/python3.11)`) or keep it to
  just the package version shown above. Not required to fix the core ambiguity described in
  the issue (a single `--version` call already tells you unambiguously which wheel backs that
  console script, in that process), but it was explicitly proposed. Leaving as-is unless
  reviewed otherwise, since the added surface (formatting `sys.version_info` and
  `sys.executable`) is easy to bolt on later without breaking the existing output's prefix.
