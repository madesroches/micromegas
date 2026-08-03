# CLI: Named Connection Profiles Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1403

## Overview

`micromegas-query` resolves its connection from `MICROMEGAS_*` env vars, then a
single flat `~/.micromegas/config.json`, then a hard-coded default. There is no
way to keep more than one environment (prod / dev / local) around and switch
between them without editing the file or juggling several env vars together.
This plan adds an AWS-CLI-style `profiles` map to the existing config file,
selected by a `--profile` flag or `MICROMEGAS_PROFILE` env var, with per-profile
OIDC token caching so switching profiles doesn't reuse another environment's
cached token. Existing flat config files keep working untouched. One breaking
change rides along: the niche `MICROMEGAS_TOKEN_FILE` env var is removed —
the token cache path is now always derived from the active profile.

## Current State

### Resolution chain

`python/micromegas/micromegas/cli/config.py`:
- `load_config(config_path=None)` (`config.py:24-35`) reads
  `~/.micromegas/config.json` (or `config_path`) and returns the raw dict, or
  `{}` if the file doesn't exist.
- `resolve_connection(config_path=None)` (`config.py:43-60`) builds a frozen
  `ConnectionConfig` dataclass (`config.py:12-21`) by picking, per field, the
  env var if set, else the config dict's value, else a default. `_pick`
  (`config.py:38-40`) implements that per-field precedence.
- The config dict shape is flat: `uri`, `client_id`, `issuers: [{issuer,
  audience}]` (first entry only, `config.py:47-49`). No `profiles` concept
  exists.

### Callers

- `python/micromegas/micromegas/cli/connection.py` `connect()` (`connection.py:6-32`)
  takes no arguments, calls `resolve_connection()` with no `config_path`, and
  picks OIDC vs. plain vs. python-module-wrapper connection based on the
  resolved fields.
- `python/micromegas/micromegas/cli/query.py` `main()` (`query.py:59-155`) builds
  an `argparse` parser with `sql`, `--file`, `--begin`, `--end`, `--all`,
  `--format`, `--max-colwidth`, then calls `connection.connect()` with no
  arguments (`query.py:135`). There is no `--profile` or `--config` flag, and
  no `try` around `connect()`.
- `python/micromegas/micromegas/cli/logout.py` `main()` (`logout.py:7-26`) does
  **not** go through `config.py` at all — it re-derives the default token path
  inline (`Path.home() / ".micromegas" / "tokens.json"`, `logout.py:14-16`) and
  reads `MICROMEGAS_TOKEN_FILE` directly, duplicating the default that
  `config.py:9` (`DEFAULT_TOKEN_FILE`) already defines.
- Token file resolution today is a single global path
  (`MICROMEGAS_TOKEN_FILE` env var, else `DEFAULT_TOKEN_FILE`,
  `config.py:58`), with no per-profile distinction.

### Tests

`python/micromegas/tests/cli/test_config.py` covers `load_config` and
`resolve_connection` against the flat shape only (missing file, valid file, env
override, no-issuers case). No test file exists for `connection.py` or
`logout.py`.

### Docs

`mkdocs/docs/query-guide/python-api.md:636-683` documents the three-source
resolution order, the env var table, and the flat config file shape/key
mapping table. `mkdocs/docs/admin/authentication.md:215-237` documents
`MICROMEGAS_ANALYTICS_URI` and friends in the client-side `CLI Tools with
OIDC` env-var section, and also hard-codes the client-side token cache as
*the* single path: `:236` lists `MICROMEGAS_TOKEN_FILE`'s default as
`~/.micromegas/tokens.json`, `:501` states tokens "are stored at
`~/.micromegas/tokens.json`", and the troubleshooting steps at `:599-611`
tell the reader to `cat ~/.micromegas/tokens.json` / run `micromegas-logout`.
Once a `profiles` map exists, the real path is `tokens-<profile>.json` and a
bare `micromegas-logout` clears every cached token file rather than just
`tokens.json`, so this file needs a one-line update too (see Files to
Modify).

## Design

### Config schema: optional `profiles` map

Extend the config file with two optional top-level keys, additive to the
existing flat shape:

