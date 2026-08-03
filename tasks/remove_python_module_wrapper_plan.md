# Remove MICROMEGAS_PYTHON_MODULE_WRAPPER Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1408

## Overview

`MICROMEGAS_PYTHON_MODULE_WRAPPER` is a deprecated escape hatch that let corporate
environments plug a custom `connect()` implementation into the Python CLI, bypassing the
documented URI/OIDC connection path entirely. That need is now served by the OIDC auth flow
(`MICROMEGAS_OIDC_ISSUER`/`MICROMEGAS_OIDC_CLIENT_ID`/etc., or `~/.micromegas/config.json`'s
`issuers` config). This plan removes the wrapper and its resolution, delegation branch, CLI
help text, documentation row, and tests.

## Current State

- `python/micromegas/micromegas/cli/config.py`:
  - `ConnectionConfig.python_module_wrapper: Optional[str] = None` (line 21) — dataclass field.
  - `resolve_connection()` sets it via `_pick("MICROMEGAS_PYTHON_MODULE_WRAPPER")` (line 59),
    with no config-file fallback (unlike every other field).
- `python/micromegas/micromegas/cli/connection.py`:
  - `connect()` imports `importlib` (line 1) solely to support the wrapper branch (lines 13-15):
    if `cfg.python_module_wrapper` is set, it imports that module and delegates to its
    `connect()`, short-circuiting before the OIDC/plain-URI logic.
- `python/micromegas/micromegas/cli/query.py`:
  - The `ArgumentParser` `epilog` (line 63) tells corporate users to set the env var for custom
    auth.
- `mkdocs/docs/query-guide/python-api.md`:
  - Line 657 documents `MICROMEGAS_PYTHON_MODULE_WRAPPER` in the environment variable
    reference table, between `MICROMEGAS_TOKEN_FILE` and the config-file section.
- `python/micromegas/tests/cli/test_config.py`:
  - Six tests reference the env var, each just adding it to a list of vars cleared via
    `monkeypatch.delenv(..., raising=False)` before asserting on unrelated config resolution
    (lines 39, 58, 93, 110, 131). None of these tests assert on `python_module_wrapper`
    itself — no dedicated test exists for the wrapper's resolution or delegation behavior, so
    there is nothing to delete beyond these cleanup entries.
- No other internal callers depend on it: confirmed via repo-wide grep for
  `PYTHON_MODULE_WRAPPER`/`python_module_wrapper` — the only hits are the five files above plus
  historical mentions in `tasks/completed/auth/analytics_auth_plan.md` and
  `tasks/completed/auth/oidc_auth_subplan.md`, which are historical records of a past
  implementation and are left as-is. The `micromegas-query` Claude Code skill
  (`claude-plugin/skills/micromegas-query/`, referenced in issue #1404) does not reference the
  wrapper.

## Design

Straight removal, no replacement behavior:

- Drop the `python_module_wrapper` field from `ConnectionConfig` and its resolution line in
  `resolve_connection()`.
- Drop the wrapper-delegation branch in `connect()`, and the now-unused `importlib` import.
- Drop the CLI epilog sentence.
- Drop the doc table row.
- Drop the env var from each test's clear-list; no test bodies otherwise reference the wrapper,
  so no assertions need updating.

## Implementation Steps

1. **`python/micromegas/micromegas/cli/config.py`**
   - Remove `python_module_wrapper: Optional[str] = None` from `ConnectionConfig`.
   - Remove the `python_module_wrapper=_pick("MICROMEGAS_PYTHON_MODULE_WRAPPER"),` line from
     `resolve_connection()`.
2. **`python/micromegas/micromegas/cli/connection.py`**
   - Remove the `if cfg.python_module_wrapper: ...` branch (lines 13-15).
   - Remove the now-unused `import importlib` (line 1).
3. **`python/micromegas/micromegas/cli/query.py`**
   - Remove the corporate-environment sentence from the `epilog` string (line 63). Drop the
     `epilog` kwarg entirely if no other epilog content remains.
4. **`mkdocs/docs/query-guide/python-api.md`**
   - Remove the `MICROMEGAS_PYTHON_MODULE_WRAPPER` row from the environment variables table
     (line 657).
5. **`python/micromegas/tests/cli/test_config.py`**
   - Remove `"MICROMEGAS_PYTHON_MODULE_WRAPPER"` from each of the five env-var clear-lists
     (lines 39, 58, 93, 110, 131).
6. Run `poetry run black python/micromegas/micromegas/cli/config.py python/micromegas/micromegas/cli/connection.py python/micromegas/micromegas/cli/query.py python/micromegas/tests/cli/test_config.py` from `python/micromegas/` (required before commit).
7. Run `poetry run pytest` from `python/micromegas/` to confirm the suite still passes.

## Files to Modify

- `python/micromegas/micromegas/cli/config.py`
- `python/micromegas/micromegas/cli/connection.py`
- `python/micromegas/micromegas/cli/query.py`
- `mkdocs/docs/query-guide/python-api.md`
- `python/micromegas/tests/cli/test_config.py`

## Trade-offs

- **Remove outright vs. deprecate-with-warning**: the issue explicitly calls this a deprecated
  hack to be removed entirely, and it's an escape hatch with no evidence of remaining internal
  usage — a deprecation warning period adds complexity with no clear beneficiary. Removing
  outright is simpler and matches the issue's stated scope.
- **Historical plan docs** (`tasks/completed/auth/*.md`): left untouched since they're a record
  of prior design decisions, not living documentation; editing them would rewrite history for no
  benefit.

## Documentation

- `mkdocs/docs/query-guide/python-api.md` — remove the env var table row (see above). No other
  docs reference this variable.

## Testing Strategy

- `poetry run pytest` in `python/micromegas/` — existing tests in `test_config.py` continue to
  pass once the env var is dropped from the clear-lists (they were never asserting wrapper
  behavior, only clearing it to avoid cross-test env leakage).
- Manual sanity check: `python/micromegas/micromegas/cli/query.py --help` should no longer
  mention the wrapper in its epilog (or should have no epilog if nothing else was in it).
