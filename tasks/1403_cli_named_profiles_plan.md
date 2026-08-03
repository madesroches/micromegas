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
cached token. Existing flat config files keep working untouched.

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
Once a `profiles` map exists, the real path is `tokens-<profile>.json` and
logout needs `--profile`, so this file needs a one-line update too (see
Files to Modify).

## Design

### Config schema: optional `profiles` map

Extend the config file with two optional top-level keys, additive to the
existing flat shape:

```json
{
  "default_profile": "prod",
  "profiles": {
    "prod": {
      "uri": "https://analytics.example.com:443",
      "client_id": "...",
      "issuers": [{ "issuer": "https://issuer.example.com/v2.0", "audience": "..." }]
    },
    "dev": {
      "uri": "https://analytics-dev.example.com:443",
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
only once `MICROMEGAS_PROFILE` is added to the env vars those tests already
scrub (see Implementation Step 5); left set, it would route a flat config
into the new "no profiles configured" `ProfileError` path.

`token_file` stays env-only (`MICROMEGAS_TOKEN_FILE`), with no per-profile
config-file key, consistent with `client_secret`/`oidc_scope` — the flat
config format has never had a `token_file` key (only `config.py:58`'s
`MICROMEGAS_TOKEN_FILE` env var), and a profile-entry `token_file` key would
break the invariant above that a profile entry is exactly the flat shape.

Once `profiles` is present, it takes over completely: any top-level flat keys
(`uri`, `client_id`, `issuers`) left in the same file are ignored in favor of
the selected profile's values, since `resolve_active_profile` returns
`profiles[name]` as `active_config` rather than merging it with the
top-level dict. This matters for a user migrating from a flat config to
`profiles` who leaves the old flat keys in place — they become dead config,
silently. The docs (Implementation Step 8) should call this out explicitly as
a warning against mixing the two shapes in one file, rather than just showing
the two shapes side by side.

### Profile name resolution

New helper in `config.py`:

```python
def resolve_profile_name(profile=None, config=None) -> Optional[str]:
    """Pick the active profile name: --profile > MICROMEGAS_PROFILE > default_profile."""
    config = config or {}
    return profile or os.environ.get("MICROMEGAS_PROFILE") or config.get("default_profile")
```

This mirrors `_pick`'s style but takes an explicit `profile` argument (the CLI
flag value) rather than reading a single env var, since the flag is the
highest-priority source per the issue's proposed order.

### `resolve_connection` becomes profile-aware

`resolve_connection` and `logout.py` both need to agree on which profile is
active (and therefore which token file it maps to), so profile resolution is
factored into one shared helper rather than duplicated in each caller:

```python
class ProfileError(ValueError):
    """Raised by `resolve_active_profile` for profile-selection problems only
    (unknown/ambiguous profile, or --profile/MICROMEGAS_PROFILE with no
    `profiles` map) — never for downstream connection failures. Subclassing
    `ValueError` keeps it compatible with any existing `except ValueError`
    handling of `load_config`'s JSON-decode error, while letting callers that
    only want profile-selection errors catch `ProfileError` specifically.
    """


def resolve_active_profile(config, profile=None):
    """Resolve the active profile name and its connection dict from `config`.

    Returns `(name, active_config)`; `name` is `None` when no `profiles` map
    exists and the flat config is used directly as `active_config`. Raises
    `ProfileError` if `--profile`/`MICROMEGAS_PROFILE` is set but there's no
    `profiles` map, if the profile is ambiguous (multiple profiles configured,
    none selected), or if the resolved name isn't in `profiles`.
    """
    profiles = config.get("profiles")
    if profiles is None:
        if profile or os.environ.get("MICROMEGAS_PROFILE"):
            raise ProfileError(
                "no profiles configured; remove --profile/MICROMEGAS_PROFILE "
                "or add a `profiles` map to the config file"
            )
        return None, config

    if not profiles:
        raise ProfileError("no profiles defined in the `profiles` map")

    name = resolve_profile_name(profile, config)
    if name is None:
        if len(profiles) == 1:
            name = next(iter(profiles))
        else:
            raise ProfileError(
                "multiple profiles configured but none selected; pass --profile, "
                f"set MICROMEGAS_PROFILE, or set default_profile (available: "
                f"{', '.join(sorted(profiles))})"
            )
    if name not in profiles:
        raise ProfileError(
            f"unknown profile '{name}' (available: {', '.join(sorted(profiles))})"
        )
    return name, profiles[name]


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
        token_file=_pick("MICROMEGAS_TOKEN_FILE", default_token_file(name)),
        python_module_wrapper=_pick("MICROMEGAS_PYTHON_MODULE_WRAPPER"),
    )
