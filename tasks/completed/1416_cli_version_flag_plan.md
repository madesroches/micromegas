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
  - `python/micromegas/micromegas/cli/query.py:60` — flat parser, no subcommands. Its
    `prog="query"` doesn't match the installed console-script name (`micromegas-query`).
  - `python/micromegas/micromegas/cli/logout.py:8` — flat parser, no arguments at all today.
    Its `prog="micromegas_logout"` (underscore) doesn't match the installed console-script
    name (`micromegas-logout`, hyphen).
  - `python/micromegas/micromegas/cli/screens.py:649` — parser with `add_subparsers(dest="command", required=True)`,
    whose `prog="micromegas-screens"` already matches its console script.
- `python/micromegas/micromegas/__init__.py` (23 lines) currently only re-exports submodules
  (`admin`, `auth`, `flightsql`, `oidc_connection`, `perfetto`, `time`) and defines `connect()`.
  It does not read or expose a package version.
- At runtime, `importlib.metadata.version("micromegas")` looks up whichever distribution is
  installed for the *running* interpreter, which is exactly the value we want to surface.

## Design

Resolve the version via `importlib.metadata.version("micromegas")`. Note this lookup happens
once per invocation, at parser-construction time in `main()` (the version string is built
eagerly when `add_argument()` runs, not deferred to when `--version` is actually passed) — a
small, cheap cost paid by every command run, not just `--version`. Safety against
`PackageNotFoundError` in an unusual/dev environment comes from `package_version()`'s own
try/except, not from deferred evaluation.

To avoid duplicating the same three lines across three files, add one small shared helper.
Rather than argparse's built-in `action="version"`, define a custom `_VerbatimVersionAction`:
argparse's built-in action routes the version string through `HelpFormatter`, which reflows
text to the terminal width and can break long tokens (like the interpreter path) mid-word at
narrow widths; printing the string as-is avoids that. The version string is also built as a
plain f-string embedding `parser.prog` directly, rather than a `%(prog)s`-style template
string, since `%`-style formatting corrupts (or crashes on) values that themselves contain a
literal `%` character, which can happen when the interpreter path does:

```python
# python/micromegas/micromegas/cli/version.py
import argparse
import importlib.metadata
import platform
import sys


def package_version():
    """Return the installed micromegas package version, or 'unknown' if not installed."""
    try:
        return importlib.metadata.version("micromegas")
    except importlib.metadata.PackageNotFoundError:
        return "unknown"


class _VerbatimVersionAction(argparse.Action):
    """Like argparse's built-in "version" action, but prints the string as-is
    instead of routing it through HelpFormatter, which reflows (and can break
    mid-token) long paths such as the interpreter path."""

    def __init__(
        self,
        option_strings,
        version=None,
        dest=argparse.SUPPRESS,
        default=argparse.SUPPRESS,
        help="show program's version number and exit",
    ):
        super().__init__(
            option_strings=option_strings,
            dest=dest,
            default=default,
            nargs=0,
            help=help,
        )
        self.version = version

    def __call__(self, parser, namespace, values, option_string=None):
        version = self.version
        if version is None:
            version = parser.version
        print(version)
        parser.exit(0)


def add_version_argument(parser):
    """Add a --version flag reporting the micromegas package version and interpreter."""
    version_string = (
        f"{parser.prog} {package_version()} "
        f"(Python {platform.python_version()} at {sys.executable})"
    )
    parser.add_argument(
        "--version",
        action=_VerbatimVersionAction,
        version=version_string,
    )
```

Each CLI's `main()` calls `add_version_argument(parser)` right after constructing its
top-level `ArgumentParser`, before `parse_args()`. The version string is built eagerly from
`parser.prog`, reusing each script's existing `prog=` value, so output stays consistent with
existing help text, e.g.:

```
$ micromegas-query --version
micromegas-query 0.29.0 (Python 3.11.9 at /usr/bin/python3.11)
$ micromegas-screens --version
micromegas-screens 0.29.0 (Python 3.11.9 at /usr/bin/python3.11)
$ micromegas-logout --version
micromegas-logout 0.29.0 (Python 3.11.9 at /usr/bin/python3.11)
```

