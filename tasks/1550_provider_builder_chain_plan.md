# `ProviderBuilder::build_chain()` Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1550

## Overview

`ProviderBuilder::build()` conflates two questions behind one `Option`: "did the operator
configure any auth source at all?" (a startup guard) and "give me the composed provider chain".
When a key store is attached but nothing counted as configured at boot, the whole chain — DB
provider included — is discarded, so a caller that only wanted the second answer loses the DB
provider it explicitly attached. Split the two: add `build_chain()`, which composes and returns
the chain with no guard and no startup existence query, and leave `build()`'s `Option` semantics
exactly as they are for the standalone binaries that rely on the guard.

## Current State

`rust/auth/src/default_provider.rs:94-161`. `build()` composes, in order, env
`ApiKeyAuthProvider` → `OidcAuthProvider` → `DbApiKeyAuthProvider`, setting `configured = true`
for each source that is present. With a key store attached it also runs one
`key_store_has_live_rows` existence query and treats a non-empty table as configured. Then:

```rust
if !configured {
    return Ok(None);   // the chain, DB provider included, is discarded
}
```

Two consequences:

1. The doc comment at `default_provider.rs:76-80` promises "**The DB provider is always pushed
   onto the chain whenever a key store is attached** … authenticates it on the very next request,
   with no restart." That holds only when `configured` became true by some other route. In the
   exact scenario the sentence describes — previously empty table, first key minted at runtime,
   no env keys, no OIDC — `build()` returns `None` and the promise silently doesn't hold.
2. The `Err` arm at `default_provider.rs:130-137` (a `key_store_has_live_rows` failure, e.g. a
   schema short of migration v5) is gated on the same `!configured` condition, so a caller that
   isn't using the guard still inherits a startup failure mode whose only purpose is to inform it.

**No in-repo binary is currently affected.** All three `build()` call sites turn `None` into a
hard abort — `rust/public/src/servers/flight_sql_server.rs:316-330` (`use_default_auth`),
`rust/monolith/src/main.rs:209-226` and `:232-250`, `rust/telemetry-ingestion-srv/src/main.rs:60-72`
— so any process that is actually running had `configured == true` and does see a
runtime-minted key with no restart. The gap is reachable only by an embedder on the
injected-provider path (`FlightSqlServer::with_auth_provider(..)`, folding `ProviderBuilder`'s
output into a larger `MultiAuthProvider` next to its own providers) that treats `None` as
"no DB keys today" and keeps serving. For that caller `None` is lost capability, not a safety
signal.

Existing coverage lives in `rust/auth/tests/default_provider_tests.rs` — four `#[ignore]`,
`#[serial]` tests requiring a live migrated Postgres, including
`empty_table_and_nothing_else_yields_none` (which pins the guard this plan must not change) and
`missing_relation_is_err_not_none`.

## Design

### New public entry point

```rust
/// Composes the provider chain with **no** "is anything configured?" guard.
pub async fn build_chain(mut self) -> Result<Arc<dyn AuthProvider>>
```

Same composition and same order as `build()`, but:

- always returns the chain — never `None`, because there is no `Option`;
- never calls `key_store_has_live_rows`, so it carries none of that query's startup failure mode;
- the DB provider is pushed whenever a key store is attached, which is what makes the
  no-restart property unconditionally true here.

When nothing at all is configured (no env keys, no OIDC, no key store) the returned chain is an
empty `MultiAuthProvider`, which rejects every request (`multi.rs:111`) — fail-closed, and the
caller asked for no guard. Emit one `warn!` in that case, using the already-present
`MultiAuthProvider::is_empty()` (`multi.rs:73`, currently unused), since an empty chain is the
one shape whose result is useless to any caller.

### Shared composition

Both entry points share one private helper, so the ordering and the "DB provider always pushed"
rule are stated once:

```rust
/// Composes the chain and reports whether env keys or OIDC counted as "configured".
///
/// Takes `&mut self` rather than `self` so `build()` can still reach the attached key
/// store's pool for its existence query afterwards.
async fn compose(&mut self) -> Result<(MultiAuthProvider, bool)>
```

`compose` holds today's body up to and including the `multi.with_provider(db_provider)` push, and
nothing else — the existence query, the `configured` upgrade it can perform, and the `!configured`
early return all stay in `build()`. `ApiKeyTable` is `Copy` and `PgPool` is cheap to clone, so
`compose` reads `self.key_store` by reference and leaves it in place.

Resulting shape:

```rust
pub async fn build_chain(mut self) -> Result<Arc<dyn AuthProvider>> {
    let (multi, _) = self.compose().await?;
    if multi.is_empty() { warn!("..."); }
    Ok(Arc::new(multi) as Arc<dyn AuthProvider>)
}

pub async fn build(mut self) -> Result<Option<Arc<dyn AuthProvider>>> {
    let (multi, mut configured) = self.compose().await?;
    if let Some((pool, table)) = &self.key_store {
        // unchanged: existence query, Err-when-!configured arm, warn-otherwise arm
    }
    if !configured { return Ok(None); }
    Ok(Some(Arc::new(multi) as Arc<dyn AuthProvider>))
}
```

`build()`'s observable behaviour is unchanged in every case: same provider order, same
`parse_key_ring` / `OidcAuthProvider::new` error propagation ahead of the existence query, same
`Err`/`warn!` arms, same `Ok(None)`.

### Doc-comment corrections

The false promise is the other half of the fix:

