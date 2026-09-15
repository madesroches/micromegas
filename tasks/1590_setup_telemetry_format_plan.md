# `micromegas-setup-telemetry --format` Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1590

## Overview

`micromegas-setup-telemetry` renders its `OTEL_EXPORTER_OTLP_*` output in exactly one
syntax — POSIX `export` lines — on both its stdout and `--env-file` paths. A caller at a
native PowerShell or `cmd.exe` prompt, or one feeding a container/CI env-file, cannot
consume that output and has to transcribe the key by hand. This plan adds
`--format {posix,powershell,cmd,dotenv}` (default `posix`) applying to both output paths,
backed by a small per-format renderer registry so a fifth dialect is an added entry rather
than an edit to the rendering logic.

## Current State

`format_env_exports(key, otlp_endpoint)`
(`python/micromegas/micromegas/cli/setup_telemetry.py:277`) returns a fixed string:

```python
return (
    "export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf\n"
    f"export OTEL_EXPORTER_OTLP_ENDPOINT={otlp_endpoint}\n"
    f'export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer {key}"\n'
)
```

`run()` calls it once and either writes the result through `write_env_file` (mode `0o600`,
binary, LF-only — see `tasks/completed/1588_setup_telemetry_windows_env_file_plan.md`) or
`sys.stdout.write`s it. The documented consumption is
`eval "$(micromegas-setup-telemetry ...)"`.

Three properties of the current code shape the design:

- **The three variable names/values are hardcoded into the rendering string**, so any second
  dialect would duplicate the knowledge of *which* variables are emitted.
- **`run()` orders its steps so a local validation error can never strand a minted key**:
  `resolve_audience` and `resolve_otlp_endpoint` both run before `client.mint_ingestion_api_key`,
  because the minted key is never retrievable again and re-minting leaves a live orphaned key
  behind (only an admin can revoke it). Any new validation must land on the same side of the
  mint.
- **`export OTEL_EXPORTER_OTLP_ENDPOINT={otlp_endpoint}` is unquoted.** An endpoint carrying a
  shell metacharacter (`&`, `;`, `?`, a space) is therefore mis-`eval`'d today — e.g.
  `export ...ENDPOINT=https://h/otlp?a=b&c=d` backgrounds a command under `bash`. Latent, but
  on the same line this plan is rewriting.

Values, and who controls them:

| value | source | known before the mint? |
| --- | --- | --- |
| `http/protobuf` | constant | yes |
| `otlp_endpoint` | `--otlp-endpoint` or `$MICROMEGAS_TELEMETRY_URL` | **yes** |
| `Authorization=Bearer <key>` | server's mint response | no |

The key is `mmk_` + base64url-nopad (`generate_key`, `rust/auth/src/db_api_key.rs:129`), so
its alphabet (`A-Za-z0-9-_`) is safe in all four target dialects. The endpoint is the only
user-controlled value, and it is resolved pre-mint — which is what makes per-format validation
possible without ever risking the key.

## Design

### 1. Split "which variables" from "how a line is rendered"

```python
def _env_var_pairs(key, otlp_endpoint):
    """The variables to export, in order. The protocol var is required because
    micromegas exposes OTLP over HTTP only ..."""
    return (
        ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf"),
        ("OTEL_EXPORTER_OTLP_ENDPOINT", otlp_endpoint),
        ("OTEL_EXPORTER_OTLP_HEADERS", f"Authorization=Bearer {key}"),
    )
```

One renderer per dialect, each `(name, value) -> str` (one line, no trailing newline):

| format | rendered line | quoting rule |
| --- | --- | --- |
| `posix` | `export NAME=<shlex.quote(value)>` | `shlex.quote` — leaves URL/`http/protobuf` bare, single-quotes anything with a space or metacharacter |
| `powershell` | `$env:NAME = '<value>'` | PowerShell single-quoted literal; `'` escaped by doubling, but a line break has no representation |
| `cmd` | `@set "NAME=value"` | the quoted-`set` form keeps the quotes out of the value |
| `dotenv` | `NAME=value` | unquoted |

