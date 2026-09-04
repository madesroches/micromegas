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

### What the issue lists that is already gone

- **`{prefix}_UNSTAMPED_AUDIENCE`** (issue Problem §3 and the third follow-on bullet) was removed
  wholesale in #1564 (`4243898da`). There is no unstamped state left to reserve or guard, so no
  work remains on that thread.
- **The placeholder-grant-row pre-flight step** and its `read '*'` examples (issue §2 and the first
  two follow-on bullets) are no longer in `mkdocs/docs/admin/authentication.md` or anywhere else in
  `mkdocs/docs/`. Once the env map is gone, `try_claim_and_mint`'s `EXISTS` check
  (`ingestion_keys.rs:666-674`) is authoritative by construction — nothing to delete, and nothing
  to fix.
- **`mkdocs/docs/admin/api-keys.md:585-590`** does not exist; the file is 521 lines. The Migration
  section is `:434-500`.

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
`merge` stay.

**Both policies keep their static `grants` field, `new(grants)`, and the corresponding loop /
disjunct in `resolve` / `resolve_audience`.** The issue asks to remove "the env-side loop in
`resolve` and the env-side disjunct in `resolve_audience`", but neither is env-side:

- On the mint path, `self.grants` is the *only* production source — `mint_key` fills it from a point
  query. Removing the disjunct would break the shipped mint flow.
- On the read path, `self.grants` is the seam that lets ~30 no-DB unit tests in
  `rust/auth/tests/policy_tests.rs` exercise `resolve` by calling
  `AudienceReadPolicy::new(grants(json))`. Removing the field would push all of them onto a live
  `DbAudienceGrantsSource`, against this project's verification-tier rule.

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

New in `rust/auth/src/env.rs`, `pub(crate)` — not published API, since this is a one-release shim
and `ProviderBuilder::compose` is the only caller:

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

`MICROMEGAS_INGESTION_AUDIENCE_GRANTS` is deliberately **not** in the list: no code ever read it, so
warning about it would tell an operator they lost a setting that never did anything.

Message shape:

```
MICROMEGAS_API_KEYS is set but no longer read -- import the keyring into
ingestion_api_keys / analytics_api_keys with `micromegas-import-keys`, then unset it
```

No object-cache caveat is needed. `object-cache-srv` never calls `ProviderBuilder`, so it never
warns; and a co-located process that shares the variable now emits one noisy log line rather than
failing to start, which is why the shared-environment problem disappears entirely under this design.

**Call site: `ProviderBuilder::compose` only** — the one startup hook every auth-enabled role passes
through, and the same site #1564's two functions used. Not `analytics-web-srv`'s
`WebServerConfig::from_cli_and_env`: unlike the admin and cache-TTL vars, neither of these five was
ever read by that binary, so a warning there would be about a variable that binary never honored.
`--disable-auth` skips `ProviderBuilder` and so skips the warning, which is correct for a
development flag. `compose` stays fallible for other reasons, but neither warning contributes a `?`.

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

### 5. Behavior change operators must be told about

A deployment authenticated **only** by the env keyring, with no OIDC and an empty key table,
previously started and served. It now fails to start via the *existing* "Authentication required but
no auth providers configured" bail — the keyring no longer counts toward `configured`, so `build()`
returns `None`. The `warn!` fires alongside it, which is what makes the cause legible: without it,
that bail names only the paths the operator isn't using. The Grafana token-auth recipe
(`mkdocs/docs/grafana/authentication.md`) is exactly this shape, which is why its env-keyring Quick
Setup section is deleted rather than annotated.

A deployment with OIDC configured **and** a still-set keyring is the case a warning buys the most:
it starts, authenticates OIDC callers, and silently stops accepting every keyring token. That is a
real partial outage a refusal would have converted into a loud one, and it is the reason the message
must name `micromegas-import-keys`, not just the dropped variable.

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
   top of `compose()`. Rewrite the provider-order doc comments on `build()` and `build_chain()` and
   the `is_empty()` `warn!` text. Delete `provider()` and `provider_with_prefix()`.
