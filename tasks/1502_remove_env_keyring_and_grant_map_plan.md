# Remove the Env API Keyring and Env Audience Grant Map Plan

## Overview

Make Postgres the sole source of API keys and audience grants. Both halves of the authorization
surface still carry a startup-resolved env-var source running in parallel with the DB store that
replaced it: `{prefix}_API_KEYS` feeds an in-memory `KeyRing` pushed as the *first* provider in
`ProviderBuilder`'s chain, and `{prefix}_AUDIENCE_GRANTS` is parsed once at startup and checked as
a second source on every read/mint decision. This removes both — `ingestion_api_keys` /
`analytics_api_keys` and `audience_grants` become the only sources, managed at runtime through
`micromegas-import-keys`, `micromegas-grants`, and `analytics-web-srv`'s admin/mint routes.

`object-cache-srv`'s `MICROMEGAS_API_KEYS` stays permanently (it has no DB access by design), so
`parse_key_ring` and `ApiKeyAuthProvider` stay in `micromegas-auth`.

This removes a configuration path, not a feature. No table, route, CLI, or SQL-visible schema
changes. Closes #1502.

**Timing.** v0.30.0 shipped 2026-09-02 and the workspace is at `0.31.0` unreleased, so this lands
at the top of a release cycle as the issue requires.

## Current State

### API keys — the env keyring arm

`rust/auth/src/default_provider.rs`:

- `ProviderBuilder::api_keys_json()` (`:64-67`) resolves `{prefix}_API_KEYS` with fallback to
  `MICROMEGAS_API_KEYS`.
- `compose()` (`:86-92`) parses it into a `KeyRing` and pushes `ApiKeyAuthProvider` as the first
  provider, ahead of OIDC and the DB store, and sets `configured = true`.
- `build()`'s and `build_chain()`'s doc comments (`:159-175`, `:211-230`) document that order, and
  `build()`'s existence-query error branch treats "env keys already configured auth" as a reason to
  downgrade a `key_store_has_live_rows` failure from `Err` to a `warn!`.
- `provider()` / `provider_with_prefix()` (`:230-290`) are thin env-only wrappers whose entire
  reason to exist is exposing the keyring+OIDC composition without a DB pool. **Neither has any
  in-repo caller.**

Live readers of `{prefix}_API_KEYS`, all three via `ProviderBuilder`: `telemetry-ingestion-srv`
(prefix `""`), `flight-sql-srv` (prefix `""`, `rust/public/src/servers/flight_sql_server.rs:327`),
and the monolith's two per-role builders (`MICROMEGAS_INGESTION` / `MICROMEGAS_ANALYTICS`,
`rust/monolith/src/main.rs:209`, `:233`).

`object-cache-srv` reads the same variable through clap (`object-cache-srv/src/cli.rs:59`) and calls
`parse_key_ring` directly (`object_cache_srv.rs:182-187`) — it never touches `ProviderBuilder`.

### Audience grants — the env map arm

`rust/auth/src/policy.rs`:

- `AudienceGrants::from_env` (`:292-308`) resolves `{prefix}_AUDIENCE_GRANTS` and parses it.
- `AudienceReadPolicy::from_env` (`:478-484`) wraps it. There is **no** `AudienceMintPolicy::from_env`
  (the issue names one; it does not exist).
- `AudienceReadPolicy::resolve` (`:502-524`) loops over `self.grants.readers()` and, separately, over
  the store snapshot's.
- `AudienceMintPolicy::resolve_audience` (`:576-600`) checks `self.grants.mint_selectors(aud)` and,
  as a second disjunct, the store snapshot's.
- `with_store(Option<Arc<DbAudienceGrantsSource>>)` on both policies (`:488-491`, `:563-566`).

Live readers of `{prefix}_AUDIENCE_GRANTS`, both via `AudienceReadPolicy::from_env`:
`flight_sql_server.rs:318` and `:354` (prefix `""`, on the injected-provider and `use_default_auth`
branches) and `monolith/src/main.rs:286` (prefix `MICROMEGAS_ANALYTICS`).
`MICROMEGAS_INGESTION_AUDIENCE_GRANTS` was **never** read — no `AudienceReadPolicy::from_env` call
passes the ingestion prefix.

`AudienceMintPolicy` has no production `with_store` caller: `mint_key`
(`analytics-web-srv/src/ingestion_keys.rs:385-400`) builds `AudienceGrants::from_rows(...)` from a
fresh, uncached point query against `audience_grants` and passes it to `new`.

### Startup guards advertising the removed vars

`telemetry-ingestion-srv/src/main.rs:9-12` (module doc) and `:68` (bail message);
`monolith/src/main.rs:218`; `flight_sql_server.rs:337`.

### Removed-var precedent

#1564 deleted `reject_removed_admin_vars` and `reject_removed_cache_ttl_vars` from
`micromegas_auth::env` as "v0.30.0 upgrade shims one release old", and chose silent-ignore for the
`MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS` form it dropped in the same commit. Their detection shape —
a `const` list of names, filter on `std::env::var(..).is_ok()`, one message naming every set var and
the replacement, called from `ProviderBuilder::compose` — is what this plan reuses, downgraded from
`Err` to `warn!`; see `git show 4243898da~1:rust/auth/src/env.rs`.

## Design

### 1. `ProviderBuilder` loses its env-keyring arm

Delete `api_keys_json()` and `compose()`'s `if let Some(keys_json)` branch. `configured` becomes
"OIDC is configured", full stop. The chain is `OidcAuthProvider` → `DbApiKeyAuthProvider`.

