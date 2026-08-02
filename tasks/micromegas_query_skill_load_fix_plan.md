# micromegas-query Skill Load Fix Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1404

## Overview

`claude-plugin/skills/micromegas-query/SKILL.md` fails to load with a hard error whenever its
own setup preconditions are unmet, because its `## Environment` section probes those
preconditions with two shell interpolations that abort the whole skill load on a non-zero exit
or a permission-rule miss. This plan removes the load-time probes, moves environment
verification to an ordinary first-use `Bash` check the agent can react to, switches persistent
configuration from a sourced env file (which never reaches an agent's tool calls) to
`~/.micromegas/config.json` (already read by the library on every invocation), and tightens
`allowed-tools` to the minimum the skill actually needs. It also fixes several smaller
documentation inaccuracies called out in the issue.

## Current State

`claude-plugin/skills/micromegas-query/SKILL.md`:

- Frontmatter (lines 1-6) declares `allowed-tools` including bare `Write, Edit` (unscoped —
  grants write access to any path) and env-file-oriented `Bash(source ~/.micromegas_env)` /
  `Bash(source ~/.micromegas_env *)` rules. No `shell:` key is set, so on Windows without Git
  Bash present, interpolation runs under PowerShell and `Bash(...)` rules never match it.
- `## Environment` (lines 22-25) probes with `` !`which micromegas-query` `` and
  `` !`printenv MICROMEGAS_ANALYTICS_URI` ``. Both exit 1 when the thing being checked for is
  absent, and a failed `` !`…` `` interpolation aborts the entire skill load — so the skill
  cannot load in precisely the two situations `## Setup` (lines 27-52) exists to repair.
- `## Setup` writes `~/.micromegas_env` with `export MICROMEGAS_ANALYTICS_URI=...` etc., then
  appends `source ~/.micromegas_env` to `~/.bashrc`/`~/.zshrc`. Each agent tool call is a fresh
  non-interactive shell, so this file only takes effect for a human's *next* interactive shell,
  never for the agent's own subsequent `Bash` calls in the same or a later session.
- `python/micromegas/micromegas/cli/config.py` already implements the alternative: it defines
  `resolve_connection()` which reads `~/.micromegas/config.json` (schema: `uri`, `client_id`,
  `issuers: [{issuer, audience}]`) with precedence env vars > config file > default
  (`grpc://localhost:50051`), and `cli/connection.py:connect()` uses it directly. This has been
  present since `micromegas` 0.25.0; `pyproject.toml` (`python/micromegas/pyproject.toml:3`)
  currently publishes 0.28.0, so it's available in every supported install. There is no
  config-file key for `oidc_scope` — `resolve_connection()` reads it only via
  `_pick("MICROMEGAS_OIDC_SCOPE")`, with no config-file fallback — so a non-default scope (e.g.
  Azure's `api://{client_id}/.default ...`) can only be set via that environment variable, not
  written into `~/.micromegas/config.json`. This is fine for the common case, since
  `auth/oidc.py:69` already defaults it to `"openid email profile offline_access"` when unset,
  but it means `## Setup` must not promise to persist a scope value the config file has no slot
  for.
- The `--begin <ts>Z` failure on Python 3.10 referenced in the issue (and tracked as #1403) was
  fixed by commit `3260dfbca` ("Accept full RFC 3339 timestamps in the Python client and CLI
  (#1407)"), landed on `main` ahead of this plan. No action needed here.
- The "Listing UDFs" and interactive-SSO/connection-error notes from the issue are plain
  documentation gaps in the SKILL.md body — no code changes needed for those.

## Design

### 1. Drop load-time probes; check on first use instead

Remove the `## Environment` section's two `` !`…` `` interpolations entirely. Replace the
up-front probe with a single instruction folded into `## Setup` telling the agent to verify the
connection with an ordinary `Bash` call *before* running the user's actual query, and to react to
its result:

```
python3 -c "from micromegas.cli.config import resolve_connection; c = resolve_connection(); print(c.uri); print(c.oidc_issuer); print(c.oidc_client_id)"
```

This is total (never a non-zero exit on missing config — a missing config just resolves to the
default URI) and answers "is `micromegas` installed and importable?", "what will it connect to?",
and "is OIDC configured?" in one call, without duplicating `resolve_connection()`'s precedence
rules in the skill text. `resolve_connection()` itself makes no network calls, so this probe can
never trigger the interactive browser login that only `connect()`'s OIDC path does — it only
reports whether that path is configured. `connect()` only takes the OIDC branch when
`cfg.oidc_issuer and cfg.oidc_client_id` are *both* truthy
(`python/micromegas/micromegas/cli/connection.py:17`), and `resolve_connection()`'s `_pick()`
resolves `oidc_issuer` and `oidc_client_id` independently (e.g. via separate env vars), so the
probe must print both fields rather than `oidc_client_id` alone. Note in the skill body that a
printed `grpc://localhost:50051` is ambiguous (unconfigured vs. deliberately local) and must not
by itself trigger overwriting an existing config, and that printed `oidc_issuer` and
`oidc_client_id` values that are *both* not `None` mean the *first* real `micromegas-query` call
may open a browser for login — see Design §5.

An ordinary `Bash` tool call surfaces a normal, recoverable tool error (e.g.
`ModuleNotFoundError`) that the agent can read and act on by following `## Setup`, rather than an
unrecoverable load-time abort.

The two failure shapes must be told apart: `ModuleNotFoundError: No module named 'micromegas'`
means the package is absent, so `## Setup` should run `pip install micromegas`; but
`ModuleNotFoundError: No module named 'micromegas.cli.config'` (or any other import error naming
something inside an already-resolvable `micromegas` package) means a pre-0.25.0 version is already
installed, so `## Setup` must run `pip install --upgrade micromegas` instead — plain
`pip install micromegas` is a no-op once any version is present and would leave the outdated,
`cli/config.py`-less install in place.

On Windows without Git Bash present, the agent's ordinary shell tool calls route through the
separate `PowerShell` tool, not `Bash` — the same Bash-vs-PowerShell mismatch Design §2 calls out
for future interpolations applies equally to this first-use probe. So `## Setup` must instruct the
agent to run the probe with whichever shell tool it would normally use on the current platform
(`Bash` or `PowerShell`), and Design §4 must grant matching `PowerShell(...)` rules alongside the
`Bash(...)` ones for the same probe script variants.

#### 1a. Don't hardcode `python3`

Stock Windows Python installs commonly expose `python` and/or the `py` launcher, not a `python3`
alias, so a probe that only ever runs `python3 -c "..."` would itself fail on the Windows boxes
this plan is trying to make more robust. Instruct `## Setup` to try the probe as `python3` first
and, only if that command is not found, fall back to `python` and then `py -3` (each with the
identical script content shown above). Since `allowed-tools` matches exact command strings, all
three interpreter variants must be granted in Design §4, not just `python3` — and, per the
Windows/PowerShell note in Design §1, granted under both the `Bash` and `PowerShell` tool names,
since which one the agent actually calls depends on the platform, not on this plan's preference.

### 2. Declare `shell: bash` in frontmatter

Add `shell: bash` to the frontmatter as a defensive default: after Design §1 removes the only
`` !`…` `` interpolations currently in the file, there is nothing left for `shell` to govern, but
setting it now means any interpolation added to this skill in the future is explicit about which
shell runs it, turning a Windows/PowerShell mismatch into an actionable error rather than a silent
permission miss, instead of relying on someone remembering to add the key later.

### 3. Move persistent config to `~/.micromegas/config.json`

Rewrite `## Setup` to write `~/.micromegas/config.json` instead of `~/.micromegas_env` +
shell-profile append:

```json
{
  "uri": "<value from user>",
  "client_id": "<value from user, only if OIDC>",
  "issuers": [{ "issuer": "<value from user>", "audience": "<value from user>" }]
}
```

This schema has no `scope` field, matching `resolve_connection()`, which reads `oidc_scope` only
via `MICROMEGAS_OIDC_SCOPE` with no config-file fallback. Update `## Setup`'s ask-user list
accordingly: ask only for the issuer URL, client ID, and audience (drop "scope"). If the user
needs a non-default scope (e.g. Azure's `api://{client_id}/.default ...`), tell them to set
`MICROMEGAS_OIDC_SCOPE` themselves in their own shell profile — this is outside the skill's
automated config-file flow, since `config.json` has no slot for it.

This takes effect immediately for every subsequent `micromegas-query` invocation in the same or
a later session — no profile edit, no `source`, no new terminal. Drop the `~/.bashrc`/`~/.zshrc`
append step and the caution about not prefixing commands with `source ~/.micromegas_env &&`
(both become moot). Keep directing the agent to read the existing file first (via `Read`) and
merge rather than clobber if it already has content, since the agent already has `Read`/`Edit`
access to it.

### 4. Tighten `allowed-tools`

Change the frontmatter `allowed-tools` line from:

```
Bash(source ~/.micromegas_env), Bash(source ~/.micromegas_env *), Bash(pip install micromegas), Bash(micromegas-query *), Bash(which micromegas-query), Bash(printenv MICROMEGAS_ANALYTICS_URI), Read, Write, Edit, Glob, Grep, WebFetch(...)
```

to:

```
Bash(pip install micromegas), Bash(pip install --upgrade micromegas), Bash(micromegas-query *), Bash(python3 -c "from micromegas.cli.config import resolve_connection; c = resolve_connection(); print(c.uri); print(c.oidc_issuer); print(c.oidc_client_id)"), Bash(python -c "from micromegas.cli.config import resolve_connection; c = resolve_connection(); print(c.uri); print(c.oidc_issuer); print(c.oidc_client_id)"), Bash(py -3 -c "from micromegas.cli.config import resolve_connection; c = resolve_connection(); print(c.uri); print(c.oidc_issuer); print(c.oidc_client_id)"), PowerShell(python3 -c "from micromegas.cli.config import resolve_connection; c = resolve_connection(); print(c.uri); print(c.oidc_issuer); print(c.oidc_client_id)"), PowerShell(python -c "from micromegas.cli.config import resolve_connection; c = resolve_connection(); print(c.uri); print(c.oidc_issuer); print(c.oidc_client_id)"), PowerShell(py -3 -c "from micromegas.cli.config import resolve_connection; c = resolve_connection(); print(c.uri); print(c.oidc_issuer); print(c.oidc_client_id)"), Read, Edit(~/.micromegas/config.json), Glob, Grep, WebFetch(...)
```

- Drop `Bash(source ~/.micromegas_env)` / `Bash(source ~/.micromegas_env *)` — no longer used.
- Drop `Bash(which micromegas-query)` / `Bash(printenv MICROMEGAS_ANALYTICS_URI)` — replaced by
  the first-use config probe above, granted as three separate exact strings (one per interpreter
  variant from Design §1a: `python3`, `python`, `py -3`), not `*`-suffixed prefixes, since a
  trailing `*` on a fixed command compiles to a prefix wildcard and would permit anything sharing
  that prefix. Each of the three is granted twice — once as `Bash(...)` and once as
  `PowerShell(...)` — per the Windows note in Design §1, since the tool name the agent's shell
  calls route through depends on the platform, not the interpreter chosen.
- Add `Bash(pip install --upgrade micromegas)` — `pip install micromegas` alone will not upgrade
  an existing pre-0.25.0 install to one that has `cli/config.py`.
- Replace bare `Write, Edit` with `Edit(~/.micromegas/config.json)` — the skill's only legitimate
  write target. Per Claude Code's permission model (v2.1.210+), file-write permissions are
  checked against `Edit(path)`/`Read(path)` rules only; a bare `Write(path)` rule is accepted but
  never consulted. An `Edit(path)` rule covers all file-editing tools, including `Write`, so the
  scoped rule is sufficient and `Write` itself doesn't need to appear in `allowed-tools`.
- Keep `Read` unscoped (needed to read arbitrary query result files / existing config) and
  `Bash(micromegas-query *)` as-is (already a legitimate wide surface — arbitrary SQL is the
  point of the skill).

### 5. Documentation corrections in the SKILL.md body

- **Interactive SSO**: add a note that the OIDC login flow is interactive and blocks. The
  `python3 -c` probe from Design §1 makes no network calls, so it cannot itself trigger this —
  only the *first* real `micromegas-query` call does, via `connect()`'s call to
  `oidc_connection.load_or_login()` when no cached token exists yet, and only when `connect()`'s
  gate (`cfg.oidc_issuer and cfg.oidc_client_id`) is satisfied. Instruct the agent to check the
  probe's `oidc_issuer` and `oidc_client_id` output and, whenever *both* are not `None`, ask the
  *user* to run that first `micromegas-query` call themselves (so a browser can open in their
  session), rather than the agent attempting it and hanging.
- **Connection error interpretation**: note that a connection error to `127.0.0.1:50051` means
  the URI never resolved (falls back to the default), not that authentication failed — call this
  out since it can appear interleaved with token-refresh output that suggests the wrong cause.
- **UDF listing**: add new guidance for listing UDFs via `information_schema.routines`, since the
  skill currently has none. Insert it as a new subsection (e.g. "Discovering UDFs") under
  `## Key functions`, using `routine_name` as the column to select (not `function_name`) and
  mentioning `description` and `syntax_example` as additional useful columns.

## Implementation Steps

1. Edit `claude-plugin/skills/micromegas-query/SKILL.md` frontmatter: add `shell: bash`; replace
   the `allowed-tools` line per Design §4.
2. Remove the `## Environment` section (lines 22-25) entirely.
3. Rewrite `## Setup` per Design §1, §1a, and §3: replace the env-file/profile instructions with
   the `~/.micromegas/config.json` read-merge-write flow, drop "scope" from the ask-user list and
   add the `MICROMEGAS_OIDC_SCOPE` env-var note in its place, and add the first-use probe (try
   `python3`, falling back to `python` then `py -3`, per §1a, via `Bash` or `PowerShell` as
   appropriate to the platform) with its `uri`/`oidc_issuer`/`oidc_client_id` output and the
   "ambiguous localhost default" caveat, as the way to verify the environment before running the
   user's query. Include the failure-mode distinction from Design §1: on a bare "module not found"
   for `micromegas` itself, run `pip install micromegas`; on an import error naming something
   inside `micromegas` (e.g. `micromegas.cli.config`), run `pip install --upgrade micromegas`
   instead, since the package is already present but predates `cli/config.py`.
4. Apply the three documentation changes from Design §5 (interactive SSO note, connection-error
   note, new UDF-listing subsection) in the appropriate sections (`## Setup` for the SSO note,
   `## Common query patterns` / troubleshooting-adjacent text for the connection-error note, and a
   new "Discovering UDFs" subsection under `## Key functions` for the UDF-listing guidance).

## Files to Modify

- `claude-plugin/skills/micromegas-query/SKILL.md`

## Trade-offs

- **First-use probe vs. keeping a (fixed) load-time probe**: a load-time probe, even made total,
  still only checks installation/env-var presence and remains vulnerable to the Windows
  permission-matching trap independent of totality. A first-use `Bash` check inside the skill
  body has no load-time failure mode at all, at the cost of surfacing a missing dependency one
  step later (after the skill has already loaded) instead of immediately. This plan follows the
  issue's suggested direction since it removes an entire class of failure rather than patching
  one instance of it.
- **`~/.micromegas/config.json` vs. keeping `~/.micromegas_env`**: the config file is read by the
  library on every invocation on every platform via `resolve_connection()`, so it needs no shell
  integration step and cannot go stale relative to a not-yet-restarted shell. The only downside
  is no config-file slot for `oidc_scope`, which is already covered by library defaults.

## Documentation

No standalone docs page covers this skill beyond the SKILL.md file itself; all changes are
in-place edits to that file.

## Testing Strategy

`SKILL.md` is a prompt/config document, not executable code — there is no unit test target.
Verify manually:

- Confirm the edited frontmatter is valid (matches the existing `name/description/argument-hint/
  context/allowed-tools` shape) and that `shell: bash` is present.
- Confirm no `!`-prefixed interpolation remains in the file (`grep -n '!\`' SKILL.md`).
- Confirm the `allowed-tools` line has no bare `Write`, no unscoped `Edit`, and no
  wildcard-suffixed fixed commands other than `Bash(micromegas-query *)`.
- Manually trace the "not installed" and "not configured" paths through the rewritten
  `## Setup` text to confirm each now leads to a recoverable action rather than a hard stop.
