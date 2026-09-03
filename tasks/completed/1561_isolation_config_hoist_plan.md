# One `MICROMEGAS_PUBLIC_VIEW_SETS`, Resolved Once Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1561

## Overview

Three removals. `MICROMEGAS_PUBLIC_VIEW_SETS` is read on only one of `FlightSqlServerBuilder`'s
three auth branches, so an embedder that injects its own auth provider gets no allowlist and no
startup error — resolve it once, unconditionally. The knob also has a per-service
`MICROMEGAS_ANALYTICS_` form that nothing needs — drop it. And every removed-env-var startup
refusal in the tree is a v0.30.0 upgrade shim one release old — delete them all.

## Current State

`rust/public/src/servers/flight_sql_server.rs`. The auth branch at `:281` produces a 3-tuple
(`AuthAndDefaults`, `:44`) whose third element is the default `IsolationConfig`:

| Branch | Default `IsolationConfig` |
|---|---|
| `with_auth_provider(..)` (injected provider) | `IsolationConfig::default()` — env never read (`:312`) |
| `with_default_auth()` | `IsolationConfig::from_env("")?` (`:346`) |
| auth disabled | `IsolationConfig::default()` (`:361`) |

`self.isolation_config` overrides whichever default the branch produced (`:365`). The per-branch
shape is presumably an analogy to `read_policy`, whose default genuinely does differ per branch:
it needs a DB-backed grant store, and the injected-provider branch deliberately skips building one
when `with_read_policy` was set (`:283`). `IsolationConfig` has no such dependency — it is one
`Vec<String>` parsed from the environment.

`IsolationConfig::from_env(prefix)` (`rust/analytics/src/lakehouse/read_scope.rs:179`) resolves
`PUBLIC_VIEW_SETS` and the removed `UNSTAMPED_AUDIENCE` knob through `resolved_var` (`:153`), a
`{prefix}_X`-then-`MICROMEGAS_X` fallback helper private to that file. The only caller passing a
prefix is the monolith (`rust/monolith/src/main.rs:300`), which resolves the config itself and
passes it in via `with_isolation_config` (`:350`) when `roles.flightsql && !args.disable_auth`.

The inert-knob bug is reachable only by an out-of-repo embedder: the monolith computes its own
value with auth on, and with `--disable-auth` the allowlist is a no-op anyway (`ReadScope::All`
makes `OwnershipRewrite` a no-op).

Three sites refuse startup on a removed variable, 11 variables between them, all removed in
v0.30.0:

| Site | Variables | Replacement |
|---|---|---|
| `reject_removed_admin_vars`, `rust/auth/src/env.rs:36` | `MICROMEGAS_ADMINS`, `MICROMEGAS_ANALYTICS_ADMINS`, `MICROMEGAS_INGESTION_ADMINS` | the `admins` group |
| `reject_removed_cache_ttl_vars`, `rust/auth/src/env.rs:71` | `MICROMEGAS_{,ANALYTICS_,INGESTION_}API_KEY_CACHE_TTL_SECONDS`, `MICROMEGAS_{,ANALYTICS_,INGESTION_}AUDIENCE_GRANT_CACHE_TTL_SECONDS` | `MICROMEGAS_AUTH_CACHE_TTL_SECONDS` |
| `IsolationConfig::from_env`, `read_scope.rs:180` | `MICROMEGAS_UNSTAMPED_AUDIENCE`, `MICROMEGAS_ANALYTICS_UNSTAMPED_AUDIENCE` | `MICROMEGAS_DEFAULT_AUDIENCE` |

Both `env.rs` functions are called from `ProviderBuilder::compose`
(`rust/auth/src/default_provider.rs:87-88`) and `WebServerConfig`
(`rust/analytics-web-srv/src/web_server.rs:79-80`).

## Design

### One variable

`IsolationConfig::from_env` loses its `prefix` parameter and reads `MICROMEGAS_PUBLIC_VIEW_SETS`
only. With the `UNSTAMPED_AUDIENCE` refusal gone too (below), `resolved_var` has no caller and is
deleted, and the whole function is the parse:

```rust
pub fn from_env() -> anyhow::Result<Self> {
    Ok(Self {
        public_view_sets: parse_comma_separated_list("MICROMEGAS_PUBLIC_VIEW_SETS")?,
    })
}
```

### One resolution site

