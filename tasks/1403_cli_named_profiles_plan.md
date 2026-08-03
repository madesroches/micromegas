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
`MICROMEGAS_ANALYTICS_URI` and friends for server-side auth setup (not
directly affected, but cross-references the same env vars).

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
today's single connection — the existing flat-config tests keep passing
unmodified.

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
def resolve_active_profile(config, profile=None):
    """Resolve the active profile name and its connection dict from `config`.

    Returns `(name, active_config)`; `name` is `None` when no `profiles` map
    exists and the flat config is used directly as `active_config`. Raises
    `ValueError` if `--profile`/`MICROMEGAS_PROFILE` is set but there's no
    `profiles` map, if the profile is ambiguous (multiple profiles configured,
    none selected), or if the resolved name isn't in `profiles`.
    """
    profiles = config.get("profiles")
    if profiles is None:
        if profile or os.environ.get("MICROMEGAS_PROFILE"):
            raise ValueError(
                "no profiles configured; remove --profile/MICROMEGAS_PROFILE "
                "or add a `profiles` map to the config file"
            )
        return None, config

    name = resolve_profile_name(profile, config)
    if name is None:
        if len(profiles) == 1:
            name = next(iter(profiles))
        else:
            raise ValueError(
                "multiple profiles configured but none selected; pass --profile, "
                f"set MICROMEGAS_PROFILE, or set default_profile (available: "
                f"{', '.join(sorted(profiles))})"
            )
    if name not in profiles:
        raise ValueError(
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

Individual `MICROMEGAS_*` env vars still win over everything, per the issue's
requirement that existing single-connection setups using env vars keep working
untouched — `_pick` already gives them top precedence per field.

The "exactly one profile, none selected → use it implicitly" rule is a small
UX addition beyond the issue text: it means a user who only ever has one
environment doesn't need `default_profile` boilerplate, while anyone with more
than one profile gets a clear error instead of a silently wrong connection.

An explicit `--profile`/`MICROMEGAS_PROFILE` against a legacy flat config (no
`profiles` key) also raises `ValueError` rather than silently falling back to
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
matching today's precedence for the non-profile case.

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
try:
    client = connection.connect(profile=args.profile)
except ValueError as e:
    parser.error(str(e))
```
turning an unknown/ambiguous profile into the same `argparse` usage-error
treatment every other bad input in `main()` already gets, rather than a raw
traceback.

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
`pro` for `prod`) raises the same `ValueError`, surfaced the same way via
`parser.error`, instead of silently printing "No saved tokens found"; and an
explicit `--profile`/`MICROMEGAS_PROFILE` against a flat config with no
`profiles` map raises instead of quietly resolving a `tokens-<profile>.json`
path that `resolve_connection` would never have picked.

## Implementation Steps

1. **`config.py`**: add `resolve_profile_name()`, `default_token_file()`, and
   the shared `resolve_active_profile(config, profile)` helper; rewrite
   `resolve_connection()` to call it and use `default_token_file(name)` as
   the token-file fallback. `resolve_active_profile()` raises `ValueError`
   for an unknown or ambiguous profile, and also for an explicit
   `--profile`/`MICROMEGAS_PROFILE` when the config has no `profiles` map.
2. **`connection.py`**: thread `profile=None, config_path=None` through
   `connect()` into `resolve_connection()`.
3. **`query.py`**: add `--profile` argument; wrap the `connection.connect()`
   call in `try/except ValueError` → `parser.error()`.
4. **`logout.py`**: add `--profile` argument; resolve the profile name via
   the shared `resolve_active_profile()` + `load_config()` (catching
   `ValueError` → `parser.error()`, matching `query.py`) and the token path
   via `default_token_file()`, replacing the inline hard-coded default.
5. **Tests** (`tests/cli/test_config.py`): add cases for
   - flat config (no `profiles` key) behaves exactly as before (regression)
   - `profiles` map with `default_profile` set, no `--profile`/env
   - `--profile` argument overrides `default_profile`
   - `MICROMEGAS_PROFILE` env var overrides `default_profile` but loses to an
     explicit `profile` argument
   - unknown profile name raises `ValueError` mentioning the available names
   - `profiles` map with exactly one entry and nothing selected → used
     implicitly
   - `profiles` map with more than one entry and nothing selected →
     `ValueError`
   - individual `MICROMEGAS_*` env vars still win over the active profile's
     values
   - `default_token_file()` returns the plain default when `profile is None`
     and a `tokens-<profile>.json` path otherwise
   - explicit `--profile` (or `MICROMEGAS_PROFILE`) with a flat config (no
     `profiles` key) raises `ValueError`
6. **Tests** (new `tests/cli/test_logout.py` or extend `tests/test_query.py`
   patterns): cover `--profile` selecting the right token file to delete, the
   case where no token file exists, and an unknown `--profile` raising the
   same `ValueError` (via `resolve_active_profile`) that `resolve_connection`
   raises for `micromegas-query`.
7. **Tests** (`tests/test_query.py`): a test following the existing
   usage-error pattern (added in PR #1407, issue #1405) — `monkeypatch.setattr(sys,
   "argv", [...])`, call `main()` directly, and assert `pytest.raises(SystemExit)`
   — asserting an unknown `--profile` exits 2 with a usage message, not a
   traceback.
8. **Docs** (`mkdocs/docs/query-guide/python-api.md:636-683`): add
   `MICROMEGAS_PROFILE` to the env var table, document the `--profile` flag
   under `micromegas-query` and `micromegas-logout`, and add a "Named
   profiles" subsection showing the `profiles`/`default_profile` config shape
   next to the existing flat-config example, explicit about the flat shape
   still being supported.
9. **CHANGELOG.md**: add an entry, following the pattern of the #1407 entry.

## Files to Modify

- `python/micromegas/micromegas/cli/config.py`
- `python/micromegas/micromegas/cli/connection.py`
- `python/micromegas/micromegas/cli/query.py`
- `python/micromegas/micromegas/cli/logout.py`
- `python/micromegas/tests/cli/test_config.py`
- `python/micromegas/tests/cli/test_logout.py` (new)
- `python/micromegas/tests/test_query.py`
- `mkdocs/docs/query-guide/python-api.md`
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
  "must specify" — mitigated by the `ValueError` being immediate and
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
  the `profiles`/`default_profile` shape; `--profile` documented under both
  `micromegas-query` and `micromegas-logout` CLI reference entries.
- `CHANGELOG.md`: new entry for the profiles feature.

## Testing Strategy

- Unit tests in `tests/cli/test_config.py` cover the resolution matrix
  described in Implementation Step 5 — these are hermetic (no live service
  needed), consistent with the existing file — including the explicit
  `--profile`/`MICROMEGAS_PROFILE` with a flat config raising `ValueError`.
- `tests/cli/test_logout.py` (new) covers profile-aware token file deletion
  and the unknown-profile error case, also hermetic (just filesystem + env
  vars, no network).
- `tests/test_query.py` gets one negative test for an unknown `--profile`,
  mirroring the existing `--begin`/`--end` usage-error test (e.g.
  `test_main_overflowing_begin_reports_usage_error`, added in PR #1407, issue
  #1405), which uses `monkeypatch.setattr(sys, "argv", [...])` and
  `pytest.raises(SystemExit)` around a direct `main()` call rather than
  `subprocess.run`.
- Manual smoke test: create a `~/.micromegas/config.json` with two profiles,
  run `micromegas-query --profile local "SELECT 1" --all` against a locally
  running monolith, then repeat with `MICROMEGAS_PROFILE=local` and no flag,
  then `micromegas-logout --profile local` and confirm only that profile's
  token file is removed.
- `poetry run pytest` and `poetry run black --check .` from
  `python/micromegas/`, then `python3 build/python_ci.py` per project
  convention.

## Open Questions

1. Should `python_module_wrapper` (currently env-var-only, no config-file key)
   gain a per-profile config-file key, or stay env-only/global regardless of
   profile? This plan leaves it as-is (global override) since the issue
   doesn't mention it and corporate-auth wrapper selection is typically an
   environment-wide concern, not a per-profile one — but worth confirming.
2. Should an explicit `token_file` key be allowed inside a profile entry (for
   users who want to name the cache file themselves rather than accept the
   `tokens-<profile>.json` convention)? Not required by the issue; omitted
   here to keep the schema minimal, but easy to add later as
   `_pick("MICROMEGAS_TOKEN_FILE", active.get("token_file"), default_token_file(name))`
   if requested.
