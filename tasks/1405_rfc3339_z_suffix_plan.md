# Accept Full RFC 3339 Timestamps (Z Suffix) in the Python Client and CLI Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1405

## Overview

The Python client and `micromegas-query` CLI both parse timestamp strings with a
bare `datetime.datetime.fromisoformat()`, which does not accept an RFC 3339 `Z`
(Zulu/UTC) suffix before Python 3.11 — a version `pyproject.toml` still supports
(`python = "^3.10"`). Meanwhile the project's docstrings, the public API
reference, and the mkdocs query guide all advertise the `Z` form as the
canonical spelling. This plan closes that gap: a single shared
`parse_datetime()` helper in `micromegas/time.py` normalizes a trailing `Z`/`z`
to `+00:00` before parsing, the duplicate `format_datetime` in
`micromegas/flightsql/time.py` (which is the copy the actual query path uses) is
deleted in favor of the shared one, the CLI turns parse failures into a readable
`argparse` error instead of an uncaught traceback and names RFC 3339 in its help
text, and a hermetic Python unit-test CI job — with a 3.10 leg and the
`time.py` doctest enabled — makes this class of version-specific regression
visible before it ships.

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
call the docs advertise — still broken on Python 3.10.

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
  `'2024-01-01T12:00:00+00:00'` as a working example. On 3.10 it raises.
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
exercises Python 3.10 specifically. Most tests under `python/micromegas/tests/`
are integration tests requiring a live service (`tests/test_utils.py:5` calls
`micromegas.connect()`, but only at *run* time — `FlightSQLClient.__init__`
builds a lazy `pyarrow.flight` channel with no I/O, so collection succeeds and
these tests only fail when actually executed, with a connection error), so a
CI job must run an explicit hermetic subset rather than the whole directory.

## Design

### 1. Shared `parse_datetime()` in `micromegas/time.py`

Add one public helper next to `format_datetime`, and route every string-parsing
site through it:

```python
_FRACTION_RE = re.compile(r"\.(\d+)")


def parse_datetime(value):
    """Parse an RFC 3339 timestamp string into a datetime.

    Accepts the full RFC 3339 grammar on every supported interpreter: the
    'Z' (Zulu/UTC) suffix, numeric offsets, and any number of
    fractional-second digits (RFC 3339's `time-secfrac = "." 1*DIGIT`).
    Python's datetime.fromisoformat() only learned to accept 'Z' in 3.11,
    and on 3.10 it only accepts a fractional-seconds part that is exactly
    3 or 6 digits long (what isoformat() emits), not an arbitrary count —
    so both the 'Z' suffix and the fractional-seconds field are normalized
    before parsing, keeping the accepted grammar identical on 3.10 and
    3.11+. This project supports 3.10.
    """
    if value.endswith(("Z", "z")):
        value = value[:-1] + "+00:00"
    match = _FRACTION_RE.search(value)
    if match:
        digits = match.group(1)[:6].ljust(6, "0")
        value = value[: match.start()] + "." + digits + value[match.end() :]
    return datetime.datetime.fromisoformat(value)
```

Public (not `_`-prefixed) so both the CLI and the flightsql client can import it
without reaching into a private name, consistent with the project's preference
for `pub` over `pub(crate)`-style narrowing.

`format_datetime`'s string branch (`time.py:65`) becomes
`return format_datetime(parse_datetime(value))`.

Notes on the normalization:

- Only a *trailing* `Z`/`z` is rewritten to `+00:00`; other offsets (`+00:00`,
  `-05:00`) and naive values are otherwise untouched.
- A fractional-seconds group, if present, is padded with trailing zeros or
  truncated to exactly six digits (microseconds) before parsing. RFC 3339
  allows any digit count there, but `fromisoformat` is inconsistent about it
  across supported interpreters: 3.10 accepts only 3 or 6 digits, while 3.11+
  accepts any count but silently truncates beyond six itself. Truncating
  beyond microsecond precision is lossy, but it matches `datetime`'s own
  resolution and reproduces exactly what 3.11+ already does — so 3.10 now
  matches it instead of raising.
- Lowercase `z` is accepted: RFC 3339 section 5.6 defines the offset via ABNF
  `"Z"`, and ABNF string literals are case-insensitive, so `z` is conformant.
  Accepting it also costs nothing.
- A `ValueError` from a genuinely malformed value still propagates, unchanged —
  callers decide how to present it.

### 2. Collapse the duplicate `flightsql/time.py`

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

### 3. CLI: friendly error and accurate help

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

### 4. Hermetic Python unit-test CI

Add `.github/workflows/python.yml`, triggered on pushes/PRs touching
`python/**` and `build/python_ci.py` (plus the workflow file itself), running
a matrix of Python **3.10** and **3.12**. Following the pattern every other
workflow uses — `rust.yml` → `build/rust_ci.py`, `analytics-web-app.yml` →
`build/analytics_web_ci.py`, `grafana-plugin.yml` → `build/grafana_ci.py`,
`blender-extension.yml` → `build/build_blender_plugin.py` — the workflow
delegates to a new `build/python_ci.py` rather than inlining shell in the
YAML:

- Workflow step: `poetry install` in `python/micromegas`, then
  `python build/python_ci.py`.
- `build/python_ci.py` runs
  `poetry run pytest --doctest-modules micromegas/time.py tests/test_time.py tests/test_flightsql_headers.py tests/cli tests/test_query.py tests/test_web_client.py tests/test_screen_files.py tests/auth/test_oidc_unit.py tests/auth/test_client_credentials_unit.py`
  from `python/micromegas` and returns its exit code.

