# Accept Full RFC 3339 Timestamps (Z Suffix) in the Python Client and CLI Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1405

## Overview

The Python client and `micromegas-query` CLI both parse timestamp strings with a
bare `datetime.datetime.fromisoformat()`, which does not accept an RFC 3339 `Z`
(Zulu/UTC) suffix before Python 3.11 — a version `pyproject.toml` still supports
(`python = "^3.10"`). Meanwhile the project's docstrings, the public API
reference, and the mkdocs query guide all advertise the `Z` form as the
canonical spelling. This plan closes that gap by raising the client's minimum
supported Python version to 3.11 — where `fromisoformat` accepts `Z` natively —
and normalizing the one thing 3.11+ still rejects, a lowercase `z` (RFC 3339
permits it; the stdlib doesn't, on any version). A single shared
`parse_datetime()` helper in `micromegas/time.py` does that normalization before
parsing, the duplicate `format_datetime` in `micromegas/flightsql/time.py`
(which is the copy the actual query path uses) is deleted in favor of the
shared one, the CLI turns parse failures into a readable `argparse` error
instead of an uncaught traceback and names RFC 3339 in its help text, and a
hermetic Python unit-test CI job — with a 3.11/3.14 matrix and the `time.py`
doctest enabled — confirms the package installs and passes on both the new
floor and the newest supported interpreter.

## Current State

### Two `format_datetime` implementations, and the bug is in both

`python/micromegas/micromegas/time.py:12-68` is the documented, public
`format_datetime`. Its string branch is:

```python
elif value_type == str:
    return format_datetime(datetime.datetime.fromisoformat(value))
```

`python/micromegas/micromegas/flightsql/time.py:5-18` is a near-verbatim,
undocumented **copy** of the same function, with the same `fromisoformat` string
branch at line 15. It is imported only by `flightsql/client.py:2`
(`from . import time`) and used at `client.py:72` and `client.py:79` in
`make_call_headers()` — i.e. **the duplicate is the one on the live query path**.
`micromegas/time.py` is reached instead by `micromegas/admin.py:11` and by users
calling `micromegas.time.format_datetime` directly. Nothing else imports
`micromegas.flightsql.time`; `flightsql/__init__.py` does not re-export it.

Consequence: fixing only `micromegas/time.py`, as the issue's suggested patch
implies, would leave `client.query(sql, "2024-01-01T00:00:00Z", ...)` — the exact
call the docs advertise — still broken on Python 3.10, which is what
`pyproject.toml` (`python = "^3.10"`) declares as supported today. The fix
adopted here is to raise the floor to 3.11, where `fromisoformat` accepts `Z`
natively, and to normalize the one spelling that remains unsupported on every
version — lowercase `z` — in the shared helper (see Design §1–§2 and
Trade-offs).

### The CLI parse site

`python/micromegas/micromegas/cli/query.py:11-30`, `parse_timestamp()`: tries
`micromegas.time.parse_time_delta` first (catching `RuntimeError`), then falls
through to `datetime.datetime.fromisoformat(value)` at line 27, defaulting a
naive result to UTC. The `ValueError` from a `Z`-suffixed value escapes `main()`
uncaught — `parse_timestamp` is called at `query.py:120-121`, well after the
`parser` object exists, but no `try` wraps it — so the user sees a traceback
rather than the `parser.error()` treatment every other bad input in `main()`
gets (`query.py:100-118`, `query.py:106-109`).

Help text at `query.py:74` and `query.py:78` says "ISO format", which is both
vaguer than what is accepted and silent on which offset spellings work.

### Documented contract

- `time.py:49-50` docstring shows `format_datetime('2024-01-01T12:00:00Z')` →
  `'2024-01-01T12:00:00+00:00'` as a working example. On 3.10 it raises; on
  3.11+ it already works natively.
- `time.py:54`: "The server requires RFC3339 format for all time-based queries."
- `flightsql/client.py:323` and `:380`: begin/end "Can be a timezone-aware
  datetime or RFC3339 string (e.g., `"2024-01-01T00:00:00Z"`)".
- `mkdocs/docs/query-guide/python-api.md:57-58`, `:88-95`, `:108-109` use
  `Z`-suffixed strings as the recommended form.
- `mkdocs/docs/query-guide/python-api.md:605-606` documents `--begin`/`--end` as
  "ISO format", mirroring the CLI help.

### Tests and CI

`python/micromegas/tests/test_time.py` has one test covering a `+00:00` string
and a naive string; no `Z` coverage. `tests/cli/` contains only
`test_config.py`; `parse_timestamp` is untested. `tests/test_query.py` already
exists as the hermetic test module for `micromegas/cli/query.py` (added for
#1399, covering `read_sql_source`), so it is the established home for
`parse_timestamp` coverage rather than a new file under `tests/cli/`.

There is **no Python CI workflow at all** — `.github/workflows/` contains
`rust.yml`, `analytics-web-app.yml`, `grafana-plugin.yml`, `blender-extension.yml`,
`capi-release.yml`, `publish-docs.yml`, and friends, none of which run
`pytest`. So nothing would have caught this even with a test, and nothing
exercises any specific Python version. Most tests under `python/micromegas/tests/`
are integration tests requiring a live service (`tests/test_utils.py:5` calls
`micromegas.connect()`, but only at *run* time — `FlightSQLClient.__init__`
builds a lazy `pyarrow.flight` channel with no I/O, so collection succeeds and
these tests only fail when actually executed, with a connection error), so a
CI job must run an explicit hermetic subset rather than the whole directory.

## Design

### 1. Bump the Python floor to 3.11

`python/micromegas/pyproject.toml` declares `python = "^3.10"`. Change it to
`python = "^3.11"`. This is the decision that makes the rest of the plan
simple: 3.11 is exactly where `datetime.fromisoformat()` learned to accept a
`Z` suffix, so the client no longer needs to shim anything for `Z` itself —
only for lowercase `z`, which the stdlib rejects on every version including
3.14 (see §2 and Trade-offs for why that one case still needs normalizing).

Python 3.10 reaches end of life in October 2026 — about two months from now —
so dropping it is not a preemptive break; it is dropping a version that is
about to stop receiving security fixes upstream. `pyarrow` (23.0.1, locked),
`grpcio`, `pandas`, and `numpy` all already publish cp311 through cp314
wheels, so there is no dependency-availability reason to stay on 3.10.

After changing the constraint, `poetry.lock` must be regenerated (`poetry
lock` from `python/micromegas`) so the lock file's resolution and hashes match
the new `python` bound; the regenerated lock file is committed alongside the
`pyproject.toml` change.

### 2. Shared `parse_datetime()` in `micromegas/time.py`

Now that the floor is 3.11, `fromisoformat` already handles the `Z` suffix and
any number of fractional-second digits natively — there is nothing left to
normalize for either of those. The one gap that remains on every supported
version, including 3.14, is a lowercase `z`: RFC 3339 section 5.6 defines the
offset via ABNF `"Z"`, and ABNF string literals are case-insensitive, so `z` is
conformant, but the stdlib parser rejects it regardless of version. Add one
small public helper next to `format_datetime`, and route every string-parsing
site through it:

```python
def parse_datetime(value):
    """Parse an RFC 3339 timestamp string into a datetime.

    datetime.fromisoformat() accepts an uppercase 'Z' offset but not a
    lowercase 'z', which RFC 3339 section 5.6 permits (its ABNF string
    literals are case-insensitive), so normalize that one case first.
    """
    if value.endswith("z"):
        value = value[:-1] + "Z"
    return datetime.datetime.fromisoformat(value)
```

Public (not `_`-prefixed) so both the CLI and the flightsql client can import it
without reaching into a private name, consistent with the project's preference
for `pub` over `pub(crate)`-style narrowing. It stays the single shared parse
site — `format_datetime`, `parse_timestamp` (CLI), and the flightsql client all
go through it — which is the DRY point of this plan, even though the helper
itself is now only a few lines: a version-specific shim would have been easy
to leave scattered across call sites and hard to remove later, whereas a
one-line normalization is just as easy to keep centralized.

`format_datetime`'s string branch (`time.py:65`) becomes
`return format_datetime(parse_datetime(value))`.

Notes on the normalization:

- Only a *trailing* lowercase `z` is rewritten to `Z`; an uppercase `Z`,
  numeric offsets (`+00:00`, `-05:00`), and naive values are already handled
  natively by `fromisoformat` on 3.11+ and are passed through untouched.
- A `ValueError` from a genuinely malformed value still propagates, unchanged —
  callers decide how to present it.
- The fractional-seconds padding/truncation logic and its `_FRACTION_RE` regex
  that a 3.10-supporting version of this helper would need are gone entirely:
  3.11+ accepts any digit count in `time-secfrac` natively (truncating beyond
  microsecond precision itself, which is `datetime`'s own resolution limit,
  not something this helper needs to reproduce).

### 3. Collapse the duplicate `flightsql/time.py`

Delete `python/micromegas/micromegas/flightsql/time.py` and point the client at
the canonical implementation. In `flightsql/client.py`, replace
`from . import time` (line 2) with:

```python
from ..time import format_datetime
```

and change `time.format_datetime(...)` at lines 72 and 79 to
`format_datetime(...)`.

Import-cycle check: `micromegas/__init__.py` imports `admin` before `flightsql`,
and `admin.py:11` already does `from micromegas.time import format_datetime`
successfully during that partial initialization — so `micromegas.time` is in
`sys.modules` well before `flightsql` is imported. The `from ..time import X`
form (rather than `from .. import time`) is used because it resolves through
`sys.modules` and does not depend on the parent package attribute being set yet,
so it is robust even if the `__init__.py` import order changes later.

This is the DRY fix the issue's one-shared-helper suggestion implies, taken one
step further: rather than patching two copies of the same function, there is
only one copy to patch. It also means `flightsql/client.py`'s query path
inherits `time.py`'s docstring/behavior contract instead of silently diverging
again.

`flightsql/time.py` is not exported from `flightsql/__init__.py` and has no
importers outside `client.py`, so deleting it is not an observable API break for
documented usage. Should any out-of-tree caller import
`micromegas.flightsql.time` directly, the canonical `micromegas.time` is a
drop-in replacement — worth a changelog line.

`make_call_headers` (`client.py:63-84`, the function that actually calls
`format_datetime` on the live query path) has no hermetic test today — the
only tests that touch the flightsql client are integration tests excluded from
CI (see Testing Strategy). A new `tests/test_flightsql_headers.py` unit test
asserts `make_call_headers` directly, so the fix is verified on the path the
issue's repro actually exercises, not just on `micromegas.time` and
`cli.query`.

### 4. CLI: friendly error and accurate help

In `cli/query.py`:

- `parse_timestamp()` (line 27) calls `micromegas.time.parse_datetime(value)`
  instead of `datetime.datetime.fromisoformat(value)`. It keeps raising
  `ValueError` on bad input — the function stays presentation-agnostic and
  unit-testable — and keeps the naive → UTC default at lines 28-29. Its
  docstring is updated to say RFC 3339.
- `main()` wraps the two `parse_timestamp` calls (lines 120-121) in a
  `try`/`except ValueError` that routes through `parser.error()`, matching the
  file's existing pattern for `--file` read failures (`query.py:105-109`). The
  message names both the flag and the offending value, e.g.:

  ```
  invalid --begin timestamp '2026-13-01T00:00:00Z': expected an RFC 3339
  timestamp (e.g. 2026-07-31T00:00:00Z) or a relative delta like '1h', '30m', '7d'
  ```

  The two calls are wrapped so that the failing flag can be named, i.e. `begin`
  and `end` are parsed in separate `try` blocks (or via a small local helper
  taking the flag name) rather than one block covering both.
- `--begin`/`--end` help (lines 74, 78) becomes: `Begin timestamp (RFC 3339 like
  '2024-01-01T00:00:00Z', or relative like '1h', '30m', '7d')` and the
  corresponding `--end` wording, preserving its "defaults to now" clause.

`parser.error()` exits with status 2 and prints usage — the standard argparse
contract for bad arguments, and the same treatment the surrounding validation
already uses.

### 5. Hermetic Python unit-test CI

Add `.github/workflows/python.yml`, triggered on pushes/PRs touching
`python/**` and `build/python_ci.py` (plus the workflow file itself), running
a matrix of Python **3.11** and **3.14** — the new floor and the newest
generally-available interpreter — on `runs-on: ubuntu-latest`. The matrix
values must be written as quoted YAML strings
(`python-version: ["3.11", "3.14"]`); bare `3.10`-style scalars parse as YAML
floats (`3.1`), which is a known footgun with `actions/setup-python`. This
workflow deliberately pins that fixed runner rather than routing through
`check-runner.yml`: the version matrix is the thing under test, so it must
stay deterministic regardless of self-hosted-runner availability. Following
the pattern every other workflow uses — `rust.yml` → `build/rust_ci.py`,
`analytics-web-app.yml` → `build/analytics_web_ci.py`, `grafana-plugin.yml` →
`build/grafana_ci.py`, `blender-extension.yml` →
`build/build_blender_plugin.py` — the workflow delegates to a new
`build/python_ci.py` rather than inlining shell in the YAML:

- Workflow steps: `actions/checkout@v4`, then `actions/setup-python@v5` pinned
  to `${{ matrix.python-version }}`, then `pipx install poetry` (GitHub-hosted
  runners don't ship Poetry), then — binding Poetry's venv to the interpreter
  `setup-python` just put on `PATH`, rather than whatever `python` the runner
  defaults to — `poetry env use python`, then `poetry install`, then
  `python build/python_ci.py ${{ matrix.python-version }}`; the three Poetry
  steps (`poetry env use`, `poetry install`, and the `python_ci.py` call) run
  with `working-directory: python/micromegas`.
- `build/python_ci.py` takes the expected Python version as an argument (or
  env var) and, before running tests, runs `poetry run python -c "import sys;
  print('%d.%d' % sys.version_info[:2])"` from `python/micromegas` and compares
  its output against the expected version, failing loudly on a mismatch. The
  assertion must observe the Poetry *venv*'s interpreter this way rather than
  checking its own `sys.version_info`: `python_ci.py` itself is invoked by the
  interpreter `setup-python` put on `PATH`, so its own `sys.version_info` is
  the matrix version by construction and could never catch a mis-pinned venv
  (e.g. Poetry silently resolving to the runner's default interpreter instead
  of the matrix's 3.11 for the `poetry run pytest` subprocess below). It then
  runs
  `poetry run pytest --doctest-modules micromegas/time.py tests/test_time.py tests/test_flightsql_headers.py tests/cli tests/test_query.py tests/test_web_client.py tests/test_screen_files.py tests/auth/test_oidc_unit.py tests/auth/test_client_credentials_unit.py`
  from `python/micromegas` and returns its exit code.

The explicit file list is deliberate: `pytest` over the whole `tests/` directory
would collect and run the integration suite, and `tests/test_utils.py:5` calls
`micromegas.connect()` — a `FlightSQLClient` construction with no I/O of its
own — which fails only when a test actually runs a query against it (a
connection error), not at collection time. `--doctest-modules` is scoped to
`micromegas/time.py` alone — `flightsql/client.py` contains many illustrative
`>>>` blocks that are not executable doctests and would fail if collected.

It is no longer true that only one leg of this matrix "proves" the fix — the
`Z` acceptance is now native on every supported interpreter (3.11 through
3.14), so both legs exercise the same code path for it. What the two-version
matrix buys instead is (a) confirming the package actually installs and its
test suite passes on both the floor (3.11) and the newest usable interpreter
(3.14), catching e.g. a dependency that lacks a wheel for one of them, and (b)
catching a lowercase-`z` regression on both, since that normalization is this
plan's one piece of interpreter-independent behavior and is worth checking on
more than a single version. The interpreter-pinning and version-assertion
steps above still matter for the same reason as always: without them, a
matrix leg could silently run on the runner's default interpreter instead of
the one it claims to be testing, and report a false green.

The new `unit-tests` checks start out **advisory**: they are not added to the
repo's branch-protection required-status-checks list, so no companion skip
workflow is needed for this change. Skip workflows like `build-skip.yml`/
`web-build-skip.yml` exist only to make *required* checks resolve instead of
hanging on PRs whose paths don't touch the relevant code
(`tasks/completed/container_based_dev_worker_plan.md:95`); since nothing here
depends on an out-of-repo branch-protection change, that machinery does not
apply yet. Follow-up: if `unit-tests (3.11)` / `unit-tests (3.14)` are later
made required in branch protection, a companion
`.github/workflows/python-build-skip.yml` mirroring those matrix leg names
must be added at that time, following the `build-skip.yml` pattern.

## Implementation Steps

1. **`python/micromegas/pyproject.toml`** — change `python = "^3.10"` to
   `python = "^3.11"`.
   **`python/micromegas/poetry.lock`** — regenerate with `poetry lock` (from
   `python/micromegas`) so the lock file matches the new constraint.
2. **`python/micromegas/micromegas/time.py`** — add `parse_datetime()` with the
   lowercase-`z` normalization and docstring; change `format_datetime`'s
   string branch (line 65) to use it. Leave the existing docstring example at
   line 49 as-is — it becomes true rather than aspirational.
3. **`python/micromegas/micromegas/flightsql/client.py`** — replace
   `from . import time` (line 2) with `from ..time import format_datetime`;
   update the two call sites at lines 72 and 79.
4. **Delete `python/micromegas/micromegas/flightsql/time.py`.**
5. **`python/micromegas/micromegas/cli/query.py`** — use
   `micromegas.time.parse_datetime` in `parse_timestamp` (line 27); update its
   docstring and the inline comment at line 26; update `--begin`/`--end` help
   (lines 74, 78); wrap the `parse_timestamp` calls in `main()` (lines 120-121)
   with per-flag `ValueError` → `parser.error()` handling.
6. **`python/micromegas/tests/test_time.py`** — extend with the cases below.
7. **`python/micromegas/tests/test_query.py`** — extend with cases covering
   `parse_timestamp`, alongside the existing `read_sql_source` regression
   tests.
   **`python/micromegas/tests/test_flightsql_headers.py`** (new) — a hermetic
   unit test for `flightsql.client.make_call_headers`, the function on the
   live query path, asserting a `Z`-suffixed `begin` produces a
   `query_range_begin` header of `2024-01-01T00:00:00+00:00`.
8. **`build/python_ci.py`** (new) — the hermetic `pytest` invocation described
   above (including the new `tests/test_flightsql_headers.py`) plus the
   venv-interpreter version assertion (`poetry run python -c "..."` compared
   against the expected version), following the existing `build/*_ci.py`
   scripts.
   **`.github/workflows/python.yml`** (new) — the 3.11/3.14 matrix job (as
   quoted YAML strings) on `runs-on: ubuntu-latest`, starting with
   `actions/checkout@v4` and `actions/setup-python` pinned to
   `${{ matrix.python-version }}`, then `pipx install poetry` before
   `poetry env use python` and `poetry install` (both with
   `working-directory: python/micromegas`), calling `build/python_ci.py` with
   the matrix version; `build/python_ci.py` included in the workflow's path
   filter. The `unit-tests` checks start out advisory (not added to branch
   protection), so no companion skip workflow is added in this step.
9. **`mkdocs/docs/query-guide/python-api.md`** — update the `--begin`/`--end`
   option descriptions (lines 605-606) to say RFC 3339 and show the `Z` form,
   update the "specific timestamps" CLI example (lines 619-621) to use
   `Z`-suffixed values so the docs demonstrate the canonical spelling, and add
   `parse_datetime` to the "Time Utilities" section (lines 693-716) alongside
   `format_datetime`/`parse_time_delta`.
   **`CLAUDE.md`** (repo root) — update line 60's `--begin`/`--end`
   description from "ISO format" to RFC 3339 wording with a `Z` example,
   matching the mkdocs and argparse-help wording above.
   **`python/notebooks/README.md`** and **`mkdocs/docs/development/build.md`**
   — update the stale "Python 3.8+" prerequisite line in each to "Python
   3.11+", matching the new floor.
10. **`CHANGELOG.md`** — one entry under `## Unreleased`, in the existing
    style, noting: the minimum supported Python version rising from 3.10 to
    3.11 (called out explicitly as a breaking change for anyone still on
    3.10, since it means `pip install micromegas` no longer works there), the
    `Z`/`z` acceptance fix, the `flightsql/time.py` removal, the CLI error
    handling, and the new Python CI job, with `(#1405)`.
11. **Format** — `poetry run black` on every touched Python file (required by
    `python/CLAUDE.md`).

## Files to Modify

- `python/micromegas/pyproject.toml`
- `python/micromegas/poetry.lock`
- `python/micromegas/micromegas/time.py`
- `python/micromegas/micromegas/flightsql/client.py`
- `python/micromegas/micromegas/flightsql/time.py` *(delete)*
- `python/micromegas/micromegas/cli/query.py`
- `python/micromegas/tests/test_time.py`
- `python/micromegas/tests/test_query.py`
- `python/micromegas/tests/test_flightsql_headers.py` *(new)*
- `build/python_ci.py` *(new)*
- `.github/workflows/python.yml` *(new)*
- `mkdocs/docs/query-guide/python-api.md`
- `CLAUDE.md`
- `python/notebooks/README.md`
- `mkdocs/docs/development/build.md`
- `CHANGELOG.md`

## Trade-offs

**Raise the floor to 3.11 vs. keep 3.10 and shim `Z`.** This decision is made,
not open: the plan raises the minimum supported Python version to 3.11 rather
than keeping 3.10 support and normalizing the `Z` suffix (and the
fractional-seconds digit-count differences that would go with it) in Python.
3.11 is exactly the version where `fromisoformat` learned native `Z` support,
so it is the natural floor for a project whose contract is "accept RFC 3339
timestamps" — anchoring on it means the stdlib does that work instead of a
hand-rolled shim having to reproduce it and stay in sync with it. The other
side of the decision: Python 3.10 reaches end of life in October 2026, about
two months from now, so dropping it now is dropping a version whose upstream
security support is about to end anyway, not preempting meaningful runway.
`pyarrow`, `grpcio`, `pandas`, and `numpy` already publish cp311+ wheels, so
there's no dependency blocker either way.

**Normalize lowercase `z` vs. reject it.** With the floor at 3.11, the only
remaining gap between what `fromisoformat` accepts and what RFC 3339 permits
is a lowercase `z` offset — RFC 3339 section 5.6's ABNF string literals are
case-insensitive, so `z` is conformant, but the stdlib rejects it on every
version through 3.14. The plan normalizes it (rewriting a trailing `z` to
`Z` before parsing) rather than leaving it to raise, since the project's own
docs and issue history already treat `Z`/`z` as interchangeable and the fix is
a single-line, version-independent rewrite with no dependency or grammar cost.

**Delete `flightsql/time.py` vs. re-export from it.** A shim module
(`from ..time import format_datetime`) would preserve
`micromegas.flightsql.time` as an import path for any out-of-tree caller. It was
rejected because the module is undocumented, unexported, and has exactly one
in-tree importer; keeping a shim preserves the illusion of two time modules,
which is the shape that let them drift apart in the first place. The changelog
note covers the theoretical caller.

**Explicit test-file list in CI vs. pytest markers.** Marking hermetic tests
(`@pytest.mark.unit`) and running `-m unit` would be more elegant and would
auto-include future unit tests. It was rejected for this change because it
means touching every existing test file to classify it, which is a much larger
diff than this fix warrants. The explicit list is honest about what is
covered; broadening it (or migrating to markers) is a natural follow-up.

**`parser.error()` in `main()` vs. inside `parse_timestamp`.** Passing the
`parser` into `parse_timestamp` would centralize the message, but it would weld
a reusable parsing function to argparse and make it awkward to unit-test.
Raising `ValueError` and translating at the CLI boundary keeps the function
testable and matches how `read_sql_source` already lets `OSError` propagate to a
`parser.error()` in `main()` (`query.py:43-56`, `:103-109`).

## Documentation

- `mkdocs/docs/query-guide/python-api.md` — `--begin`/`--end` option
  descriptions (lines 605-606) and the specific-timestamps CLI example
  (lines 619-621). The Python API sections (lines 57-58, 88-95, 108-109)
  already document the `Z` form correctly and need no change — they become
  accurate rather than aspirational. The "Time Utilities" section
  (lines 693-716), currently headed
  ``### `format_datetime(value)` and `parse_time_delta(user_string)` ``, gains
  `parse_datetime` in its heading, import example, and prose, since it is now
  a public function in the same module.
- `python/micromegas/micromegas/cli/query.py` — `parse_timestamp` docstring and
  argparse help strings (the CLI's in-tool documentation).
- `python/micromegas/micromegas/time.py` — `parse_datetime` docstring; the
  existing `format_datetime` docstring needs no correction.
- `CLAUDE.md` (repo root) — line 60 currently says "ISO format" for
  `--begin`/`--end`, the same stale wording being fixed in the mkdocs guide
  and the argparse help; update it to RFC 3339 wording with a `Z` example so
  it doesn't go stale relative to the CLI it documents.
- `python/notebooks/README.md:67` and `mkdocs/docs/development/build.md:8` —
  both state "Python 3.8+" as the prerequisite; update to "Python 3.11+" to
  match the new floor. The notebooks README is directly affected since those
  notebooks `import micromegas`, which no longer installs on 3.8–3.10 after
  the bump.
- No `.github/copilot-instructions.md` change: it documents only Rust CI, so
  neither the timestamp fix nor the new Python CI workflow touches it. The
  new Python CI workflow itself also needs no `CLAUDE.md` change — it's
  additive and the existing Python commands (`poetry run pytest`,
  `poetry run black`) are already documented in `python/CLAUDE.md`; the line
  60 fix above is about the pre-existing `--begin`/`--end` docs, not the new
  workflow.

## Testing Strategy

### `tests/test_time.py` — extend

Keep the existing `test_format_string`, and add coverage for `format_datetime`
and `parse_datetime`:

- `"2024-08-26T17:32:00Z"` → `"2024-08-26T17:32:00+00:00"` (the issue's exact
  repro, and the docstring's example).
- `"2024-08-26T17:32:00z"` — lowercase, same result.
- `"2024-08-26T17:32:00.000+00:00"` — unchanged behavior (already covered).
- `"2024-08-26T17:32:00-05:00"` — non-UTC offset preserved, not coerced to UTC.
- `"2024-08-26T17:32:00"` — naive string still raises `RuntimeError` from
  `format_datetime` (existing behavior, already covered; keep it).
- `"not-a-timestamp"` → `ValueError` from `parse_datetime`.
- Fractional seconds with `Z`: `"2024-08-26T17:32:00.123456Z"`.
- `"2024-08-26T17:32:00.5Z"` → `"2024-08-26T17:32:00.500000+00:00"` (a
  single-digit `time-secfrac`; asserts that `parse_datetime` behaves exactly
  as the stdlib does on 3.11+ — there is no normalization logic of this
  helper's own left to exercise here, since `fromisoformat` already pads any
  digit count to microseconds natively).
- `"2024-08-26T17:32:00.1234567Z"` → `"2024-08-26T17:32:00.123456+00:00"`
  (more than six fractional digits; likewise just confirms `parse_datetime`
  passes through `fromisoformat`'s own truncation behavior unchanged).

### `tests/test_query.py` — extend

Direct unit tests of `micromegas.cli.query.parse_timestamp`, added alongside
the existing `read_sql_source` regression tests in this module:

- `None` → `None`.
- `"1h"` / `"30m"` / `"7d"` → a tz-aware datetime roughly that far in the past
  (assert `tzinfo is not None` and that the delta is in a tolerant window; do
  not assert an exact instant).
- `"2026-07-31T00:00:00Z"` → tz-aware UTC datetime (the CLI repro from the
  issue).
- `"2026-07-31T00:00:00z"` → same.
- `"2026-07-31T00:00:00+00:00"` → same.
- `"2026-07-31T00:00:00-04:00"` → offset preserved.
- `"2026-07-31T00:00:00"` (naive) → defaulted to UTC.
- `"garbage"` → `ValueError` (not `RuntimeError`, and not a silent pass).

Assert on return values directly; no service, no subprocess, no mocking of
argparse internals.

### `tests/test_flightsql_headers.py` — new

A direct, hermetic unit test of `flightsql.client.make_call_headers` — the
function Current State identifies as the one actually on the live query path
(`client.py:72,79`), which neither `test_time.py` nor `test_query.py` reaches:

- `make_call_headers("2024-01-01T00:00:00Z", None)` includes a
  `query_range_begin` header of `(b"query_range_begin",
  b"2024-01-01T00:00:00+00:00")` — headers are `(bytes, bytes)` tuples, not
  strings.

`make_call_headers` is a module-level pure function with no I/O, so this
needs no service, mocking, or subprocess.

### Doctest

`pytest --doctest-modules micromegas/time.py` executes the `format_datetime`
docstring examples, including the `Z` case at `time.py:49` — the check the issue
identifies as the one that would have caught this originally.

### Manual verification

With services running (`python3 local_test_env/ai_scripts/start_services.py`):

```
micromegas-query --begin 2026-07-31T00:00:00Z --end 2026-07-31T00:05:00Z "SELECT 1"
micromegas-query --begin 1h "SELECT 1"
micromegas-query --begin nonsense "SELECT 1"   # expect a usage error, not a traceback
```

The first two should run; the third should print a usage message and exit 2.

### Python-version note

Unlike the pre-3.11-floor version of this plan, there is no version-specific
gap left to worry about here: the lowercase-`z` normalization this plan adds
is plain Python with no interpreter-dependent behavior, so it behaves
identically on 3.11 through 3.14. A local run on whatever interpreter the
developer has installed (3.11+) proves the fix; the CI matrix (§5) exists to
confirm installability and test-suite health on the floor and newest
interpreter, not to reveal behavior a local run couldn't.
