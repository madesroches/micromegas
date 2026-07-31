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
UTF-8 instead of escape sequences, and adds a regression test that forces a
non-UTF-8 locale so the CI catches any regression.

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

No other function in `screens.py` opens files in text mode.

### Wider audit (issue item 4)

Grepping `python/micromegas` for `open(` without `encoding=` on text-mode
calls that touch user-controlled or server-controlled content:

- `micromegas/cli/query.py:92` — `pathlib.Path(args.file).read_text()` reads
  a `--file` SQL file with no `encoding=`. Same class of bug (a SQL file with
  a non-ASCII comment or string literal, edited on a non-UTF-8-locale
  machine, would mis-decode). Fix: `read_text(encoding="utf-8")`.
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
2. `python/micromegas/micromegas/cli/query.py`:
   - `pathlib.Path(args.file).read_text(encoding="utf-8")` (line 92)
3. `python/micromegas/micromegas/auth/oidc.py`:
   - Add `encoding="utf-8"` to the token-file read (line 539) and the
     `os.fdopen(fd, "w")` write (line 505)
4. `python/micromegas/tests/test_screen_files.py`: add a test class that
   forces a non-UTF-8 default encoding and round-trips non-ASCII content
   (em dash, accented characters, CJK) through `write_screen_file` /
   `read_screen_file`, asserting the content survives intact. Force the
   non-default encoding by monkeypatching `locale.getpreferredencoding`
   (which `io.open`/`TextIOWrapper` consult when `encoding=None`) to return
   `"cp1252"` — this exercises exactly the code path that misbehaves without
   an explicit `encoding=`, without depending on the CI runner's actual
   locale.
5. Run `poetry run black python/micromegas/micromegas/cli/screens.py
   python/micromegas/micromegas/cli/query.py
   python/micromegas/micromegas/auth/oidc.py
   python/micromegas/tests/test_screen_files.py` before committing.

## Files to Modify

- `python/micromegas/micromegas/cli/screens.py`
- `python/micromegas/micromegas/cli/query.py`
- `python/micromegas/micromegas/auth/oidc.py`
- `python/micromegas/tests/test_screen_files.py`

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

- New unit test(s) in `test_screen_files.py` that monkeypatch
  `locale.getpreferredencoding` to a non-UTF-8 encoding (e.g. `"cp1252"`)
  and round-trip em dash / accented / CJK content through
  `write_screen_file` → `read_screen_file`, asserting exact content
  preservation. This fails on the current code (mis-decodes or raises) and
  passes once `encoding=` is pinned.
- Existing `test_screen_files.py` round-trip tests continue to pass
  unchanged (ASCII content is unaffected by the encoding pin).
- Run `poetry run pytest tests/test_screen_files.py` from
  `python/micromegas/` to verify.

## Open Questions

- Whether to also reconfigure CLI stdout encoding (`query.py`,
  `screens.py`) for non-ASCII query results/diffs on legacy Windows console
  codepages — the issue mentions it (item 4) but it's a broader change
  than the file-corruption bug this issue is titled after. Left as a
  follow-up unless the reviewer wants it folded in here.