- Move the "**The DB provider is always pushed onto the chain whenever a key store is attached**"
  paragraph onto `build_chain` (and the mechanical half onto `compose`), where it is
  unconditionally true.
- Rewrite `build()`'s copy to state the actual contract: the DB provider is pushed, but the whole
  chain is discarded when nothing counted as configured, so the no-restart property holds only
  for a chain that survived the guard — which is every process that successfully started, since
  each in-repo caller aborts on `None`. Point an embedder that does not want the guard at
  `build_chain()`.
- Note on `build_chain` that it runs no existence query, so it cannot fail the way `build()` can
  on a schema short of v5.

## Implementation Steps

1. `rust/auth/src/default_provider.rs`: extract `compose(&mut self)` from the top of `build()`,
   returning `(MultiAuthProvider, bool)`; leave the existence query and the `!configured` return
   in `build()`.
2. Same file: add `pub async fn build_chain(mut self) -> Result<Arc<dyn AuthProvider>>` with the
   empty-chain `warn!`.
3. Same file: correct the doc comments per **Doc-comment corrections** above.
4. `rust/auth/tests/default_provider_tests.rs`: add the three tests below and update the module
   header, which currently claims every test in the file needs a live Postgres.
5. `CHANGELOG.md`: one bullet under `## Unreleased` → `**Auth:**`.

## Files to Modify

- `rust/auth/src/default_provider.rs`
- `rust/auth/tests/default_provider_tests.rs`
- `CHANGELOG.md`

## Trade-offs

The issue offered two fixes. **Rejected**: making `build()` return the chain whenever a key store
is attached, regardless of `has_live_rows`. It is fewer lines, but it makes the startup guard
vacuous for all three in-repo binaries — an operator who configured nothing would get a server
that boots and rejects every request instead of the current clear boot error naming the env vars
and the table. **Chosen**: a second entry point, which leaves `use_default_auth`'s `bail!` and
`empty_table_and_nothing_else_yields_none` untouched while giving an embedder an honest one.

Also considered and rejected: giving `build_chain` an `Option`-free variant of the existence query
(e.g. to keep the migration-v5 diagnostic as a `warn!`). It would reintroduce the DB round trip
this method exists to avoid, and the same diagnostic is already reachable through
`warn_if_data_lake_schema_stale`: the FlightSQL wiring's `use_default_auth` branch calls it
unconditionally, but its injected-provider branch (`with_auth_provider(..)`, the path this plan's
`build_chain()` embedder uses) only calls it when the caller left `read_policy` unset. An embedder
that sets its own read policy — the in-repo monolith always does — is expected to call
`warn_if_data_lake_schema_stale` itself if it wants the diagnostic.

## Documentation

Rustdoc on `default_provider.rs` is the whole documentation surface here. `mkdocs/docs/admin/`
describes operator-visible behaviour, which is unchanged — no page needs an edit. In particular
`mkdocs/docs/admin/api-keys.md:434` ("a schema still short of v5 makes flight-sql **fail to
start**") stays accurate, since flight-sql keeps using `build()`.

## Testing Strategy

New tests in `rust/auth/tests/default_provider_tests.rs`:

1. `build_chain_authenticates_key_minted_into_empty_table` (`#[ignore]`, `#[serial]`, live PG) —
   the issue's exact scenario. Reuse `empty_table_and_nothing_else_yields_none`'s throwaway-schema
   setup (its own empty `ingestion_api_keys`, so the shared table's contents can't make this
   flaky), with `MICROMEGAS_API_KEYS` and `MICROMEGAS_OIDC_CONFIG` both unset. Assert `build()`
   returns `None` on that pool and `build_chain()` returns a chain; then insert a live key into
   the throwaway table and assert the chain authenticates it, with no rebuild. The `build()`
   assertion in the same test is what pins the two methods as genuinely different. Follow the
   existing deferred-`Result` pattern so the schema is dropped even when an assertion fails; the
   throwaway table needs the full column set `DbApiKeyAuthProvider` reads, not just
   `key_id`/`revoked_at`.
2. `build_chain_ignores_missing_relation` (`#[serial]`, no DB, not `#[ignore]`) — `build_chain()`
   never calls `key_store_has_live_rows`, so composing it issues no query. Reuse
   `db_api_key_tests.rs`'s `unreachable_pool()` (`PgPoolOptions::new().acquire_timeout(50ms).connect_lazy(..)`),
   same precedent as `dedicated_key_store_pool_is_small_and_lazy`: assert `build_chain()` returns
   `Ok` promptly against that pool, where `build()` returns `Err` (its existence query fails on
   connect). Use the file's `EnvGuard` and `remove_var` both `MICROMEGAS_API_KEYS` and
   `MICROMEGAS_OIDC_CONFIG` at the top, like every other test in this file — otherwise it races
   with test 3's process-wide `MICROMEGAS_API_KEYS` and the `build()` assertion flips to `Ok`.
3. `build_chain_with_env_keys_only_authenticates` (`#[serial]`, no DB, not `#[ignore]`) — no key
   store attached, `MICROMEGAS_API_KEYS` set; assert the returned chain authenticates the env key.
   Cheap, and one of the two new tests that runs in ordinary CI.

Then `cargo test -p micromegas-auth`, plus `cargo clippy` / `cargo fmt` over the workspace. The
one remaining `#[ignore]` case needs a live migrated Postgres via `MICROMEGAS_SQL_CONNECTION_STRING`
(`python3 local_test_env/ai_scripts/start_services.py`) and `cargo test -p micromegas-auth --
--ignored`.