3. **`rust/auth/src/policy.rs`** — delete `AudienceGrants::from_env` and
   `AudienceReadPolicy::from_env`. Change both `with_store` signatures to take
   `Arc<DbAudienceGrantsSource>`. Update the module doc comment's opening line ("a JSON grant map
   keyed by audience name"), `AudienceGrants`'s doc comment (it currently justifies one env map "only
   because there is no store yet to split them across"), `RawAudienceGrants`'s and `parse`'s
   `{prefix}_AUDIENCE_GRANTS` references (they become "a grant map JSON document" — `parse` survives
   only as `micromegas-import`-shaped input and test input), and the two-separate-sources comments in
   `resolve` / `resolve_audience` (the static map is no longer "the env map"). `merge`'s doc comment
   loses its "they check the env map and the DB store snapshot" clause.

### Phase 2 — Wiring

4. **`rust/public/src/servers/flight_sql_server.rs`** — both `from_env("")?.with_store(Some(..))`
   sites become `default().with_store(..)`; update the `with_read_policy` doc comment (`:150-159`),
   the injected-provider branch's comment (which says "builds the same env+store-backed policy" and
   cites "a needless `from_env("")` failure mode on a malformed *unprefixed* env var"), and the bail
   message at `:337`.
5. **`rust/monolith/src/main.rs`** — same policy change at `:286`; rewrite the `:253-262` comment
   block and the `:218` bail message.
6. **`rust/telemetry-ingestion-srv/src/main.rs`** — module doc `:9-12` and bail message `:68`.

### Phase 3 — Tests