The explicit file list is deliberate: `pytest` over the whole `tests/` directory
would collect and run the integration suite, and `tests/test_utils.py:5` calls
`micromegas.connect()` — a `FlightSQLClient` construction with no I/O of its
own — which fails only when a test actually runs a query against it (a
connection error), not at collection time. `--doctest-modules` is scoped to
`micromegas/time.py` alone — `flightsql/client.py` contains many illustrative
`>>>` blocks that are not executable doctests and would fail if collected.

The 3.10 leg is the part that actually earns its keep here: on 3.11+ the `Z`
regression tests pass whether or not the shim exists, because `fromisoformat`
handles `Z` natively. Only the 3.10 leg distinguishes a fixed build from a
broken one.

## Implementation Steps

1. **`python/micromegas/micromegas/time.py`** — add `parse_datetime()` with the
   `Z`/`z` normalization and docstring; change `format_datetime`'s string branch
   (line 65) to use it. Leave the existing docstring example at line 49 as-is —
   it becomes true rather than aspirational.
2. **`python/micromegas/micromegas/flightsql/client.py`** — replace
   `from . import time` (line 2) with `from ..time import format_datetime`;
   update the two call sites at lines 72 and 79.
3. **Delete `python/micromegas/micromegas/flightsql/time.py`.**
4. **`python/micromegas/micromegas/cli/query.py`** — use
   `micromegas.time.parse_datetime` in `parse_timestamp` (line 27); update its
   docstring and the inline comment at line 26; update `--begin`/`--end` help
   (lines 74, 78); wrap the `parse_timestamp` calls in `main()` (lines 120-121)
   with per-flag `ValueError` → `parser.error()` handling.
5. **`python/micromegas/tests/test_time.py`** — extend with the cases below.
6. **`python/micromegas/tests/test_query.py`** — extend with cases covering
   `parse_timestamp`, alongside the existing `read_sql_source` regression
   tests.
   **`python/micromegas/tests/test_flightsql_headers.py`** (new) — a hermetic
   unit test for `flightsql.client.make_call_headers`, the function on the
   live query path, asserting a `Z`-suffixed `begin` produces a
   `query_range_begin` header of `2024-01-01T00:00:00+00:00`.
7. **`build/python_ci.py`** (new) — the hermetic `pytest` invocation described
   above (including the new `tests/test_flightsql_headers.py`), following the
   existing `build/*_ci.py` scripts.
   **`.github/workflows/python.yml`** (new) — the 3.10/3.12 matrix job that
   installs Poetry and calls `build/python_ci.py`, with `build/python_ci.py`
   included in the workflow's path filter.
8. **`mkdocs/docs/query-guide/python-api.md`** — update the `--begin`/`--end`
   option descriptions (lines 605-606) to say RFC 3339 and show the `Z` form,
   update the "specific timestamps" CLI example (lines 619-621) to use
   `Z`-suffixed values so the docs demonstrate the canonical spelling, and add
   `parse_datetime` to the "Time Utilities" section (lines 693-716) alongside
   `format_datetime`/`parse_time_delta`.
9. **`CHANGELOG.md`** — one entry under `## Unreleased`, in the existing style,
   noting the `Z` acceptance fix, the `flightsql/time.py` removal, the CLI error
   handling, and the new Python CI job, with `(#1405)`.
10. **Format** — `poetry run black` on every touched Python file (required by
    `python/CLAUDE.md`).

## Files to Modify

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
- `CHANGELOG.md`

## Trade-offs

**Normalize `Z` vs. drop Python 3.10.** The issue notes that 3.11 handles `Z`
natively, making the shim unnecessary. Dropping 3.10 is a far larger, breaking
decision for downstream users than a three-line helper, and it would not by
itself fix the CLI's traceback-on-bad-input or the duplicated module. The shim
is cheap and removable later; when 3.10 support is eventually dropped,
`parse_datetime` can become a thin alias for `fromisoformat` (or be deleted)
without touching any call site — which is precisely the point of routing every
site through one helper.

**Normalize `Z` and fractional seconds vs. adopt a parsing dependency.**
`dateutil` or `ciso8601` would handle the whole of RFC 3339 (and more), but
`dateutil` is not currently a declared dependency of the client, and both
accept far *more* than RFC 3339, which would loosen rather than sharpen the
contract. A small amount of normalization in front of the stdlib parser —
rewriting the `Z` suffix and padding/truncating the fractional-seconds field to
microseconds — keeps the accepted grammar exactly "what `fromisoformat` takes
on 3.11+", rather than a narrower, interpreter-dependent subset of it.

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
- No `CLAUDE.md` or `.github/copilot-instructions.md` change: the new workflow
  is additive and the existing Python commands (`poetry run pytest`,
  `poetry run black`) are already documented in `python/CLAUDE.md`.

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
  single-digit `time-secfrac`, padded to microseconds — this is the case that
  fails on 3.10 without the fractional-seconds normalization, even after the
  `Z` rewrite).
- `"2024-08-26T17:32:00.1234567Z"` → `"2024-08-26T17:32:00.123456+00:00"`
  (more than six fractional digits, truncated — lossy, but matches what
  `fromisoformat` itself does on 3.11+ and what `datetime` can represent).

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

On the developer's local interpreter (3.12) every `Z` assertion passes with or
without the fix, since `fromisoformat` handles `Z` natively there. The fix is
only *proved* by the 3.10 CI leg. Anyone verifying locally on 3.11+ should treat
a green run as necessary but not sufficient, and rely on CI's 3.10 job for the
real signal.
