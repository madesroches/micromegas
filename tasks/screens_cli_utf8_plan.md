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
encoding is whatever the interpreter picked for the console stream. Actually
reconfiguring `sys.stdout` (e.g. `sys.stdout.reconfigure(encoding="utf-8")`)
is a bigger, separate change affecting all CLI output paths and isn't part
of the screens JSON corruption this issue reports — noting it here per the
issue's ask, but leaving it as a follow-up rather than folding it into this
fix (see Open Questions).

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
   - `pathlib.Path(args.file).read_text(encoding="utf-8")` (line 92)
   - stdin branch (line 89): `sys.stdin.reconfigure(encoding="utf-8")` before
     `sys.stdin.read()`
3. `python/micromegas/micromegas/auth/oidc.py`:
   - Add `encoding="utf-8"` to the token-file read (line 539) and the
     `os.fdopen(fd, "w")` write (line 505)
4. `python/micromegas/tests/test_screen_files.py`: add a test that spawns a
   subprocess via `sys.executable` (not the literal string `"python"`, which
   doesn't exist on bare Ubuntu/Debian CI runners — only `python3` does) with
   both `LC_ALL=C` and `PYTHONUTF8=0` set in its environment, running a small
   inline script that first asserts `sys.flags.utf8_mode == 0` (confirming
   the locale-coercion bypass actually took effect) and then round-trips
   non-ASCII content (em dash, accented characters, CJK) through
   `write_screen_file` / `read_screen_file` in a temp directory. Since the
   forced locale also makes the child's own `sys.stdout` ASCII-encoded (a
   plain `print()` of the round-tripped content would itself raise
   `UnicodeEncodeError` in the child, failing the test for the wrong reason),
   the child script instead writes the round-tripped content directly as
   UTF-8 bytes via `sys.stdout.buffer.write(content.encode("utf-8"))`, and the
   parent process reads and decodes those bytes to compare against the
   original. Both variables are required together: on Python
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
5. `python/micromegas/tests/test_query.py` (new file — no test currently
   exercises `cli/query.py`): add a regression test using the same
   `sys.executable` + `LC_ALL=C`/`PYTHONUTF8=0` subprocess technique as step
   4. The child script writes a temp SQL file containing non-ASCII content
   (em dash, accented characters, CJK) as UTF-8 bytes, asserts
   `sys.flags.utf8_mode == 0`, then runs the equivalent of query.py's
   file-reading line — `pathlib.Path(path).read_text(encoding="utf-8").strip()`
   — and writes the result back to the parent as UTF-8 bytes for comparison
   against the original. `query.py`'s `main()` isn't invoked directly, since
   it opens a live server connection immediately after parsing the SQL; this
   test instead exercises the fixed `read_text(encoding="utf-8")` call from
   step 2 in isolation.
