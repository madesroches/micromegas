# Screens CLI UTF-8 Encoding Fix Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1399

## Overview

`micromegas-screens` opens screen JSON files with Python's text-mode `open()`
and no explicit `encoding=`, so reads and writes fall back to the platform's
locale-preferred encoding (cp1252 on Windows, ASCII under a `C`/`POSIX`
locale). A UTF-8 screen file read on such a platform gets mis-decoded before
it's ever sent to the server, corrupting non-ASCII text (em dashes, accents,
CJK) as mojibake. This plan pins every text-mode `open()` in the screens CLI
to `encoding="utf-8"` (`utf-8-sig` on reads, to tolerate a stray BOM), switches
`write_screen_file()` to `ensure_ascii=False` so pulled files are readable
UTF-8 instead of escape sequences, and adds regression tests — for
`screens.py`, `query.py`, and `oidc.py` alike — that force a non-UTF-8
environment so the bug can be caught before it ships in any of the three
fixed modules.

## Current State

`python/micromegas/micromegas/cli/screens.py` has four text-mode `open()`
calls with no `encoding=`, all locale-dependent:

- `screens.py:34` — `read_config()`: `open(path, "r")` then `json.load(f)`
- `screens.py:48` — `read_screen_file()`: same pattern
- `screens.py:62` — `write_screen_file()`: `open(path, "w")` then
  `json.dump(ordered, f, indent=2)` — `json.dump` defaults to
  `ensure_ascii=True`, so today's output happens to be pure ASCII regardless
  of the file's encoding, but the `open()` call itself is still locale-bound
  and would emit locale-specific bytes the moment `ensure_ascii=False` is
  used (which this plan does, in the interest of human-readable pulled
  files).
- `screens.py:182` — `cmd_init()`: `open(CONFIG_FILE, "w")` for
  `micromegas-screens.json`, same `ensure_ascii=True` default so currently
  ASCII-safe, but still locale-bound.

Transport is already safe: `micromegas/web_client.py` uses `requests`'
`json=` parameter (`ensure_ascii=True` by default), so non-ASCII round-trips
as `\uXXXX` escapes over HTTP regardless of local file encoding. The bug is
purely at file-read time in the CLI.

`python/micromegas/tests/test_screen_files.py` already covers
`read_screen_file`/`write_screen_file` round-trips but only with ASCII
content, and never touches locale/encoding, so it wouldn't have caught this.

## Design

### Encoding fixes in `screens.py`

- `read_config()` (line 34) and `read_screen_file()` (line 48): add
  `encoding="utf-8-sig"`. `utf-8-sig` transparently strips a leading BOM if
  present (common from Windows editors like Notepad) and behaves exactly
  like `utf-8` otherwise, so it's a strict improvement over `utf-8` on the
  read side.