```json
{
  "default_profile": "prod",
  "profiles": {
    "prod": {
      "uri": "grpc://analytics.example.com:50051",
      "client_id": "...",
      "issuers": [{ "issuer": "https://issuer.example.com/v2.0", "audience": "..." }]
    },
    "dev": {
      "uri": "grpc://analytics-dev.example.com:50051",
      "client_id": "...",
      "issuers": [{ "issuer": "https://issuer.example.com/v2.0", "audience": "..." }]
    },
    "local": { "uri": "grpc://localhost:50051" }
  }
}
```

Each entry under `profiles` has exactly the shape today's flat config has
(`uri`, `client_id`, `issuers`). Backward compatibility falls out of this for
free: if `profiles` is absent, `resolve_connection` treats the whole dict as
today's single connection — the existing flat-config tests keep passing, but
only under the new autouse env-scrub fixture (see Implementation Step 5): a
developer's exported `MICROMEGAS_PROFILE` would otherwise route a flat config
into the new "no profiles configured" `ProfileError` path.

`token_file` is not configurable at all: it is always derived from the active
profile (`default_token_file(name)`, see Per-profile token caching). The
`MICROMEGAS_TOKEN_FILE` env var is removed, and no per-profile `token_file`
config key is added either — a profile entry stays exactly the flat shape,
which has never had a `token_file` key.

Once `profiles` is present, it takes over completely: any top-level flat keys
(`uri`, `client_id`, `issuers`) left in the same file are ignored in favor of
the selected profile's values, since `resolve_active_profile` returns
`profiles[name]` as `active_config` rather than merging it with the
top-level dict. This matters for a user migrating from a flat config to
`profiles` who leaves the old flat keys in place — they become dead config,
silently. The docs (Implementation Step 8) should call this out explicitly as
a warning against mixing the two shapes in one file, rather than just showing
the two shapes side by side.

### `resolve_connection` becomes profile-aware