```

Individual `MICROMEGAS_*` env vars still win over the active profile's
values *for each field they set*, once a profile has been selected — `_pick`
already gives them top precedence per field, per the issue's requirement that
existing single-connection setups using env vars keep working untouched. This
precedence is scoped to *after* profile selection, not a bypass of it:
`resolve_active_profile` runs first and raises `ProfileError` for "multiple
profiles configured but none selected" regardless of whether env vars alone
would otherwise fully specify the connection. So on a machine whose
`config.json` already has two or more profiles, an env-var-only invocation
(e.g. `MICROMEGAS_ANALYTICS_URI=... micromegas-query ...` in CI) still fails
with a usage error unless `--profile`, `MICROMEGAS_PROFILE`, or
`default_profile` picks one — it is not, in fact, unaffected once profiles
exist.

`MICROMEGAS_PYTHON_MODULE_WRAPPER` is a deprecated legacy escape hatch for
corporate auth wrappers, not a path this plan integrates with profiles: it
stays env-var-only (no config-file key, per-profile or otherwise), and
`connection.py`'s `connect()` keeps short-circuiting on it *before* using any
of the other resolved fields (`connection.py:13-15`). Consequently, when it's
set, it bypasses the selected profile entirely — `--profile`/
`MICROMEGAS_PROFILE` still pick a `uri`/`client_id`/`issuers`, but `connect()`
discards them all and defers to the wrapper module, so the flag silently has
no effect. This is an existing limitation of the deprecated wrapper, not a
new gap introduced by profiles, and this plan does not attempt to fix it.

The "exactly one profile, none selected → use it implicitly" rule is a small
UX addition beyond the issue text: it means a user who only ever has one
environment doesn't need `default_profile` boilerplate, while anyone with more
than one profile gets a clear error instead of a silently wrong connection.

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
    return DEFAULT_TOKEN_FILE
```

Used as the fallback in `resolve_connection` (above) and reused by
`logout.py` (below) so both places derive the same path from the same
profile. `MICROMEGAS_TOKEN_FILE` still overrides it explicitly when set,
matching today's precedence for the non-profile case. This means an
exported `MICROMEGAS_TOKEN_FILE` — which both `authentication.md:236` and
`python-api.md`'s env var table advertise — overrides the per-profile
fallback for *every* profile, collapsing them onto one shared token cache;
this defeats the point of per-profile caching, so a user adopting profiles
needs to leave `MICROMEGAS_TOKEN_FILE` unset (see Implementation Step 8).

This plan only changes `micromegas-query`/`micromegas-logout`'s own
resolution path. `python/micromegas/micromegas/cli/screens.py`'s
`make_client()` builds its OIDC provider straight from `MICROMEGAS_OIDC_*`
env vars and calls `load_or_login()` with no `token_file`, so it keeps
reading/writing the plain default `~/.micromegas/tokens.json` regardless of
any `profiles`/`MICROMEGAS_PROFILE` config — as does any other direct
`oidc_connection` caller. Once `micromegas-logout` becomes profile-scoped
(deleting `tokens-<profile>.json` instead), it can no longer clear that
shared default file — including any pre-adoption `tokens.json` left over
from before a `profiles` map was introduced. This plan leaves that default
path outside `micromegas-logout`'s reach rather than having the bare,
no-`--profile` invocation delete both; a user who needs to clear
`micromegas-screens`' cached token still has to `rm
~/.micromegas/tokens.json` by hand.

### `connect()` and `--profile`

`connection.py`:
```python
def connect(profile=None, config_path=None):
    cfg = resolve_connection(config_path=config_path, profile=profile)
    ...  # unchanged body
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
turning an unknown/ambiguous profile into the same `argparse` usage-error
treatment every other bad input in `main()` already gets, rather than a raw
traceback. Catching `ProfileError` specifically (not the broader `ValueError`)
matters because `connect()` does the whole connection, not just config
resolution: e.g. a malformed `uri` in a profile makes `FlightSQLClient` raise
`pyarrow.lib.ArrowInvalid`, itself a `ValueError` subclass, which must
propagate as a real error rather than being misreported as an argparse usage
error.

