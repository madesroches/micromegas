# Remove Env-Keyring Startup Warnings Plan

## Overview
The startup warnings for still-set `MICROMEGAS_API_KEYS`, `MICROMEGAS_INGESTION_API_KEYS`, `MICROMEGAS_ANALYTICS_API_KEYS`, `MICROMEGAS_AUDIENCE_GRANTS` and `MICROMEGAS_ANALYTICS_AUDIENCE_GRANTS` were upgrade shims scheduled for removal in v0.32.0, which shipped them unchanged. Remove the warnings and their helper code, and record the removal in `CHANGELOG.md`. `object-cache-srv`'s own `MICROMEGAS_API_KEYS` keyring is unaffected.

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1698

## Current State
- `rust/auth/src/env.rs` holds `removed_vars_that_are_set` (`pub`, `#[doc(hidden)]`, only so tests can reach it), `warn_removed_api_key_vars` and `warn_removed_audience_grant_vars` (both `pub(crate)`). `resolve_prefixed_var` in the same file stays.
- `rust/auth/src/default_provider.rs` imports both warn functions and calls them at the top of `compose()` (lines ~10, 72-73).
- `rust/auth/tests/env_tests.rs` only tests `removed_vars_that_are_set`.
- The module doc comment of `env.rs` does not mention the warnings, so it needs no change.
- Docs that describe the warning: `mkdocs/docs/admin/api-keys.md` ("Migrating from the env keyring" section, which says a still-set variable "logs a `warn!`" and that "the `warn!` line is the only signal"), plus step 3 of the migration list ("leaving it set logs a warning and changes nothing").
- `rust/auth/tests/default_provider_tests.rs` mentions the upgrade-shim warning in a comment near line 470.

## Design
Delete the three functions from `env.rs`, the import and two calls in `default_provider.rs`, and `tests/env_tests.rs` (its only subject disappears). No behavior change besides the absence of the log line: the variables were already not read.

## Implementation Steps
1. `rust/auth/src/env.rs`: remove `removed_vars_that_are_set`, `warn_removed_api_key_vars`, `warn_removed_audience_grant_vars`.
2. `rust/auth/src/default_provider.rs`: drop the two functions from the `use crate::env::{...}` import (leaving `resolve_prefixed_var`) and remove the two calls in `compose()`.
3. Delete `rust/auth/tests/env_tests.rs`.4. `rust/auth/tests/default_provider_tests.rs`: reword the comment near line 470 that refers to the warning, if it would otherwise be stale.
5. `mkdocs/docs/admin/api-keys.md`: update "Migrating from the env keyring" to stop describing a `warn!` line (three places: the intro paragraph, the consequences paragraph, and migration step 3); state that the variables are ignored silently and that skipping the migration leaves keyring tokens unauthenticated. Keep docs free of issue numbers.
6. `CHANGELOG.md`: add an Unreleased entry (with `#1698`) saying the startup warnings were removed and that the variables are now ignored silently. **Minor breaking change:** `micromegas_auth::env::removed_vars_that_are_set` is removed.

## Files to Modify
- `rust/auth/src/env.rs`
- `rust/auth/src/default_provider.rs`
- `rust/auth/tests/env_tests.rs` (delete)
- `rust/auth/tests/default_provider_tests.rs` (comment only, if stale)
- `mkdocs/docs/admin/api-keys.md`
- `CHANGELOG.md`

## Trade-offs
Keeping the warnings longer was rejected: the issue schedules removal for v0.32.0 and the shim has served its window.

## Documentation
`mkdocs/docs/admin/api-keys.md` as in step 5; check `authentication.md` and `object-cache.md` remain accurate (they describe `object-cache-srv`'s keyring, which is unchanged).

## Testing Strategy
Pure deletion. `cargo build` / `cargo clippy --workspace -- -D warnings` confirm nothing else referenced the removed items; existing `default_provider_tests` still pass, including the test showing a still-set `MICROMEGAS_API_KEYS` does not authenticate. No new tests.