Profile selection lives in its own helper, `resolve_active_profile`, so
`resolve_connection` stays a thin field-picker and the selection rules are
testable in isolation. The name-selection precedence (`--profile` flag
argument > `MICROMEGAS_PROFILE` > `default_profile`, the flag being the
highest-priority source per the issue's proposed order) is a one-line
`or`-chain inlined in its body — with only one caller it doesn't warrant a
separate helper:

```python
class ProfileError(ValueError):
    """Raised by `resolve_active_profile` for profile-selection problems only
    (unknown profile, none selected, or --profile/MICROMEGAS_PROFILE with no
    `profiles` map) — never for downstream connection failures. Subclassing
    `ValueError` keeps it compatible with any existing `except ValueError`
    handling of `load_config`'s JSON-decode error, while letting callers that
    only want profile-selection errors catch `ProfileError` specifically.
    """


def resolve_active_profile(config, profile=None):
    """Resolve the active profile name and its connection dict from `config`.

    The name is picked with precedence: `profile` argument (the --profile
    flag) > `MICROMEGAS_PROFILE` > `default_profile`.

    Returns `(name, active_config)`; `name` is `None` when no `profiles` map
    exists and the flat config is used directly as `active_config`. Raises
    `ProfileError` if `--profile`/`MICROMEGAS_PROFILE` is set but there's no
    `profiles` map, if a `profiles` map exists but no profile is selected,
    if the resolved name isn't in `profiles`, or if `profiles` (or the
    selected profile's entry) is malformed (not a map).
    """
    profiles = config.get("profiles")
    if profiles is None:
        if profile or os.environ.get("MICROMEGAS_PROFILE"):
            raise ProfileError(
                "no profiles configured; remove --profile/MICROMEGAS_PROFILE "
                "or add a `profiles` map to the config file"
            )
        return None, config

    if not isinstance(profiles, dict):
        raise ProfileError("`profiles` must be a map of profile name to profile config")

    if not profiles:
        raise ProfileError("no profiles defined in the `profiles` map")

    name = profile or os.environ.get("MICROMEGAS_PROFILE") or config.get("default_profile")
    if name is None:
        raise ProfileError(
            "no profile selected; pass --profile, set MICROMEGAS_PROFILE, or "
            f"set default_profile (available: {', '.join(sorted(profiles))})"
        )
    if not isinstance(name, str):
        raise ProfileError("default_profile must be a profile name (string)")
    if name not in profiles:
        raise ProfileError(
            f"unknown profile '{name}' (available: {', '.join(sorted(profiles))})"
        )
    active = profiles[name]
    if not isinstance(active, dict):
        raise ProfileError(f"profile '{name}' must be a map of connection settings")
    return name, active


def resolve_connection(config_path=None, profile=None) -> ConnectionConfig:
    config = load_config(config_path)
    name, active = resolve_active_profile(config, profile)

    issuers = active.get("issuers") or []
    issuer = issuers[0].get("issuer") if issuers else None
    audience = issuers[0].get("audience") if issuers else None

    return ConnectionConfig(
        uri=_pick("MICROMEGAS_ANALYTICS_URI", active.get("uri"), DEFAULT_URI),
        oidc_issuer=_pick("MICROMEGAS_OIDC_ISSUER", issuer),
        oidc_client_id=_pick("MICROMEGAS_OIDC_CLIENT_ID", active.get("client_id")),
        oidc_client_secret=_pick("MICROMEGAS_OIDC_CLIENT_SECRET"),
        oidc_audience=_pick("MICROMEGAS_OIDC_AUDIENCE", audience),
        oidc_scope=_pick("MICROMEGAS_OIDC_SCOPE"),
        token_file=default_token_file(name),
    )
```

Individual `MICROMEGAS_*` env vars still win over the active profile's
values *for each field they set*, once a profile has been selected — `_pick`
already gives them top precedence per field, per the issue's requirement that
existing single-connection setups using env vars keep working untouched. This
precedence is scoped to *after* profile selection, not a bypass of it:
`resolve_active_profile` runs first and raises `ProfileError` when a
`profiles` map exists but no profile is selected, regardless of whether env
vars alone would otherwise fully specify the connection. So on a machine
whose `config.json` has a `profiles` map, an env-var-only invocation
(e.g. `MICROMEGAS_ANALYTICS_URI=... micromegas-query ...` in CI) still fails
with a usage error unless `--profile`, `MICROMEGAS_PROFILE`, or
`default_profile` picks one — it is not, in fact, unaffected once profiles
exist.

There is no implicit selection when only one profile is configured: a
`profiles` map always requires a selected profile, so even a single-profile
config sets `default_profile` (one line of boilerplate). One selection rule,
no special cases — adding a second profile later never changes how the first
one is picked.

An explicit `--profile`/`MICROMEGAS_PROFILE` against a legacy flat config (no
`profiles` key) also raises `ProfileError` rather than silently falling back to
the flat config: a user with a stale `MICROMEGAS_PROFILE` env var or a typo'd
`--profile` gets an explicit error instead of being routed to a connection
with no indication the flag had any effect.

### Per-profile token caching

```python
def default_token_file(profile: Optional[str] = None) -> str:
    if profile:
        return str(Path.home() / ".micromegas" / f"tokens-{profile}.json")
    return str(Path.home() / ".micromegas" / "tokens.json")
```

Both branches compute from `Path.home()` at call time, absorbing the
module-level `DEFAULT_TOKEN_FILE` constant — whose only use is `config.py:58`
— which gets deleted; tests can then redirect every token path with a `HOME`
monkeypatch alone. Used by `resolve_connection` (above) and
reused by `logout.py` (below) so both places derive the same path from the
same profile.

The `MICROMEGAS_TOKEN_FILE` env var — which both `authentication.md:236` and
`python-api.md`'s env var table advertise today — is removed outright rather
than kept as an override (a breaking change, noted in the CHANGELOG). Its
original use case, pointing different environments at different token caches,
is exactly what per-profile files now do; its only consumers are these two
CLI entry points; and kept in place, an exported value would override the
per-profile fallback for *every* profile, collapsing them onto one shared
token cache — a footgun that would need doc warnings and extra precedence
tests. Anyone who was exporting it logs in once more, with tokens landing in
the default location.

This plan only changes `micromegas-query`/`micromegas-logout`'s own
resolution path. `python/micromegas/micromegas/cli/screens.py`'s
`make_client()` builds its OIDC provider straight from `MICROMEGAS_OIDC_*`
env vars and calls `load_or_login()` with no `token_file`, so it keeps
reading/writing the plain default `~/.micromegas/tokens.json` regardless of
any `profiles`/`MICROMEGAS_PROFILE` config — as does any other direct
`oidc_connection` caller. This is why a bare `micromegas-logout` clears *all*
token files (see below) rather than resolving the active profile and deleting
only its file: the plain `tokens.json` — `micromegas-screens`' cache, and any
pre-adoption `tokens.json` left over from before a `profiles` map was
introduced — stays clearable without a flag.

### `connect()` and `--profile`

`connection.py`:
```python
def connect(profile=None):
    cfg = resolve_connection(profile=profile)
    ...  # rest of body unchanged
```

`query.py` gets one new argument:
```python
parser.add_argument("--profile", help="Named connection profile from ~/.micromegas/config.json")
```
and the call site becomes:
```python
from micromegas.cli.config import ProfileError

try:
    client = connection.connect(profile=args.profile)
except ProfileError as e:
    parser.error(str(e))
```
turning an unknown/unselected profile into the same `argparse` usage-error
treatment every other bad input in `main()` already gets, rather than a raw
traceback. Catching `ProfileError` specifically (not the broader `ValueError`)
matters because `connect()` does the whole connection, not just config
resolution: e.g. a malformed `uri` in a profile makes `FlightSQLClient` raise
`pyarrow.lib.ArrowInvalid`, itself a `ValueError` subclass, which must
propagate as a real error rather than being misreported as an argparse usage
error.

`resolve_connection()` keeps its `config_path` parameter (useful for tests
and future callers), but `connect()` does not thread it through — nothing
passes it — and this plan does not add a `--config <path>` CLI flag: the
issue's "smaller alternative" is deferred, not implemented here. See
Trade-offs.

### `logout.py`: clear token files, no config resolution

Logout doesn't need to know which profile is *active* — only which files to
delete — so it never reads `config.json` at all:

```python
from micromegas.cli.config import default_token_file

def main():
    parser = argparse.ArgumentParser(...)
    parser.add_argument("--profile", help="Only clear this profile's cached tokens")
    args = parser.parse_args()

    if args.profile:
        targets = [Path(default_token_file(args.profile))]
    else:
        token_dir = Path.home() / ".micromegas"
        targets = [token_dir / "tokens.json", *sorted(token_dir.glob("tokens-*.json"))]

    removed = False
    for token_file in targets:
        if token_file.exists():
            token_file.unlink()
            print(f"Tokens cleared from {token_file}")
            removed = True
    if not removed:
        print("No saved tokens found")
```

A bare `micromegas-logout` means "log out of everything": the plain
`tokens.json` plus every `tokens-<profile>.json`. That keeps the existing
docs/troubleshooting instruction ("run `micromegas-logout`") valid unchanged,
covers `micromegas-screens`' cache, and removes the inline duplicate of the
default token path. `--profile X` narrows deletion to that one file via the
shared `default_token_file` helper, so logout and `resolve_connection` can't
disagree about which file a profile maps to. Logout
deliberately ignores `MICROMEGAS_PROFILE`: the `--profile` flag is its only
narrowing mechanism, and a bare invocation with `MICROMEGAS_PROFILE=dev`
exported still clears everything (a superset of the dev tokens — harmless,
and closer to what "logout" means than silently scoping to one profile).

Accepted trade-off: because logout no longer consults the config, a typo'd
`--profile pro` (for `prod`) prints "No saved tokens found" rather than
listing available profiles — harmless (nothing is deleted) and
self-describing, in exchange for logout needing no `load_config`/
`resolve_active_profile`/`ProfileError` plumbing at all.

## Implementation Steps

1. **`config.py`**: add `default_token_file()`
   (absorbing the `DEFAULT_TOKEN_FILE` constant, which is deleted — its only
   use is `config.py:58`) and the
   `resolve_active_profile(config, profile)` helper (name-selection
   precedence inlined in its body); rewrite
   `resolve_connection()` to call it and use `default_token_file(name)` as
   the token-file value, dropping the `MICROMEGAS_TOKEN_FILE` env-var
   override (the variable is removed). `resolve_active_profile()` raises `ProfileError`
   for an unknown profile, for a `profiles` map with no profile selected,
   and for an explicit `--profile`/`MICROMEGAS_PROFILE` when the config has
   no `profiles` map.
2. **`connection.py`**: thread `profile=None` through `connect()` into
   `resolve_connection()`.
3. **`query.py`**: add `--profile` argument; wrap the `connection.connect()`
   call in `try/except ProfileError` → `parser.error()` (catching the
   dedicated `ProfileError`, not the broader `ValueError`, so a real
   connection failure like `pyarrow.lib.ArrowInvalid` from a malformed
   profile `uri` isn't misreported as a usage error).
4. **`logout.py`**: add `--profile` argument; delete
   `default_token_file(args.profile)` when `--profile` is given, else the
   plain `tokens.json` plus every `tokens-<profile>.json` in `~/.micromegas`
   (glob), replacing the inline hard-coded default and dropping the
   `MICROMEGAS_TOKEN_FILE` read. No `config.json` reads, no `ProfileError`
   handling.
5. **Tests** (`tests/cli/test_config.py` + new `tests/cli/conftest.py`):
   add an autouse fixture scrubbing every `MICROMEGAS_*` env var before each
   test —

   ```python
   @pytest.fixture(autouse=True)
   def scrub_micromegas_env(monkeypatch):
       for key in list(os.environ):
           if key.startswith("MICROMEGAS_"):
               monkeypatch.delenv(key)
   ```

   — and delete the now-redundant per-test scrubbing (the 8-var `delenv`
   lists in `test_resolve_no_config_no_env`, `test_resolve_reads_config_file`,
   and `test_config_without_issuers`; the 7-var list in
   `test_uri_from_env_without_oidc`; the individual `delenv` calls in
   `test_env_vars_override_config`). Tests that need a var set still
   `monkeypatch.setenv` it in their body, which runs after the fixture. The
   scrubbing itself is load-bearing: without it, a developer's exported
   `MICROMEGAS_PROFILE` feeds a flat config into `resolve_active_profile`'s
   "no profiles configured" `ProfileError` path and breaks the flat-config
   tests — and the fixture covers every new test below and any future
   `MICROMEGAS_*` variable automatically.
   Add cases for:
   - flat config (no `profiles` key) behaves exactly as before (regression)
   - `profiles` map with `default_profile` set, no `--profile`/env
   - `--profile` argument overrides `default_profile`
   - `MICROMEGAS_PROFILE` env var overrides `default_profile` but loses to an
     explicit `profile` argument
   - unknown profile name raises `ProfileError` mentioning the available names
   - `profiles` map present (whether one entry or several) and nothing
     selected → `ProfileError` mentioning the available names
   - individual `MICROMEGAS_*` env vars still win over the active profile's
     values
   - `default_token_file()` returns the plain default when `profile is None`
     and a `tokens-<profile>.json` path otherwise
   - explicit `--profile` (or `MICROMEGAS_PROFILE`) with a flat config (no
     `profiles` key) raises `ProfileError`
   - `resolve_connection()` against a `profiles` config returns
     `cfg.token_file == default_token_file(name)` (i.e. the
     `tokens-<profile>.json` path for the active profile), not the plain
     `tokens.json` default
   - a set `MICROMEGAS_TOKEN_FILE` has no effect on `resolve_connection()`'s
     output (regression guard for the removal)
6. **Tests** (new `tests/cli/test_logout.py`): cover a bare
   `micromegas-logout` deleting both the plain `tokens.json` and every
   `tokens-<profile>.json` present, `--profile` deleting only that profile's
   file and leaving the rest (including `tokens.json`) untouched, and the
   no-files case printing "No saved tokens found". `logout.main()` reads no
   config or env vars, and every path it touches (the glob and
   `default_token_file()`) derives from `Path.home()` at call time, so
   monkeypatching `HOME` (or `Path.home`) is the only seam these tests need —
   without it they would `unlink()` the developer's real token files. Also
   cover logout ignoring `MICROMEGAS_PROFILE` (bare invocation clears
   everything even with it set).
7. **Tests** (`tests/test_query.py`): a test following the existing
   usage-error pattern (added in PR #1407, issue #1405) — `monkeypatch.setattr(sys,
   "argv", [...])`, call `main()` directly, and assert `pytest.raises(SystemExit)`
   — asserting an unknown `--profile` exits 2 with a usage message, not a
   traceback. `main()` has no `--config` flag, and reads
   `~/.micromegas/config.json` through the module-level `config.CONFIG_PATH`
   constant, so this test needs a
   `monkeypatch.setattr(config, "CONFIG_PATH", <tmp file>)` seam (pointing at
   a tmp file with a `profiles` map and an unresolvable name) before calling
   `main()`, so the test doesn't depend on whatever is in the developer's
   real `~/.micromegas/config.json`.
8. **Docs** (`mkdocs/docs/query-guide/python-api.md:636-683`): add
   `MICROMEGAS_PROFILE` to the env var table and delete its
   `MICROMEGAS_TOKEN_FILE` row (the variable is removed), document the `--profile` flag
   under `micromegas-query` and `micromegas-logout`, and add a "Named
   profiles" subsection showing the `profiles`/`default_profile` config shape
   next to the existing flat-config example, explicit about the flat shape
   still being supported and with a callout warning against mixing top-level
   flat keys with a `profiles` map in the same file (the flat keys are
   ignored once `profiles` is present), and noting that `default_profile` has
   no effect unless a `profiles` map is also present. This subsection must
   also spell out the profile *selection* rules themselves — the precedence
   chain (`--profile` > `MICROMEGAS_PROFILE` > `default_profile`) and the
   usage error raised when a `profiles` map is present but no profile is
   selected (there is no implicit selection, even with a single profile —
   set `default_profile`) — matching how `:636-645` today documents
   the (pre-profiles) three-source resolution order. Also amend that existing
   `:638-644` "three sources" text itself: once a `profiles` map exists,
   per-field env vars are no longer the documented way to switch
   environments/configure CI on a machine with a `profiles` map (an
   env-var-only invocation now fails with the usage error above unless a
   profile is also selected), so the "override individual settings via env
   vars ... for switching environments" sentence needs to be corrected to
   describe profile selection as happening first, with per-field env vars
   only overriding individual settings *within* the selected profile. Also
   call out that adding a `profiles` map moves
   the OIDC token cache to a per-profile `tokens-<profile>.json` file, so
   turning profiles on forces one fresh login even for an otherwise-unchanged
   connection (renaming the existing `tokens.json` to match beforehand avoids
   it). Also document `micromegas-logout`'s
   semantics: a bare invocation clears every cached token file — the plain
   `~/.micromegas/tokens.json` (still used by `micromegas-screens` and any
   other direct `oidc_connection` caller, see Per-profile token caching)
   plus every `tokens-<profile>.json` — while `--profile <name>` clears only
   that profile's file.
9. **Docs** (`mkdocs/docs/admin/authentication.md`): delete the `:236`
   `MICROMEGAS_TOKEN_FILE` row from the `### CLI Tools with OIDC` env-var
   table (the variable is removed) and add a `MICROMEGAS_PROFILE` row to
   the same table, alongside the one-line notes already planned next to the
   `:501` token-storage statement and the `:599-611` troubleshooting steps
   (see Documentation).
10. **Skill doc** (`claude-plugin/skills/micromegas-query/SKILL.md`): the Setup
   section's config probe calls `resolve_connection()` with no arguments,
   which will raise `ProfileError` (an unhandled Python exception in the probe
   command, not a clean CLI usage error) for a user who has added a
   `profiles` map with no `default_profile`/`MICROMEGAS_PROFILE` set, or who
   has a stale `MICROMEGAS_PROFILE` set against a flat config. This directly contradicts the existing sentence "This call is
   total — it never fails just because nothing is configured yet, since a
   missing config resolves to the default `grpc://localhost:50051`" (added by
   #1409 specifically so the skill wouldn't mis-diagnose probe failures), so
   this step must amend that sentence — not just append a note after it — to
   carve out the new no-profile-selected failure mode, and add a probe-outcome
   bullet alongside the existing import-error/success bullets covering it
   (treat the error as "no profile selected" and ask the user which profile
   to use, or to set a `default_profile`). Also mention `MICROMEGAS_PROFILE` /
   `--profile` as an alternative to the flat `uri`/`client_id`/`issuers` keys
   this section currently walks users through writing.
   The Setup section's config-*writing* flow (the "read `~/.micromegas/config.json`
   first ... and merge in the new values" step) must also be amended: as-is, it
   always merges the user's values into top-level `uri`/`client_id`/`issuers`
   keys, which are silently ignored once a `profiles` map exists (per the
   Config schema section above) — so on a `profiles`-bearing config the skill
   would write dead config and the post-write re-verification probe would then
   show the unchanged active profile's values, driving a mis-diagnosis loop.
   Change this step so that when the existing `config.json` already has a
   `profiles` map, the skill writes the user's values into the selected
   `profiles.<name>` entry instead (asking the user which profile if none is
   already active), and additionally sets `default_profile` to that name so
   the no-argument probe keeps resolving the right profile without needing
   `--profile`/`MICROMEGAS_PROFILE` on every call.
   The existing bare `Bash(micromegas-logout)` / `PowerShell(micromegas-logout)`
   allowed-tools entries gain `Bash(micromegas-logout *)` / `PowerShell(micromegas-logout *)`
   wildcard counterparts, and the Interactive SSO stale-token recovery
   instruction is updated to run `micromegas-logout --profile <active profile>`
   when a `profiles` map is in use (narrowing the clear to that one profile,
   since a bare `micromegas-logout` wipes every profile's cached token) and
   bare `micromegas-logout` otherwise.
11. **CHANGELOG.md**: add an entry, following the pattern of the #1407 entry,
   noting the bare `micromegas-logout` behavior change (it now clears every
   cached token file, not just `tokens.json`) and the removal of the
   `MICROMEGAS_TOKEN_FILE` env var (breaking; the token cache path is now
   always derived from the active profile).

## Files to Modify

- `python/micromegas/micromegas/cli/config.py`
- `python/micromegas/micromegas/cli/connection.py`
- `python/micromegas/micromegas/cli/query.py`
- `python/micromegas/micromegas/cli/logout.py`
- `python/micromegas/tests/cli/conftest.py` (new)
- `python/micromegas/tests/cli/test_config.py`
- `python/micromegas/tests/cli/test_connection.py` (new)
- `python/micromegas/tests/cli/test_logout.py` (new)
- `python/micromegas/tests/test_query.py`
- `mkdocs/docs/query-guide/python-api.md`
- `mkdocs/docs/admin/authentication.md`
- `claude-plugin/skills/micromegas-query/SKILL.md`
- `CHANGELOG.md`

## Trade-offs

- **Profiles map vs. `--config <path>` only**: the issue frames these as
  alternatives. This plan implements only the `profiles` map; a `--config
  <path>` flag is deferred rather than bundled in, to keep this pass scoped
  to one mechanism (`resolve_connection()` already accepts a
  `config_path` parameter, so adding the flag later is a small follow-up,
  not a redesign).
- **No implicit single-profile selection**: a `profiles` map always requires
  a selected profile, so even a single-profile config sets `default_profile`
  (one line of boilerplate). Chosen over implicitly using a lone profile to
  keep one selection rule with no special cases — adding a second profile
  later never silently changes how the first one is picked.
- **Logout clears files, not profiles**: `micromegas-logout` never reads
  `config.json`; a bare invocation deletes the plain `tokens.json` plus every
  `tokens-<profile>.json`, and `--profile` just picks a filename. This keeps
  logout free of `ProfileError`/config plumbing and leaves no token file it
  cannot clear (including `micromegas-screens`' shared `tokens.json`), at two
  costs: a typo'd `--profile` prints "No saved tokens found" instead of
  listing valid names, and a bare logout now deletes more than before (all
  profiles' tokens — noted in the CHANGELOG entry).
- **Removing `MICROMEGAS_TOKEN_FILE` vs. keeping it as an override**: removed.
  Its use case (separate token caches per environment) is subsumed by
  per-profile files, its only consumers are `micromegas-query`/
  `micromegas-logout` themselves, and kept, an exported value would collapse
  every profile onto one shared cache — a footgun needing doc warnings and
  precedence tests. Breaking, but mild: anyone exporting it logs in once
  more and tokens land in the default location. If cache relocation is ever
  needed again, a per-profile config key would be a cleaner reintroduction
  than a global env var.
- **Token file naming (`tokens-<profile>.json`) vs. a profile-keyed map inside
  one `tokens.json`**: the issue proposes both; this plan picks the
  filename-suffix approach because it requires no change to
  `OidcAuthProvider.from_file`/`login` (which already take a `token_file`
  path) and no new file format, whereas a keyed map would require changing
  the token file's internal structure and everything that reads it.

## Documentation

- `mkdocs/docs/query-guide/python-api.md`: update the "Configuration" section
  (`:636-683`) — env var table gets `MICROMEGAS_PROFILE` and loses
  `MICROMEGAS_TOKEN_FILE`; new subsection for
  the `profiles`/`default_profile` shape that also documents the selection
  precedence (`--profile` > `MICROMEGAS_PROFILE` > `default_profile`) and the
  none-selected usage error (no implicit single-profile selection — set
  `default_profile`); `--profile` documented under both `micromegas-query`
  and `micromegas-logout` CLI reference entries, with bare `micromegas-logout`
  documented as clearing all cached token files; the existing `:638-644`
  "three sources"/"switching environments" wording corrected to describe
  profile selection happening before per-field env-var overrides.
- `claude-plugin/skills/micromegas-query/SKILL.md`: amend the existing "this
  call is total" sentence in Setup and add a probe-outcome bullet covering the
  case where the config probe (`resolve_connection()` with no arguments) can
  raise `ProfileError` when a `profiles` map is present and no profile is
  selected (or a stale `MICROMEGAS_PROFILE` is set against a flat config), and
  mention `MICROMEGAS_PROFILE`/`--profile` alongside the existing flat-config
  write-up. Also update the config-*writing* step so it writes into the
  selected `profiles.<name>` entry (and sets `default_profile`) instead of
  merging top-level flat keys when a `profiles` map is already present. The
  `micromegas-logout` allowed-tools entries gain wildcard
  (`Bash(micromegas-logout *)` / `PowerShell(micromegas-logout *)`) forms, and
  the Interactive SSO recovery instruction now runs
  `micromegas-logout --profile <active profile>` when a profile is active,
  falling back to bare logout otherwise — see Implementation Step 10 for the
  full rationale.
- `mkdocs/docs/admin/authentication.md`: delete the `:236`
  `MICROMEGAS_TOKEN_FILE` row and add a `MICROMEGAS_PROFILE` row (see
  Implementation Step 9), and add a one-line note next to the `:501`
  token-storage statement and the `:599-611` troubleshooting steps that a
  `profiles` map moves the cache to `tokens-<profile>.json`; the existing
  bare `micromegas-logout` instruction stays valid, since it clears every
  cached token file (`--profile <name>` narrows it to one).
- `CHANGELOG.md`: new entry for the profiles feature, the bare-logout
  behavior change, and the `MICROMEGAS_TOKEN_FILE` removal.

## Testing Strategy

- Unit tests in `tests/cli/test_config.py` cover the resolution matrix
  described in Implementation Step 5 — these are hermetic (no live service
  needed), consistent with the existing file — including the explicit
  `--profile`/`MICROMEGAS_PROFILE` with a flat config raising `ProfileError`,
  the per-profile `token_file` wiring in `resolve_connection()`, and every
  `MICROMEGAS_*` var scrubbed by the new autouse `tests/cli/conftest.py`
  fixture, per Step 5.
- `tests/cli/test_logout.py` (new) covers bare `micromegas-logout` deleting
  the plain `tokens.json` plus every `tokens-<profile>.json`, `--profile`
  deleting only its own file, and the no-files case — hermetic via a
  `HOME`/`Path.home` monkeypatch (every path logout touches derives from
  `Path.home()` at call time), per Implementation Step 6; without the `HOME`
  patch these tests would `unlink()` the developer's real token files. No
  config or env seam is needed — `logout.main()` reads neither `config.json`
  nor any env var.
- `tests/test_query.py` gets one negative test for an unknown `--profile`,
  mirroring the existing `--begin`/`--end` usage-error test (e.g.
  `test_main_overflowing_begin_reports_usage_error`, added in PR #1407, issue
  #1405), which uses `monkeypatch.setattr(sys, "argv", [...])` and
  `pytest.raises(SystemExit)` around a direct `main()` call rather than
  `subprocess.run` — with the `config.CONFIG_PATH` monkeypatch described in
  Implementation Step 7 so it doesn't depend on the real config file either.
- Manual smoke test: create a `~/.micromegas/config.json` with two profiles,
  run `micromegas-query --profile local "SELECT 1" --all` against a locally
  running monolith, then repeat with `MICROMEGAS_PROFILE=local` and no flag,
  then `micromegas-logout --profile local` and confirm only that profile's
  token file is removed, then a bare `micromegas-logout` and confirm every
  remaining token file is removed.
- `poetry run pytest` and `poetry run black --check .` from
  `python/micromegas/`, then `python3 build/python_ci.py` per project
  convention.
