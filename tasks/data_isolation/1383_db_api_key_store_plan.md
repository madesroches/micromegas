# DB-backed API Key Store + Key-Management API Plan (#1383)

## Overview

Move API keys out of `MICROMEGAS_API_KEYS` (plaintext JSON in an env var, parsed once at startup)
into two Postgres tables — `ingestion_api_keys` and `analytics_api_keys` — holding only a SHA-256
hash of each key, plus a `created_at` / `last_used_at` / `revoked_at` audit trail. A new
`DbApiKeyAuthProvider` validates by hash-indexed lookup behind a short-TTL `moka` cache, and three
OIDC-authenticated, admin-gated HTTP routes on the ingestion service (`POST`/`GET`/`DELETE
/auth/api_keys`) let an operator mint, list and revoke keys without a redeploy. A python tool
converts existing env keyrings into rows so current key strings keep working.

This is **Stage 0** of the AbAC rollout (`tasks/data_isolation/audience_based_access_control_plan.md`
§"Stage 0"), the one stage that depends on neither the policy seam nor any isolation config and the
only one that ships value on its own. The `audience` column and everything that reads it stay out of
scope (#1372).

Two things this plan settles that the umbrella plan left implicit, both called out below rather than
buried: the tables need a **`key_id`** column (the umbrella schema has only `key_hash` as PK, but
`DELETE /auth/api_keys/<id>` and a `GET` that never returns the hash both need a non-secret handle),
and the "**Postgres grants enforce the split, not application logic**" claim is *not* true of a
deployment as shipped today — every service shares one DB role via
`MICROMEGAS_SQL_CONNECTION_STRING`. The split is enforced in code here and documented as a
grant recipe for operators who separate roles (see [Security](#security)).

## Current State

### The env keyring

- `rust/auth/src/api_key.rs` — `parse_key_ring(json) -> HashMap<Key, String>` (key → name) and
  `ApiKeyAuthProvider`. `validate_request` (lines 106–130) compares the bearer token against
  **every** key with `subtle::ConstantTimeEq` and deliberately never early-exits. Correct for a
  handful of team keys; O(N) per request. Every context it produces is `is_admin: false` (line 124)
  and `allow_delegation: true` (line 126).
- `rust/auth/src/default_provider.rs:51` — `provider_with_prefix(prefix)` is a pure env-var factory:
  resolves `{prefix}_API_KEYS` / `{prefix}_OIDC_CONFIG` / `{prefix}_ADMINS` with fallback to the
  unprefixed names, builds an `ApiKeyAuthProvider` and/or `OidcAuthProvider`, composes them in a
  `MultiAuthProvider`, and returns `Ok(None)` when neither is configured. `provider()` is
  `provider_with_prefix("")`.
- Three construction sites:
  - `rust/telemetry-ingestion-srv/src/main.rs:51` — `provider()` (unprefixed).
  - `rust/public/src/servers/flight_sql_server.rs:226` — `provider()` (unprefixed), inside
    `build_and_serve`, after the lakehouse is resolved.
  - `rust/monolith/src/main.rs:193,207` — `provider_with_prefix("MICROMEGAS_INGESTION")` and
    `("MICROMEGAS_ANALYTICS")`.
- A fourth key-validating surface, `rust/object-cache-srv/src/cli.rs:59`, reads `MICROMEGAS_API_KEYS`
  directly and never calls `default_provider`. It has no `sqlx` dependency and no DB connection
  string. **Out of scope** (decided on the issue): it keeps the env keyring, so
  `ApiKeyAuthProvider` / `parse_key_ring` are permanent, not transitional.

Consequence of the two unprefixed call sites: in a split deployment, *every key in
`MICROMEGAS_API_KEYS` is currently valid on both ingestion and flight-sql.*

### Schema and migrations

`rust/ingestion/src/sql_migration.rs` — `LATEST_DATA_LAKE_SCHEMA_VERSION = 4`; `execute_migration`
creates the v1 schema via `sql_telemetry_db.rs::create_tables` when the version reads 0, then applies
`upgrade_data_lake_schema_v2/v3/v4` in sequence and asserts the final version. New tables therefore
belong **only** in a new step, never in `create_tables` (same shape as v4, which adds
`streams.format` even though `create_tables` creates `streams`). **v5 is not exclusively claimed**:
`tasks/1245_partition_blocks_by_insert_time/plan.md:550,577` also reserves v5 for the
blocks-partitioning work (plan committed 2026-07-15, not yet implemented). **Hard prerequisite: this
plan claims v5 and must merge and deploy before `tasks/1245`'s schema work lands.** Coordinate with
that plan's owner if it is close to landing. **This coordination is two-sided, not just an ordering
question, and merging first does not by itself protect this migration**: `tasks/1245`'s plan
(`tasks/1245_partition_blocks_by_insert_time/plan.md:600-620`) changes `create_migration_table` to
stamp a fresh install directly to `LATEST_DATA_LAKE_SCHEMA_VERSION` rather than 1, which skips *every*
numbered `if N == current_version` arm on a fresh install — including this plan's v5 arm — once it
lands, regardless of which plan merged first. So whichever plan lands second, `tasks/1245`'s
fresh-install path (`create_tables` / `create_migration_table`) must also be updated to create
`ingestion_api_keys` / `analytics_api_keys`, or the numbered-arm chain must be preserved for fresh
installs — this is a coordination item to raise with #1245's owner, not a fact this plan can guarantee
by merging first. Separately, if `tasks/1245` lands first, a same-numbered v5 migration added
afterward would never run on a new database either way, so this plan must additionally be re-versioned
as a deliberate follow-up edit against whatever version is then latest. See Implementation Steps
Phase 1 step 1 for the version this migration claims, and for the caveat this implies for
`create_tables`.

`connect_to_remote_data_lake` (`rust/ingestion/src/remote_data_lake.rs:45`) runs `execute_migration`
under a DB lock (the `execute_migration` call at `:34` runs under the advisory lock taken at `:29`),
so both ingestion binaries and the monolith reach the tables before serving.
`DataLakeConnection` (`rust/ingestion/src/data_lake_connection.rs`) exposes `pub db_pool: PgPool`.

Standalone `flight-sql-srv` does **not** go through this path: it builds its lakehouse via
`LakehouseContext::from_env()` (`rust/analytics/src/lakehouse/lakehouse_context.rs:48-58`), which calls
`connect_to_data_lake` + `migrate_lakehouse` only — never `execute_migration`. Its own doc comment on
`from_connection` says the caller is responsible for running `migrate_db` (the ingestion schema) first.
This is a real deployment ordering constraint in a split deployment: an ingestion binary or the
monolith must reach migration v5 before flight-sql is deployed/rolled, or flight-sql's own startup
existence query (§3) fails at startup, naming the missing table — that failure *is* the startup
signal Migration step 1 relies on, not a silent gap. Stated explicitly in [Migration](#migration)
step 1.

### HTTP plumbing

- `rust/public/src/servers/ingestion.rs::serve_ingestion` builds a `health_router` (unauthenticated),
  a `protected_app` (ingestion + OTLP + webhook routes) wrapped in
  `micromegas_auth::axum::auth_middleware` **only when `auth_provider.is_some()`**, and two Firehose
  routers. The Firehose routers do not carry a separate keyring: `firehose_auth_middleware`
  (`rust/public/src/servers/firehose_common.rs:70-110`) synthesizes
  `Authorization: Bearer <X-Amz-Firehose-Access-Key>` and validates it through the *same*
  `Arc<dyn AuthProvider>` that `serve_ingestion` passes to the other two routers
  (`ingestion.rs:131-132,154-156`) — i.e. the same `MultiAuthProvider` chain from §3. A DB-backed
  `ingestion_api_keys` row therefore works as a Firehose access key exactly like an env key does today.
- `auth_middleware` (`rust/auth/src/axum.rs:39`) validates, strips client-supplied `x-auth-*`
  headers, and inserts the `AuthContext` into request extensions. When auth is disabled, **no
  extension is inserted** — an `Extension<AuthContext>` extractor would 500.
- Admin-gated HTTP precedent: `rust/analytics-web-srv/src/data_sources.rs:77` — `require_admin(&user)
  -> Err(Forbidden)`, checked at the top of each handler, with a typed error implementing
  `IntoResponse`.
- `moka` cache precedent: `rust/auth/src/oidc.rs:375` — `Cache::builder().max_capacity(n)
  .time_to_live(Duration::from_secs(ttl)).build()`.
- Route-level test precedent without a live DB: `rust/public/tests/firehose_tests.rs:31` —
  `sqlx::PgPool::connect_lazy("postgres://localhost/unused")` + `tower::ServiceExt::oneshot`.

### Docs and dead code

- `mkdocs/docs/admin/authentication.md` (680 lines) is the reference for both methods; it currently
  describes API keys as "Legacy" with "no automatic expiration", "manual key distribution and
  rotation", "no user identity for audit logging".
- `mkdocs/docs/admin/ingestion.md:25-58`, `admin/monolith.md`, `admin/flight-sql.md`,
  `admin/object-cache.md`, `otlp/index.md`, `docker/README.md` all document `MICROMEGAS_API_KEYS`.
- `rust/public/src/servers/key_ring.rs` is an unreferenced subset duplicate of the `api_key.rs`
  keyring half (types + `parse_key_ring`, no provider; it additionally logs key names at `info!`),
  exported from `servers/mod.rs:57` and otherwise unreferenced — dead code internally. **It is,
  however, published API**: `rust/public/src/lib.rs:171-172` declares `pub mod servers` behind the
  `server` feature, `servers/mod.rs:57` declares `pub mod key_ring`, and `rust/public/Cargo.toml` has
  no `publish = false` — so `micromegas::servers::key_ring::{Key, KeyRingEntry, parse_key_ring}` is
  reachable by any external consumer of the published `micromegas` crate. Deleting it is still the
  right call (it duplicates `api_key.rs` and has no caller), but it is a public-API removal, not an
  internal cleanup, and is recorded as such in [Files to Modify](#files-to-modify) and the
  `CHANGELOG.md` bullet (see [Documentation](#documentation)).

## Design

### 1. Schema (migration v5)

```sql
CREATE TABLE ingestion_api_keys (
  key_id       UUID PRIMARY KEY,
  key_hash     BYTEA NOT NULL,          -- sha256 of the full key string, 32 bytes
  name         VARCHAR(255) NOT NULL,
  created_at   TIMESTAMPTZ NOT NULL,
  created_by   VARCHAR(255) NOT NULL,   -- OIDC email/subject of the minter, or 'import'
  last_used_at TIMESTAMPTZ,
  revoked_at   TIMESTAMPTZ,
  revoked_by   VARCHAR(255)
);
CREATE UNIQUE INDEX ingestion_api_keys_key_hash ON ingestion_api_keys(key_hash);
-- analytics_api_keys: identical, and it never gains the #1372 audience column
CREATE TABLE analytics_api_keys ( ... same ... );
CREATE UNIQUE INDEX analytics_api_keys_key_hash ON analytics_api_keys(key_hash);
```

- **`key_id` is the design change from the umbrella schema.** `DELETE /auth/api_keys/<id>` needs a
  handle, and `GET` must never hand out `key_hash` (there is no reason to distribute the lookup value
  even though it is not reversible). A UUID PK plus a unique index on `key_hash` gives both without
  making the secret-derived value the row identity.
- **Unique** on `key_hash`, not merely indexed: it gives the O(1) validation lookup *and* makes the
  import tool's `ON CONFLICT (key_hash) DO NOTHING` re-runnable.
- No cleartext column. A plaintext column would be strictly worse than the env var (backups,
  replicas, read access, query logs).
- SHA-256 with no KDF is safe **only** because these are high-entropy random keys, not passwords.
  Argon2 would be both unindexable and too slow per request. Freshly minted keys are 256 bits of
  OS entropy; imported legacy keys are whatever the operator chose, which is why the import tool
  warns on low-entropy strings (§5).
- `created_by` / `revoked_by` deliver the "no per-key audit" half of the issue's problem statement at
  the cost of two `VARCHAR` columns.
- **`name` carries no uniqueness constraint, deliberately.** Rotation-under-a-stable-name (§5,
  [Security](#security): "the migration guide says rotate") means a second live row with the same
  `name` is an expected, not exceptional, state while an old key is being phased out — both
  `POST /auth/api_keys` and the import tool's `ON CONFLICT (key_hash) DO NOTHING` will happily create
  one. The consequence is that every revoke path (the `DELETE` route, and the analytics runbook in §4)
  must key on `key_id`, never `name` — a by-name revoke during rotation would match and revoke the
  freshly minted replacement along with the row being retired.

### 2. `DbApiKeyAuthProvider` (`rust/auth/src/db_api_key.rs`, new)

```rust
/// Which key table a provider (or the management API) is bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiKeyTable { Ingestion, Analytics }

impl ApiKeyTable {
    /// Static table name. Never derived from caller input, so the SQL below is
    /// built from a `&'static str` and can never be injected into.
    pub fn table_name(self) -> &'static str { /* "ingestion_api_keys" | "analytics_api_keys" */ }
}

/// Cache and audit knobs, read from env with defaults.
pub struct DbApiKeyConfig {
    pub cache_size: u64,               // MICROMEGAS_API_KEY_CACHE_SIZE, default 10_000
    pub cache_ttl_secs: u64,           // MICROMEGAS_API_KEY_CACHE_TTL_SECONDS, default 60
    pub unknown_cache_ttl_secs: u64,   // MICROMEGAS_API_KEY_UNKNOWN_CACHE_TTL_SECONDS, default 10
    pub unknown_cache_size: u64,       // MICROMEGAS_API_KEY_UNKNOWN_CACHE_SIZE, default 10_000
}

impl DbApiKeyConfig {
    /// `from_env()` is `from_env_with_prefix("")`. `from_env_with_prefix(prefix)` resolves each of
    /// the four names above as `{prefix}_API_KEY_CACHE_*` first, falling back to the unprefixed
    /// name — the same fallback `provider_with_prefix` already uses for `{prefix}_API_KEYS` /
    /// `{prefix}_OIDC_CONFIG` / `{prefix}_ADMINS` (`default_provider.rs:53-80`). This is what lets a
    /// monolith, which runs both providers in one process, give `MICROMEGAS_INGESTION_API_KEY_CACHE_TTL_SECONDS`
    /// and `MICROMEGAS_ANALYTICS_API_KEY_CACHE_TTL_SECONDS` independent values.
    pub fn from_env() -> Self;
    pub fn from_env_with_prefix(prefix: &str) -> Self;
}

pub struct DbApiKeyAuthProvider {
    pool: PgPool,
    table: ApiKeyTable,
    /// hash -> (key_id, name) for keys known to be live. max_capacity: cache_size.
    valid: Cache<[u8; 32], Arc<KeyRow>>,
    /// hash -> () for tokens the DB answered "no such live key" for.
    /// max_capacity: unknown_cache_size, so a flood of distinct bogus tokens
    /// evicts (LRU) rather than growing without bound — moka's builder is
    /// unbounded unless `max_capacity` is set (cf. `oidc.rs:375`).
    unknown: Cache<[u8; 32], ()>,
}

/// SHA-256 of the full key string. The only place a key is hashed; the import
/// tool and the mint route both go through the same digest definition.
pub fn hash_key(key: &str) -> [u8; 32];

/// 256 bits of OS entropy, base64url-nopad, `mmk_` prefixed. `mmk_` is settled — confirmed by the
/// user — not an open default; see [Open Questions](#open-questions).
pub fn generate_key() -> String;

/// Builds a small, dedicated connection pool for key-store lookups from an
/// existing pool's connect options (`lake_pool.connect_options()` — sqlx's
/// `Pool::connect_options`, so no new connection string needs to be threaded
/// in), rather than sharing the caller's lake pool directly. `max_connections(4)`,
/// `acquire_timeout(Duration::from_secs(2))`. See [Trade-offs](#trade-offs) →
/// "A negative cache" for why the lake pool must not be shared here.
pub fn dedicated_key_store_pool(lake_pool: &PgPool) -> PgPool;
```

`validate_request`:

1. `bearer_token()` or bail.
2. `hash_key(token)`.
3. `unknown` cache hit ⇒ bail without touching the DB (a *repeated* probe of the same bogus token is
   free after the first attempt — see [Trade-offs](#trade-offs) for what this does and does not
   bound).
4. **`valid.try_get_with(hash, loader)`**, not a plain `get`/`insert` pair. moka 0.12.15's `Cache::get`
   and `Cache::insert` do not coalesce: a plain `get` miss followed later by an `insert` lets every
   in-flight request for a hash that just expired run its own loader concurrently. `try_get_with`
   (`Cache<[u8; 32], Arc<KeyRow>>::try_get_with`) is moka's coalescing entry API — among concurrent
   callers for the same key, exactly one runs `init` (the loader below); the rest await its result. A
   cache hit returns the cached `Arc<KeyRow>` with no loader call at all. The loader:

```sql
UPDATE <table> SET last_used_at = now()
WHERE key_hash = $1 AND revoked_at IS NULL
RETURNING key_id, name
```

   returns `Result<Arc<KeyRow>, LookupError>` where `LookupError` is `NotFound` or `Db(anyhow::Error)`:
   `Some(row)` ⇒ `Ok(Arc::new(row))`, which `try_get_with` inserts into `valid` and returns; `None` ⇒
   `Err(LookupError::NotFound)`. On `Ok`, build the `AuthContext` and return. On
   `Err(LookupError::NotFound)`, insert into `unknown` (outside the `try_get_with` call itself, since a
   "no such live key" result is a legitimate outcome for the `unknown` cache, not a failure to retry)
   and bail "invalid API token".
5. **A DB *error* is propagated and cached in neither map.** `try_get_with` never inserts into `valid`
   on an `Err` from the loader — so an `Err(LookupError::Db(e))` populates neither `valid` nor
   `unknown`, exactly like the `NotFound` arm above populates only `unknown`, never `valid`. Caching an
   outage as `unknown` would
   turn a transient failure into a TTL-long outage for every affected key. The provider itself logs
   the error at `error!` (with the table name) before returning it, because the composition layer does
   not: `MultiAuthProvider` (`rust/auth/src/multi.rs:83-91`) catches every provider error and logs it
   at `debug!`. **This DB error is also wrapped in a new `micromegas_auth::types::ProviderUnavailable`
   (a `thiserror` newtype around the `anyhow::Error`, per `rust/CLAUDE.md`'s rule that a typed error is
   warranted when a caller must branch on error kind) before being returned**, so that a store outage
   is distinguishable from a rejected credential all the way out to the HTTP response. Without this,
   `auth_middleware` (`rust/auth/src/axum.rs:39-58`) maps *any* provider error to `AuthError::InvalidToken`
   → **401**, and `rust/telemetry-sink/src/http_event_sink.rs:525-534` classifies every `400..=499` as
   `IngestionClientError::Permanent` (no retry) — so once the 60s positive-cache entries age out, a
   Postgres outage would 401 every authenticated ingestion request and every client sink would drop its
   batch permanently instead of retrying. `MultiAuthProvider::validate_request` is changed to track
   whether any provider in the chain failed with `ProviderUnavailable` (checked via
   `anyhow::Error::downcast_ref`); if the loop exhausts every provider with no success, it re-raises
   `ProviderUnavailable` when at least one provider saw it, instead of the generic "authentication
   failed with all providers" — a request that might have matched a live DB key had the store been
   reachable must not be treated as a confirmed rejection. `auth_middleware` in turn downcasts the
   error it receives: `ProviderUnavailable` maps to a new `AuthError::Unavailable` →
   **503 Service Unavailable** (retryable per `http_event_sink.rs`'s `500..=599` classification);
   every other error keeps mapping to `AuthError::InvalidToken` → 401. **The same downcast-and-remap is
   applied at the two other surfaces a DB-backed provider reaches**, not only plain axum ingestion
   routes: `rust/auth/src/tower.rs`'s gRPC `AuthService` — the path `flight_sql_server.rs` wraps around
   the analytics provider, which §3 attaches `ApiKeyTable::Analytics` to — downcasts the same way and
   maps `ProviderUnavailable` to `Status::unavailable` instead of its current single `Err(e)` arm
   (`Status::unauthenticated("invalid token")`); and `rust/public/src/servers/firehose_common.rs`'s
   `firehose_auth_middleware` maps it to **503** instead of its current unconditional
   `StatusCode::UNAUTHORIZED`, since both Firehose routers validate through the same `MultiAuthProvider`
   chain as every other ingestion route (see [Current State](#current-state)) and a DB-backed
   `ingestion_api_keys` outage must not look like a rejected Firehose access key either. This is stated
   here and in [Security](#security) as the chosen behavior: an outage in the key store is
   client-visible as a retryable failure — 503, or `Status::unavailable` on the gRPC path — never a
   permanent rejection, at every surface a DB-backed provider reaches. **This `error!` is
   rate-limited to at most one line per `cache_ttl_secs` window (per table), not one per rejected
   request**: §3 puts `DbApiKeyAuthProvider` last in the chain, so during an outage every
   non-env-key, non-JWT request reaches it, and once the positive cache's entries age out that
   includes every legitimately-keyed request on the highest-volume service in the deployment —
   `error!` per request would flood `log_entries` with the outage's own noise, the same
   self-ingestion property the plan cites against the UDF approach in §4. Implementation: a small
   `AtomicI64` "last logged at" timestamp on the provider, checked-and-set before emitting. **Every DB
   error still emits `imetric!("db_api_key_error_count", "count", table_tags(self.table.table_name()),
   1_u64)` unconditionally** — the `imetric!("<name>", "<unit>", ...)` convention already used by
   `rust/public/src/servers/axum_utils.rs:30` and `rust/public/src/servers/pg_stats.rs:86-104`, with the
   table name carried as a `{table}` tag via `PropertySet::find_or_create`, the same
   `table_tags`/`index_tags` pattern `pg_stats.rs:38-42` already uses to keep one metric name shared
   across the two tables rather than one static name per `ApiKeyTable` variant — so the outage is fully
   visible in metrics, per table, even on the requests whose `error!` line was suppressed.

Design notes:

- **`last_used_at` is folded into the lookup**, so there is no second round trip, no throttling
  machinery, and no per-entry timestamp state. It is rate-limited to *once per cache TTL per key per
  process* **because the lookup goes through `valid.try_get_with`, moka's coalescing entry API, not a
  plain `get`/`insert` pair** — a plain `get` miss followed by an `insert` does not coalesce concurrent
  misses (moka 0.12.15 only coalesces through `get_with`/`try_get_with`/`optionally_get_with`), so every
  in-flight request for a hash that just expired would otherwise run its own `UPDATE`, all against the
  same row. With `try_get_with`, a burst of concurrent requests for the same live key collapses to
  exactly one loader call — one `UPDATE` a minute at the default TTL for a key seen continuously by one
  process, however many concurrent requests are in flight — and the resulting column granularity
  (±TTL) is exactly what an approximately-last-used audit column wants.
- **This stage assumes a live-key population well under `cache_size` (thousands, not tens of
  thousands).** The "once per cache TTL per key per process" guarantee above holds only while the
  working set of distinct live keys fits in the `valid` cache's `max_capacity` (default 10,000); past
  that point the cache thrashes and the `last_used_at` `UPDATE` degrades toward one Postgres write per
  authenticated request. #1374's plan to mint one ingestion key per user/machine — including game
  clients — could push the live-key count well past this stage's assumption; revisiting the
  `last_used_at` write strategy (e.g. sampling or throttling the `UPDATE`) and the cache-size/TTL
  defaults is explicitly deferred to that follow-on stage, not solved here.
- **Revocation takes effect within `cache_ttl_secs` (default 60s). This is a property, not an
  accident**: the `DELETE` route writes `revoked_at` and cannot invalidate remote caches. It is
  stated in the route's response body, in the admin docs, and asserted by a test. Raising the TTL
  trades revocation latency for DB load; 60s keeps the headline value of the stage intact. The
  codebase already has this exact knob for the other auth cache — `rust/auth/src/oidc.rs:119-143`'s
  `DEFAULT_TOKEN_CACHE_TTL_SECS = 300` for `OidcAuthProvider`, the same
  validated-credential-cached/revocation-delayed-by-TTL trade-off. 60s here is deliberately 5x tighter
  than that precedent, because bounded revocation latency is this stage's advertised deliverable; it
  stays env-overridable **per service**, via `DbApiKeyConfig::from_env_with_prefix(prefix)`
  (mirroring `provider_with_prefix`'s `{prefix}_*`-with-unprefixed-fallback convention,
  `default_provider.rs:53-80`) rather than a single unprefixed `MICROMEGAS_API_KEY_CACHE_TTL_SECONDS`
  — the monolith runs both providers in one process from one environment, so an unprefixed-only name
  would leave an operator unable to give ingestion keys a tighter revocation latency than analytics
  keys, exactly the capability this bullet advertises.
- A newly minted key is live immediately unless something already probed that exact string within
  `unknown_cache_ttl_secs` — hence a much shorter TTL on the negative cache than the positive one.
- The `AuthContext` this provider produces is identical to the env provider's: `issuer: "api_key"`,
  `auth_type: ApiKey`, `audience: None`, `is_admin: false` (API keys can *never* be admin),
  `allow_delegation: true`, `subject: name`. `#1372` adds `bound_audience` here.
- No constant-time comparison is needed or wanted: the token is never compared against a secret. It
  is hashed and the hash is used as an index key, so lookup time is independent of how many leading
  bytes of a guess are correct.
- `micromegas-auth` gains `sqlx` and `uuid` (both already workspace deps). Only `micromegas-public`
  depends on `micromegas-auth`, so this does not widen any other crate's tree, and nothing in the
  wasm build path is affected.

### 3. Provider wiring (`rust/auth/src/default_provider.rs`)

The env factory becomes a builder so that adding the DB store — and later a policy — does not
re-break the signature:

```rust
pub struct ProviderBuilder {
    prefix: String,
    key_store: Option<(PgPool, ApiKeyTable)>,
}

impl ProviderBuilder {
    pub fn new(prefix: &str) -> Self;
    pub fn with_db_key_store(self, pool: PgPool, table: ApiKeyTable) -> Self;
    pub async fn build(self) -> Result<Option<Arc<dyn AuthProvider>>>;
}

// Kept, but not because any internal caller or test still needs them — Phase 4
// migrates all four call sites (`telemetry-ingestion-srv`, `flight_sql_server.rs`,
// `monolith/src/main.rs` x2) onto `ProviderBuilder`, and `rust/auth/tests/` has no
// test referencing `default_provider` today. `provider()` / `provider_with_prefix()`
// are `pub` on the published `micromegas-auth` crate, carry a rustdoc example
// (`default_provider.rs:26-38`), and are the only documented env-only entry point for
// an external caller that wants API-key + OIDC composition without a DB pool. Kept as
// thin env-only wrappers to preserve that public API surface, not to avoid touching
// internal callers.
pub async fn provider() -> Result<Option<Arc<dyn AuthProvider>>>;
pub async fn provider_with_prefix(prefix: &str) -> Result<Option<Arc<dyn AuthProvider>>>;
```

`build()` composes, in this order: env `ApiKeyAuthProvider` (in-memory, cheapest, preserves today's
precedence) → `OidcAuthProvider` → `DbApiKeyAuthProvider`. `MultiAuthProvider` tries providers in
order (`rust/auth/src/multi.rs:84-93`), so this matters: `OidcAuthProvider::validate_jwt_token`
starts with `decode_header(token)` (`oidc.rs:430`), which fails locally and instantly for a non-JWT
API key, with no network or JWKS access — so putting it before the DB provider costs API-key
requests nothing. Putting the DB provider last means only tokens that are neither an env key nor a
valid JWT ever reach it, and it is the DB provider, not `OidcAuthProvider`'s 300s token cache
(`oidc.rs:121`), that would otherwise be the binding cost on every OIDC request if it ran first.
Env and DB compose so nothing breaks mid-migration.

**`DbApiKeyAuthProvider` is always pushed onto the chain whenever a key store is attached** —
registration never depends on the existence query below. That query feeds only the *"is anything
configured at all"* decision (the `Ok(None)` early-out described next), never whether the DB
provider itself is constructed. This matters because `MultiAuthProvider::is_empty()`
(`rust/auth/src/multi.rs:74-76`) is `providers.is_empty()`: if the DB provider were conditionally
pushed based on the existence check, a deployment that starts with an empty table (Migration step
1, or a fresh install with OIDC only) would mint its first key through `POST /auth/api_keys` and
that key would not authenticate until the process restarts — directly contradicting "grant/revoke
without a redeploy." Always registering the provider means a newly minted key is live on the next
request, cache-TTL aside, with no restart.

**A DB key store with at least one live key counts as "auth configured"; an empty one does not.**
When a key store is attached, `build()` runs one cheap startup query —
`SELECT EXISTS(SELECT 1 FROM <table> WHERE revoked_at IS NULL)` — and treats a non-empty result the
same as env keys or OIDC being present. **A failure of this query — e.g. a missing relation because
the schema has not reached v5 yet — is propagated as an error from `build()`; it is never treated as
"empty."** The error message names the table and states the migration-ordering requirement (the
ingestion binary or monolith must reach v5 before flight-sql starts, per
[Current State](#current-state)), so a pre-v5 rollout or DB-only deployment fails loudly and legibly
at startup instead of surfacing later as a confusing auth error or silently starting with no
providers configured. This is what makes Migration step 3 possible: once
`micromegas-import-api-keys` has populated the table (step 2) and `MICROMEGAS_API_KEYS` is removed,
the DB rows alone keep the service serving — no OIDC required. It also covers flight-sql specifically,
which never mints and is documented (`mkdocs/docs/grafana/authentication.md`) to run key-only for
Grafana; it must not be forced to stand up OIDC just to satisfy this check. An empty key store still
does *not* count: `build()` returns `Ok(None)` when the table has no live rows and neither env keys nor
OIDC are configured, so the startup guard in `telemetry-ingestion-srv` ("Authentication required but
no auth providers configured") still fires for a genuinely empty deployment, and an operator gets a
clear failure instead of a process that silently 401s every request. Minting itself still requires
OIDC — that is unchanged and orthogonal to whether the *table* is empty — but a deployment that only
ever revokes, or mints via the import tool / direct SQL, no longer needs OIDC merely to pass this
check.

The four call sites (in three files) each pass a **dedicated** key-store pool, built by
`db_api_key::dedicated_key_store_pool` from the lake pool's own connect options (§2) — not a clone of
the lake pool itself. See [Trade-offs](#trade-offs) → "A negative cache" for why sharing it is a
blast-radius problem this plan does not accept:

| Site | Today | After |
|---|---|---|
| `telemetry-ingestion-srv/src/main.rs:51` | `provider()` | `ProviderBuilder::new("").with_db_key_store(dedicated_key_store_pool(&data_lake.db_pool), ApiKeyTable::Ingestion).build()` |
| `public/src/servers/flight_sql_server.rs:226` | `provider()` | `ProviderBuilder::new("").with_db_key_store(dedicated_key_store_pool(&pool), ApiKeyTable::Analytics).build()` |
| `monolith/src/main.rs:193` | `provider_with_prefix("MICROMEGAS_INGESTION")` | same + `.with_db_key_store(dedicated_key_store_pool(&lake_pool), ApiKeyTable::Ingestion)` |
| `monolith/src/main.rs:207` | `provider_with_prefix("MICROMEGAS_ANALYTICS")` | same + `.with_db_key_store(dedicated_key_store_pool(&lake_pool), ApiKeyTable::Analytics)` |

The two unprefixed sites now name their table explicitly, since `""` carries no hint. In
`flight_sql_server.rs` the pool must still be cloned from `lakehouse.lake().db_pool` **before** line
213, where `lakehouse` is moved into `FlightSqlServiceImpl::new` — the same expression already appears
at line 199 — so `dedicated_key_store_pool` has a pool to read connect options from.

### 4. Key-management API (`rust/public/src/servers/api_keys.rs`, new)

```rust
/// Router for the ingestion key-management routes. Hardcodes
/// `ApiKeyTable::Ingestion`: there is no parameter an operator or a defaulting
/// bug could point at `analytics_api_keys`. `config.cache_ttl_secs` is what the
/// `DELETE` response's `effective_within_seconds` reports;
/// `serve_ingestion_with_api_key_config` takes this same `DbApiKeyConfig` as a
/// parameter and threads it in here, so
/// every caller builds it with the identical prefix it gave
/// `ProviderBuilder::with_db_key_store` (empty for `telemetry-ingestion-srv`,
/// `MICROMEGAS_INGESTION` for the monolith) and the two cannot disagree.
pub fn api_keys_router(pool: PgPool, config: DbApiKeyConfig) -> Router;
```

| Route | Body / result |
|---|---|
| `POST /auth/api_keys` | `{"name": "..."}` → **201** `{"key_id", "name", "created_at", "key"}` — the cleartext, returned exactly once. **400** if `name` is empty or exceeds 255 bytes (deliberately stricter than the `VARCHAR(255)` column, which bounds characters, not bytes). |
| `GET /auth/api_keys?limit=&offset=&include_revoked=` | **200** `[{"key_id","name","created_at","created_by","last_used_at","revoked_at","revoked_by"}]`, newest first. `limit` defaults to 100; values above 500 are silently clamped to 500 rather than rejected (a read endpoint, so capping is safer than erroring); `limit <= 0` is **400**. `offset` defaults to 0, `include_revoked` defaults to `true`. **Never `key_hash`, never the key.** |
| `DELETE /auth/api_keys/{key_id}` | **200** `{"revoked_at", "effective_within_seconds"}`, or **404** for an unknown `key_id` |

- Merged into `serve_ingestion_with_api_key_config`'s `protected_app` **before** the `auth_middleware`
  layer is applied, so it reuses the existing middleware rather than re-implementing auth. Handlers read
  `Extension<AuthContext>`.
- **Registered only when `auth_provider.is_some()`.** With `--disable-auth` there is no
  `AuthContext` in extensions and the extractor would 500; skipping registration (with a `warn!`)
  removes the hazard entirely and is correct on the merits — there is nothing to authenticate in that
  mode. Consequence: local dev needs OIDC configured for the *ingestion* server specifically.
  `local_test_env/ai_scripts/start_services_with_oidc.py` does not do this today — it copies the same
  OIDC-carrying environment to the ingestion process (line 129) but launches it with `--disable-auth`
  (line 194), so the config is present but ignored — so it must be extended (see
  [Testing Strategy](#testing-strategy)) before it can exercise these routes at all. Enabling auth on
  port 9000 also means every `#[micromegas_main]` binary's own self-telemetry POSTs to it (ingestion,
  flight-sql, and the maintenance daemon all share the OIDC-carrying `env`, but none of that config
  feeds the *sink* — `with_auth_from_env` only attaches an `Authorization` header from
  `MICROMEGAS_INGESTION_API_KEY` or the OIDC client-credentials trio) start getting 401'd, so the
  script must also provision an ingestion credential, not just drop the flag — see
  [Testing Strategy](#testing-strategy).
- **New entry point `serve_ingestion_with_api_key_config(..., api_key_config: DbApiKeyConfig)`**,
  mirroring §3's `ProviderBuilder`/`provider()` split: the pool is already reachable as `lake.db_pool`,
  cloned before `lake` moves into `WebIngestionService::new`, but the cache-TTL config is not otherwise
  derivable inside `serve_ingestion` — it has no way to learn which prefix (if any) the caller used
  when building its provider. `serve_ingestion` itself stays a thin wrapper —
  `serve_ingestion_with_api_key_config(..., DbApiKeyConfig::from_env())` — so its published signature
  on the `server`-feature crate is unchanged, the same reasoning §3 already applies to `provider()` /
  `provider_with_prefix()`. `telemetry-ingestion-srv` keeps calling `serve_ingestion` (the unprefixed
  default is correct for it); the monolith calls `serve_ingestion_with_api_key_config` directly with
  `DbApiKeyConfig::from_env_with_prefix("MICROMEGAS_INGESTION")`, since the wrapper's unprefixed default
  would be the wrong config for it. This is what keeps `effective_within_seconds` matching the TTL the
  running provider actually uses. The table is always `Ingestion` for this service. Neither call site's
  public signature breaks — only a new function is added, recorded alongside the `key_ring` removal in
  the `CHANGELOG.md` bullet (see [Documentation](#documentation)).
- Gate, checked first in every handler (`fn require_key_admin(&AuthContext) -> Result<(), ApiKeyError>`):
  1. `auth_type != AuthType::Oidc` ⇒ **403** `ApiKeyError::NotOidc` ("caller is not an OIDC identity;
     API keys cannot manage keys"). Redundant with `is_admin: false` on key contexts, but it states
     the rule directly: *no API key can manage keys.*
  2. `!is_admin` ⇒ **403** `ApiKeyError::NotAdmin` ("OIDC identity is not in the admin list"). Distinct
     variant/message from step 1 so an operator's 403 tells them which of the two conditions failed.
  - **This gate needs `MICROMEGAS_ADMINS` (or `MICROMEGAS_INGESTION_ADMINS` on the monolith, which
    falls back to `MICROMEGAS_ADMINS`, per `rust/auth/src/default_provider.rs:71-80`) populated on the
    *ingestion* service, not just OIDC configured**: `is_admin` is set only by `OidcAuthProvider`, from
    `load_admin_users(admin_var)` (`rust/auth/src/oidc.rs:264-269`), which returns `vec![]` when the
    var is unset. Without it, every `/auth/api_keys` call 403s at step 2 regardless of how correctly
    OIDC itself is configured — call this out in the admin docs (see
    [Documentation](#documentation)) rather than leaving it as a silent gap.
- `POST` validates the body first — `name` non-empty and ≤255 bytes, deliberately stricter than the
  `VARCHAR(255)` column (which bounds characters, not bytes) — returning
  **400** `ApiKeyError::BadRequest` before any hashing or DB access; only a validated request generates
  `mmk_<43 base64url chars>` from 256 bits of `OsRng`, stores `hash_key(&key)`, and logs `key_id` /
  `name` / `created_by` at info — never the key. The `mmk_` prefix makes keys recognizable to secret
  scanners; it is cosmetic to validation, since the hash covers the whole string (which is what lets
  imported legacy keys of any shape keep working).
- `DELETE` is idempotent in one statement, preserving the original revocation time:

```sql
UPDATE ingestion_api_keys
SET revoked_at = COALESCE(revoked_at, now()),
    revoked_by = COALESCE(revoked_by, $2)
WHERE key_id = $1
RETURNING revoked_at
```

  `Some` ⇒ 200, `None` ⇒ 404. `effective_within_seconds` in the response is *this process's*
  configured `cache_ttl_secs` (the `config` parameter threaded in by
  `serve_ingestion_with_api_key_config`, built with the same prefix the caller gave
  `ProviderBuilder::with_db_key_store`, so it cannot silently disagree with the provider actually
  running), so the revocation-latency property shows up where the operator is looking; the docs
  note that a fleet with mixed configuration takes the longest configured TTL.
- **Analytics keys are not mintable through this API.** They are few, manually issued (§5, or direct
  SQL by an operator with DB access), and stay out of every HTTP write path: issuing read credentials
  from the fleet-facing service is the wrong direction for the write/read asymmetry, and keeping them
  out is what confines the ingestion service's DB writes to one table. The operator procedure for the
  full lifecycle — list: `SELECT key_id, name, created_at, last_used_at, revoked_at FROM
  analytics_api_keys ORDER BY created_at DESC`, since there is no HTTP `GET` for this table and the
  import tool's `uuid.uuid4()`-generated `key_id` is otherwise never surfaced to the operator; mint:
  generate the key, its digest, and a `key_id` with an explicit, trailing-newline-safe recipe —
  `KEY="mmk_$(openssl rand -base64 48 | tr -d '=+/\n' | head -c 43)"; HASH=$(printf '%s' "$KEY" |
  sha256sum | cut -d' ' -f1); ID=$(uuidgen)` (`printf`, not `echo`, because `hash_key` covers the full
  key string and `echo` would append a newline the validator never sees, silently minting a key that
  can never authenticate; `uuidgen`, not `gen_random_uuid()`, to keep §5's no-minimum-PG-version
  property — the table has no `DEFAULT` on `key_id`, so nothing else supplies one) — then the same
  `INSERT ... ON CONFLICT (key_hash) DO NOTHING` shape the import tool emits (§5), substituting `$ID`
  for `key_id` and `$HASH` for the hash; revoke: the
  same `UPDATE ... SET revoked_at = COALESCE(...)` statement `DELETE /auth/api_keys/{key_id}` runs
  above, keyed **only** on the `key_id` the list step just produced — never on `name`, since `name` has
  no uniqueness constraint (§1) and a rotation-under-a-stable-name workflow (§5, [Security](#security))
  can leave two live rows sharing a name, where a by-name revoke would silently kill the freshly minted
  replacement along with the old row — is written up as a runbook in `mkdocs/docs/admin/api-keys.md`,
  recipe and all, rather
  than left implicit. That runbook, not the import tool, is the durable answer to "how do I revoke an
  analytics key at 2am."
- **Rejected: admin-gated lakehouse UDFs** for key management, despite the precedent in #1382. Two
  reasons, both from the umbrella plan: `flight_sql_service_impl.rs:330` logs `sql={sql:?}` at info
  and micromegas ingests its own logs, so key material in a SQL literal would land in `log_entries` —
  worse than the env var this replaces; and a write UDF would give the *read* service INSERT/revoke
  access to the key tables — a category of write beyond the column-scoped `UPDATE (last_used_at)` the
  read service is already granted by design ([Security](#security)) to fold the audit timestamp into
  the validation lookup (§2). Client-side hashing fixes the first but not the second, and the mint
  route is needed for #1374 regardless.

### 5. Import tool (`python/micromegas/micromegas/cli/import_api_keys.py`, new)

A one-shot migration for *legacy key strings* — the one thing the mint route cannot do, since it
generates fresh keys. Deletable once every deployment has run it **for `ingestion_api_keys`** — but
the by-hand `INSERT` shape it produces is exactly what the `admin/api-keys.md` runbook (§4) points
operators at for `analytics_api_keys`, which has no HTTP mint/revoke path of its own, so that statement
shape stays documented as a template even after this tool is deleted.

**It emits SQL on stdout rather than connecting to Postgres.** The repo has no python DB driver in any
`pyproject.toml`, and the established convention for python-touches-Postgres here is to shell out to
`psql` (`local_test_env/db/*.py`). Emitting SQL adds no dependency, lets the operator review exactly
what will be inserted before applying it, and keeps cleartext keys inside the tool's own process —
only hashes appear in the output.

```bash
# monolith deployment: the prefixed keyrings (MICROMEGAS_INGESTION_API_KEYS /
# MICROMEGAS_ANALYTICS_API_KEYS) route to their tables unambiguously
micromegas-import-api-keys --from-prefixed | psql "$MICROMEGAS_SQL_CONNECTION_STRING"

# split deployment: only the unprefixed MICROMEGAS_API_KEYS is ever read here
# (provider() == provider_with_prefix("") never sees the prefixed vars), so
# an explicit destination per key is required, no default; --skip excludes
# names used *exclusively* as an object-cache-srv client credential, per
# docker/README.md and admin/object-cache.md. A name that also doubles as a
# service's own MICROMEGAS_INGESTION_API_KEY (e.g. flight-sql, maintenance)
# must go to --ingestion instead, not --skip — see the guidance below.
micromegas-import-api-keys --keys-env MICROMEGAS_API_KEYS \
  --ingestion game-client,build-agent,flight-sql,maintenance \
  --analytics grafana,analyst-tools \
  --skip object-cache-client
```

- `--from-prefixed` maps `MICROMEGAS_INGESTION_API_KEYS` → `ingestion_api_keys` and
  `MICROMEGAS_ANALYTICS_API_KEYS` → `analytics_api_keys`. It reads only those two prefixed names —
  unlike `default_provider::provider_with_prefix`, it does **not** fall back to the unprefixed
  `MICROMEGAS_API_KEYS` (`rust/auth/src/default_provider.rs:53-59`), because a single unprefixed
  keyring has no table to route to on its own. A monolith deployment that relies on that fallback
  (e.g. `docker/docker-compose.monolith.yaml:52`) would otherwise get a silent no-op migration —
  note `docker/README.md:131` and `:206` are the unrelated **object-cache** keyring, permanently
  out of scope, not a monolith fallback. So when neither prefixed variable is set,
  `--from-prefixed` exits non-zero
  instead of emitting an empty `BEGIN; COMMIT;`, with a message pointing at the explicit form:
  `--keys-env MICROMEGAS_API_KEYS --ingestion ... --analytics ...`.
- Otherwise **every key name in the source keyring must be listed in exactly one of `--ingestion` /
  `--analytics` / `--skip`**; unassigned names are a non-zero exit listing them. A name in *both*
  `--ingestion` and `--analytics` is also an error, with a message explaining that a dual-use key must
  be split into two distinct key strings — see [Migration](#migration). `--skip name1,name2` is the
  escape hatch for names that must stay env-only rather than land in either table — in particular the
  `object-cache-srv` client keys (e.g. `flight-sql`, `maintenance`) that `docker/README.md:131` and
  `mkdocs/docs/admin/object-cache.md:149` document as sharing the unprefixed `MICROMEGAS_API_KEYS`
  keyring. **`--skip` is correct only for a name used *exclusively* as an object-cache client
  credential.** A name like `flight-sql` or `maintenance` is commonly *also* that service's own
  `MICROMEGAS_INGESTION_API_KEY` — the bearer token it presents to ingestion for its own self-telemetry
  (`rust/telemetry-sink/src/api_key_decorator.rs:22`, via `with_auth_from_env`) — since both env vars
  are commonly drawn from the same shared unprefixed keyring. A name that is also a service's ingestion
  self-telemetry credential must instead be passed to `--ingestion`, so it survives in
  `ingestion_api_keys`; it stays unchanged in `object-cache-srv`'s own env keyring regardless, since
  that service is validated separately and keeps reading `MICROMEGAS_API_KEYS` permanently. Skipped
  names are not validated for length/entropy and are not emitted as SQL.
- **Name validation and quoting.** Names come from operator-authored `MICROMEGAS_API_KEYS` JSON —
  `parse_key_ring` (`rust/auth/src/api_key.rs:58`) places no constraint on `name` — but they are
  interpolated into a SQL string literal that gets piped to `psql` running as the schema owner. Before
  emitting anything, every name is validated: non-empty, ≤255 bytes (deliberately stricter than the
  `VARCHAR(255)` column, which bounds characters, not bytes — this check can therefore only reject a
  name early, never let one through that the column would then reject), containing no comma (so it
  round-trips through the `--ingestion`/`--analytics` comma-separated lists), and containing no `$`
  (Postgres dollar-quoting terminates at the first repeat of the tag, so a name containing the literal
  `$mmk$` tag used below would otherwise close the quoted literal early and let the remainder of the
  name be parsed as SQL). Any violation exits non-zero naming the offending key, before any SQL is
  emitted. Every name is then emitted with Postgres dollar-quoting (`$mmk$...$mmk$`) rather than a
  single-quoted literal, so no character other than the quoting tag itself — which validation rejects
  — can ever break out of the literal or inject SQL.
- Per key: `uuid.uuid4()` in python (no `gen_random_uuid()`, so no minimum PG version),
  `hashlib.sha256(key.encode()).hexdigest()`, `created_by = 'import'`:

```sql
BEGIN;
INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by)
VALUES ('<uuid4>', decode('<hex>', 'hex'), $mmk$game-client$mmk$, now(), 'import')
ON CONFLICT (key_hash) DO NOTHING;
COMMIT;
```

- Warns on **stderr** for any key shorter than 24 characters or drawn from fewer than 16 distinct
  characters. SHA-256 without a KDF is only safe for high-entropy keys, so a low-entropy legacy key
  should be rotated rather than imported — the tool is where the operator will notice.

Registered as `micromegas-import-api-keys` in `python/micromegas/pyproject.toml`, alongside
`micromegas-query` / `micromegas-logout`.

## Implementation Steps

### Phase 1 — Schema

1. `rust/ingestion/src/sql_migration.rs`: claim v5. Add `upgrade_data_lake_schema_v5` creating both
   tables and both unique indexes, ending with `UPDATE migration SET version=5` (matching every other
   `upgrade_data_lake_schema_vN`, e.g. `rust/ingestion/src/sql_migration.rs:52,72,86`, and required
   because `execute_migration` re-reads the version in the same transaction and asserts
   `current_version == LATEST_DATA_LAKE_SCHEMA_VERSION`); bump `LATEST_DATA_LAKE_SCHEMA_VERSION` to 5;
   add the corresponding `if 4 == current_version` arm to `execute_migration`. This requires this plan
   to merge and deploy before `tasks/1245_partition_blocks_by_insert_time`'s schema work lands (see
   [Current State](#current-state) → Schema and migrations); if `tasks/1245` lands first instead, this
   plan must be re-versioned as a deliberate follow-up edit rather than claiming v5. **Do not** touch
   `sql_telemetry_db.rs::create_tables` — fresh databases reach the new version through the same
   upgrade path, **but this holds only as long as `create_migration_table` still stamps a fresh install
   to `1`**; if `tasks/1245`'s fresh-install stamping change lands (see [Current State](#current-state)
   → Schema and migrations), the fresh-install path must be revisited so new databases still get these
   two tables.

### Phase 2 — Provider

2. `rust/auth/Cargo.toml`: add `sqlx.workspace = true`, `thiserror.workspace = true`,
   `uuid.workspace = true` (alphabetical order) to `[dependencies]` — `thiserror` backs the
   `types::ProviderUnavailable` newtype (§2 step 5, Phase 2 step 3); add `serial_test = "3.2"` to
   `[dev-dependencies]`, matching the version pinned in `rust/ingestion/Cargo.toml` and the other
   crates that already use it — needed both by the `default_provider_tests.rs` bullets below that
   assert on `build()`, which reads process-wide env vars, and by the `db_api_key_tests.rs` metric test
   and its DB-error-triggering sibling tests (see [Testing Strategy](#testing-strategy)), which share
   global tracing state and the `db_api_key_error_count` metric within one test binary and so must not
   run concurrently.
3. `rust/auth/src/db_api_key.rs` (new): `ApiKeyTable`, `DbApiKeyConfig` (+ `from_env` and
   `from_env_with_prefix`, the latter mirroring `provider_with_prefix`'s `{prefix}_*`-with-fallback
   convention so a monolith can set ingestion and analytics cache TTLs independently — see §2), `hash_key`,
   `generate_key`, `dedicated_key_store_pool` (builds the small dedicated pool used at all four §3
   call sites, instead of sharing the lake pool — see [Trade-offs](#trade-offs) → "A negative cache"),
   `DbApiKeyAuthProvider` with the two `moka` caches (the `valid` lookup going through
   `try_get_with`, not a plain `get`/`insert` pair — see §2 step 4) and the
   `UPDATE ... RETURNING` lookup, wrapping every DB error in `types::ProviderUnavailable` before
   returning it and emitting `imetric!("db_api_key_error_count", "count", table_tags(...), 1_u64)`
   unconditionally (§2 step 5). Register in `rust/auth/src/lib.rs`. Also, in the same step (§2 step 5's
   store-unavailable-vs-rejected distinction):
   - `rust/auth/src/types.rs`: add `ProviderUnavailable`, a `thiserror` newtype around `anyhow::Error`
     — the one place in this crate a typed error is warranted, per `rust/CLAUDE.md`'s
     anyhow-vs-thiserror rule, since two callers below must branch on this specific kind.
   - `rust/auth/src/multi.rs`: `MultiAuthProvider::validate_request` downcasts each provider error for
     `ProviderUnavailable` and, if every provider failed but at least one failed with it, re-raises
     `ProviderUnavailable` instead of the generic "authentication failed with all providers".
   - `rust/auth/src/axum.rs`: `auth_middleware` downcasts the error from `validate_request`; a
     `ProviderUnavailable` maps to a new `AuthError::Unavailable` → **503**, everything else keeps
     mapping to `AuthError::InvalidToken` → 401.
   - `rust/auth/src/tower.rs`: the gRPC `AuthService`'s single `Err(e)` arm (currently
     `Status::unauthenticated("invalid token")` unconditionally) downcasts the same way; a
     `ProviderUnavailable` maps to `Status::unavailable`, everything else keeps the existing mapping.
4. `rust/auth/src/default_provider.rs`: add `ProviderBuilder`; `with_db_key_store` builds the attached
   provider's `DbApiKeyConfig` via `DbApiKeyConfig::from_env_with_prefix(&self.prefix)`, so the cache
   TTL a `DbApiKeyAuthProvider` actually runs with follows the same prefix its `ApiKeyAuthProvider` /
   `OidcAuthProvider` siblings already do; reimplement `provider()` /
   `provider_with_prefix()` on top of it (env-only, unchanged behavior, so the `Ok(None)` rule on
   those thin wrappers is untouched). Keep both wrappers even though every internal caller moves off
   them in Phase 4 — they stay solely as the published crate's documented env-only entry point (see
   §3's comment on the rationale), not because anything internal still calls them.
   `ProviderBuilder::build()` additionally treats a non-empty attached key store as "configured" via
   the startup existence query described in §3. Extract that query into a separately callable
   function (e.g. `key_store_has_live_rows(&PgPool, ApiKeyTable) -> Result<bool>`) so
   `rust/auth/tests/default_provider_tests.rs` (see [Testing Strategy](#testing-strategy)) can exercise
   the four §3 rules without needing full env-var isolation around `build()`.

### Phase 3 — Management API

5. `rust/public/src/servers/api_keys.rs` (new): `ApiKeyError` + `IntoResponse` (400 / 403 / 404 / 500,
   modeled on `data_sources.rs`'s `BadRequest`/`ValidationError` precedent), `require_key_admin`, the
   three handlers — `POST` validating `name` (non-empty, ≤255 bytes) and `GET` clamping/rejecting
   `limit` per the route table above — `api_keys_router(pool, config)`. Export from
   `rust/public/src/servers/mod.rs`.
6. `rust/public/src/servers/ingestion.rs`: add `serve_ingestion_with_api_key_config(..., api_key_config:
   DbApiKeyConfig)` as the new entry point, and reimplement `serve_ingestion` as a thin wrapper that
   calls it with `DbApiKeyConfig::from_env()` — mirroring §3's `ProviderBuilder`/`provider()` split, so
   `serve_ingestion`'s published signature on the `server`-feature crate is unchanged (non-breaking; see
   the `CHANGELOG.md` bullet in [Documentation](#documentation)); clone `lake.db_pool` before
   constructing `WebIngestionService`; merge
   `api_keys_router(pool, api_key_config)` into `protected_app` inside the `if let Some(provider)`
   branch, before the middleware layer; `warn!` when skipped. Also in this step:
   `rust/public/src/servers/firehose_common.rs`: `firehose_auth_middleware`'s `Err(e)` arm downcasts
   for `ProviderUnavailable` and returns 503 instead of its current unconditional
   `StatusCode::UNAUTHORIZED` (§2 step 5).
7. Delete the dead duplicate `rust/public/src/servers/key_ring.rs` and its `mod.rs:57` export.

### Phase 4 — Call sites

8. `rust/telemetry-ingestion-srv/src/main.rs`: switch to `ProviderBuilder` with
   `(dedicated_key_store_pool(&data_lake.db_pool), ApiKeyTable::Ingestion)` (§2, [Trade-offs](#trade-offs)
   → "A negative cache" — not a clone of `data_lake.db_pool` itself); its `serve_ingestion` call is unchanged —
   the wrapper's `DbApiKeyConfig::from_env()` default is already the same unprefixed config the
   provider was built with; update the module doc comment (lines 6–10) and the startup error message.
9. `rust/public/src/servers/flight_sql_server.rs`: clone the pool before `lakehouse` is moved; switch
   to `ProviderBuilder` with `(dedicated_key_store_pool(&pool), ApiKeyTable::Analytics)`. flight-sql
   runs no migration of its own (see
   [Current State](#current-state)); the deployment ordering constraint in Migration step 1 applies.
   Update the startup error message at `flight_sql_server.rs:230` ("Authentication required but no
   auth providers configured. Set MICROMEGAS_API_KEYS or MICROMEGAS_OIDC_CONFIG") to also mention the
   DB key store as a way to satisfy the check, matching step 8's treatment of telemetry-ingestion-srv.
10. `rust/monolith/src/main.rs`: both providers get `.with_db_key_store(dedicated_key_store_pool(&lake_pool), ...)`
    built from `lakehouse.lake().db_pool`, with `Ingestion` / `Analytics` respectively (§2,
    [Trade-offs](#trade-offs) → "A negative cache"). Note the pool must be
    taken while `lakehouse` is still borrowed, before the role `join_set` spawns. Update the two
    `bail!` messages at `rust/monolith/src/main.rs:196-199` (ingestion: "Set
    MICROMEGAS_INGESTION_API_KEYS, MICROMEGAS_API_KEYS, or --disable-auth") and `:210-214`
    (analytics: "Set MICROMEGAS_ANALYTICS_OIDC_CONFIG, MICROMEGAS_OIDC_CONFIG, or --disable-auth...")
    to mention the DB key store, since once §3 lands a non-empty key table also counts as "auth
    configured" and these messages would otherwise name env vars as the only remedies — misleading
    exactly during Migration step 3, which removes those env vars. Switch the `serve_ingestion` call at
    `rust/monolith/src/main.rs:246` to `serve_ingestion_with_api_key_config`, passing
    `DbApiKeyConfig::from_env_with_prefix("MICROMEGAS_INGESTION")` — the same prefix used to build the
    ingestion provider — as its `api_key_config` argument; the unprefixed default `serve_ingestion`'s
    wrapper supplies would be the wrong config for the monolith.

### Phase 5 — Import tool

11. `python/micromegas/micromegas/cli/import_api_keys.py` + the `[tool.poetry.scripts]` entry.

### Phase 6 — Tests and docs

12. Tests per [Testing Strategy](#testing-strategy), including the `rust/public/Cargo.toml`
    `[[test]]` entry for `api_keys_tests` (see [Files to Modify](#files-to-modify)).
13. Docs per [Documentation](#documentation).

## Files to Modify

**New**

- `rust/auth/src/db_api_key.rs`
- `rust/public/src/servers/api_keys.rs`
- `rust/auth/tests/db_api_key_tests.rs`
- `rust/auth/tests/default_provider_tests.rs`
- `rust/public/tests/api_keys_tests.rs`
- `python/micromegas/micromegas/cli/import_api_keys.py`
- `python/micromegas/tests/cli/test_import_api_keys.py`
- `mkdocs/docs/admin/api-keys.md`

**Modified**

- `rust/ingestion/src/sql_migration.rs`
- `rust/auth/Cargo.toml`, `rust/auth/src/lib.rs`, `rust/auth/src/default_provider.rs`
- `rust/auth/src/types.rs` (adding `ProviderUnavailable`), `rust/auth/src/multi.rs` (re-raising it when
  every provider fails and at least one saw it), `rust/auth/src/axum.rs` (mapping it to a new
  `AuthError::Unavailable` → 503), `rust/auth/src/tower.rs` (mapping it to `Status::unavailable` in the
  gRPC `AuthService`), `rust/public/src/servers/firehose_common.rs` (mapping it to 503 in
  `firehose_auth_middleware`) — see §2 step 5 and [Security](#security)
- `rust/auth/tests/multi_tests.rs`, `rust/auth/tests/axum_tests.rs` (existing files, new cases for the
  `ProviderUnavailable` → 503 path — see [Testing Strategy](#testing-strategy))
- `rust/public/src/servers/mod.rs`, `rust/public/src/servers/ingestion.rs` (adding
  `serve_ingestion_with_api_key_config`, with `serve_ingestion` reimplemented as a thin
  `DbApiKeyConfig::from_env()` wrapper around it — mirroring §3's `ProviderBuilder`/`provider()` split,
  so the published `serve_ingestion` signature on the `server`-feature crate is unchanged; recorded in
  the `CHANGELOG.md` bullet, see [Documentation](#documentation)),
  `rust/public/src/servers/flight_sql_server.rs`
- `rust/public/Cargo.toml` — add a `[[test]]` entry for `api_keys_tests` (`path =
  "tests/api_keys_tests.rs"`, `required-features = ["server"]`), matching the seven existing blocks
  at lines 97–130; without it the new file is auto-discovered unguarded and fails to compile under
  `cargo test -p micromegas` (`default = []`)
- `rust/telemetry-ingestion-srv/src/main.rs`, `rust/monolith/src/main.rs`
- `python/micromegas/pyproject.toml`
- `local_test_env/ai_scripts/start_services_with_oidc.py`
- `mkdocs/mkdocs.yml`, `mkdocs/docs/admin/authentication.md`, `mkdocs/docs/admin/ingestion.md`,
  `mkdocs/docs/admin/monolith.md`, `mkdocs/docs/admin/flight-sql.md`,
  `mkdocs/docs/admin/object-cache.md`, `mkdocs/docs/otlp/index.md`, `docker/README.md`,
  `mkdocs/docs/grafana/authentication.md`, `docker/docker-compose.monolith.yaml`
- `tasks/data_isolation/audience_based_access_control_plan.md` (record the `key_id` column and the
  single-DB-role caveat)
- `CHANGELOG.md` (`Unreleased` bullet — see [Documentation](#documentation))

**Deleted**

- `rust/public/src/servers/key_ring.rs` (dead duplicate internally, but a **public-API removal** on
  the published `micromegas` crate — `micromegas::servers::key_ring::{Key, KeyRingEntry,
  parse_key_ring}` — see [Current State](#current-state))

## Trade-offs

- **Two tables vs. one with a `scopes` column.** Per the umbrella plan: the security model is
  asymmetric (a stolen write key is an integrity problem; a read credential is a confidentiality
  one), only ingestion keys will ever carry an `audience` (#1372), and #1374 puts one ingestion key
  per user/machine on dev boxes and game clients. A shared table would make every one of those a read
  credential too. With "never both" as a rule, a `scopes` column is always a singleton. Cost: one
  behavior change at migration (below) and a duplicated `CREATE TABLE`; the *code* is shared, since
  one provider parameterized by `ApiKeyTable` serves both. **This is a schema-shape trade-off, not a
  scale one**: #1374's per-user/per-machine keys are what push the live-key population past this
  stage's cardinality assumption (§2's design notes), a separate concern from whether they land in one
  table or two.
- **Emitting SQL instead of connecting from python.** Costs the operator one extra pipe; avoids
  adding a Postgres driver to the published `micromegas` pip package for a deletable one-shot tool,
  and makes the insert reviewable before it runs. Alternative considered: add `psycopg[binary]` as a
  dev-group dependency — rejected because the tool needs to run wherever the operator's DB
  credentials are, not only in a poetry dev env.
- **Surfacing a key-store outage as 503, not 401.** Costs a new `ProviderUnavailable` typed error
  threaded through `AuthProvider`'s otherwise-`anyhow` error path, plus the `downcast_ref` checks in
  `MultiAuthProvider` and `auth_middleware` (§2 step 5). Buys correct retry behavior at the client: the
  env keyring is in-memory and cannot fail, so today's env-only deployments never hit `axum.rs`'s
  blanket "any provider error ⇒ 401" mapping on an outage — a DB-backed store can, and
  `rust/telemetry-sink/src/http_event_sink.rs:525-534` treats every `4xx` as permanent and drops the
  batch. Without this distinction, a Postgres outage would silently convert into data loss at every
  ingestion client once the positive cache ages out, rather than a retried 503 during the outage
  window.
- **Folding `last_used_at` into the lookup statement** instead of a throttled background write. Costs
  granularity (±cache TTL) and turns the miss-path `SELECT` into an `UPDATE ... RETURNING`; buys the
  removal of all per-entry timestamp state and any second round trip. This only rate-limits the write
  to once per cache TTL per key because the lookup goes through `valid.try_get_with` (§2 step 4), not a
  plain `get`/`insert` pair — the latter would let every concurrent miss for the same just-expired key
  issue its own `UPDATE`. For an approximately-last-used audit column this is the right resolution.
- **A negative cache.** It only makes *repeated* probes of the same token free: keyed by
  `hash_key(token)`, it misses on every distinct token, so a flood of randomly generated bearer
  tokens — the actual shape of an unauthenticated attack on the ingestion endpoint — still costs one
  DB round trip per request, same as without the cache. **This is not merely comparable to the
  in-memory `ct_eq` scan the env provider runs today — sharing the lake pool would make it materially
  worse**: all four call sites (§3) originally would have handed `DbApiKeyAuthProvider` a clone of the
  same pool the write path uses (`data_lake.db_pool` / `lakehouse.lake().db_pool`), built by
  `remote_data_lake.rs` with `PgPoolOptions::new().connect(db_uri)` and no overrides —
  sqlx's defaults are `max_connections: 10`, `acquire_timeout: 30s`. On that pool, an unauthenticated
  flood would contend for the same 10 connections `WebIngestionService`'s inserts need, degrading
  ingestion writes, not just costing the flood itself a round trip; and during an outage, each lookup
  could block up to 30s before returning its 503 — the same default the test setup already works around
  with a 50ms `acquire_timeout` (see [Testing Strategy](#testing-strategy)) to keep test runs fast, but
  which the production path would not otherwise override. **Resolution: `DbApiKeyAuthProvider` gets its
  own small dedicated pool instead** — `db_api_key::dedicated_key_store_pool` (§2) builds it from the
  lake pool's connect options with `max_connections(4)` and `acquire_timeout(2s)`, and all four call
  sites in §3 use it. A credential flood or a DB outage on the key-lookup path therefore can never
  starve the write path of connections, and a lookup fails fast (2s) into its 503 rather than blocking
  up to 30s. What this does *not* bound is the flood itself: nothing here (no rate limit, no
  shared-failure breaker) caps the rate of distinct bogus tokens hitting those 4 connections; that is
  accepted as out of scope, same as the unbounded per-request `ct_eq` scan the env provider already
  runs today — the difference this resolution buys is that the blast radius stops at those 4
  connections instead of spilling onto ingestion writes. What *is* bounded elsewhere is the cache's own
  memory: `unknown` carries an explicit `max_capacity` (`unknown_cache_size`, default 10_000, one
  32-byte-keyed entry per distinct token), so the same flood evicts old entries instead of growing the
  process's memory without limit. Key creation can also be delayed by up to `unknown_cache_ttl_secs`
  for a string that was probed just before it existed — hence 10s, not 60s.
- **`GET` returns revoked rows by default.** Slightly noisier listing; an operator investigating an
  incident needs to see that a key *was* revoked and when. `include_revoked=false`, plus
  `limit`/`offset`, bound the response as the table grows (§4) rather than returning every row
  unconditionally.
- **Routes on the ingestion service.** It mildly expands the surface of the most exposed process.
  Accepted deliberately: that is where the DB write grant belongs, OIDC is required, and no API key
  can be admin, so no key can mint another.

## Security

- **The single-DB-role caveat, stated plainly.** The umbrella plan says "Postgres grants enforce it,
  not application logic." That is achievable but not true of a deployment as shipped: all services
  share one connection string (`MICROMEGAS_SQL_CONNECTION_STRING`), and the migration runs as the
  owner. What ships here is a *code*-level boundary — `api_keys_router` hardcodes
  `ApiKeyTable::Ingestion`, and the analytics provider is constructed with `ApiKeyTable::Analytics` —
  plus a documented grant recipe for operators who separate roles:

```sql
-- ingestion role: its own table only
GRANT SELECT, INSERT ON ingestion_api_keys TO micromegas_ingestion;
GRANT UPDATE (last_used_at, revoked_at, revoked_by) ON ingestion_api_keys TO micromegas_ingestion;
-- and no grant of any kind on analytics_api_keys

-- analytics role: read + touch only
GRANT SELECT ON analytics_api_keys TO micromegas_analytics;
GRANT UPDATE (last_used_at) ON analytics_api_keys TO micromegas_analytics;
-- and no grant of any kind on ingestion_api_keys
```

  Recorded in `mkdocs/docs/admin/api-keys.md` and back-annotated onto the umbrella plan so the claim
  is not repeated as fact.
- **No cleartext at rest, and one exposure in flight.** `POST /auth/api_keys` returns the key once,
  over whatever TLS the deployment terminates in front of the ingestion service — the service itself
  is plain HTTP (`serve_ingestion` binds a bare `tokio::net::TcpListener`; there is no rustls/TLS
  acceptor anywhere in `rust/public/src/servers/`). A TLS-terminating ingress in front of the ingestion
  service is therefore an explicit prerequisite for running `POST /auth/api_keys`, called out in
  `mkdocs/docs/admin/api-keys.md` (see [Documentation](#documentation)). It is never logged (the mint
  log line carries `key_id` only) and never retrievable afterwards.
- **Revocation latency is bounded by `cache_ttl_secs`**, not zero. Asserted by a test, reported in
  the `DELETE` response, documented in the admin guide.
- **API keys cannot manage keys**, by two independent mechanisms: `is_admin` is hardcoded `false` on
  every API-key context, and `require_key_admin` rejects any non-OIDC `auth_type` outright.
- **No timing side channel** on the DB path: the token is hashed, never compared against a secret.
  The env provider's `ct_eq` scan is untouched and still correct for its (small, static) keyring.
- **Low-entropy legacy keys** are the one place SHA-256-without-KDF is thin. The import tool warns;
  the migration guide says rotate.
- **A key-store outage is not client-visible as a rejected credential, at every surface a DB-backed
  provider reaches.** `DbApiKeyAuthProvider`
  wraps every DB error in `ProviderUnavailable` (§2 step 5); `MultiAuthProvider` re-raises it when no
  provider in the chain succeeded, and `auth_middleware` (`rust/auth/src/axum.rs`) maps it to
  **503**, not the blanket 401 every other provider error gets. The same downcast-and-remap covers the
  gRPC `AuthService` (`rust/auth/src/tower.rs`, mapping to `Status::unavailable` — the path
  `flight_sql_server.rs` wraps around the `Analytics` provider) and the Firehose routers
  (`rust/public/src/servers/firehose_common.rs`'s `firehose_auth_middleware`, mapping to 503), since
  both validate through the same `MultiAuthProvider` chain as the plain axum ingestion routes. This
  matters because
  `rust/telemetry-sink/src/http_event_sink.rs` treats `4xx` as permanent (drop, no retry) and `5xx` as
  transient (retry): without the distinction, a Postgres outage would look identical to an invalid
  key to every ingestion client, and telemetry would be silently and permanently dropped for the
  duration of the outage instead of retried.

## Migration

Ordering, for both split and monolith deployments:

1. Deploy the new binaries. The migration creates the tables. **Nothing changes**: the env keyring
   still authenticates every existing key, and the DB tables are empty. **Deployment model: all
   services are upgraded together, in one coordinated deploy — there is no supported mixed-version
   window.** In a split deployment, that coordinated deploy still starts the ingestion service (or the
   monolith) before flight-sql-srv, since ingestion is what runs the migration and flight-sql never
   migrates on its own (see [Current State](#current-state)); its own startup existence query (§3)
   fails loudly, naming the table, if this ordering is violated. Because no old binary is left running
   or restarted against the new schema, `execute_migration`'s
   `assert_eq!(current_version, LATEST_DATA_LAKE_SCHEMA_VERSION)` panic path
   (`rust/ingestion/src/sql_migration.rs:150-199`) is never exercised by a stale binary here — that
   failure mode is a property of an unsupported deployment model, not a live constraint under
   coordinated upgrades. flight-sql and the maintenance daemon do not run `execute_migration` and are
   unaffected. **The version-number coordination with `tasks/1245_partition_blocks_by_insert_time`** is
   a hard sequencing prerequisite, not a deploy-time risk: this plan must land and claim v5 before that
   plan's schema work lands — see [Current State](#current-state) → Schema and migrations for what
   happens if the ordering is violated.
2. Run `micromegas-import-api-keys` and apply its SQL, passing `--skip` only for a key name that is an
   `object-cache-srv` client key used *exclusively* as such (see §5) — those names stay env-only rather
   than be imported into either table. A name that is also a service's own ingestion self-telemetry
   credential (`MICROMEGAS_INGESTION_API_KEY`) must instead be passed to `--ingestion`, so it survives
   in `ingestion_api_keys` once step 3 removes `MICROMEGAS_API_KEYS`; it is unaffected in
   `object-cache-srv`'s own env keyring either way. Because §3 places the env `ApiKeyAuthProvider`
   first in the chain, the env provider keeps answering for every existing key for as long as
   `MICROMEGAS_API_KEYS` is still set — i.e. through the rest of this step — so `DbApiKeyAuthProvider`
   is never reached and the imported rows are proven only by their presence in the table, not by any
   traffic; `last_used_at` stays NULL for every imported key until step 3 removes the env keyring and
   lets the DB provider actually be exercised.
3. Remove `MICROMEGAS_API_KEYS` (and the prefixed variants) from ingestion and flight-sql, and
   redeploy. Safe once step 2 has populated the table: per §3, a non-empty key store counts as "auth
   configured" on its own, so both services keep serving without OIDC configured — including the
   key-only flight-sql deployment `mkdocs/docs/grafana/authentication.md` documents. Keys now live
   only in the DB, and revoke/rotate no longer needs a redeploy. This also carries the two Firehose
   routes safely: as noted in [Current State](#current-state), they validate through the same
   `MultiAuthProvider` chain as every other ingestion route (`firehose_common.rs`), so a Firehose
   access key minted into `ingestion_api_keys` keeps CloudWatch Metric Streams / CloudWatch Logs
   ingestion working once `MICROMEGAS_API_KEYS` is gone.
   `object-cache-srv` keeps its `MICROMEGAS_API_KEYS` **permanently** — it has no DB access, and its
   keys are service-held, few, and never distributed to users or machines. Its keys remain
   non-revocable without a redeploy; accepted.

**The one client-visible change.** `telemetry-ingestion-srv/src/main.rs:51` and
`flight_sql_server.rs:226` both read the unprefixed `MICROMEGAS_API_KEYS` today, so in every split
deployment *every existing key is currently valid on both surfaces*. "Never both" cannot preserve
that: a genuinely dual-use key must become two keys, and any client that used one key for both
ingestion and queries must be updated. The import tool refuses to place one key in both tables, and
the migration guide states this rather than leaving operators to discover it. This is the single
place the zero-client-change claim in the issue does not hold.

## Documentation

- **New `mkdocs/docs/admin/api-keys.md`**, added to `mkdocs.yml` nav under Administration: a stated
  prerequisite, ahead of the route documentation, that `POST /auth/api_keys` must sit behind a
  TLS-terminating ingress — the ingestion service itself serves plain HTTP, so the one-time cleartext
  key in the response is otherwise exposed in flight (see [Security](#security)); the two
  tables and why they are split; the three routes with request/response examples; the
  revocation-latency property and the cache env knobs; the `db_api_key_error_count` metric (§2 step 5)
  as the signal to alert on for a key-store outage, alongside the four cache env knobs; the grant recipe; the import procedure
  including the dual-use split; the `object-cache-srv` exception; a runbook for listing, minting, and
  revoking an `analytics_api_keys` row by hand (list: `SELECT key_id, name, created_at, last_used_at,
  revoked_at FROM analytics_api_keys ORDER BY created_at DESC` — the only way to discover a `key_id` to
  revoke, since this table has no HTTP `GET`; mint: the full executable recipe from §4 — generate the
  key, compute its digest with a trailing-newline-safe command (`printf '%s' "$KEY" | sha256sum`,
  never `echo`, since `hash_key` covers the full key string and a stray newline would mint a key that
  can never authenticate), and generate the `key_id` with `uuidgen` (the table has no `DEFAULT`, so
  nothing else supplies one), then the import tool's `INSERT` shape with the computed hash and id; revoke: the
  same `UPDATE ... SET revoked_at = COALESCE(...)` the `DELETE` route runs, keyed only on the `key_id`
  from the list step — never on `name`, called out explicitly as unsafe during rotation since `name`
  has no uniqueness constraint (§1)), since that table has no HTTP
  lifecycle of its own.
- **`admin/authentication.md`**: of the "API Keys (Legacy)" section's Limitations list, "manual key
  distribution and rotation" and "no user identity for audit logging" are now wrong for DB-backed keys
  (mint/revoke via HTTP; `created_by`/`revoked_by` audit trail). "No automatic expiration" stays
  **true** — this design adds revocation, not expiry, so keep it listed as a remaining limitation of
  DB-backed keys. Rewrite as two subsections — env keyring (static, still used by `object-cache-srv`)
  and DB-backed keys — and link to `api-keys.md`. Two other sections of this same file go stale
  otherwise and must be rewritten alongside it: `### API Key Configuration` (lines 96–111) documents
  `MICROMEGAS_API_KEYS` / `openssl rand -base64 512` as *the* way to configure keys — reframe it as the
  legacy/bootstrap path (still accurate for `object-cache-srv`, which stays env-only permanently) and
  replace the `openssl rand` recipe with a pointer to `api-keys.md`'s `POST /auth/api_keys` mint route
  as the steady-state way to obtain an `mmk_`-prefixed key — the same correction already applied to the
  `mm_abc123def...` placeholders in `otlp/index.md`. And `## Migration from API Keys to OIDC` (lines
  635–663) currently tells operators OIDC is the destination for *all* API keys; that is only true for
  human/service-account identities — machine credentials (service keys, Firehose access keys) migrate
  to DB-backed `ingestion_api_keys`, not OIDC. Retitle it (e.g. "Migration from Env API Keys to
  DB-Backed Keys and OIDC") and rework the worked example so the closing `unset MICROMEGAS_API_KEYS`
  step is explicitly gated on `micromegas-import-api-keys` having populated the table first, matching
  this plan's own Migration step 3 — unsetting the env var before the table is populated would leave
  machine clients unauthenticated.
- **`admin/ingestion.md`**: note the `/auth/api_keys` routes and that they require OIDC + admin; add
  a `MICROMEGAS_ADMINS` row to the environment-variable table (mirroring `admin/flight-sql.md:30`,
  which already documents it), and state plainly that minting/listing/revoking keys 403s until an
  OIDC identity is in that list — `MICROMEGAS_ADMINS` is otherwise unset for ingestion and every
  `/auth/api_keys` call fails with no other symptom; mark `MICROMEGAS_API_KEYS` as the legacy/bootstrap
  path; and correct the "refuses to start unless `MICROMEGAS_API_KEYS` or `MICROMEGAS_OIDC_CONFIG` is
  set" sentence (lines 49–51) to list a non-empty DB key store as a third way to satisfy the check,
  mirroring the startup-message fix in Implementation
  step 8 — this stops being accurate the moment Migration step 3 removes the env vars.
- **`admin/monolith.md`, `admin/flight-sql.md`**: point at `api-keys.md`; state that flight-sql
  validates `analytics_api_keys` and mints nothing; and, in `flight-sql.md`, correct the same "refuses
  to start unless ... is set" sentence (lines 49–50) to add the non-empty DB key store as a third way
  to satisfy the check, mirroring Implementation step 9.
- **`admin/object-cache.md`**: state explicitly that its `MICROMEGAS_API_KEYS` is permanent and its
  keys are not revocable without a redeploy — otherwise it reads as an oversight.
- **`otlp/index.md`, `docker/README.md:192`** (the *Ingestion Server* env table — the only
  `MICROMEGAS_API_KEYS` row in scope here; the FlightSQL Server table at `:195-199` documents no
  `MICROMEGAS_API_KEYS` at all): document `MICROMEGAS_API_KEYS` as the transitional (Migration
  steps 1–2) path rather than the steady state; update to point at `api-keys.md` and describe
  DB-backed keys as the destination. This includes the two Firehose "Access key" bullets at
  `otlp/index.md:426` and `:557`, which currently describe the value as "a micromegas API key — the
  value from `MICROMEGAS_API_KEYS`" — reword both to say the access key is any live
  `ingestion_api_keys` (or, transitionally, `MICROMEGAS_API_KEYS`) key, since the two Firehose routes
  validate through the same provider chain as every other ingestion route (see
  [Current State](#current-state)). Also update the four `mm_abc123def...` placeholder key examples at
  `otlp/index.md:42,47,231,366` to the `mmk_` shape minted keys actually have (see §2's `generate_key`
  and [Open Questions](#open-questions)), so the published examples match what the service mints.
  **Leave the dated blog post (`mkdocs/docs/blog/posts/2026-05-04-*.md`) untouched** — historical posts
  stay as published and are not in scope for this placeholder update. **Leave `docker/README.md:131`
  and `:206` alone** — both are
  the object-cache keyring, which stays env-only permanently (see [Current State](#current-state));
  they must not be pointed at the import tool or `api-keys.md`.
- **`docker/docker-compose.monolith.yaml:52`**: the comment naming
  `MICROMEGAS_INGESTION_API_KEYS`/`MICROMEGAS_API_KEYS` as the ingestion/FlightSQL auth fallback is
  the genuine monolith-fallback reference (see §5); update it to also mention the DB-backed path
  once it exists.
- **`grafana/authentication.md`**: the key-only flight-sql deployment it documents (line 30) is exactly
  the case §3 and Migration rely on; confirm it still applies to DB-backed `analytics_api_keys` and
  cross-link `api-keys.md`.
- **`tasks/data_isolation/audience_based_access_control_plan.md`**: record the `key_id` column and
  replace the "Postgres grants enforce it" claim with the code-boundary-plus-documented-grants
  reality.
- **`CHANGELOG.md`**: an `Unreleased` bullet covering the DB-backed key store (`ingestion_api_keys`
  / `analytics_api_keys`, migration v5), the three new `/auth/api_keys`
  HTTP routes, the four new
  cache/audit env knobs (`MICROMEGAS_API_KEY_CACHE_SIZE`, `MICROMEGAS_API_KEY_CACHE_TTL_SECONDS`,
  `MICROMEGAS_API_KEY_UNKNOWN_CACHE_TTL_SECONDS`, `MICROMEGAS_API_KEY_UNKNOWN_CACHE_SIZE`), the new
  `db_api_key_error_count` metric (§2 step 5) emitted on every key-store DB error regardless of log
  suppression, the
  **public-API removal** of `micromegas::servers::key_ring` (dead, unreferenced duplicate of
  `api_key.rs`'s keyring half — see [Current State](#current-state)), the new (non-breaking)
  `serve_ingestion_with_api_key_config` entry point — `serve_ingestion` (published under the `server`
  feature, see [Files to Modify](#files-to-modify)) keeps its existing signature as a thin wrapper
  around it — the new `micromegas_auth::axum::AuthError::Unavailable` variant and
  `micromegas_auth::types::ProviderUnavailable` type (§2 step 5), flagged as a **minor breaking
  change**: `AuthError` is published API with no `publish = false` on `rust/auth/Cargo.toml`, so any
  downstream exhaustive `match` on it will need a new arm — and the
  one client-visible breaking change: a key valid on both ingestion and flight-sql today must
  become two distinct keys (see [Migration](#migration)).

## Testing Strategy

Rust unit tests live under each crate's `tests/` folder (per `rust/CLAUDE.md`); DB-backed tests are
`#[ignore]`d and read `MICROMEGAS_SQL_CONNECTION_STRING`, matching
`analytics/tests/thread_spans_ordering_db_test.rs` and `public/tests/pg_stats_test.rs`.

**`rust/auth/tests/db_api_key_tests.rs` — no DB** (`PgPoolOptions::new()
.acquire_timeout(Duration::from_millis(50)).connect_lazy("postgres://localhost/unused")` — the
`firehose_tests.rs` trick, but with an explicit short `acquire_timeout`: sqlx's default is 30s
(`sqlx-core-0.8.6/src/pool/options.rs:160`), and the last bullet below makes two DB attempts, which at
the default would cost ~60s per run):

- `hash_key` matches a known SHA-256 vector, and its output never contains the key bytes.
- `generate_key` produces the `mmk_` prefix, 32 decoded bytes, and distinct values across calls.
- `ApiKeyTable::table_name` maps to the two expected literals.
- A missing bearer token fails before any DB access.
- **A DB error is not cached as `unknown`**: two calls with the same token both attempt the DB
  (asserted via the error surfacing twice rather than the second call returning the cached rejection).
- **A key-store outage is a `ProviderUnavailable`, not a generic rejection**: against this same
  unreachable pool, assert the error `validate_request` returns downcasts to
  `types::ProviderUnavailable`.
- **`db_api_key_error_count` fires on a DB error**: **`#[serial]`**, using
  `micromegas_tracing::test_utils::init_in_memory_tracing` and the `count_integer_metric` helper
  (`rust/object-cache/tests/telemetry_tests.rs:39-`, guard constructed at line 84), following
  `rust/tracing/src/test_utils.rs:42-45`'s requirement that any test using `init_in_memory_tracing` be
  `#[serial]` since it shares global dispatch state via `init_event_dispatch`; assert two
  DB attempts against the unreachable pool each increment `db_api_key_error_count` by one, including
  when the rate-limited `error!` line itself is suppressed on the second attempt — the metric is
  unconditional, the log line is not. **The two sibling bullets above in this same file — "A DB error
  is not cached as `unknown`" and "A key-store outage is a `ProviderUnavailable`" — also trigger the
  same unconditional `imetric!` and must likewise be `#[serial]`**, so none of the three can run
  concurrently and land events in each other's shared `InMemorySink`, which would make the "two
  attempts ⇒ two increments" assertion wrong rather than merely flaky.

**`rust/auth/tests/db_api_key_tests.rs` — `#[ignore]`, live Postgres**, each test creating its own
rows with a unique name prefix and cleaning up:

- A row's key authenticates; the resulting `AuthContext` has `auth_type: ApiKey`, `is_admin: false`,
  `allow_delegation: true`, `subject == name`.
- An unknown key is rejected.
- **Revocation latency**: build the provider with `cache_ttl_secs: 0`, authenticate, set
  `revoked_at`, authenticate again ⇒ rejected. Then repeat with a nonzero TTL and assert the key
  still authenticates immediately after revocation — the property is *bounded* latency, not
  instantaneous invalidation, and both halves deserve a test.
- **No cleartext is stored**: after a mint, `key_hash = hash_key(key)` and no text column contains
  the key string.
- **`last_used_at` is written** on a cache miss and not on a cache hit.
- **Env + DB compose**: a `MultiAuthProvider` holding an `ApiKeyAuthProvider` and a
  `DbApiKeyAuthProvider` authenticates a key from either.
- **Surface separation, both directions**: a key row in `ingestion_api_keys` is rejected by a provider
  bound to `Analytics`, and vice versa.

**`rust/auth/tests/multi_tests.rs`** (existing file, new cases) — a fake `AuthProvider` returning
`Err(types::ProviderUnavailable(...).into())`:

- One unavailable provider, no others configured ⇒ `MultiAuthProvider::validate_request`'s error
  downcasts to `ProviderUnavailable`.
- One unavailable provider plus one that plainly rejects (e.g. a wrong env key) ⇒ still
  `ProviderUnavailable`, not the generic "authentication failed with all providers" — an outage must
  not be masked by an ordinary rejection elsewhere in the chain.
- All providers plainly reject, none unavailable ⇒ unchanged generic error (no regression on today's
  behavior).

**`rust/auth/tests/axum_tests.rs`** (existing file, new case), following the existing
`test_invalid_api_key` shape: a fake provider that always returns `Err(ProviderUnavailable(...).into())`
⇒ `auth_middleware` responds `StatusCode::SERVICE_UNAVAILABLE`, not `UNAUTHORIZED` — this is the
assertion the issue requires: a key-store failure yields 503, not 401.

**`rust/auth/tests/default_provider_tests.rs`** — `ProviderBuilder` / §3's startup-existence rules,
using the extracted `key_store_has_live_rows` where full env isolation around `build()` would be
awkward. The three bullets below that assert on `build()`'s return value depend on
`MICROMEGAS_API_KEYS` / `MICROMEGAS_OIDC_CONFIG` being present or absent, and every test in one
`tests/*.rs` file shares a process and runs on parallel threads, so each is `#[serial]` (via the new
`serial_test` dev-dependency, Phase 2 step 2) with an `EnvGuard` that sets/removes the relevant vars
and restores them on drop — the same pattern as `rust/ingestion/tests/data_lake_config_tests.rs:1-45`:

- **Provider always registered**: `with_db_key_store` attached to an *empty* table, alongside an env
  keyring (or OIDC) so `build()` returns `Some` at all, still produces a `MultiAuthProvider` containing
  the DB provider — asserted by inserting a row into that table *after* `build()` returns and
  authenticating its key through the returned provider, with no restart. This is the regression the
  design calls out in §3: without it, a first-minted key would not authenticate until the process
  restarts.
- **Non-empty table ⇒ `Some`**: a table with one live row and no env keys / OIDC configured still
  yields `Ok(Some(_))` from `build()`.
- **Empty table + nothing else configured ⇒ `Ok(None)`**: an empty table with no env keys / OIDC
  configured yields `Ok(None)`, preserving the "genuinely empty deployment" startup guard.
- **Missing relation ⇒ `Err` naming the table**: the other three bullets above run against the live,
  already-migrated-to-v5 `MICROMEGAS_SQL_CONNECTION_STRING` pool, so a missing relation cannot be
  simulated by pointing at an unmigrated (v4) or `connect_lazy` pool — the former isn't available once
  this table exists, and the latter never reaches a server, so it produces a *connection* error, not
  a missing-relation one. Instead, build a **separate, throwaway single-connection pool** whose
  `search_path` is set in the connect options themselves — e.g.
  `PgConnectOptions::from_str(&conn_str)?.options([("search_path", "<throwaway empty schema>")])`
  fed to `PgPoolOptions::new().max_connections(1).connect_with(...)` — rather than issuing a bare
  `SET search_path` against a connection borrowed from the shared pool. `SET` (not `SET LOCAL`) is
  per-session state on whichever pooled connection happens to be checked out, and
  `key_store_has_live_rows(&PgPool, ...)` acquires its own connection from the pool with no guarantee
  it's the same one the test issued the `SET` on — and if it is, the mutated `search_path` leaks onto
  that connection afterward, contaminating any other test that reuses the shared pool. Setting the
  search path in the connect options instead means every connection this throwaway pool ever hands
  out already resolves the unqualified table name to nothing, and the pool is dropped at the end of
  the test with no shared state to clean up. Assert the resulting `Err` is a `sqlx` undefined-table
  error (SQLSTATE `42P01`) — not merely that the message contains the table name, which
  would also pass for an unrelated error that happened to mention it — and that `build()` propagates it
  rather than treating it as `Ok(None)`.

**`rust/public/tests/api_keys_tests.rs`** — `tower::ServiceExt::oneshot` against `api_keys_router`,
with the `AuthContext` injected as an extension (no middleware needed). Needs the matching
`[[test]]` block in `rust/public/Cargo.toml` (`required-features = ["server"]`), like the other
seven integration test files in this crate:

- Every route returns 403 for `auth_type: ApiKey`, and 403 for a non-admin OIDC context. Both
  directions, since these are the whole gate.
- **§4's `400` validation, no DB** (validated before any hashing or DB access, so these run against
  the same `connect_lazy`/never-touched pool as the 403 bullets above): `POST` with an empty `name`
  returns 400; `POST` with a `name` over 255 bytes returns 400; `GET` with `limit=0` (and a negative
  `limit`) returns 400; `GET` with `limit=1000` does *not* return 400 — proving the clamp accepts and
  rewrites the value rather than rejecting it — with the clamp's effect on the resulting row count
  covered by the live-DB `GET` bullet below.
- `#[ignore]`, live DB: `POST` returns the cleartext once and a `key_id`; the returned key
  authenticates through a `DbApiKeyAuthProvider`; `GET` lists the key **without** `key_hash` or the
  key; with 501 rows present, `GET ?limit=1000` returns exactly 500, confirming the clamp; `DELETE`
  returns 200 and is idempotent on a second call; `DELETE` of an unknown `key_id` returns 404; the
  original `revoked_at` survives the second `DELETE`.
- **No route inserts into `analytics_api_keys`**: `api_keys_router` takes no table parameter, so this
  is true by construction; a unit test asserts the mint statement names `ingestion_api_keys` to keep
  it that way under refactoring.

**`python/micromegas/tests/cli/test_import_api_keys.py`**:

- The emitted hex digest equals `hashlib.sha256(key.encode()).hexdigest()` for a known key.
- `--from-prefixed` routes each variable to the right table.
- `--from-prefixed` with neither `MICROMEGAS_INGESTION_API_KEYS` nor `MICROMEGAS_ANALYTICS_API_KEYS`
  set (including when only the unprefixed `MICROMEGAS_API_KEYS` is present) exits non-zero and
  points at the explicit `--keys-env` form, and emits no SQL.
- An unassigned key name exits non-zero and names the key.
- A key name given to both `--ingestion` and `--analytics` exits non-zero with the split-it guidance.
- The output contains `ON CONFLICT (key_hash) DO NOTHING` and is wrapped in `BEGIN`/`COMMIT`.
- No cleartext key appears anywhere in the output.
- **§5's name-validation guard**: a name that is empty, a name over 255 bytes, a name containing a
  comma, and a name containing `$` each exit non-zero naming the offending key, with no SQL emitted
  on stdout.
- **Dollar-quoting round-trips arbitrary content**: a name containing a single quote and a name
  containing a newline are each emitted inside `$mmk$...$mmk$` unmodified, rather than escaped or
  truncated.

**Round-trip (the zero-client-change claim), `#[ignore]`**: a Rust DB test that computes the same
digest the python tool emits for a fixture key, inserts the row, and asserts the *original key
string* authenticates through `DbApiKeyAuthProvider`. Paired with the python test above, this covers
the claim end to end without needing a live `psql` in CI.

**Full-stack smoke, manual**: `local_test_env/ai_scripts/start_services_with_oidc.py:182-199` currently
launches `telemetry-ingestion-srv` with `--disable-auth`, so `/auth/api_keys` would not even be
registered there — even though the process already receives the OIDC config, since `env =
os.environ.copy()` (line 129) is the same environment passed to ingestion, flight-sql, and the
maintenance daemon alike. Removing `--disable-auth` from the ingestion launch (line 194) is not
enough on its own: every `#[micromegas_main]` binary appends `.with_auth_from_env()` when no literal
key is given, and that only attaches an `Authorization` header if `MICROMEGAS_INGESTION_API_KEY` (or
the OIDC client-credentials trio) is set in the *sink's* environment — the server-side
`MICROMEGAS_OIDC_CONFIG` the script already copies does not feed it. Without a credential, ingestion,
flight-sql, and the maintenance daemon would all start 401ing their own self-telemetry POSTs to port
9000, the only data this local env has to query. So the script must also: mint (or hardcode for local
use) an ingestion key, set `MICROMEGAS_API_KEYS` on the ingestion process so it accepts that key, and
set the matching `MICROMEGAS_INGESTION_API_KEY` in the shared `env` (line 129) before it is passed to
all three service processes. Then: `python3
local_test_env/ai_scripts/start_services_with_oidc.py`, mint a key over HTTP, use it to POST telemetry,
revoke it, and observe the rejection after the TTL.

**Gates**: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`,
`python3 build/rust_ci.py`, and, from `python/micromegas/`, `poetry run black
micromegas/cli/import_api_keys.py tests/cli/test_import_api_keys.py` and `poetry run pytest
tests/cli/test_import_api_keys.py` — scoped to the new file, not a bare `poetry run pytest`: most
other files under `tests/` need a live analytics service to *run*, so the gate is scoped to the new
file, which needs no live service. (`pytest` itself is declared in the `test` dependency group,
`python/micromegas/pyproject.toml:27-30`, so that group must still be installed for `poetry run
pytest` to work at all, scoped or not — scoping only avoids the live service and the
`opentelemetry-proto` dependency the other files also need.)

## Open Questions

None. `cache_ttl_secs = 60` is settled by the `oidc.rs` precedent (see §2). The minted-key prefix is
settled too: `mmk_`, confirmed by the user — see §2's `generate_key`.