```rust
/// The env is not read at all when the caller supplied a config: `with_isolation_config` wins
/// regardless, so reading anyway would invent a startup failure on a variable the caller
/// deliberately overrode.
fn resolve_isolation_config(
    explicit: Option<Arc<IsolationConfig>>,
) -> Result<Arc<IsolationConfig>> {
    match explicit {
        Some(config) => Ok(config),
        None => Ok(Arc::new(IsolationConfig::from_env()?)),
    }
}
```

Called at the top of `build_and_serve`, before `LakehouseContext::from_env()`.
`FlightSqlServerBuilder` has no `Drop` impl and `build_and_serve` already moves fields out of
`self` one at a time, so taking `self.isolation_config` first is a normal partial move.

Then: `AuthAndDefaults` (`:44`) drops its third element; the three branch tails stop returning an
`IsolationConfig` (`:312`, `:346`, `:361`); the `unwrap_or` at `:365` goes away;
`with_isolation_config`'s doc comment (`:158-165`) names the one default. In the monolith,
`analytics_isolation_config` (`:294-303`), the `with_isolation_config` call (`:349-351`), and the
`IsolationConfig` import (`:26`) are deleted — it takes the builder's resolution like any other
embedder.

### Drop every removed-var refusal

Delete `reject_removed_admin_vars` and `reject_removed_cache_ttl_vars` from `rust/auth/src/env.rs`,
leaving only `resolve_prefixed_var` (whose module doc comment lists `{prefix}_ADMINS` among the
knobs it serves and needs updating). Remove both calls and the import at
`default_provider.rs:11,87-88` and the doc-comment mention at `:82`, both calls at
`web_server.rs:79-80`, and the two tests
`default_provider_tests.rs::removed_{admins,api_key_cache_ttl}_var_set_is_rejected_with_no_db_needed`
with the three `*_ADMINS_VAR` constants and their `EnvGuard` entries (`:30-32`).

Delete the `UNSTAMPED_AUDIENCE` arm of `IsolationConfig::from_env` (`read_scope.rs:180-188`) and
the three tests covering it in `ownership_rewrite_config_tests.rs`.

`mkdocs/docs/admin/groups.md:171-176` is the only page promising a refusal; it becomes a plain
instruction to unset the vars.

## Implementation Steps

1. `rust/analytics/src/lakehouse/read_scope.rs`: drop `from_env`'s `prefix` parameter, delete
   `resolved_var` and the `UNSTAMPED_AUDIENCE` refusal, update the doc comments.
2. `rust/public/src/servers/flight_sql_server.rs`: add `resolve_isolation_config` and call it at
   the top of `build_and_serve`.
3. Same file: shrink `AuthAndDefaults`, drop the `IsolationConfig` from the three branch tails,
   delete the `unwrap_or` line, rewrite `with_isolation_config`'s doc comment.
4. Same file: add `#[cfg(test)] mod tests` (see Testing Strategy).
5. `rust/monolith/src/main.rs`: delete `analytics_isolation_config`, the `with_isolation_config`
   call, and the import.
6. `rust/auth/src/env.rs` + `default_provider.rs` + `analytics-web-srv/src/web_server.rs` +
   `default_provider_tests.rs`: delete both `reject_removed_*` functions and everything
   referencing them; update `env.rs`'s module doc comment.
7. Rewrite `rust/analytics/tests/ownership_rewrite_config_tests.rs` for the prefix-free signature
   and drop its three `UNSTAMPED_AUDIENCE` tests.
8. Add `rust/public/tests/isolation_config_fail_fast_tests.rs` and its `[[test]]` entry in
   `rust/public/Cargo.toml`.
9. `mkdocs/docs/admin/monolith.md:51`: rename to `MICROMEGAS_PUBLIC_VIEW_SETS`, drop the
   "falls back to unprefixed" clause, add it to the "One prefix asymmetry" note (`:60-68`).
   `mkdocs/docs/admin/groups.md:171-176`: drop the refusal promise.
10. `CHANGELOG.md`: an **Analytics** bullet and an **Auth** bullet under `## Unreleased`.
11. `cargo fmt`, `cargo clippy --workspace --all-targets`, `cargo test` from `rust/`.

## Files to Modify

- `rust/analytics/src/lakehouse/read_scope.rs`
- `rust/public/src/servers/flight_sql_server.rs`
- `rust/monolith/src/main.rs`
- `rust/auth/src/env.rs`, `rust/auth/src/default_provider.rs`,
  `rust/analytics-web-srv/src/web_server.rs`