`connect()`/`resolve_connection()` keep the `config_path` parameter (useful
for callers that already have a path, and for tests), but this plan does not
add a `--config <path>` CLI flag — the issue's "smaller alternative" is
deferred, not implemented here. See Trade-offs.

### `logout.py` becomes profile-aware and stops duplicating the default

```python
from micromegas.cli.config import default_token_file, resolve_active_profile, load_config

def main():
    parser = argparse.ArgumentParser(...)
    parser.add_argument("--profile", help="Named connection profile to log out of")
    args = parser.parse_args()

    try:
        config = load_config()
        name, _ = resolve_active_profile(config, args.profile)
    except ValueError as e:
        parser.error(str(e))
    token_file = os.environ.get("MICROMEGAS_TOKEN_FILE") or default_token_file(name)
    ...  # unchanged unlink/print logic
```

This removes the inline duplicate of `DEFAULT_TOKEN_FILE` and makes
`micromegas-logout --profile dev` clear the right file, or `micromegas-logout`
with `MICROMEGAS_PROFILE=dev` set clear the same file `micromegas-query` would
have used. Going through `resolve_active_profile` (the same helper
`resolve_connection` uses) rather than calling `resolve_profile_name`
directly means `logout.py` can't disagree with `resolve_connection` about
which profile/token file is active: an unknown `--profile` (e.g. a typo'd
`pro` for `prod`) raises the same `ProfileError`, surfaced the same way via
`parser.error`, instead of silently printing "No saved tokens found"; and an
explicit `--profile`/`MICROMEGAS_PROFILE` against a flat config with no
`profiles` map raises instead of quietly resolving a `tokens-<profile>.json`
path that `resolve_connection` would never have picked.

## Implementation Steps

1. **`config.py`**: add `resolve_profile_name()`, `default_token_file()`, and
   the shared `resolve_active_profile(config, profile)` helper; rewrite
   `resolve_connection()` to call it and use `default_token_file(name)` as
   the token-file fallback. `resolve_active_profile()` raises `ProfileError`
   for an unknown or ambiguous profile, and also for an explicit
   `--profile`/`MICROMEGAS_PROFILE` when the config has no `profiles` map.
2. **`connection.py`**: thread `profile=None, config_path=None` through
   `connect()` into `resolve_connection()`.