`build()`'s existence-query error branch keeps both arms unchanged in shape — `configured` is now
only ever true via OIDC, which is exactly the case where the query's result would be unused. Its doc
comment drops the env-keyring provider from the documented order; `build_chain()`'s `is_empty()`
`warn!` text drops "env keys" too.

Delete `provider()` and `provider_with_prefix()`. Post-removal they are a two-line wrapper over
`ProviderBuilder::new(prefix).build()` with no distinguishing behavior, and both doc comments exist
to advertise `MICROMEGAS_API_KEYS`. An embedder writes `ProviderBuilder::new("").build()`.

### 2. `AudienceGrants` / the policies lose env resolution only

Delete `AudienceGrants::from_env` and `AudienceReadPolicy::from_env`. `parse`, `from_rows`, and
`merge` stay (`parse` as a published-but-test-only entry point; see `## Decisions`).

**Both policies keep their static `grants` field, `new(grants)`, and the corresponding loop /
disjunct in `resolve` / `resolve_audience`.** (See `## Decisions`.)

What changes instead is the constructor shape, so the compiler enumerates every wiring site:

```rust
// before
pub fn with_store(mut self, store: Option<Arc<DbAudienceGrantsSource>>) -> Self
// after
pub fn with_store(mut self, store: Arc<DbAudienceGrantsSource>) -> Self
```

on both `AudienceReadPolicy` and `AudienceMintPolicy`. The `None` spelling had no caller and its only
purpose — "clear a store that env resolution attached" — disappears with `from_env`. The field stays
`Option<...>`, `None` by default, reachable only by not calling `with_store`.

After this, the only remaining production `AudienceReadPolicy::new(AudienceGrants::empty())` sites are
the two in `flight_sql_server.rs` (`:320`, `:368`) whose policy is documented as never resolved
(auth disabled) or always discarded (`with_read_policy` set) — unchanged.

### 3. Startup warning for the five removed variables

Nothing fails. A process that used to read one of these variables logs a `warn!` naming it and the
replacement, then starts normally.

New in `rust/auth/src/env.rs`, `pub(crate)`:

```rust
/// Pure detection, unit-tested directly. Returns the subset of `removed` that is set.
fn removed_vars_that_are_set(removed: &[&'static str]) -> Vec<&'static str>
pub(crate) fn warn_removed_api_key_vars()
pub(crate) fn warn_removed_audience_grant_vars()
```

Splitting detection from logging keeps the part with logic testable without capturing a log sink;
the `warn!` wrappers hold no branching worth a test. Set to *any* value, empty string included,
warns — an empty `MICROMEGAS_API_KEYS` previously meant a parse failure at startup and an empty
`MICROMEGAS_AUDIENCE_GRANTS` meant `empty()`, so neither was a no-op the operator can be assumed to
have meant.

| Function | Variables detected | Replacement named in the message |
|---|---|---|
| `warn_removed_api_key_vars` | `MICROMEGAS_API_KEYS`, `MICROMEGAS_INGESTION_API_KEYS`, `MICROMEGAS_ANALYTICS_API_KEYS` | `micromegas-import-keys` into `ingestion_api_keys` / `analytics_api_keys` |
| `warn_removed_audience_grant_vars` | `MICROMEGAS_AUDIENCE_GRANTS`, `MICROMEGAS_ANALYTICS_AUDIENCE_GRANTS` | `micromegas-grants create <audience> <axis> <selector>` |

Message shape:

```
MICROMEGAS_API_KEYS is set but no longer read -- import the keyring into
ingestion_api_keys / analytics_api_keys with `micromegas-import-keys`, then unset it
```

`compose` stays fallible for other reasons, but neither warning contributes a `?`.

These are **v0.31.0 upgrade shims**. Per the #1564 precedent they get deleted in v0.32.0, after
which a set variable is silently ignored; the plan's final step files that follow-up.

### 4. Wiring

```
flight_sql_server.rs  AudienceReadPolicy::from_env("")?.with_store(Some(s))
                   -> AudienceReadPolicy::default().with_store(s)          (x2)
monolith/main.rs      AudienceReadPolicy::from_env("MICROMEGAS_ANALYTICS")?.with_store(Some(s))
                   -> AudienceReadPolicy::default().with_store(s)
```

`AudienceReadPolicy` already derives `Default` (empty map, no store), so `default().with_store(s)` is
the store-only spelling with no new constructor. The `?` goes away at all three sites.

Startup-guard messages, all four sites, drop the removed variables and name the DB path:

- `telemetry-ingestion-srv/src/main.rs:9-12` module doc — drop the `MICROMEGAS_API_KEYS` line and
  the "any of" sentence's keyring arm.
- `telemetry-ingestion-srv/src/main.rs:68` — "Set MICROMEGAS_OIDC_CONFIG, populate the
  `ingestion_api_keys` DB table, or use --disable-auth for development".
- `monolith/src/main.rs:218` — same shape for the ingestion role.
- `flight_sql_server.rs:337` — "Set MICROMEGAS_OIDC_CONFIG, or populate the `analytics_api_keys`
  DB table".

`monolith/src/main.rs:253-262`'s comment block explaining what an unset `{prefix}_AUDIENCE_GRANTS`
resolves to loses its premise; it collapses to a note that the store snapshot is the whole source.

## Implementation Steps

### Phase 1 — `micromegas-auth`

1. **`rust/auth/src/env.rs`** — add the private `removed_vars_that_are_set` plus the two
   `pub(crate)` `warn_removed_*` wrappers, and an inline `#[cfg(test)] mod tests` for the detection
   helper (step 7). Update the module doc comment's surviving-suffix list: drop `{prefix}_API_KEYS`
   and `{prefix}_AUDIENCE_GRANTS`, leaving `{prefix}_OIDC_CONFIG`, `{prefix}_DEFAULT_AUDIENCE`, and
   the three `{prefix}_API_KEY_*CACHE*` knobs. Fix `resolve_prefixed_var`'s own example list the
   same way.