- `write_screen_file()` (line 62) and the config-file write in `cmd_init()`
  (line 182): add `encoding="utf-8"` to the `open()` call, and change
  `json.dump(..., indent=2)` to `json.dump(..., indent=2, ensure_ascii=False)`
  so non-ASCII content is written as readable UTF-8 text instead of `\uXXXX`
  escapes. This is safe only because the paired read side is now pinned to
  UTF-8 too (per the issue's own caveat) — doing this without the read-side
  fix would just move the corruption to the write path.
- `format_screen_diff()` (lines 328-329): the two `json.dumps(server_dict, ...)`
  / `json.dumps(local_dict, ...)` calls that build the `plan`/`apply` diff view
  also default to `ensure_ascii=True`; add `ensure_ascii=False` to both for the
  same human-readability reason as `write_screen_file()` — otherwise a screen
  edit containing an em dash or CJK text still renders as `\uXXXX` escapes in
  the diff a user reviews before approving a change, even after the write-side
  fix above ships. No `open()` call is involved here (the dicts are already
  in-memory), so this is a `json.dumps` argument change only.

No other function in `screens.py` opens files in text mode.

### Wider audit (issue item 4)

Grepping `python/micromegas` for `open(` without `encoding=` on text-mode
calls that touch user-controlled or server-controlled content:

- `micromegas/cli/query.py:92` — `pathlib.Path(args.file).read_text()` reads
  a `--file` SQL file with no `encoding=`. Same class of bug (a SQL file with
  a non-ASCII comment or string literal, edited on a non-UTF-8-locale
  machine, would mis-decode). Fix: `read_text(encoding="utf-8")`.
- `micromegas/cli/query.py:89` — the `--file -` (stdin) branch does
  `sys.stdin.read().strip()` with no encoding pin. Identical bug class:
  under a non-UTF-8 locale (e.g. `LC_ALL=C PYTHONUTF8=0`, the same
  environment this plan's own regression test forces), piping non-ASCII SQL
  into stdin mis-decodes before it reaches the server. `sys.stdin` has no
  `encoding=` kwarg to pass since it isn't an `open()` call; fix by
  reconfiguring it explicitly: `sys.stdin.reconfigure(encoding="utf-8")`
  immediately before the `.read()` call.
- This file-reading logic (lines 87-96) currently lives inline in `main()`,
  which makes it untestable without invoking `main()` itself (not viable — it
  opens a live server connection right after). Following the same pattern
  already used by `micromegas/cli/config.py` (`load_config`,
  `resolve_connection`, imported directly by `tests/cli/test_config.py`),
  this plan extracts the `--file <path>`/stdin branch into a standalone
  `read_sql_source(args)` function in `query.py`, with `main()` calling it in
  place of the inlined logic. Only the raw read calls move into
  `read_sql_source(args)` — the `-` stdin branch's `sys.stdin.read().strip()`
  and the file branch's `pathlib.Path(args.file).read_text(...).strip()` —
  and `OSError` from the file branch propagates out of `read_sql_source()`
  uncaught, since `read_sql_source()` takes only `args` and has no `parser`
  to call `.error(...)` on. `main()` keeps its own `try/except OSError`
  wrapped around the call to `read_sql_source(args)`, calling
  `parser.error(...)` there exactly as it does today. This makes both
  encoding fixes above (the `read_text(encoding="utf-8")` and the
  `sys.stdin.reconfigure` call) part of a function the new `test_query.py`
  can import and call directly, rather than a copy of the expression
  re-typed inside the test.
- `micromegas/auth/oidc.py:539` — `open(token_file)` reads a cached OIDC
  token JSON file. Content is a JWT/JSON blob that in practice is always
  ASCII, but pin `encoding="utf-8"` anyway for consistency and to close off
  the same failure mode if that ever changes.
- `micromegas/auth/oidc.py:505` — `os.fdopen(fd, "w")` writes the same token
  file; add `encoding="utf-8"` to match.
- `micromegas/flightsql/client.py:220` — `open(certifi.where(), "r")` reads a
  CA bundle (PEM, pure ASCII by format). Left alone: not a text-mode read of
  arbitrary user/server content, and out of scope for this fix — this is
  `certifi`'s own file, not something a user edits.
- `micromegas/perfetto.py:88` — `open(trace_filepath, "wb")` is binary mode,
  not affected.

Stdout encoding for query results (issue item 4's second half — CLIs
printing non-ASCII on a legacy Windows console codepage): `query.py` prints
`tabulate`/CSV/JSON output straight to `sys.stdout` via `print()`, whose
encoding is whatever the interpreter picked for the console stream.
`screens.py`'s `main()` now unconditionally calls
`sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")` before
dispatching to any subcommand, since `format_screen_diff()`'s
`ensure_ascii=False` output would otherwise raise `UnicodeEncodeError` on a
non-UTF-8 stdout (e.g. a legacy Windows console codepage) instead of just
mis-rendering; `errors="backslashreplace"` keeps `plan`/`apply` usable even
on a console that truly cannot display the characters. `query.py`'s stdout
is not reconfigured by this fix — that remains a follow-up (see Open
Questions).

### `unreadable` tracking in `list_local_screens()`/`compute_plan()`/`cmd_pull()`

Branch review added a second return value to `list_local_screens()`: it now
returns `(screens, unreadable)`, where `unreadable` is the set of file stems
that exist locally but failed to decode or parse (non-UTF-8 bytes, invalid
JSON, or a missing required field) — as opposed to files that are simply
absent. This matters because `compute_plan()` treats a server-tracked name
(`managed_by == managed_by`) with no matching local file as a deletion
candidate; without the `unreadable` set, a locally corrupt file would look
identical to a genuinely deleted one and `apply` would delete the
server-side screen purely because the local copy failed to parse.

An initial version of this fix had `compute_plan()` exclude any name in
`unreadable` from `deletes` by matching `unreadable` (file stems) directly
against server screen *names* — but a file's stem is not guaranteed to match
its internal `name` field, and for a file that fails to decode/parse, the
real `name` is fundamentally unknowable (that's the whole reason it's
unreadable), so there is no way to look up what name to exclude. A follow-up
fix (from a later branch review) corrected this: `compute_plan()` now treats
any non-empty `unreadable` set as a reason to skip delete computation
entirely (leaving `deletes` empty) and prints a warning explaining that
deletes were skipped because N local file(s) could not be read, rather than
attempting name-based filtering. `cmd_pull()` separately re-checks each
target file itself before overwriting it, skipping (with a warning) rather
than silently clobbering a file it cannot read.

## Implementation Steps

1. `python/micromegas/micromegas/cli/screens.py`:
   - `read_config()`: `open(path, "r", encoding="utf-8-sig")`
   - `read_screen_file()`: `open(path, "r", encoding="utf-8-sig")`
   - `write_screen_file()`: `open(path, "w", encoding="utf-8")` and
     `json.dump(ordered, f, indent=2, ensure_ascii=False)`
   - `cmd_init()`: `open(CONFIG_FILE, "w", encoding="utf-8")` and
     `json.dump(config_data, f, indent=2, ensure_ascii=False)`
   - `format_screen_diff()`: both `json.dumps(server_dict, indent=2,
     sort_keys=True)` and `json.dumps(local_dict, indent=2, sort_keys=True)`
     (lines 328-329) gain `ensure_ascii=False`
2. `python/micromegas/micromegas/cli/query.py`:
   - Extract the SQL-source-resolution logic currently inlined in `main()`
     (lines 87-96: the `args.file` truthiness check, the `-` stdin branch,
     the file-read branch, and the `args.sql` fallback) into a new
     `read_sql_source(args)` function taking only `args` (no `parser`
     parameter). The `try/except OSError` stays in `main()`, wrapped around
     the call to `read_sql_source(args)`, with `main()` calling
     `parser.error(...)` in the `except` clause exactly as today;
     `read_sql_source()` itself contains only the raw read calls and lets
     `OSError` propagate out uncaught.
   - Within `read_sql_source()`: `pathlib.Path(args.file).read_text(encoding="utf-8")`
     (was line 92)
   - Within `read_sql_source()`, stdin branch (was line 89):
     `sys.stdin.reconfigure(encoding="utf-8")` before `sys.stdin.read()`
3. `python/micromegas/micromegas/auth/oidc.py`:
   - Add `encoding="utf-8"` to the token-file read (line 539) and the
     `os.fdopen(fd, "w")` write (line 505)
4. `python/micromegas/tests/test_screen_files.py`: add a test that spawns a
   subprocess via `sys.executable` (not the literal string `"python"`, which
   doesn't exist on bare Ubuntu/Debian CI runners — only `python3` does) with
   both `LC_ALL=C` and `PYTHONUTF8=0` set in its environment, running a small
   inline script that first asserts `sys.flags.utf8_mode == 0` (confirming
   the locale-coercion bypass actually took effect) and then reads back
   non-ASCII content (em dash, accented characters, CJK) through
   `read_screen_file` from a fixture file written directly as UTF-8 bytes
   (bypassing `write_screen_file`, so this test isolates the read-side
   `encoding="utf-8-sig"` pin — see step 4a below for the paired write-side
   test). Since the forced locale also makes the child's own `sys.stdout`
   ASCII-encoded (a plain `print()` of the round-tripped content would itself
   raise `UnicodeEncodeError` in the child, failing the test for the wrong
   reason), the child script instead writes the round-tripped content
   directly as UTF-8 bytes via `sys.stdout.buffer.write(content.encode("utf-8"))`,
   and the parent process reads and decodes those bytes to compare against
   the original. Both variables are required together: on Python
   3.10+, `LC_ALL=C` alone is not sufficient to reproduce the bug — PEP 538
   (C-locale coercion) and PEP 540 (UTF-8 mode) make CPython auto-coerce to a
   UTF-8-based locale/mode in that case (verified empirically: under
   `LC_ALL=C` alone, `sys.flags.utf8_mode == 1` and
   `locale.getpreferredencoding(False) == 'UTF-8'`, so a round-trip succeeds
   intact even on unfixed code). Only `LC_ALL=C` combined with
   `PYTHONUTF8=0` forces ASCII decoding (verified: `preferredencoding`
   becomes `ANSI_X3.4-1968` and an unencoded write of non-ASCII content
   raises `UnicodeEncodeError`). A subprocess is required in either case:
   CPython's `TextIOWrapper` resolves its default encoding via OS locale
   APIs (`_Py_GetLocaleEncoding`), not by calling the Python-level
   `locale.getpreferredencoding()` function, so monkeypatching that function
   in-process has no effect on `open()`'s actual behavior — only a real
   environment change on a subprocess forces it.
   - **4a. Write-side coverage (added after branch review):** the read-side
     test above says nothing about `write_screen_file`'s own
     `encoding="utf-8"` + `ensure_ascii=False` pin, since it deliberately
     bypasses `write_screen_file` to isolate the read-side fix. A separate
     `test_write_survives_non_utf8_locale` test uses the same forced-locale
     subprocess technique, but calls `write_screen_file()` itself with
     non-ASCII content (embedded in the child script as an `ascii()`-escaped
     literal, so no non-ASCII bytes travel through argv or `os.environ`) and
     asserts (a) no `UnicodeEncodeError` is raised and (b) the resulting
     on-disk bytes are literal UTF-8 — the actual characters, not `\uXXXX`
     escapes. A small in-process (no subprocess needed) test also extends
     `TestFormatScreenDiff` with a non-ASCII case, since `format_screen_diff`
     only does in-memory `json.dumps` and has no file I/O whose encoding
     could depend on process locale.
5. `python/micromegas/tests/test_query.py` (new file — no test currently
   exercises `cli/query.py`): add regression tests using the same
   `sys.executable` + `LC_ALL=C`/`PYTHONUTF8=0` subprocess technique as step
   4, both importing and calling the real `read_sql_source()` from step 2
   (not a reimplementation of its logic) — matching the pattern already used
   by `tests/cli/test_config.py`, which imports and calls `load_config`/
   `resolve_connection` from `micromegas/cli/config.py` rather than
   reimplementing CLI parsing inline:
   - **`--file <path>` case**: the child script writes a temp SQL file
     containing non-ASCII content (em dash, accented characters, CJK) as
     UTF-8 bytes, asserts `sys.flags.utf8_mode == 0`, builds an
     `argparse.Namespace(file=<path>, sql=None)`, calls
     `query.read_sql_source(args)`, and writes the result back to the parent
     as UTF-8 bytes for comparison against the original.
   - **`--file -` (stdin) case**: the child script writes the same
     non-ASCII content to its own stdin pipe, asserts
     `sys.flags.utf8_mode == 0`, builds an
     `argparse.Namespace(file="-", sql=None)`, calls
     `query.read_sql_source(args)` (exercising the
     `sys.stdin.reconfigure(encoding="utf-8")` fix from step 2), and writes
     the result back to the parent as UTF-8 bytes for comparison against the
     original. Without this case, the stdin-reconfigure fix has no
     regression coverage anywhere in this plan.
   `query.py`'s `main()` isn't invoked directly in either case, since it
   opens a live server connection immediately after parsing the SQL; both
   tests instead exercise `read_sql_source()` in isolation.
6. `python/micromegas/tests/auth/test_oidc_unit.py`: add
   `test_oidc_token_load_non_ascii_locale()` alongside the existing
   `test_oidc_token_save_and_load`, using the same subprocess/env-forcing
   technique. Note `save()`'s `json.dump(data, f, indent=2)` (line 506) keeps
   `ensure_ascii`'s default of `True`, so a `save()`/`from_file()` round-trip
   test would write pure-ASCII `\uXXXX` escapes regardless of locale and
   would pass on both fixed and unfixed code — it wouldn't exercise the
   `open()` read-side pin at all. Instead, the child script (with
   `LC_ALL=C`/`PYTHONUTF8=0` set) writes a token file directly by encoding a
   JSON document containing a non-ASCII `client_id` (em dash/CJK — standing
   in for the "if that ever changes" case noted in the Design section, since
   real JWTs are ASCII in practice) as raw UTF-8 bytes (bypassing
   `json.dump`'s ASCII-escaping and `save()` entirely). `OidcAuthProvider.__init__`
   (and therefore `from_file()`, which calls it) unconditionally does a live
   OIDC discovery request via `_fetch_oidc_metadata()`
   (`requests.get(f"{issuer}/.well-known/openid-configuration", timeout=10)`),
   so — mirroring `test_oidc_token_save_and_load` — the child script must
   itself `patch("micromegas.auth.oidc.requests.get")` and
   `patch("micromegas.auth.oidc.OAuth2Session")` (mocking a discovery
   response and a session whose `.token` echoes the token file's contents)
   before calling `from_file()`; these mocks do not carry over from the
   parent process since they run in a separate subprocess. The child then
   calls `OidcAuthProvider.from_file()` on the raw token file's path and
   writes the loaded `client_id` back to the parent as UTF-8 bytes for
   comparison against the original.
7. From `python/micromegas/`, run `poetry run black
   micromegas/cli/screens.py micromegas/cli/query.py micromegas/auth/oidc.py
   tests/test_screen_files.py tests/test_query.py
   tests/auth/test_oidc_unit.py` before committing (`poetry run` only finds
   `pyproject.toml` by searching the cwd and its ancestors, and it lives at
   `python/micromegas/pyproject.toml`, not the repo root).
8. `mkdocs/docs/web-app/notebooks/screens-as-code.md`, "File Format" section:
   add a bullet noting that screen files are read and written as UTF-8
   (`utf-8-sig` tolerated on read), and that non-ASCII content in a pulled
   file now appears as literal UTF-8 characters (e.g. em dashes, accents,
   CJK) rather than `\uXXXX`-escaped — a user-visible change to the
   documented file format from this plan's `ensure_ascii=False` switch in
   `write_screen_file()`.

## Files to Modify

- `python/micromegas/micromegas/cli/screens.py`
- `python/micromegas/micromegas/cli/query.py`
- `python/micromegas/micromegas/auth/oidc.py`
- `python/micromegas/tests/test_screen_files.py`
- `python/micromegas/tests/test_query.py` (new file)
- `python/micromegas/tests/auth/test_oidc_unit.py`
- `mkdocs/docs/web-app/notebooks/screens-as-code.md`

## Trade-offs

- `utf-8-sig` vs plain `utf-8` for reads: `utf-8-sig` is chosen because it
  silently handles a BOM-prefixed file (common from Windows tooling) while
  being byte-identical to `utf-8` for BOM-less files — no downside for the
  common case, and it directly addresses a scenario the issue calls out.
- `ensure_ascii=False` for writes is only adopted together with the encoding
  fix, per the issue's explicit warning; doing one without the other would
  trade one corruption bug for another.
- The `query.py` and `oidc.py` fixes are included since the issue explicitly
  asks for a broader audit and they're the same one-line fix. The
  stdout-console-codepage concern is resolved for `screens.py` (see
  "Encoding fixes in `screens.py`" above) but left as a follow-up for
  `query.py` (see Open Questions), since reconfiguring `query.py`'s stdout
  touches all of its output paths (table/CSV/JSON) and is a materially
  larger, separate change.

## Testing Strategy

- `test_read_survives_non_utf8_locale` in `test_screen_files.py` spawns a
  subprocess via `sys.executable` (not the literal string `"python"`, which
  is absent on bare Ubuntu/Debian CI runners — only `python3` is guaranteed)
  with both `LC_ALL=C` and `PYTHONUTF8=0` set in its environment (on Python
  3.10+, `LC_ALL=C` alone is coerced back to a UTF-8-based locale/mode by PEP
  538/540 and would not reproduce the bug — both variables together are
  required), running a small script that first asserts
  `sys.flags.utf8_mode == 0` to confirm the non-UTF-8 environment actually
  took effect, then reads em dash / accented / CJK content back through
  `read_screen_file` from a fixture file that is written directly as raw
  UTF-8 bytes (bypassing `write_screen_file`/`json.dump` entirely, per the
  test's own docstring) so this test exercises only the read-side
  `encoding="utf-8-sig"` pin. The forced locale makes the child's own
  `sys.stdout` ASCII-encoded too, so a plain `print()` of that content in the
  child would itself raise `UnicodeEncodeError`, masking the actual bug
  under test; instead the child writes the read-back content as raw UTF-8
  bytes via `sys.stdout.buffer.write(content.encode("utf-8"))`, and the
  parent reads and decodes those bytes to assert exact content preservation.
  This fails on the current code (mis-decodes or raises) and passes once
  `encoding=` is pinned. A subprocess is required because CPython's
  `TextIOWrapper` resolves its default encoding via OS locale APIs, not the
  Python-level `locale.getpreferredencoding()` function, so an in-process
  monkeypatch of that function has no effect on `open()`'s actual behavior —
  this also matches issue #1399 item 3, which specifies forcing the locale
  via environment variables rather than an in-process patch.
- `test_write_survives_non_utf8_locale` in `test_screen_files.py` covers the
  write side that the read-only test above doesn't touch: it uses the same
  forced-locale subprocess technique, but calls `write_screen_file()` itself
  with non-ASCII content (embedded in the child script as an
  `ascii()`-escaped literal so no non-ASCII bytes need to travel through
  argv or `os.environ`, the same technique `test_query.py`'s stdin case
  uses) and asserts both that no `UnicodeEncodeError` is raised and that the
  resulting on-disk bytes are literal UTF-8 (the actual characters, not
  `\uXXXX` escapes). Verified empirically: under `LC_ALL=C PYTHONUTF8=0`,
  the pre-fix `write_screen_file` (plain `open(path, "w")`, no `encoding=`)
  raises `UnicodeEncodeError` on this content, since `ensure_ascii=False`
  output containing e.g. an em dash cannot be encoded via the locale's ASCII
  default — i.e. the write-side pin is load-bearing and this test is what
  verifies it, which the read-only test does not. A small in-process
  addition to `TestFormatScreenDiff` (no subprocess needed, since
  `format_screen_diff` only does in-memory `json.dumps` with no file I/O
  whose encoding could depend on process locale) separately asserts that
  non-ASCII content renders literally rather than as `\uXXXX` escapes.
- Existing `test_screen_files.py` round-trip tests continue to pass
  unchanged (ASCII content is unaffected by the encoding pin).
- New `test_query.py` (step 5) covers the `query.py` fix with the same
  forced-locale technique, importing and calling the real
  `read_sql_source()` extracted in step 2 (rather than reimplementing its
  logic in the test, matching the `test_config.py`/`config.py` pattern):
  a non-ASCII `--file` SQL file mis-decodes (or raises) on the current code
  and round-trips intact once `read_text(encoding="utf-8")` is in place;
  separately, non-ASCII content piped via `--file -` (stdin) mis-decodes on
  the current code and round-trips intact once
  `sys.stdin.reconfigure(encoding="utf-8")` is in place. Without this
  second case, the stdin fix would have no regression test anywhere in this
  plan. Without either test, the `query.py` fix would ship with the exact
  same masking risk the Current State section calls out for `screens.py` —
  ASCII-only fixtures wouldn't have caught the original bug, and there was
  previously no test file for `query.py` at all.
- New `test_oidc_token_load_non_ascii_locale()` in `test_oidc_unit.py` (step
  6) covers the `oidc.py` read-side fix: `save()`'s `json.dump` keeps
  `ensure_ascii=True`, so a `save()`/`from_file()` round-trip test would
  only ever see pure-ASCII escapes and pass regardless of the `open()`
  encoding pin, giving no real coverage. Instead the test writes a raw
  UTF-8-encoded token file directly (bypassing `json.dump`'s escaping) with
  a non-ASCII `client_id`, then (after re-establishing the
  `requests.get`/`OAuth2Session` mocks inside the child subprocess, since
  `from_file()` still triggers a live OIDC discovery call and the parent
  process's mocks don't apply across the subprocess boundary) calls
  `from_file()` under the forced `LC_ALL=C`/`PYTHONUTF8=0` environment: this
  mis-decodes on the current code and reads back intact once
  `open(token_file)` is pinned to `encoding="utf-8"`. The existing
  `test_oidc_token_save_and_load` only
  round-trips a token under the default (already-UTF-8) test-runner locale,
  so it would not have caught this.
- Run `poetry run pytest tests/test_screen_files.py tests/test_query.py
  tests/auth/test_oidc_unit.py` from `python/micromegas/` to verify locally.
  Note: no workflow under `.github/workflows/` currently runs the Python
  test suite for `python/micromegas`, so these tests are local regression
  tests for now, not a CI gate — adding that CI job is out of scope for
  this plan.

## Open Questions

- Whether to also reconfigure `query.py`'s stdout encoding for non-ASCII
  query results on legacy Windows console codepages — the issue mentions it
  (item 4) and `screens.py`'s stdout is already reconfigured to UTF-8 (see
  above), but doing the same for `query.py` is a broader change than the
  file-corruption bug this issue is titled after. Left as a follow-up
  unless the reviewer wants it folded in here.