```python
_FORMAT_RENDERERS = {
    "posix": _render_posix,
    "powershell": _render_powershell,
    "cmd": _render_cmd,
    "dotenv": _render_dotenv,
}
```

`format_env_exports(key, otlp_endpoint, fmt="posix")` keeps its name and its default, joins
the rendered lines with `"\n"`, and ends with a trailing `"\n"` exactly as today. Adding a
dialect means one renderer plus one registry entry — the flag's `choices` and the docs table
are the only other places that name formats, and `choices` is derived from the registry.

Insertion order of `_FORMAT_RENDERERS` is the order `--help` lists the choices, so it is kept
as the issue lists them (`posix` first, as the default).

### 2. Quoting choices, and what each one buys

- **`posix` via `shlex.quote`** rather than the current unconditional `"..."`: it fixes the
  unquoted-endpoint hole above, and it single-quotes (no `$`/backtick/`\` expansion inside),
  which is what a literal credential wants. Output for a typical run differs from today only
  in the header line's quote character:

  ```
  export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
  export OTEL_EXPORTER_OTLP_ENDPOINT=https://analytics.example.com/ingestion/otlp
  export OTEL_EXPORTER_OTLP_HEADERS='Authorization=Bearer mmk_...'
  ```

  Identical under `eval` and identical when sourced from a shell profile.

- **`powershell` single-quoted, not the issue's `"..."`**: PowerShell expands `$` inside
  double quotes, and a value is a credential, never a template. `'` → `''` is PowerShell's
  own escape and represents any value except a line break, which the `| Invoke-Expression`
  pipeline cannot consume (§3).

- **`cmd`'s `@set "NAME=value"`**: the quoted-`set` form is the only one that keeps the quote
  characters out of the value — `cmd.exe` has no escape for a `"` inside it, and `%` means
  different things in a batch file (`%%`) than at the interactive prompt, so unsafe characters
  are rejected rather than escaped context-dependently (§3). The leading `@` is what keeps the
  credential out of the console: `call`ing a file of plain `set` lines echoes each line as it
  runs, printing the bearer token into the terminal and its scrollback; `@` suppresses that
  echo without changing the stored value, and is accepted both in a batch file and at the
  interactive prompt.

- **`dotenv` unquoted**: loaders disagree on whether surrounding quotes are stripped (older
  `docker compose --env-file` kept them literally, `python-dotenv` strips them), while the
  bare `NAME=value` form — value is everything after the first `=`, interior spaces preserved
  — is read identically by `python-dotenv`, `docker compose`, and the common app-config
  loaders. `OTEL_EXPORTER_OTLP_HEADERS=Authorization=Bearer mmk_...` is correct for all of
  them. The cost is that a value with a leading/trailing space or a `#` is not representable
  (§3).

### 3. Per-format unsafe characters, checked before the mint

One table, keyed by format, listing the characters a dialect cannot represent under the
quoting rule above:

```python
_FORMAT_UNSAFE_CHARS = {
    "posix": (),                  # shlex.quote represents any value
    "powershell": ("\r", "\n"),   # '' escaping represents any value except a line break
    "cmd": ('"', "%", "\r", "\n"),
    "dotenv": ("#", "\r", "\n"),
}
```

Used twice, for the two values that can carry one:

- **The endpoint — a hard error, pre-mint.** In `run()`, immediately after
  `resolve_otlp_endpoint` and before `client.mint_ingestion_api_key`, a
  `check_format_endpoint(args.format, otlp_endpoint, parser)` helper calls `parser.error` when
  the resolved endpoint contains an unsafe character for the requested format, naming the
  character and the format and suggesting another format. Placed on the pre-mint side for the
  same reason `resolve_otlp_endpoint` is: nothing that can fail locally may run after a key
  exists. `dotenv` additionally rejects a leading/trailing-whitespace endpoint, which a
  loader would silently trim.
- **The header value — a stderr warning, post-mint, never an error.** The minted key's
  alphabet is safe in every format today, so this arm is unreachable; it exists so that a
  future change to the server's key alphabet degrades to a warning rather than to silently
  mangled output, and it warns instead of raising because by then the key exists and must not
  be discarded. The content is still emitted unchanged.

### 4. Line endings stay LF for every format

Renderers join with `"\n"` only, and `write_env_file` keeps its byte-exact binary write
(#1588's guarantee). `cmd.exe` parses plain `set` lines with LF endings, and a Windows console
redirect (`... --format cmd > telemetry.cmd`) gets CRLF for free from Python's text-mode
stdout translation. Emitting `"\r\n"` from the renderer would instead produce `\r\r\n` on that
path. The same text-mode translation also turns the `posix`/`dotenv` stdout path into CRLF on
Windows, and `eval`/`source` under bash then fold that trailing `\r` into the last quoted
value — this is pre-existing (today's hardcoded `export ... "..."` string has the same shape)
and is neither caused nor fixed by this plan; see Decisions.

### 5. `--format` applies to both output paths; `--env-file` keeps the `posix` default

```python
parser.add_argument(
    "--format",
    choices=tuple(_FORMAT_RENDERERS),
    default="posix",
    help="Output syntax for the env vars (default: posix)",
)
```

`run()` passes `args.format` into the single `format_env_exports` call that already feeds both
the `--env-file` and the stdout branch, so the flag covers both with no branching. `--env-file`
does **not** switch its default to `dotenv`: existing callers source that file from a shell
profile, and `dotenv` output is not a shell script.

## Implementation Steps

1. **`python/micromegas/micromegas/cli/setup_telemetry.py`**
   - `import shlex`.
   - Add `_env_var_pairs`, the four `_render_*` helpers, `_FORMAT_RENDERERS`,
     `_FORMAT_UNSAFE_CHARS`, `_unsafe_chars_in(fmt, value)`, and
     `check_format_endpoint(fmt, otlp_endpoint, parser)`.
   - Rewrite `format_env_exports(key, otlp_endpoint, fmt="posix")` over the registry; update
     its docstring to cover the per-dialect quoting rules and drop the now-stale claim that
     the output is `export` lines.
   - `build_parser`: add `--format`.
   - `run()`: call `check_format_endpoint` right after `resolve_otlp_endpoint`; pass
     `args.format` to `format_env_exports`; warn on stderr when the header value carries a
     character unsafe for the chosen format.
2. **`python/micromegas/tests/cli/test_setup_telemetry.py`** — add `"format": "posix"` to
   `make_args`' defaults, delete `test_format_env_exports_includes_protocol_endpoint_and_bearer_header`
   (whose coverage the posix exact-output assertion absorbs), and add the cases in Testing
   Strategy.
3. **Docs** — `mkdocs/docs/query-guide/python-api.md` and
   `mkdocs/docs/admin/authorization.md` (see Documentation).
4. **`CHANGELOG.md`** — one `**Python:**` bullet under `## Unreleased`.

## Files to Modify

- `python/micromegas/micromegas/cli/setup_telemetry.py`
- `python/micromegas/tests/cli/test_setup_telemetry.py`
- `mkdocs/docs/query-guide/python-api.md`
- `mkdocs/docs/admin/authorization.md`
- `CHANGELOG.md`

## Trade-offs

- **No OS auto-detection**, per the issue: the OS does not determine the shell (Git Bash on
  Windows wants `posix`; `pwsh` runs on Linux/macOS), and guessing wrong fails *after* a
  non-retryable key has been minted.
- **Ship `cmd` even though the issue rates it the weakest of the four.** It is ~3 lines and
  one test on top of the registry the other formats need, and #1588's own reporter was at a
  `cmd.exe` prompt.

## Decisions

- `posix` output changes one character in practice (the header line's `"` becomes `'`) rather
  than staying byte-identical: accepted, because the same edit closes the unquoted-endpoint
  `eval` hole, and both forms are identical to every shell consumer.
- The minted-key alphabet is treated as a fact to *degrade gracefully* against, not to depend
  on: no pre-mint key validation (impossible), a post-mint warning instead.
- `--env-file`'s default format stays `posix`.
- `cmd` renders `@set` (not plain `set`) so that `call`ing the generated file does not echo the
  key to the console.
- The Windows stdout CRLF-folding-into-value behavior on `posix`/`dotenv` (verified: bash's
  `eval`/`source` fold the trailing `\r` into the last quoted value) is a pre-existing
  limitation, unchanged by this plan; fixing it (e.g. forcing `newline=""` on `sys.stdout`) is
  out of scope.

## Documentation

- **`mkdocs/docs/query-guide/python-api.md`** § `micromegas-setup-telemetry`: document
  `--format` next to `--env-file` — the four-row table of rendered shape and consumption
  command (`eval "$(...)"`, `| Invoke-Expression`, redirect-to-`.cmd`-and-`call`,
  `docker compose --env-file`/`python-dotenv`), the quoting convention per dialect, that the
  default is `posix` and is never inferred from the OS, that the flag covers both stdout and
  `--env-file`, and that `cmd`/`dotenv` reject an endpoint carrying a character they cannot
  represent. Add a PowerShell and a dotenv example to the existing example block.
- **`mkdocs/docs/admin/authorization.md`** § `micromegas-setup-telemetry` wraps login…: add a
  one-line PowerShell equivalent beside the existing `eval "$(...)"` line, pointing at
  `python-api.md` for the rest.
- `README.md` needs no change (its CLI line is a past release's highlight); the dated blog
  post's POSIX example stays as written.

## Testing Strategy

Everything here is pure rendering, flag parsing, and `run()` sequencing, all reachable with
constructed inputs and the existing `FakeClient`/`FakeParser` — so it is all unit-tested, with
no live DB or service.

`python/micromegas/tests/cli/test_setup_telemetry.py`:

- `build_parser` defaults `--format` to `posix`, accepts each of the four, and `SystemExit`s on
  an unknown value.
- One exact-output assertion per format for a fixed key and endpoint, covering line order, the
  per-dialect quoting, and the trailing newline (for `posix`, this pins `http/protobuf` and the
  endpoint bare and the header value quoted, matching the documented `eval` usage).
- `posix` quotes an endpoint containing `&` (the latent-hole regression).
- `powershell` doubles a `'` in a value.
- `cmd` with a `%`-bearing endpoint and `dotenv` with a `#`-bearing endpoint both exit through
  `parser.error`, and `FakeClient.calls` records **no** `mint` — pinning that the check is on
  the pre-mint side.
- `dotenv` rejects a trailing-whitespace endpoint.
- A mint result whose key carries a `cmd`-unsafe character warns on stderr and still emits the
  key (pins "never lose the key").
- `run(--format dotenv --env-file PATH)` writes dotenv content to the file and prints only the
  path on stdout; `run(--format powershell)` with no `--env-file` writes PowerShell to stdout.

## Manual Verification

Only the two dialects whose real consumer cannot be exercised from a Linux test run. Both need
a Windows (or `pwsh`) prompt against a deployment with self-service mint enabled, and both
fail visibly and immediately if wrong, which is why they are not automated.

1. From PowerShell:
   `micromegas-setup-telemetry --url <url> --name pwsh-check --format powershell | Invoke-Expression`
   then `$env:OTEL_EXPORTER_OTLP_HEADERS` — expected: `Authorization=Bearer mmk_...`, and the
   three variables set in the session.
2. From `cmd.exe`:
   `micromegas-setup-telemetry --url <url> --name cmd-check --format cmd > %TEMP%\telemetry.cmd`
   then `call %TEMP%\telemetry.cmd` and `echo %OTEL_EXPORTER_OTLP_HEADERS%` — expected: the
   same value, with no surrounding quote characters (the `set "VAR=value"` form's whole point).