- `rust/auth/tests/default_provider_tests.rs`,
  `rust/analytics/tests/ownership_rewrite_config_tests.rs`
- `rust/public/tests/isolation_config_fail_fast_tests.rs` (new), `rust/public/Cargo.toml`
- `mkdocs/docs/admin/monolith.md`, `mkdocs/docs/admin/groups.md`, `CHANGELOG.md`

## Decisions

- Resolve at the top of `build_and_serve`, not just before the auth branch — fail-fast before the
  Postgres/object-store setup, at no cost.
- Keep `with_isolation_config` though no in-repo caller uses it after step 5: it mirrors
  `with_read_policy` and is the embedder's escape hatch.
- Skip the env read when `with_isolation_config` was set — same reasoning as the injected-provider
  `ReadPolicy` default at `:293`.
- No startup log of the resolved allowlist, and no refusal for the dropped
  `MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS` — consistent with removing the other 11.

## Documentation

`mkdocs/docs/admin/monolith.md` is the only page documenting the prefixed form, and `groups.md`
the only one promising the admin refusal. The other `PUBLIC_VIEW_SETS` hits
(`admin/flight-sql.md:33`, `admin/authorization.md:22` and `:193`,
`admin/functions-reference.md:75`) document the unprefixed variable with no branch precondition,
which the fix makes true. Check with `grep -rn PUBLIC_VIEW_SETS mkdocs/docs/` — scoped to `docs/`,
since `mkdocs/site/` is generated output.

## Testing Strategy

**`#[cfg(test)] mod tests` in `flight_sql_server.rs`** over `resolve_isolation_config`, which is
private to the builder. First unit-test module in `rust/public/src`; `serial_test` is already a
dev-dependency (`Cargo.toml:92`). The `servers` module is behind the non-default `server` feature,
so `cargo test -p micromegas` alone builds zero of these silently — use `cargo test` from `rust/`,
or `-p micromegas --features server`.

Each test is `#[serial]` with an `EnvGuard` clearing `MICROMEGAS_PUBLIC_VIEW_SETS`. Cases: env set → parsed; env unset → empty; env malformed → `Err`;
explicit config → wins, with the env set to a different and then a malformed value, pinning that
it isn't read on that path.

**`ownership_rewrite_config_tests.rs`**, rewritten for the prefix-free signature. The four
`PUBLIC_VIEW_SETS` parsing tests and `unset_vars_resolve_to_default` carry over against
`from_env()`; the three `UNSTAMPED_AUDIENCE` tests go with the refusal. Add one pinning that
`MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS` is *not* read — otherwise nothing distinguishes the
intended drop from an accidental one. Its module doc comment claims these vars are touched by no
other test in the repo, which the new `rust/public` tests falsify.

**`rust/public/tests/isolation_config_fail_fast_tests.rs`**: one test asserting
`build_and_serve()` returns the "comma-separated, not a JSON array" error with
`MICROMEGAS_PUBLIC_VIEW_SETS` malformed. Its `EnvGuard` must also clear
`MICROMEGAS_SQL_CONNECTION_STRING` and `MICROMEGAS_OBJECT_STORE_URI` — those are usually exported
in a dev shell, and without clearing them the test still passes after connecting to a real lake,
reducing it to a duplicate of the unit case. Clearing them is what makes reaching the error
*without* a lake the assertion that resolution runs ahead of `LakehouseContext::from_env()`. No
`#[serial]` — cargo gives it its own single-test binary. `required-features = ["server"]`.

## Manual Verification

`python3 local_test_env/ai_scripts/start_services.py`, then again with `--monolith`, with
`MICROMEGAS_PUBLIC_VIEW_SETS=log_entries` exported. Both should come up and
`micromegas-query "SELECT count(*) FROM log_entries" --begin 1h` should return rows — a smoke
check that the monolith still boots after step 5 deletes its own resolution. Both default to
`--disable-auth`, under which `ReadScope::All` makes the allowlist inert everywhere it's consumed,
so this doesn't distinguish whether `from_env` was actually reached; that path is covered instead
by the automated `rust/public/tests/isolation_config_fail_fast_tests.rs` and
`flight_sql_server.rs::tests::env_set_is_parsed`. Reaching `with_default_auth` or the
injected-provider branch manually needs `flight-sql-srv` without `--disable-auth` plus
`MICROMEGAS_API_KEYS`, or `--monolith` with an OIDC config set.