3. **`query.py`**: add `--profile` argument; wrap the `connection.connect()`
   call in `try/except ProfileError` → `parser.error()` (catching the
   dedicated `ProfileError`, not the broader `ValueError`, so a real
   connection failure like `pyarrow.lib.ArrowInvalid` from a malformed
   profile `uri` isn't misreported as a usage error).
4. **`logout.py`**: add `--profile` argument; resolve the profile name via
   the shared `resolve_active_profile()` + `load_config()`, catching the
   broader `ValueError` (not just `ProfileError`) → `parser.error()`, since
   this also covers `load_config`'s malformed-JSON error — unlike
   `connect()` in `query.py`, `load_config()`/`resolve_active_profile()` here
   is the only thing that can raise, so there's no unrelated `ValueError`
   (e.g. `pyarrow.lib.ArrowInvalid`) to avoid misreporting as a usage error —
   and the token path via `default_token_file()`, replacing the inline
   hard-coded default.
5. **Tests** (`tests/cli/test_config.py`): first, add `MICROMEGAS_PROFILE` to
   each existing test's env scrubbing — the 8-var `delenv` lists in
   `test_resolve_no_config_no_env`, `test_resolve_reads_config_file`, and
   `test_config_without_issuers`; the 7-var list in
   `test_uri_from_env_without_oidc`; and the individual `delenv` calls in
   `test_env_vars_override_config` — without
   this, a developer's exported `MICROMEGAS_PROFILE` feeds a flat config into
   `resolve_active_profile`'s "no profiles configured" `ProfileError` path and
   breaks these tests. Every new test below must likewise explicitly set or
   `delenv` `MICROMEGAS_PROFILE` rather than relying on ambient environment.
   Add cases for:
   - flat config (no `profiles` key) behaves exactly as before (regression)
   - `profiles` map with `default_profile` set, no `--profile`/env
   - `--profile` argument overrides `default_profile`
   - `MICROMEGAS_PROFILE` env var overrides `default_profile` but loses to an
     explicit `profile` argument
   - unknown profile name raises `ProfileError` mentioning the available names
   - `profiles` map with exactly one entry and nothing selected → used
     implicitly
   - `profiles` map with more than one entry and nothing selected →
     `ProfileError`
   - individual `MICROMEGAS_*` env vars still win over the active profile's
     values
   - `default_token_file()` returns the plain default when `profile is None`
     and a `tokens-<profile>.json` path otherwise
   - explicit `--profile` (or `MICROMEGAS_PROFILE`) with a flat config (no
     `profiles` key) raises `ProfileError`
   - `resolve_connection()` against a `profiles` config returns
     `cfg.token_file == default_token_file(name)` (i.e. the
     `tokens-<profile>.json` path for the active profile), not the plain
     `DEFAULT_TOKEN_FILE`
   - `MICROMEGAS_TOKEN_FILE`, when set, still overrides that per-profile
     default in `resolve_connection()`'s output
6. **Tests** (new `tests/cli/test_logout.py` or extend `tests/test_query.py`
   patterns): cover `--profile` selecting the right token file to delete, a
   bare `micromegas-logout` (no `--profile`) against a flat config deleting
   the plain `DEFAULT_TOKEN_FILE` — today's behavior, and the one most at
   risk of regressing here — the case where no token file exists, and an
   unknown `--profile` raising the same `ProfileError` (via
   `resolve_active_profile`) that `resolve_connection`
   raises for `micromegas-query`. `logout.main()` has no `config_path`/
   `--config` seam and reads `~/.micromegas/config.json` through the
   module-level `config.CONFIG_PATH` constant, so these tests must
   `monkeypatch.setattr(config, "CONFIG_PATH", <tmp file>)` rather than
   exercising the developer's real config; for the token-file path itself,
   `default_token_file()` calls `Path.home()` at call time (not import time),
   so monkeypatching `HOME` (or `Path.home`) is enough to redirect the
   per-profile `tokens-<profile>.json` case to a tmp directory —
   `MICROMEGAS_TOKEN_FILE` cannot be used as that seam instead, since setting
   it short-circuits the very per-profile branch under test. `DEFAULT_TOKEN_FILE`
   (`config.py:9`), used for the `profile is None` case, is computed once at
   `config.py` import time from the real `Path.home()`, so a test-time `HOME`
   patch does *not* affect it — that branch needs `monkeypatch.setattr(config,
   "DEFAULT_TOKEN_FILE", <tmp file>)` directly instead.
7. **Tests** (`tests/test_query.py`): a test following the existing
   usage-error pattern (added in PR #1407, issue #1405) — `monkeypatch.setattr(sys,
   "argv", [...])`, call `main()` directly, and assert `pytest.raises(SystemExit)`
   — asserting an unknown `--profile` exits 2 with a usage message, not a
   traceback. `main()` has no `--config` flag either, so this test needs the
   same `monkeypatch.setattr(config, "CONFIG_PATH", <tmp file>)` seam as step 6
   (pointing at a tmp file with a `profiles` map and an unresolvable name)
   before calling `main()`, so the test doesn't depend on whatever is in the
   developer's real `~/.micromegas/config.json`.
8. **Docs** (`mkdocs/docs/query-guide/python-api.md:636-683`): add
   `MICROMEGAS_PROFILE` to the env var table, document the `--profile` flag
   under `micromegas-query` and `micromegas-logout`, and add a "Named
   profiles" subsection showing the `profiles`/`default_profile` config shape
   next to the existing flat-config example, explicit about the flat shape
   still being supported and with a callout warning against mixing top-level
   flat keys with a `profiles` map in the same file (the flat keys are
   ignored once `profiles` is present), and noting that `default_profile` has
   no effect unless a `profiles` map is also present. This subsection must
   also spell out the profile *selection* rules themselves — the precedence
   chain (`--profile` > `MICROMEGAS_PROFILE` > `default_profile`), the
   implicit-selection rule when exactly one profile is configured and none is
   selected, and the usage error raised when two or more profiles are
   configured and none is selected — matching how `:636-645` today documents
   the (pre-profiles) three-source resolution order. Also amend that existing
   `:638-644` "three sources" text itself: once a `profiles` map exists,
   per-field env vars are no longer the documented way to switch
   environments/configure CI on a machine with 2+ profiles (an env-var-only
   invocation now fails with the usage error above unless a profile is also
   selected), so the "override individual settings via env vars ... for
   switching environments" sentence needs to be corrected to describe
   profile selection as happening first, with per-field env vars only
   overriding individual settings *within* the selected profile. Also call
   out that adding a `profiles` map — even a single implicit entry — moves
   the OIDC token cache to a per-profile `tokens-<profile>.json` file, so
   turning profiles on forces one fresh login even for an otherwise-unchanged
   connection (renaming the existing `tokens.json` to match beforehand avoids
   it), and that an exported `MICROMEGAS_TOKEN_FILE` overrides that
   per-profile fallback for every profile, so it should be left unset when
   using profiles. Also add a single short line noting that the deprecated
   `MICROMEGAS_PYTHON_MODULE_WRAPPER` escape hatch bypasses `--profile`/
   `MICROMEGAS_PROFILE` entirely — not new documentation promoting it as a
   profiles-aware corporate-auth option. Also note that `micromegas-logout`
   is profile-scoped: `micromegas-screens` and any other direct
   `oidc_connection` caller keep using the plain `~/.micromegas/tokens.json`
   default (see Per-profile token caching), which a profile-scoped
   `micromegas-logout` invocation cannot clear.
9. **Docs** (`mkdocs/docs/admin/authentication.md`): update the `:236`
   `MICROMEGAS_TOKEN_FILE` row in the `### CLI Tools with OIDC` env-var
   table — its documented default (`~/.micromegas/tokens.json`) only holds
   when no `profiles` map is active — and add a `MICROMEGAS_PROFILE` row to
   the same table, alongside the one-line notes already planned next to the
   `:501` token-storage statement and the `:599-611` troubleshooting steps
   (see Documentation).
10. **Skill doc** (`claude-plugin/skills/micromegas-query/SKILL.md`): the Setup
   section's config probe calls `resolve_connection()` with no arguments,
   which will raise `ProfileError` (an unhandled Python exception in the probe
   command, not a clean CLI usage error) for a user who has added a
   `profiles` map with more than one entry and no `default_profile`/
   `MICROMEGAS_PROFILE`, or who has a stale `MICROMEGAS_PROFILE` set against a
   flat config. This directly contradicts the existing sentence "This call is
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
   the no-argument probe and bare `micromegas-logout` (see below) keep
   resolving the right profile without needing `--profile`/`MICROMEGAS_PROFILE`
   on every call.
   Also add `Bash(micromegas-logout *)` / `PowerShell(micromegas-logout *)`
   alongside the existing `Bash(micromegas-logout)` /
   `PowerShell(micromegas-logout)` entries (keep both — a trailing `*` on a
   fixed command compiles to a prefix wildcard that matches
   `micromegas-logout <args>` but not the bare, argument-less command the
   Interactive SSO note still runs), and amend that Interactive SSO stale-token
   recovery instruction itself to run `micromegas-logout --profile <name>`
   when a profile is active, falling back to the bare command otherwise — the
   new wildcard grant is otherwise never exercised by anything in the skill.
11. **CHANGELOG.md**: add an entry, following the pattern of the #1407 entry.

## Files to Modify

- `python/micromegas/micromegas/cli/config.py`
- `python/micromegas/micromegas/cli/connection.py`
- `python/micromegas/micromegas/cli/query.py`
- `python/micromegas/micromegas/cli/logout.py`
- `python/micromegas/tests/cli/test_config.py`
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
  to one mechanism (`connect()`/`resolve_connection()` already accept a
  `config_path` parameter, so adding the flag later is a small follow-up,
  not a redesign).
- **Implicit single-profile selection**: chosen over requiring
  `default_profile` even for a single-profile setup, to minimize config
  boilerplate for the common case (one non-default environment). Risk: a
  second profile added later silently changes behavior from "just works" to
  "must specify" — mitigated by the `ProfileError` being immediate and
  explicit, not a wrong-but-silent connection.
- **Token file naming (`tokens-<profile>.json`) vs. a profile-keyed map inside
  one `tokens.json`**: the issue proposes both; this plan picks the
  filename-suffix approach because it requires no change to
  `OidcAuthProvider.from_file`/`login` (which already take a `token_file`
  path) and no new file format, whereas a keyed map would require changing
  the token file's internal structure and everything that reads it.

## Documentation

- `mkdocs/docs/query-guide/python-api.md`: update the "Configuration" section
  (`:636-683`) — env var table gets `MICROMEGAS_PROFILE`; new subsection for
  the `profiles`/`default_profile` shape that also documents the selection
  precedence (`--profile` > `MICROMEGAS_PROFILE` > `default_profile`), the
  implicit single-profile rule, and the multiple-profiles-none-selected usage
  error; `--profile` documented under both `micromegas-query` and
  `micromegas-logout` CLI reference entries; the existing `:638-644`
  "three sources"/"switching environments" wording corrected to describe
  profile selection happening before per-field env-var overrides; and a note
  that an exported `MICROMEGAS_TOKEN_FILE` overrides per-profile token
  caching for every profile.
- `claude-plugin/skills/micromegas-query/SKILL.md`: amend the existing "this
  call is total" sentence in Setup and add a probe-outcome bullet covering the
  case where the config probe (`resolve_connection()` with no arguments) can
  raise `ProfileError` when `profiles` has multiple entries and none is
  selected (or a stale `MICROMEGAS_PROFILE` is set against a flat config), and
  mention `MICROMEGAS_PROFILE`/`--profile` alongside the existing flat-config
  write-up. Also update the config-*writing* step so it writes into the
  selected `profiles.<name>` entry (and sets `default_profile`) instead of
  merging top-level flat keys when a `profiles` map is already present, and
  add `micromegas-logout *` `allowed-tools` entries alongside the existing
  bare-command ones, and amend the Interactive SSO stale-token recovery
  instruction to run `micromegas-logout --profile <name>` when a profile is
  active (so the new grant is actually exercised) — see Implementation Step
  10 for the full rationale.
- `mkdocs/docs/admin/authentication.md`: update the `:236`
  `MICROMEGAS_TOKEN_FILE` default and add a `MICROMEGAS_PROFILE` row (see
  Implementation Step 9), and add a one-line note next to the `:501`
  token-storage statement and the `:599-611` troubleshooting steps that a
  `profiles` map moves the cache to `tokens-<profile>.json` and that
  `micromegas-logout --profile <name>` (not the bare command) clears it.
- `CHANGELOG.md`: new entry for the profiles feature.

## Testing Strategy

- Unit tests in `tests/cli/test_config.py` cover the resolution matrix
  described in Implementation Step 5 — these are hermetic (no live service
  needed), consistent with the existing file — including the explicit
  `--profile`/`MICROMEGAS_PROFILE` with a flat config raising `ProfileError`,
  the per-profile `token_file` wiring in `resolve_connection()`, and
  `MICROMEGAS_PROFILE` scrubbed from (or explicitly set in) every test's env,
  per Step 5.
- `tests/cli/test_logout.py` (new) covers profile-aware token file deletion,
  the bare `micromegas-logout` flat-config case (deleting the plain
  `DEFAULT_TOKEN_FILE` via the `config.DEFAULT_TOKEN_FILE` monkeypatch), and
  the unknown-profile error case, also hermetic (just filesystem + env
  vars, no network) — per Implementation Step 6, hermetic here means
  monkeypatching `config.CONFIG_PATH` and `HOME`/`Path.home`, not just
  avoiding the network, since `logout.main()` otherwise reads the developer's
  real `~/.micromegas/config.json` and could `unlink()` their real token file.
- `tests/test_query.py` gets one negative test for an unknown `--profile`,
  mirroring the existing `--begin`/`--end` usage-error test (e.g.
  `test_main_overflowing_begin_reports_usage_error`, added in PR #1407, issue
  #1405), which uses `monkeypatch.setattr(sys, "argv", [...])` and
  `pytest.raises(SystemExit)` around a direct `main()` call rather than
  `subprocess.run` — with the same `config.CONFIG_PATH` monkeypatch as step 6
  so it doesn't depend on the real config file either.
- Manual smoke test: create a `~/.micromegas/config.json` with two profiles,
  run `micromegas-query --profile local "SELECT 1" --all` against a locally
  running monolith, then repeat with `MICROMEGAS_PROFILE=local` and no flag,
  then `micromegas-logout --profile local` and confirm only that profile's
  token file is removed.
- `poetry run pytest` and `poetry run black --check .` from
  `python/micromegas/`, then `python3 build/python_ci.py` per project
  convention.