The issue's proposal additionally suggests reporting the interpreter, matching the exact
ambiguity this feature is meant to resolve (which interpreter/venv a console script's wheel
came from). This plan includes it directly in `--version` output via `platform.python_version()`
and `sys.executable`, rather than leaving it as a follow-up.

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
2. In `python/micromegas/micromegas/cli/query.py`: fix `prog="query"` to
   `prog="micromegas-query"` (to match the console-script name declared in
   `pyproject.toml:38`), import `add_version_argument` from `.version` and call
   `add_version_argument(parser)` after the `ArgumentParser(...)` construction in `main()`
   (around `python/micromegas/micromegas/cli/query.py:60`), before the other `add_argument`
   calls (order among flags doesn't matter to argparse, but keep it near the top for
   readability).
3. In `python/micromegas/micromegas/cli/logout.py`: fix `prog="micromegas_logout"` to
   `prog="micromegas-logout"` (to match the console-script name declared in
   `pyproject.toml:37`), same pattern — call `add_version_argument(parser)` after the
   `ArgumentParser(...)` construction in `main()` (`python/micromegas/micromegas/cli/logout.py:8`).
4. In `python/micromegas/micromegas/cli/screens.py`: call `add_version_argument(parser)`
   right after the top-level `ArgumentParser(...)` construction (`python/micromegas/micromegas/cli/screens.py:649`)
   and before `add_subparsers(...)`.
5. In `python/micromegas/micromegas/__init__.py`: import `package_version` from
   `.cli.version` and set `__version__ = package_version()`.
6. Add tests (see Testing Strategy).
7. Update the "**Options:**" list for `query.py` in
   `mkdocs/docs/query-guide/python-api.md` (around lines 603-609) to add a `--version` bullet;
   the `micromegas-logout` section on the same page doesn't enumerate options the same way, so
   nothing to change there. `micromegas-screens` isn't documented on `python-api.md` at all —
   its docs live in `mkdocs/docs/web-app/notebooks/screens-as-code.md`, which also needs no
   change since it doesn't enumerate a top-level flag list either.
8. Append a bullet under the `## Unreleased` section of `CHANGELOG.md` (under a `**Python:**`
   heading, adding one if not already present) describing the new `--version` flag on all
   three console scripts and the new `micromegas.__version__`.
9. Run `poetry run black` on all changed files.

## Files to Modify

- `python/micromegas/micromegas/cli/version.py` (new)
- `python/micromegas/micromegas/cli/query.py`
- `python/micromegas/micromegas/cli/logout.py`
- `python/micromegas/micromegas/cli/screens.py`
- `python/micromegas/micromegas/__init__.py`
- `python/micromegas/tests/cli/test_version.py` (new)
- `mkdocs/docs/query-guide/python-api.md`
- `CHANGELOG.md`

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
  containing the package version and interpreter details when invoked with `--version`, using
  `monkeypatch.setattr(sys, "argv", [...])` + `pytest.raises(SystemExit)` (the standard
  pattern for testing `parser.exit()`-based version actions, which `_VerbatimVersionAction`
  also uses). Capture stdout via `capsys` and assert the printed output contains
  `importlib.metadata.version("micromegas")`, `platform.python_version()`, and
  `sys.executable`.
- `micromegas-screens --version` (top-level, no subcommand) also exits `0` and prints the
  version, confirming the `required=True` subparser check doesn't preempt it.
- `import micromegas; micromegas.__version__` equals
  `importlib.metadata.version("micromegas")`.
- A narrow-terminal regression test (`COLUMNS=30`) confirms `--version` output isn't reflowed
  or split mid-token by `HelpFormatter`, which `_VerbatimVersionAction` fixes by printing the
  version string directly instead of going through argparse's built-in `action="version"`.
- A regression test with `sys.executable` monkeypatched to a path containing a literal `%`
  character confirms the interpreter path is embedded verbatim (not `%`-interpolated), which
  `_VerbatimVersionAction`'s plain f-string construction of the version string fixes relative
  to `%(prog)s`-style templating.

Run via `poetry run pytest tests/cli/test_version.py` from `python/micromegas/`.

## Open Questions

None outstanding — the plan previously left open whether `--version` should also report the
interpreter path/version; the design above now includes it directly.