2. **`rust/auth/src/default_provider.rs`** — delete `api_keys_json`, the `compose()` keyring branch,
   and the `parse_key_ring`/`ApiKeyAuthProvider` import. Call both `warn_removed_*` functions at the
   top of `compose()`. Delete `provider()` and `provider_with_prefix()`. Rewrite every doc comment
   that still advertises the removed keyring: the module doc (`:1-5`, "initialize authentication with
   API key, OIDC, and …"), `ProviderBuilder`'s struct doc (`:23-28`, which references the
   soon-to-be-deleted `provider()`/`provider_with_prefix()`), `ProviderBuilder::new`'s doc (`:37-38`,
   "the same … convention as `provider_with_prefix`"), `compose`'s own doc (`:74-75`, "reports whether
   env keys or OIDC counted as 'configured'"), the provider-order doc comments on `build()` and
   `build_chain()` (`:159-175`, `:211-230`), `build()`'s existence-query paragraph (`:163-169`), the
   inline comment at `:188-192` ("Auth is already configured via another provider (env keys or
   OIDC)"), and the `is_empty()` `warn!` text. Also reword `db_api_key.rs`'s cache-knob fallback
   comment (`:96-98`), which currently justifies itself as "the same fallback `provider_with_prefix`
   already uses for `{prefix}_API_KEYS` / `{prefix}_OIDC_CONFIG`" — a function this step deletes — to
   describe the fallback on its own terms.
3. **`rust/auth/src/policy.rs`** — delete `AudienceGrants::from_env` and
   `AudienceReadPolicy::from_env`. Change both `with_store` signatures to take
   `Arc<DbAudienceGrantsSource>`. Update the module doc comment's opening line ("a JSON grant map
   keyed by audience name"), `AudienceGrants`'s doc comment (it currently justifies one env map "only
   because there is no store yet to split them across"), `RawAudienceGrants`'s and `parse`'s
   `{prefix}_AUDIENCE_GRANTS` references (they become "a grant map JSON document" — `parse` becomes
   a published-but-test-only entry point, kept as the documented grant-map JSON format), including
   `parse`'s doc comment's
   dangling intra-doc link ("Split out from [`Self::from_env`] so tests can exercise parsing without
   mutating the environment", `:253-254`), which loses the `[`Self::from_env`]` reference since
   `from_env` no longer exists, and the two-separate-sources comments in
   `resolve` / `resolve_audience` (the static map is no longer "the env map"). `merge`'s doc comment
   loses its "they check the env map and the DB store snapshot" clause. Also rewrite
   `db_audience_grants.rs`'s module doc (`:1-7`), which opens with "checked alongside the existing
   `{prefix}_AUDIENCE_GRANTS` env map by `AudienceReadPolicy`/`AudienceMintPolicy` … the env map stays
   the static/bootstrap layer" — describe the static-map/store split without the env map.

### Phase 2 — Wiring

4. **`rust/public/src/servers/flight_sql_server.rs`** — both `from_env("")?.with_store(Some(..))`
   sites become `default().with_store(..)`; update the `with_read_policy` doc comment (`:150-159`),
   the injected-provider branch's comment (which says "builds the same env+store-backed policy" and
   cites "a needless `from_env("")` failure mode on a malformed *unprefixed* env var"), the bail
   message at `:337`, and the `use_default_auth` branch's comment at `:342-346` ("Same prefix (`""`)
   `AudienceReadPolicy::from_env` beside it resolves under"), which also cites the deleted function.