6. `python/micromegas/tests/auth/test_oidc_unit.py`: add
   `test_oidc_token_save_and_load_non_ascii_locale()` alongside the existing
   `test_oidc_token_save_and_load`, using the same subprocess/env-forcing
   technique: a child script (with `LC_ALL=C`/`PYTHONUTF8=0` set, and the
   existing test's `requests.get`/`OAuth2Session` mocks reproduced inline)
   builds an `OidcAuthProvider` whose `client_id` contains non-ASCII content
   (em dash/CJK — standing in for the "if that ever changes" case noted in
   the Design section, since real JWTs are ASCII in practice), calls
   `.save()`, then `OidcAuthProvider.from_file()` on the same path, and
   writes the reloaded `client_id` back to the parent as UTF-8 bytes for
   comparison against the original.
7. From `python/micromegas/`, run `poetry run black
   micromegas/cli/screens.py micromegas/cli/query.py micromegas/auth/oidc.py
   tests/test_screen_files.py tests/test_query.py
   tests/auth/test_oidc_unit.py` before committing (`poetry run` only finds
   `pyproject.toml` by searching the cwd and its ancestors, and it lives at
   `python/micromegas/pyproject.toml`, not the repo root).

## Files to Modify

- `python/micromegas/micromegas/cli/screens.py`
- `python/micromegas/micromegas/cli/query.py`
- `python/micromegas/micromegas/auth/oidc.py`
- `python/micromegas/tests/test_screen_files.py`
- `python/micromegas/tests/test_query.py` (new file)
- `python/micromegas/tests/auth/test_oidc_unit.py`

## Trade-offs

- `utf-8-sig` vs plain `utf-8` for reads: `utf-8-sig` is chosen because it
  silently handles a BOM-prefixed file (common from Windows tooling) while
  being byte-identical to `utf-8` for BOM-less files — no downside for the
  common case, and it directly addresses a scenario the issue calls out.
- `ensure_ascii=False` for writes is only adopted together with the encoding
  fix, per the issue's explicit warning; doing one without the other would
  trade one corruption bug for another.
- The `query.py` and `oidc.py` fixes are included since the issue explicitly
  asks for a broader audit and they're the same one-line fix, but the
  stdout-console-codepage concern is left as a follow-up (see Open
  Questions) since it's a materially larger, separate change.

## Testing Strategy

- New unit test(s) in `test_screen_files.py` that spawn a subprocess via
  `sys.executable` (not the literal string `"python"`, which is absent on
  bare Ubuntu/Debian CI runners — only `python3` is guaranteed) with both
  `LC_ALL=C` and `PYTHONUTF8=0` set in its environment (on Python 3.10+,
  `LC_ALL=C` alone is coerced back to a UTF-8-based locale/mode by PEP
  538/540 and would not reproduce the bug — both variables together are
  required), running a small script that first asserts
  `sys.flags.utf8_mode == 0` to confirm the non-UTF-8 environment actually
  took effect, then round-trips em dash / accented / CJK content through
  `write_screen_file` → `read_screen_file`. The forced locale makes the
  child's own `sys.stdout` ASCII-encoded too, so a plain `print()` of that
  content in the child would itself raise `UnicodeEncodeError`, masking the
  actual bug under test; instead the child writes the round-tripped content
  as raw UTF-8 bytes via `sys.stdout.buffer.write(content.encode("utf-8"))`,
  and the parent reads and decodes those bytes to assert exact content
  preservation. This fails on the current code (mis-decodes or raises) and
  passes once `encoding=` is pinned. A subprocess is required because
  CPython's `TextIOWrapper` resolves its default encoding via OS locale
  APIs, not the Python-level `locale.getpreferredencoding()` function, so an
  in-process monkeypatch of that function has no effect on `open()`'s actual
  behavior — this also matches issue #1399 item 3, which specifies forcing
  the locale via environment variables rather than an in-process patch.
- Existing `test_screen_files.py` round-trip tests continue to pass
  unchanged (ASCII content is unaffected by the encoding pin).
- New `test_query.py` (step 5) covers the `query.py` fix with the same
  forced-locale technique: a non-ASCII `--file` SQL file mis-decodes (or
  raises) on the current code and round-trips intact once
  `read_text(encoding="utf-8")` is in place. Without this test, the
  `query.py` fix would ship with the exact same masking risk the Current
  State section calls out for `screens.py` — ASCII-only fixtures wouldn't
  have caught the original bug, and there was previously no test file for
  `query.py` at all.
- New `test_oidc_token_save_and_load_non_ascii_locale()` in
  `test_oidc_unit.py` (step 6) covers the `oidc.py` fix the same way: a
  non-ASCII `client_id` fails to survive a `save()`/`from_file()`
  round-trip under the forced `LC_ALL=C`/`PYTHONUTF8=0` environment on the
  current code, and survives once both `open()` calls in `oidc.py` are
  pinned to `encoding="utf-8"`. The existing `test_oidc_token_save_and_load`
  only round-trips a token under the default (already-UTF-8) test-runner
  locale, so it would not have caught this.
- Run `poetry run pytest tests/test_screen_files.py tests/test_query.py
  tests/auth/test_oidc_unit.py` from `python/micromegas/` to verify locally.
  Note: no workflow under `.github/workflows/` currently runs the Python
  test suite for `python/micromegas`, so these tests are local regression
  tests for now, not a CI gate — adding that CI job is out of scope for
  this plan.

## Open Questions

- Whether to also reconfigure CLI stdout encoding (`query.py`,
  `screens.py`) for non-ASCII query results/diffs on legacy Windows console
  codepages — the issue mentions it (item 4) but it's a broader change
  than the file-corruption bug this issue is titled after. Left as a
  follow-up unless the reviewer wants it folded in here.