7. **`rust/auth/src/env.rs`, inline `#[cfg(test)] mod tests`** — `removed_vars_that_are_set` is
   `pub(crate)`-adjacent, so it is tested in-file, the same way `resolve_isolation_config` is in
   `flight_sql_server.rs` (#1564). Cases: each variable set individually → returned alone;
   empty-string value → still returned (pins that empty is not treated as unset); none set → empty
   vec; two set at once → both returned, in the `const` list's order. `#[serial]` with a guard that
   clears all five on drop.
8. **`rust/auth/tests/default_provider_tests.rs`** —
   - `build_chain_with_env_keys_only_authenticates` (`:455-482`) inverts: with `MICROMEGAS_API_KEYS`
     set and nothing else, `build_chain()` still returns `Ok`, and the chain **rejects** that key.
     This is the direct assertion that the keyring arm is gone — stronger than the startup-error
     check a refusal would have allowed, which could pass with the arm still present.
   - New, no DB: with `MICROMEGAS_API_KEYS` set and no key store, `build()` returns `Ok(None)` — the
     keyring no longer counts toward `configured`, which is what turns the removal into the
     existing "no auth providers configured" bail at each binary rather than a silent start.
   - `provider_always_registered_authenticates_key_minted_after_build` (`:90-132`, `#[ignore]`) used
     the env keyring to force `build()` into `Some`. Rework: insert one live key row *before*
     `build()` (so `has_live_rows` makes it `Some`), then insert a *second* key after `build()`
     returns and authenticate that one. Same property, no keyring.
   - Keep `API_KEYS_VAR` in `EnvGuard`'s clear list and the remaining
     `std::env::remove_var(API_KEYS_VAR)` calls at `:144`, `:190`, `:268`, `:328`, `:432`: a value
     leaked from another test would now make `build()` return `None` where the test expects `Some`,
     so clearing it stays load-bearing. Update the module doc comment.
9. **`rust/auth/tests/policy_tests.rs`** — delete the `{prefix}_AUDIENCE_GRANTS` env-fallback section
   (`:618-690`, three tests plus the `PREFIXED_VAR`/`UNPREFIXED_VAR` consts) and the module doc
   comment's paragraph about env mutation. Every other test in the file is unaffected.
10. **`rust/auth/tests/db_audience_grants_tests.rs`** — `with_store(Some(store))` →
    `with_store(store)` at `:81`, `:99`, `:463`, `:423`. `live_mint_policy_with_store_merges_a_store_granted_selector`'s
    doc comment references "the env-equivalent map passed to `AudienceMintPolicy::new`" — reword to
    "the static map".
11. **`rust/public/tests/read_policy_threading_tests.rs`** —
    `unconfigured_deployment_resolves_a_scope_and_query_results_are_unaffected` (`:447-457`) becomes
    `AudienceReadPolicy::new(AudienceGrants::empty())`; rename it and its doc comment to say
    "a policy with no grant source" rather than "env var unset". Its `api_key_provider` helper is
    unaffected — it constructs `ApiKeyAuthProvider` directly, which stays published.

### Phase 4 — Scripts

12. **`local_test_env/ai_scripts/start_services_with_oidc.py`** — this is the only in-repo script
    that runs a `ProviderBuilder` binary on the env keyring, so it breaks outright. Its ingestion
    server runs with auth ON and needs a credential for every `#[micromegas_main]` process's
    self-telemetry sink. Migrate to a DB row: keep `generate_local_ingestion_key()` and the
    `MICROMEGAS_INGESTION_API_KEY` sink-side export, drop the `MICROMEGAS_API_KEYS` server-side
    export, and after the ingestion server is up (it runs the schema migration, so the table exists
    only from that point) insert the row via the `docker exec teledb psql` path `local_test_env/db/utils.py`
    already uses:

    ```sql
    INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by, audience)
    VALUES (gen_random_uuid(), decode('<sha256 hex>','hex'), 'local-self-telemetry', now(), 'start_services_with_oidc', 'public')
    ```

    `key_hash` is `hashlib.sha256(key.encode()).hexdigest()` — `hash_key`
    (`rust/auth/src/db_api_key.rs:118`) is a plain SHA-256 over the whole key string. `DbApiKeyAuthProvider`
    validates against the table live, so a row inserted after startup authenticates on the next
    request with no restart; the server itself starts because `MICROMEGAS_OIDC_CONFIG` is already
    configured. Insert it before starting the remaining services so their sinks never see a 401.
13. **`local_test_env/ai_scripts/start_services.py`** — no functional change. Its
    `MICROMEGAS_API_KEYS` at `:135` is scoped to the `object-cache-srv` child env, which keeps the
    variable; the other services run `--disable-auth`. Add a one-line comment saying so, since the
    variable now means "object-cache only".
14. **`build/run_flight_container.py`** — drop `-e MICROMEGAS_API_KEYS` from the `docker run` line;
    `flight-sql-srv` no longer reads it, so passing it through only produces a startup warning in
    every container run this way.
15. **`docker/docker-compose.monolith.yaml:52-56`** — rewrite the auth comment block: the DB tables
    (and `MICROMEGAS_*_OIDC_CONFIG`) are the only options.

### Phase 5 — Docs

16. **`mkdocs/docs/admin/authentication.md`** — `:45-51` "Two flavors coexist" becomes DB-backed keys
    for ingestion/flight-sql plus the `object-cache-srv`-only keyring. Delete the "**Env keyring**"
    configuration block at `:117-131` and the `export MICROMEGAS_API_KEYS=...` line at `:305`,
    keeping the `object-cache-srv` pointer at `:685`.
17. **`mkdocs/docs/admin/authorization.md`** — delete the `MICROMEGAS_AUDIENCE_GRANTS` row from the
    env table (`:20`) and the whole "Deprecated: the env grant map" section (`:99-128`). Check for
    inbound anchor links to `#deprecated-the-env-grant-map` and remove them (`flight-sql.md:31`,
    `monolith.md:50`). In "Audience stamping" (`:139`), drop "env-keyring key" from the
    no-bound-audience list, leaving OIDC token and no-auth-provider.
18. **`mkdocs/docs/admin/api-keys.md`** — `:4` intro drops the "or in `MICROMEGAS_API_KEYS`"
    alternative. `:191` drops the deprecated-env-map clause. `:233` (data ingested through the env
    keyring carries no audience) is deleted. `:359` loses `MICROMEGAS_API_KEYS` from its
    "same convention" list. Rewrite the "Migrating from the env keyring" section (`:434-500`): it is
    now a **v0.31.0 upgrade requirement, not an option** — step 1's "nothing changes yet: the env
    keyring still authenticates every existing key" is false, and step 3's "remove
    `MICROMEGAS_API_KEYS`" becomes "unset it; leaving it set logs a warning and changes nothing".
    Add the consequence of *skipping* the migration, since a warning does not stop a deployment
    from getting there: with OIDC configured the service starts and every keyring token stops
    working; without OIDC and with an empty table it does not start at all. Keep the
    `micromegas-import-keys` recipe and the three `--only`/`--exclude` routing rules verbatim; keep
    the object-cache-forever bullet.
19. **`mkdocs/docs/admin/ingestion.md`** — drop the `MICROMEGAS_API_KEYS` table row (`:29`), the
    `export` example (`:58`), and the keyring arm of the "If none of ..." sentence (`:50`); `:89`'s
    no-bound-audience note drops env-keyring keys.
20. **`mkdocs/docs/admin/flight-sql.md`** — drop the `MICROMEGAS_API_KEYS` (`:28`) and
    `MICROMEGAS_AUDIENCE_GRANTS` (`:31`) rows and the keyring arm at `:56`. `:72`'s
    "An API-key (`MICROMEGAS_API_KEYS`) caller" becomes an `analytics_api_keys` caller.
21. **`mkdocs/docs/admin/monolith.md`** — drop the `MICROMEGAS_ANALYTICS_AUDIENCE_GRANTS` row (`:50`),
    the `MICROMEGAS_INGESTION_API_KEYS` note (`:61`) and example (`:96`), and the API-keys half of
    `:100`'s prefix-fallback sentence (the OIDC half stays).
22. **`mkdocs/docs/admin/object-cache.md`** — no removals; add one sentence stating this keyring is
    unaffected and permanent, and that a deployment sharing one environment across roles will see
    the other roles log a "no longer read" warning about the same variable name.
23. **`mkdocs/docs/otlp/index.md`** — `:40` and `:47` drop the keyring; `:517`, `:647`, `:708` drop
    the "or, transitionally, a value from `MICROMEGAS_API_KEYS`" clauses.
24. **`mkdocs/docs/grafana/authentication.md`** — delete the whole "Quick Setup (env-var keyring)"
    section (`:37-53`). The DB-backed recipe immediately above it already covers token auth.
25. **`mkdocs/docs/admin/functions-reference.md:478`** — drop the `MICROMEGAS_AUDIENCE_GRANTS` clause
    from `list_audience_grants()`'s visibility note.
26. **`docker/README.md`** — drop the `MICROMEGAS_API_KEYS` row from the Ingestion Server table
    (`:193`). The only `-e MICROMEGAS_API_KEYS` in the file is the object-cache `docker run` (`:132`),
    which stays along with its table row (`:207`); no other run block passes the variable.
27. **`python/micromegas/micromegas/cli/import_keys.py`** — no behavior change; the tool reads these
    variables as its migration *source*, which is the point. Update the `DEFAULT_VAR` (`:26-30`) and
    `FALLBACK_VAR` (`:32-42`) comments and `read_keyring`'s docstring (`:57-62`): they currently
    justify the names and the fallback by "the same names `ProviderBuilder` reads" / "mirroring
    `ProviderBuilder`'s convention". Reword to "the legacy server-side names, no longer read by any
    server — kept here because that is what an un-migrated deployment still has set."
28. **`rust/analytics/src/lakehouse/read_scope.rs:120,141`** and
    **`rust/analytics/tests/ownership_rewrite_config_tests.rs:96`** cite `MICROMEGAS_API_KEYS` only
    as a *shape* comparison ("comma-separated, not a JSON array like MICROMEGAS_API_KEYS"). Still
    accurate — `object-cache-srv` keeps it. No change.
29. **`CHANGELOG.md`** — one `* **Auth:**` bullet under Unreleased covering the five removed
    variables, the startup warning, the upgrade action per role, and the two ways a skipped
    migration surfaces (§5 of Design), plus a **Minor breaking change** clause:
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
- `rust/auth/src/policy.rs`

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

**Scripts / packaging**
- `local_test_env/ai_scripts/start_services_with_oidc.py`
- `local_test_env/ai_scripts/start_services.py` (comment only)
- `build/run_flight_container.py`
- `docker/docker-compose.monolith.yaml`
- `docker/README.md`
- `python/micromegas/micromegas/cli/import_keys.py` (comments only)

**Docs**
- `mkdocs/docs/admin/authentication.md`, `authorization.md`, `api-keys.md`, `ingestion.md`,
  `flight-sql.md`, `monolith.md`, `object-cache.md`, `functions-reference.md`
- `mkdocs/docs/otlp/index.md`, `mkdocs/docs/grafana/authentication.md`
- `CHANGELOG.md`

**Untouched, deliberately**
- `rust/auth/src/api_key.rs` (`parse_key_ring`, `ApiKeyAuthProvider`, `KeyRingEntry`) —
  `object-cache-srv`'s only auth path.
- `rust/object-cache-srv/**` — no change at all.
- `rust/analytics-web-srv/**` — reads neither variable.
- `rust/ingestion/src/sql_migration.rs:207,212` — historical prose about where the table's shape came
  from, in the file that owns the migration; leave as is.

## Trade-offs

- **Warn vs. refuse vs. silently ignore a set variable.** The issue asks for a refusal ("fail
  loudly, don't fall back silently"); #1564 went the other way, deleting 11 refusals as
  one-release-old shims and choosing silent-ignore for the `MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS`
  form it dropped. **Warning is the settled middle** (user call, recorded in Decisions): the operator
  gets the named replacement at startup, and no deployment loses a process over a stale variable a
  config template left behind. The case a refusal would have caught more forcefully — OIDC
  configured plus a still-set keyring, where keyring tokens silently stop working — is now covered
  by the message text and by `admin/api-keys.md`'s rewritten migration section rather than by a
  crash. #1564's precedent still governs the *lifetime*: v0.31.0 shims, deleted in v0.32.0.
- **`MICROMEGAS_API_KEYS` stays the name `object-cache-srv` requires.** Under a refusal this was a
  real hazard (a shared compose `env_file` or k8s `envFrom` would take down every other role) and
  the plan carried a caveat in the message plus a follow-up issue to rename the knob to
  `MICROMEGAS_OBJECT_CACHE_API_KEYS`. Warning instead removes the hazard: the co-located process
  logs one line and serves. The rename is therefore not proposed at all — it would be an
  operator-facing break bought for nothing.
- **Keeping the static `grants` field on both policies** rather than making the store the sole
  source, as the issue's wording implies. Dropping it would break the shipped mint path outright and
  would push ~30 no-DB `resolve` unit tests onto a live DB. The compiler-enumeration goal the issue
  actually wants is met by the `Option<Arc<_>>` → `Arc<_>` change on `with_store`.
- **Keeping `AudienceMintPolicy::with_store`**, which still has no production caller. Deleting it and
  its `store` field would simplify `resolve_audience` to a single source, but it is not this issue's
  scope and the symmetry with the read policy is deliberate and documented.
- **Deleting `provider`/`provider_with_prefix` vs. keeping them as OIDC-only wrappers.** Kept, they
  are a callerless two-line wrapper whose doc comments advertise a removed variable — pure
  maintenance surface. `micromegas-auth`'s Rust API is explicitly changeable (CLAUDE.md).

## Decisions

- `{prefix}_UNSTAMPED_AUDIENCE` (issue Problem §3, follow-on bullet 3) needs no work: #1564 removed
  the variable entirely, so there is no env-only audience name left for `try_claim_and_mint` to be
  blind to.
- The placeholder-grant-row prerequisite and its `read '*'` examples (issue §2, follow-on bullets 1
  and 2) need no work: neither is in `mkdocs/docs/` any more.
- `AudienceMintPolicy::from_env`, named in the issue's Remove list, does not exist. Nothing to remove.
- **A still-set variable logs a `warn!`; it does not refuse startup.** User call, overriding the
  issue's "fail loudly, don't fall back silently" instruction. See Trade-offs for what this gives up.
- `MICROMEGAS_INGESTION_AUDIENCE_GRANTS` is excluded from the warning list — it was never read, so
  warning about it would tell an operator they lost a setting that never did anything.
- The warnings are called from `ProviderBuilder::compose` only, not from `analytics-web-srv`'s
  `WebServerConfig::from_cli_and_env` (which never read either variable), unlike #1564's now-deleted
  pair.
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

## Documentation

Ten `mkdocs/docs/` pages plus `docker/README.md` and `CHANGELOG.md` — enumerated per-file with line
anchors in Phase 5 above. The load-bearing rewrite is `admin/api-keys.md`'s "Migrating from the env
keyring" section, which changes from an optional migration to a required v0.31.0 upgrade step, and
`admin/authorization.md`, which loses its "Deprecated: the env grant map" section and every inbound
anchor to it.

## Testing Strategy

Everything here is reachable by calling code with constructed inputs, so it is all no-DB unit tests
except the two existing `#[ignore]` live-DB tests that need reworking.

**New, no DB (`rust/auth/src/env.rs`, inline `mod tests`)** — `removed_vars_that_are_set`, the pure
half of the warning: each variable set individually → returned alone; empty-string value → still
returned (pins that empty is not an opt-out); none set → empty; two set → both, in list order.
`#[serial]` with a guard clearing all five. The `warn!` wrappers themselves are one call each with no
branching and are not tested — the alternative, capturing a log sink, would assert the logging
framework rather than this change.

**New, no DB (`rust/auth/tests/default_provider_tests.rs`)** — two tests, replacing
`build_chain_with_env_keys_only_authenticates`, both with `MICROMEGAS_API_KEYS` set and nothing else:

- `build_chain()` returns `Ok` and the resulting chain **rejects** that key. This is the direct
  assertion that the keyring arm is gone. Note it is a *stronger* check than the refusal design
  allowed: an `Err`-on-startup assertion would pass even if the keyring arm were still present
  behind the warning.
- `build()` returns `Ok(None)`. Pins that the keyring no longer counts toward `configured`, which is
  the mechanism behind §5's behavior change — a set keyring alone can no longer start a service.

**Deleted** — `policy_tests.rs`'s three `{prefix}_AUDIENCE_GRANTS` env-fallback tests (`:618-690`),
which test a removed code path.

**Reworked, live DB (`#[ignore]`, existing)** — `provider_always_registered_authenticates_key_minted_after_build`
loses its env-keyring dependency by inserting a live row before `build()` to force `Some`, then a
second row after, and authenticating the second. It stays a live-DB test because it exists to pin the
"a key minted after startup authenticates with no restart" property against the real
`ingestion_api_keys` relation and `key_store_has_live_rows`; a lazily-connected pool or a fake store
cannot distinguish "provider registered but table empty at build time" from "provider not
registered", which is the whole assertion.

**Unchanged and still meaningful** — every `AudienceReadPolicy::new(grants(json))` /
`AudienceMintPolicy::new(grants(json))` test in `policy_tests.rs`. The static map they exercise is
still the shipped mint path's only source.

**Full-suite checks** (the set `build/rust_ci.py native` runs, minus dependency audits — no Cargo
changes): `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo machete`,
`cargo test` from `rust/` (not `-p micromegas`, which does not build the `server`-feature modules).

## Manual Verification

1. **Split and monolith still start with the variables unset.**
   `python3 local_test_env/ai_scripts/start_services.py`, then
   `micromegas-query "SELECT count(*) FROM log_entries" --begin 1h`; repeat with `--monolith`.
   Expected: both come up, query returns rows, and `/tmp/object_cache.log` shows the object cache
   authenticating — the one binary whose keyring survives. Not automated: this is end-to-end process
   wiring across four binaries, and a failure would be immediately obvious to anyone running the
   script.
2. **The OIDC dev path still authenticates self-telemetry off a DB row.** With an OIDC config
   sourced, `python3 local_test_env/ai_scripts/start_services_with_oidc.py`, then
   `micromegas-query "SELECT count(*) FROM log_entries" --begin 5m`. Expected: non-zero, and
   `/tmp/ingestion.log` shows no 401s. This is the only step that exercises step 12's insert-after-
   startup ordering against a real migrated table; the SHA-256 agreement between the Python insert
   and Rust's `hash_key` has no in-repo test that spans both languages.
3. **A still-set variable warns and the service still starts.** With `MICROMEGAS_OIDC_CONFIG` set
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