5. **`rust/monolith/src/main.rs`** — same policy change at `:286`; rewrite the `:253-262` comment
   block, the `:264-268` comment ("Resolved under the same `MICROMEGAS_ANALYTICS` prefix
   `AudienceReadPolicy::from_env` beside it uses"), and the `:218` bail message.
6. **`rust/telemetry-ingestion-srv/src/main.rs`** — module doc `:9-12` and bail message `:68`.
6a. **`rust/analytics/src/lakehouse/ownership_rewrite.rs:114`** — the operational-mitigation bullet
    ("don't run ingestion with an env-keyring key, OIDC, or `--disable-auth` alongside them") drops
    "an env-keyring key,": that arm becomes impossible once ingestion no longer has one.

### Phase 3 — Tests

7. **`rust/auth/src/env.rs`, inline `#[cfg(test)] mod tests`** — `removed_vars_that_are_set` is
   `pub(crate)`-adjacent, so it is tested in-file, the same way `resolve_isolation_config` is in
   `flight_sql_server.rs` (#1564). Cases: each variable set individually → returned alone;
   empty-string value → still returned (pins that empty is not treated as unset); none set → empty
   vec; two set at once → both returned, in the `const` list's order. `#[serial]` with a guard that
   clears all five on drop.
8. **`rust/auth/tests/default_provider_tests.rs`** —
   - `build_chain_with_env_keys_only_authenticates` (`:455-482`) inverts and is renamed to
     `build_chain_with_env_keys_only_rejects_them`: with `MICROMEGAS_API_KEYS` set and nothing else,
     `build_chain()` still returns `Ok`, and the chain **rejects** that key. This is the direct
     assertion that the keyring arm is gone — stronger than the startup-error check a refusal would
     have allowed, which could pass with the arm still present.
   - New, no DB: with `MICROMEGAS_API_KEYS` set and no key store, `build()` returns `Ok(None)` — the
     keyring no longer counts toward `configured`, which is what turns the removal into the
     existing "no auth providers configured" bail at each binary rather than a silent start.
   - `provider_always_registered_authenticates_key_minted_after_build` (`:90-132`, `#[ignore]`) used
     the env keyring to force `build()` into `Some`. Rework: insert one live key row *before*
     `build()` (so `has_live_rows` makes it `Some`), then insert a *second* key after `build()`
     returns and authenticate that one. Same property, no keyring.
   - Keep `API_KEYS_VAR` in `EnvGuard`'s clear list and the remaining
     `std::env::remove_var(API_KEYS_VAR)` calls at `:144`, `:190`, `:268`, `:328`, `:432`: once
     `compose()` sets `configured` from OIDC alone, a leaked `MICROMEGAS_API_KEYS` can no longer
     change `build()`'s result in either direction, so the clears are hygiene only, not load-bearing.
     Update the module doc comment.
9. **`rust/auth/tests/policy_tests.rs`** — delete the `{prefix}_AUDIENCE_GRANTS` env-fallback section
   (`:617-687`): the three tests, the `PREFIXED_VAR`/`UNPREFIXED_VAR` consts, `const PREFIX` (`:621`),
   and the `EnvGuard` struct with its `Drop` impl (`:626-636`) — all become dead code once the section
   is gone, and `EnvGuard` would otherwise trip `dead_code` under `clippy -D warnings`. Reword, don't
   delete, the module doc comment's paragraph about the `#[serial]`/guard pattern (`:4-8`): it still
   explains the rationale the surviving `default_audience_from_env_*` and `resolve_prefixed_var_*`
   tests rely on. `merge_unions_disjoint_and_overlapping_audiences`'s doc comment (`:546-548`), which
   describes `resolve`'s runtime behavior as checking "the env map and the DB store snapshot as two
   separate sources", loses "env map" for "the static map"; its `env_grants` local (`:554`) is renamed
   to `static_grants` to match. Every other test in the file is unaffected.
10. **`rust/auth/tests/db_audience_grants_tests.rs`** — `with_store(Some(store))` →
    `with_store(store)` at `:81`, `:99`, `:463`, `:423`. `live_mint_policy_with_store_merges_a_store_granted_selector`'s
    doc comment references "the env-equivalent map passed to `AudienceMintPolicy::new`" — reword to
    "the static map". `read_policy_with_unreachable_store_fails_closed_even_with_permissive_env_grants`
    (`:78-86`) is renamed to drop "env_grants" (e.g. `..._permissive_static_grants`), its doc comment
    ("even when the env grant map alone would be permissive") reworded to "the static grant map", and
    its assertion message ("must not silently fall back to the env map alone") reworded to "the
    static map alone".
11. **`rust/public/tests/read_policy_threading_tests.rs`** —
    `unconfigured_deployment_resolves_a_scope_and_query_results_are_unaffected` (`:447-457`) becomes
    `AudienceReadPolicy::default()`; rename it and its doc comment to say
    "a policy with no grant source" rather than "env var unset". Its `api_key_provider` helper is
    unaffected — it constructs `ApiKeyAuthProvider` directly, which stays published.

### Phase 4 — Scripts

12. **`local_test_env/ai_scripts/start_services_with_oidc.py`** — this is the only in-repo script
    that runs a `ProviderBuilder` binary on the env keyring, so it breaks outright. Migrate to a DB
    row: keep `generate_local_ingestion_key()` and the `MICROMEGAS_INGESTION_API_KEY` sink-side
    export, drop the `MICROMEGAS_API_KEYS` server-side export, and poll for schema migration version
    >= 6 (`SELECT version FROM migration`) rather than for the `ingestion_api_keys` table's existence:
    the table is created in `upgrade_data_lake_schema_v5` but its `audience` column, which the INSERT
    below needs, is only added in `upgrade_data_lake_schema_v6`, and `execute_migration` commits each
    version in its own transaction, so a poll that fires as soon as the table exists can land between
    the two commits on a fresh database. The poll must also tolerate `relation "migration" does not
    exist` on a fresh database (the ingestion binary creates the table itself) and use a bounded
    attempt count, the same shape as `wait_for_service`'s, but on a sub-second interval —
    `wait_for_service`'s 1-second granularity is too coarse here, since the sink's own retry schedule
    starts at ~10ms; the poll's real floor is the latency of each `psql` exec, not a fixed sleep. Poll
    via `docker exec teledb psql -U $MICROMEGAS_DB_USERNAME`, the same shape
    `local_test_env/db/utils.py`'s `ensure_app_database` already uses to reach the data lake: with
    `MICROMEGAS_SQL_CONNECTION_STRING` carrying no path (`doc/GETTING_STARTED.md:30`) and
    `local_test_env/db/run.py` starting Postgres with only `POSTGRES_USER` set, the role-named default
    database *is* the lake, so no `-d` is needed. Insert the row as soon as the schema can accept it —
    i.e. as soon as migration version >= 6 is observed — first revoking any live row a previous run
    left behind, since `name` is not unique and a fresh `INSERT` alone would accumulate one live
    credential per run:

    ```sql
    UPDATE ingestion_api_keys SET revoked_at = now(), revoked_by = 'start_services_with_oidc'
    WHERE name = 'local-self-telemetry' AND revoked_at IS NULL;

    INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by, audience)
    VALUES (gen_random_uuid(), decode('<sha256 hex>','hex'), 'local-self-telemetry', now(), 'start_services_with_oidc', 'public')
    ```

    `key_hash` is `hashlib.sha256(key.encode()).hexdigest()` — `hash_key`
    (`rust/auth/src/db_api_key.rs:118`) is a plain SHA-256 over the whole key string.
    Its ingestion server runs with auth ON and needs a credential for every `#[micromegas_main]`
    process's self-telemetry sink — including the ingestion process's own sink. The row should exist
    as early as possible relative to the sink's `insert_process` attempts. Start the migration-version
    poll and issue the `INSERT` immediately after the ingestion `Popen` call and before
    `wait_for_service` — `connect_to_remote_data_lake` (which runs `execute_migration` and commits v6
    in its own transaction) executes before `serve_ingestion` binds the listener, so polling from that
    point normally lands the row before the port ever opens, leaving only a narrow residual race on
    the very first request (see Manual Verification #1). Also add the same `MICROMEGAS_TELEMETRY_URL`
    / `MICROMEGAS_FLUSH_PERIOD` default-if-unset block `start_services.py` already has (`:364-368`),
    since this script currently sets neither and the self-telemetry path this migration targets is
    otherwise only enabled by accident, if the operator's shell happens to export them.
    `DbApiKeyAuthProvider` validates against the table live, but a 401 on the *first* request is not
    isolated: `DbApiKeyAuthProvider`'s unknown-key cache (`rust/auth/src/db_api_key.rs:369-372`)
    negatively caches the pre-row miss for `MICROMEGAS_API_KEY_UNKNOWN_CACHE_TTL_SECONDS` (default 10s,
    `:109`), so every retry the ingestion sink sends in that window is also rejected without touching
    the table, and since the 401 is `Permanent` (`http_event_sink.rs:522-528`) the sink never re-sends
    that `insert_process`, permanently leaving the ingestion process without its own `processes` row.
    Set `MICROMEGAS_API_KEY_UNKNOWN_CACHE_TTL_SECONDS=0` on the ingestion child env (`ingestion_env`,
    `:210-211`) so the dev path does not widen the race into a ~10s window of 401s.
13. **`local_test_env/ai_scripts/start_services.py`** — no functional change. Its
    `MICROMEGAS_API_KEYS` at `:135` is scoped to the `object-cache-srv` child env, which keeps the
    variable; the other services run `--disable-auth`. Add a one-line comment saying so, since the
    variable now means "object-cache only".
14. **`build/run_flight_container.py`** — drop `-e MICROMEGAS_API_KEYS` from the `docker run` line;
    `flight-sql-srv` no longer reads it. Add an `-e MICROMEGAS_OIDC_CONFIG` passthrough in its place
    (same bare-passthrough shape as the other three vars), with a one-line comment noting it depends
    on the caller's environment having `MICROMEGAS_OIDC_CONFIG` set or the `analytics_api_keys` table
    already populated.
15. **`docker/docker-compose.monolith.yaml:52-56`** — rewrite the auth comment block: the DB tables
    (and `MICROMEGAS_*_OIDC_CONFIG`) are the only options.

### Phase 5 — Docs

16. **`mkdocs/docs/admin/authentication.md`** — `:45-51` "Two flavors coexist" becomes DB-backed keys
    for ingestion/flight-sql plus the `object-cache-srv`-only keyring. Reframe, don't drop, the
    "**Env keyring**" block at `:116-132` (the "**Env keyring**" lead-in through the "**Format:**"
    list that follows it, which documents the JSON shape `object-cache-srv` still requires): keep the
    format under an `object-cache-srv`-only heading, or point to `admin/object-cache.md:41`, which
    documents the same shape. Delete the `export MICROMEGAS_API_KEYS=...` line at `:305`, keeping the
    `object-cache-srv` pointer at `:685`. Rewrite `:16-18` ("When multiple providers are configured,
    they are tried in order until one succeeds (API key first for performance, then OIDC)"), which
    the Design §1 chain order (`OidcAuthProvider` → `DbApiKeyAuthProvider`) inverts, to OIDC first,
    then the DB-backed key store. Scope `:58`'s "Fast validation (HashMap lookup for the env keyring;
    cached hash lookup for DB-backed keys)" so the HashMap-lookup claim applies only to
    `object-cache-srv`'s keyring, not to ingestion/flight-sql.
17. **`mkdocs/docs/admin/authorization.md`** — delete the `MICROMEGAS_AUDIENCE_GRANTS` row from the
    env table (`:20`) and the whole "Deprecated: the env grant map" section (`:99-128`). Check for
    inbound anchor links to `#deprecated-the-env-grant-map` and remove them (`flight-sql.md:31`,
    `monolith.md:50`, and this same file's `:292-296`, which also drops the "[deprecated env
    map](#deprecated-the-env-grant-map)" link and rewrites the sentence so the store snapshot is
    the sole read-axis source). In "Audience stamping" (`:139`), drop "env-keyring key" from the
    no-bound-audience list, leaving OIDC token and no-auth-provider.
18. **`mkdocs/docs/admin/api-keys.md`** — `:4` intro drops the "or in `MICROMEGAS_API_KEYS`"
    alternative. `:26-28`, which says the env keyring "still works and is still checked" and that
    migrating is "an operator decision", is rewritten: the keyring is no longer read by ingestion or
    flight-sql, and migration is required in v0.31.0. `:191` drops the deprecated-env-map clause. `:233` (data ingested through the env
    keyring carries no audience) is deleted. `:359` loses `MICROMEGAS_API_KEYS` from its
    "same convention" list. Rewrite the "Migrating from the env keyring" section (`:434-500`): it is
    now a **v0.31.0 upgrade requirement, not an option** — step 1's "nothing changes yet: the env
    keyring still authenticates every existing key" is false, and step 3's "remove
    `MICROMEGAS_API_KEYS`" becomes "unset it; leaving it set logs a warning and changes nothing".
    Add the consequence of *skipping* the migration, since a warning does not stop a deployment
    from getting there: with OIDC configured the service starts and every keyring token stops
    working; without OIDC and with an empty table it does not start at all. Keep the
    `micromegas-import-keys` recipe and the three `--only`/`--exclude` routing rules verbatim; keep
    the object-cache-forever bullet. Preserve the heading text and its implicit
    `#migrating-from-the-env-keyring` slug (or add an explicit
    `{#migrating-from-the-env-keyring}` id) — five inbound links depend on it: this file's `:28`,
    `:44`, and `:158`, `mkdocs/docs/admin/authentication.md:683`, and
    `mkdocs/docs/query-guide/python-api.md:965`. Re-check all five still resolve after the rewrite.
19. **`mkdocs/docs/admin/ingestion.md`** — drop the `MICROMEGAS_API_KEYS` table row (`:29`), the
    `# API keys for machine-to-machine producers (legacy/bootstrap path)` comment and the `export`
    line below it (`:57-58`), and the keyring arm of the "If none of ..." sentence (`:50`); `:89`'s
    no-bound-audience note drops env-keyring keys.
20. **`mkdocs/docs/admin/flight-sql.md`** — drop the `MICROMEGAS_API_KEYS` (`:28`) and
    `MICROMEGAS_AUDIENCE_GRANTS` (`:31`) rows and the keyring arm at `:56`. `:72`'s
    "An API-key (`MICROMEGAS_API_KEYS`) caller" becomes an `analytics_api_keys` caller.
21. **`mkdocs/docs/admin/monolith.md`** — drop the `MICROMEGAS_ANALYTICS_AUDIENCE_GRANTS` row (`:50`).
    Rewrite the `:93-100` section as a whole: it is titled "API keys for ingestion only, OIDC for
    analytics" and its body is just the two exports (`:96-97`), so deleting the
    `MICROMEGAS_INGESTION_API_KEYS` line alone would leave a heading promising ingestion-key setup
    with no content under it. Retitle it (e.g. "Ingestion keys via the database, OIDC for
    analytics"), point the ingestion half at populating the `ingestion_api_keys` table with
    `micromegas-import-keys` instead of exporting a variable, and drop the API-keys half of `:100`'s
    prefix-fallback sentence (the OIDC half stays). The "One prefix asymmetry" note (`:59-67`) stays —
    its surviving point is that `MICROMEGAS_DEFAULT_AUDIENCE`, the self-service knobs, and
    `MICROMEGAS_PUBLIC_VIEW_SETS` are read unprefixed, and both `monolith.md:53`'s table row and
    `ingestion.md:31` still point at it — but swap its prefixed counterexample from
    `MICROMEGAS_INGESTION_API_KEYS` to `MICROMEGAS_INGESTION_OIDC_CONFIG`, which still resolves with
    prefix fallback in `resolve_prefixed_var` after this change.
22. **`mkdocs/docs/admin/object-cache.md`** — no removals; add one sentence stating this keyring is
    unaffected and permanent, and that a deployment sharing one environment across roles will see
    the other roles log a "no longer read" warning about the same variable name.
23. **`mkdocs/docs/otlp/index.md`** — `:40` and `:47` drop the keyring, along with the two-line
    comment introducing `:47` ("# Server side — mint a key (see admin/api-keys.md), or use the
    transitional / # env keyring telemetry-ingestion-srv also accepts:", `:45-46`), which otherwise
    dangles with nothing left to introduce; `:517`, `:647`, `:708` drop
    the "or, transitionally, a value from `MICROMEGAS_API_KEYS`" clauses. `:105` ("A credential with
    no bound audience (an env-keyring key, OIDC, or no auth provider at all) resolves…") drops the
    env-keyring arm, the same sentence shape as `authorization.md`'s "Audience stamping" fix above.
24. **`mkdocs/docs/grafana/authentication.md`** — delete the whole "Quick Setup (env-var keyring)"
    section (`:37-53`). The DB-backed recipe immediately above it already covers token auth. Rewrite
    the section intro at `:20-22` ("flight-sql accepts keys from two sources: a DB-backed
    `analytics_api_keys` table, or a static env-var keyring. Either source alone is sufficient"),
    which otherwise still advertises the deleted keyring, to name the `analytics_api_keys` table as
    the single key source.
25. **`mkdocs/docs/admin/functions-reference.md:478`** — drop the `MICROMEGAS_AUDIENCE_GRANTS` clause
    from `list_audience_grants()`'s visibility note.
25a. **`analytics-web-app/src/routes/AudienceAccessPage.tsx`** — delete the "**Env-map grants:**"
    paragraph (`:621-625`) naming `MICROMEGAS_AUDIENCE_GRANTS`; reword the "No read grants" empty-state
    text (`:776-780`) to name only the per-key `read_audiences` list.
26. **`docker/README.md`** — drop the `MICROMEGAS_API_KEYS` row from the Ingestion Server table
    (`:193`). The only `-e MICROMEGAS_API_KEYS` in the file is the object-cache `docker run` (`:132`),
    which stays along with its table row (`:207`); no other run block passes the variable.
27. **`python/micromegas/micromegas/cli/import_keys.py`** — no behavior change; the tool reads these
    variables as its migration *source*, which is the point. Update the `DEFAULT_VAR` (`:26-30`) and
    `FALLBACK_VAR` (`:32-42`) comments and `read_keyring`'s docstring (`:57-62`): they currently
    justify the names and the fallback by "the same names `ProviderBuilder` reads" / "mirroring
    `ProviderBuilder`'s convention". Reword to "the legacy server-side names, no longer read by any
    server — kept here because that is what an un-migrated deployment still has set."
    **`python/micromegas/tests/cli/test_import_keys.py`** — comments only, same reword. The
    fallback-path comment at `:89-91` ("exercises the fallback-to-unprefixed path, which is exactly
    what a split deployment's `telemetry-ingestion-srv` (built with `ProviderBuilder::new("")`)
    needs"), `test_read_keyring_uses_ingestion_default_var_when_prefixed_is_set`'s docstring at
    `:109-111` ("as the monolith's ingestion-role `ProviderBuilder` would populate it"), and the
    regression-test docstring at `:125-131` ("`flight-sql-srv` builds its provider with
    `ProviderBuilder::new("")` … so the analytics keyring only ever lives in the unprefixed
    `MICROMEGAS_API_KEYS`") all justify the fallback by what a server reads; reword all three to
    "the legacy server-side names, no longer read by any server" per step 27's framing.
27a. **`python/micromegas/tests/test_otlp_e2e.py:854`** —
    `test_firehose_dev_mode_open_without_access_key`'s docstring, which says "a deployment with
    MICROMEGAS_API_KEYS configured would instead reject this same request", is rewritten: ingestion
    no longer reads that variable, so the contrast is now against a deployment with a populated
    `ingestion_api_keys` table (or OIDC).
27b. **`local_test_env/claude_code_otel.py:24-25`** — the `MICROMEGAS_INGESTION_API_KEY` doc line
    ("optional bearer token (matches an entry in MICROMEGAS_API_KEYS on the server)") is rewritten to
    point at a live `ingestion_api_keys` row instead of the removed server-side variable.
28. **`rust/analytics/tests/ownership_rewrite_config_tests.rs`** — reword the module doc's opening
    line (`:1`, ``//! Unit tests for `IsolationConfig::from_env`, modeled on
    `AudienceReadPolicy::from_env` ``), which cites the constructor this plan deletes; drop or
    re-point the comparison. `read_scope.rs:120,141` and this file's `:96` cite `MICROMEGAS_API_KEYS`
    only as a *shape* comparison ("comma-separated, not a JSON array like MICROMEGAS_API_KEYS"), which
    stays accurate — `object-cache-srv` keeps it — and needs no change; see **Untouched,
    deliberately**.
29. **`CHANGELOG.md`** — one `* **Auth:**` bullet under Unreleased covering the five removed
    variables, the startup warning, the upgrade action per role, and the two ways a skipped
    migration surfaces (`## Decisions`), plus a **Minor breaking change** clause:
    `provider`/`provider_with_prefix` removed from `micromegas_auth::default_provider`;
    `AudienceGrants::from_env` and `AudienceReadPolicy::from_env` removed from
    `micromegas_auth::policy`; both `with_store` methods take `Arc<DbAudienceGrantsSource>` instead
    of `Option<Arc<..>>`. The `warn_removed_*` functions are `pub(crate)`, so they add no published
    API surface and get no clause.
30. **File the follow-up issue** to delete the two warnings in v0.32.0, citing #1564 as the
    precedent and this issue as the shim's origin.

## Files to Modify

**Rust — core**
- `rust/auth/src/env.rs`
- `rust/auth/src/default_provider.rs`
- `rust/auth/src/db_api_key.rs` (comment only)
- `rust/auth/src/policy.rs`
- `rust/auth/src/db_audience_grants.rs` (comment only)

**Rust — wiring**
- `rust/public/src/servers/flight_sql_server.rs`
- `rust/monolith/src/main.rs`
- `rust/telemetry-ingestion-srv/src/main.rs`

**Rust — tests**
- `rust/auth/src/env.rs` — inline `#[cfg(test)] mod tests` (same file as the core change)
- `rust/auth/tests/default_provider_tests.rs`
- `rust/auth/tests/policy_tests.rs`
- `rust/auth/tests/db_audience_grants_tests.rs`
- `rust/public/tests/read_policy_threading_tests.rs`
- `rust/analytics/tests/ownership_rewrite_config_tests.rs` (comment only)

**Scripts / packaging**
- `local_test_env/ai_scripts/start_services_with_oidc.py`
- `local_test_env/ai_scripts/start_services.py` (comment only)
- `local_test_env/claude_code_otel.py` (comment only)
- `build/run_flight_container.py`
- `docker/docker-compose.monolith.yaml`
- `docker/README.md`
- `python/micromegas/micromegas/cli/import_keys.py` (comments only)
- `python/micromegas/tests/cli/test_import_keys.py` (comments only)

**Docs**
- `mkdocs/docs/admin/authentication.md`, `authorization.md`, `api-keys.md`, `ingestion.md`,
  `flight-sql.md`, `monolith.md`, `object-cache.md`, `functions-reference.md`
- `mkdocs/docs/otlp/index.md`, `mkdocs/docs/grafana/authentication.md`
- `analytics-web-app/src/routes/AudienceAccessPage.tsx`
- `rust/analytics/src/lakehouse/ownership_rewrite.rs`
- `python/micromegas/tests/test_otlp_e2e.py`
- `CHANGELOG.md`

**Untouched, deliberately**
- `rust/auth/src/api_key.rs` (`parse_key_ring`, `ApiKeyAuthProvider`, `KeyRingEntry`) —
  `object-cache-srv`'s only auth path.
- `rust/object-cache-srv/**` — no change at all.
- `rust/analytics-web-srv/**` — reads neither variable.
- `rust/analytics/src/lakehouse/read_scope.rs:120,141` and
  `rust/analytics/tests/ownership_rewrite_config_tests.rs:96` — cite `MICROMEGAS_API_KEYS` only as a
  *shape* comparison; still accurate since `object-cache-srv` keeps it.
- `rust/ingestion/src/sql_migration.rs:207,212` — historical prose about where the table's shape came
  from, in the file that owns the migration; leave as is.

## Decisions

- `{prefix}_UNSTAMPED_AUDIENCE` (issue Problem §3, follow-on bullet 3) needs no work: #1482 removed
  the variable; #1564 dropped its startup refusal, so there is no env-only audience name left for
  `try_claim_and_mint` to be blind to.
- The placeholder-grant-row prerequisite and its `read '*'` examples (issue §2, follow-on bullets 1
  and 2) need no work: neither is in `mkdocs/docs/` any more.
- `AudienceMintPolicy::from_env`, named in the issue's Remove list, does not exist. Nothing to remove.
- `AudienceGrants::parse` keeps no production caller once `from_env` is deleted (only
  `rust/auth/tests/policy_tests.rs` and `rust/auth/tests/db_audience_grants_tests.rs` call it); it
  stays as a published, test-only entry point for the documented grant-map JSON format.
- The issue's "remove the env-side loop in `resolve` and the env-side disjunct in
  `resolve_audience`" is not followed: neither is env-side. The static `grants` field, `new`, and
  both loop/disjunct stay — mint's only production source (`mint_key` fills it from a point query)
  and the read path's no-DB test seam (`AudienceReadPolicy::new(grants(json))` in
  `rust/auth/tests/policy_tests.rs`) both depend on them.
- **A still-set variable logs a `warn!`; it does not refuse startup.** User call, overriding the
  issue's "fail loudly, don't fall back silently" instruction.
- `AudienceMintPolicy::with_store` is kept even though it still has no production caller: deleting it
  and its `store` field would simplify `resolve_audience` to a single source, but that is not this
  issue's scope, and the symmetry with the read policy is deliberate.
- `MICROMEGAS_INGESTION_AUDIENCE_GRANTS` is excluded from the warning list — it was never read, so
  warning about it would tell an operator they lost a setting that never did anything.
- The warnings are called from `ProviderBuilder::compose` only, not from `analytics-web-srv`'s
  `WebServerConfig::from_cli_and_env` (which never read either variable), unlike #1564's now-deleted
  pair.
- `FlightSqlServerBuilder`'s `with_auth_provider` branch (`flight_sql_server.rs:300-322`) builds its
  own policy and store and never calls `ProviderBuilder`, so an embedder on that branch with a stale
  variable set gets no warning at all. Accepted: not every auth-enabled role passes through
  `compose`, only the ones this plan wires.
- The `warn_removed_*` functions are `pub(crate)`, not `pub`: #1564's refusals were published and
  their removal cost a breaking-change clause. A one-release shim with one in-crate caller should not
  be in the published API at all.
- `--disable-auth` skips the warning, because it skips `ProviderBuilder`. Accepted: it is a
  development-only flag and the removed variables have no effect under it either way.
- Accepted risk: a key-only, no-OIDC, empty-table deployment (the Grafana env-keyring recipe) stops
  starting — not from the warning, but from the pre-existing "no auth providers configured" bail now
  that the keyring no longer counts. The recipe's env-keyring section is deleted and the DB-backed
  one above it takes over.
- Accepted risk: an OIDC deployment that skips the migration starts and silently stops accepting
  keyring tokens. This is the failure mode the issue's refusal was aimed at; under the warning
  design the startup log line is the only in-process signal.

## Testing Strategy

Everything here is reachable by calling code with constructed inputs, so it is all no-DB unit tests
except the one existing `#[ignore]` live-DB test that needs reworking. The new and reworked no-DB
tests are enumerated in Implementation Steps 7–9; this section covers the live-DB tier and the
full-suite checks.

**Reworked, live DB (`#[ignore]`, existing)** — `provider_always_registered_authenticates_key_minted_after_build`
loses its env-keyring dependency by inserting a live row before `build()` to force `Some`, then a
second row after, and authenticating the second. It stays a live-DB test because it exists to pin the
"a key minted after startup authenticates with no restart" property against the real
`ingestion_api_keys` relation and `key_store_has_live_rows`; a lazily-connected pool or a fake store
cannot distinguish "provider registered but table empty at build time" from "provider not
registered", which is the whole assertion.

**Full-suite checks** (the set `build/rust_ci.py native` runs, minus dependency audits — no Cargo
changes): `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo machete`,
`cargo test` from `rust/` (not `-p micromegas`, which does not build the `server`-feature modules).

## Manual Verification

1. **The OIDC dev path still authenticates self-telemetry off a DB row, including the ingestion
   process's own.** With an OIDC config sourced, `python3
   local_test_env/ai_scripts/start_services_with_oidc.py`, then `micromegas-query "SELECT count(*)
   FROM processes WHERE exe LIKE '%telemetry-ingestion-srv%'" --begin 5m`. Expected: >= 1 — this
   directly observes the ingestion binary's own `insert_process`, the one call the residual race
   described in step 12 can still lose.
2. **A still-set variable warns and the service still starts.** With `MICROMEGAS_OIDC_CONFIG` set
   (so auth is configured and startup proceeds):
   `MICROMEGAS_API_KEYS='[]' MICROMEGAS_AUDIENCE_GRANTS='{}' cargo run --bin flight-sql-srv`.
   Expected: two `warn!` lines naming the two variables and their replacement CLIs, then a normal
   startup that serves queries. Detection is unit-tested; what only a real run shows is that the
   lines actually reach the operator's log at `warn` level through `#[micromegas_main]`'s tracing
   setup rather than being swallowed before the sink is up — `compose()` runs early in startup.

## Open Questions

None. The one open call — refuse or warn on a still-set variable — is settled as warn (Decisions),
which also closed the `MICROMEGAS_OBJECT_CACHE_API_KEYS` rename question that a refusal would have
raised.
