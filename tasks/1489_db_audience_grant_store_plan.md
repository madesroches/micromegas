# DB-Backed Audience Grant Store Plan (#1489, AbAC Stage 6a)

## Overview

Moves the AbAC audience grant map out of startup env config (`{prefix}_AUDIENCE_GRANTS`,
`rust/auth/src/policy.rs`) and into the telemetry DB, so grants can be created without a service
restart. This is what makes the per-user privacy profile deliverable at all: a per-user audience
needs one grant row per user, and neither "restart every service per new user" nor "one env var
with thousands of entries" scales. It is Stage 6a of the staged AbAC rollout in
`tasks/data_isolation/audience_based_access_control_plan.md` — a flat, selector-based store that
is a 1:1 stand-in for the long-term model's `group_read_grants`/`group_mint_grants` tables
([Long-term model](../data_isolation/audience_based_access_control_plan.md#long-term-model--groups-nested-membership-and-grants)),
not an implementation of nested groups itself.

The env map is kept as the static/bootstrap layer, unioned with the store — additive, no forced
migration. Open and per-team deployments keep working untouched.

## Current State

- **`AudienceGrants`** (`rust/auth/src/policy.rs:202-279`) is a parsed, validated, in-memory map
  built once via `AudienceGrants::from_env(prefix)` (`policy.rs:253-267`), which resolves
  `{prefix}_AUDIENCE_GRANTS` (falling back to `MICROMEGAS_AUDIENCE_GRANTS`). The wire format is
  `{"<audience>": [...]}` (bare array = read-only shorthand) or
  `{"<audience>": {"read": [...], "mint": [...]}}`; every audience name must satisfy
  `is_valid_audience` (`[A-Za-z0-9_-]{1,255}`, `policy.rs:45-51`) and every selector must be `*`,
  `user:<id>`, or `group:<id>` (`valid_selector`, `policy.rs:89-97`).
- **`ReadPolicy`/`MintPolicy`** (`policy.rs:318-341`) are the only seam anything downstream sees:
  `async fn resolve(&self, caller: &AuthContext) -> Result<ReadableAudiences>` and
  `async fn resolve_audience(&self, caller: &AuthContext, requested: Option<&str>) -> Result<String>`.
  Both are already `async` and fallible — deliberately, so a store-backed implementation can land
  behind them with no trait change. `AudienceReadPolicy`/`AudienceMintPolicy` (`policy.rs:363-461`)
  are the only production implementations, each holding one `AudienceGrants` value for the
  lifetime of the process.
- **`DbApiKeyAuthProvider`** (`rust/auth/src/db_api_key.rs`) is the existing DB-cache precedent:
  a `moka::future::Cache<[u8;32], Arc<KeyRow>>` keyed per API key, TTL from
  `MICROMEGAS_API_KEY_CACHE_TTL_SECONDS` (default 60s, `db_api_key.rs:68-71`), refreshed lazily via
  `try_get_with` on a cache miss. A DB error during refresh is wrapped as `ProviderUnavailable` and
  **never cached** — it does not serve a stale value, because there is no "value" to a per-key
  lookup once its TTL entry has been evicted. `dedicated_key_store_pool` (`db_api_key.rs:135-141`,
  4 connections, 2s acquire timeout) builds a small pool from an existing pool's connect options,
  used by both flight-sql and analytics-web-srv.
- **`ingestion_api_keys`** is the DDL/reader split this stage mirrors: DDL lives in
  `rust/ingestion/src/sql_migration.rs` (`upgrade_data_lake_schema_v5`, lines 100-141; the
  `audience` column added in v6, lines 152-174), reader lives in `rust/auth/src/db_api_key.rs` —
  keeping `micromegas-ingestion` free of any dependency on `micromegas-auth`.
- **Admin HTTP routes** for keys (`rust/analytics-web-srv/src/ingestion_keys.rs`, mirrored by
  `analytics_keys.rs`) are the shape the new grants route copies: `POST`/`GET`/`DELETE` handlers,
  gated by the `AdminUser` extractor (`rust/analytics-web-srv/src/auth/handlers.rs:563-579`),
  layered via `Extension<...State>` in `web_server.rs::build_protected_routes`. Under
  `--disable-auth` (`auth_state.is_none()`), the real routers are not merged at all: that mode
  layers a hardcoded `ValidatedUser { is_admin: true, .. }` on every request in place of running
  `cookie_auth_middleware`, which would otherwise let any unauthenticated caller pass the
  `AdminUser` gate, so `key_management_disabled_router` is merged instead, answering both
  key-management prefixes with a fixed 503 (`web_server.rs:260-290`). This is a separate mechanism
  from the per-request 503 each `*KeysState.pool: Option<PgPool>` already returns when the DB pool
  itself is unconfigured.
- **CLI precedent**: no Rust binary; Python CLIs under `python/micromegas/micromegas/cli/`, each
  registered as a Poetry script (`pyproject.toml:36-40`) and talking to `analytics-web-srv` over
  HTTP via `WebClient` (`python/micromegas/micromegas/web_client.py`) — never direct Postgres
  access. `screens.py` is the closest precedent for a multi-subcommand CLI (`argparse`
  `add_subparsers`, one subparser per verb).
- `LATEST_DATA_LAKE_SCHEMA_VERSION` is currently `6` (`sql_migration.rs:8`).

## Design

### 1. Schema — migration v7, one table with an `axis` column

Settled by the issue itself and confirmed against the long-term model: **one table**, not two,
because this stage is explicitly a 1:1 image of today's env map (also one map for both axes), kept
splittable later without touching `ReadPolicy`/`MintPolicy`. `rust/ingestion/src/sql_migration.rs`:

```sql
-- upgrade_data_lake_schema_v7
CREATE TABLE audience_grants (
    audience   VARCHAR(255) NOT NULL,
    axis       VARCHAR(4) NOT NULL CHECK (axis IN ('read', 'mint')),
    selector   VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    created_by VARCHAR(255) NOT NULL,
    PRIMARY KEY (audience, axis, selector),
    CONSTRAINT audience_grants_audience_name CHECK (audience ~ '^[A-Za-z0-9_-]+$'),
    CONSTRAINT audience_grants_selector_shape
        CHECK (selector = '*' OR selector ~ '^(user|group):.+$')
);
UPDATE migration SET version=7;
```

`LATEST_DATA_LAKE_SCHEMA_VERSION` becomes `7`; `execute_migration` gets one more
`if 6 == current_version { ... }` block, following the v5/v6 pattern exactly.

No surrogate `grant_id`, unlike `ingestion_api_keys.key_id`: `(audience, axis, selector)` is
already the row's natural, non-secret identity, so a surrogate would just be a second name for the
same thing (see Trade-offs). No `revoked_at`/`revoked_by` either — a removed grant has no ongoing
artifact whose provenance a caller might later need (also in Trade-offs).

### 2. `AudienceGrants` gains a DB-row constructor and a merge

`rust/auth/src/policy.rs` additions (same file — `readers()`/`mint_selectors()` stay private, and
the new code needs them):

- `pub enum GrantAxis { Read, Mint }` — the Rust side of the `axis` column.
- `pub fn from_rows(rows: impl IntoIterator<Item = (String, GrantAxis, String)>) -> Result<Self>` —
  builds an `AudienceGrants` from `(audience, axis, selector)` triples, running the *same*
  `is_valid_audience`/`valid_selector` checks `parse` runs. This is the one place both the JSON
  path and the DB path fail closed on a malformed row, so a hand-edited row that slipped past the
  table's own `CHECK` constraints (e.g. via a direct `psql` session) still can't reach a policy
  decision.
- `pub(crate) fn merge(&self, other: &Self) -> Self` — unions each audience's `read`/`mint`
  selector lists across both maps. No dedup: `selector_matches` is called with `.any()`, so a
  selector present in both the env map and the store costs one redundant comparison, never a
  wrong answer. This is what makes env-and-store additive per design question 3 (below) with no
  special-cased "duplicate" handling — a selector present in either source grants access, full
  stop.

### 3. `DbAudienceGrantsSource` — new file `rust/auth/src/db_audience_grants.rs`

The whole-table snapshot cache. Modeled on `db_api_key.rs`'s config/pool conventions, but a single
cached value rather than a per-key `moka` cache — the issue is explicit that the whole map is
small enough to hold as one snapshot, and `moka`'s eviction/LRU machinery has nothing to do here.

```rust
pub struct DbAudienceGrantsConfig {
    /// `MICROMEGAS_AUDIENCE_GRANT_CACHE_TTL_SECONDS`, default 60 — mirrors
    /// `MICROMEGAS_API_KEY_CACHE_TTL_SECONDS`'s name and default.
    pub cache_ttl_secs: u64,
}

struct Snapshot {
    grants: AudienceGrants,
    /// Time of the last *successful* load — never advanced on a failed refresh.
    /// This is the number an ops dashboard should chart: "how stale is the grant
    /// view right now", independent of `fetched_at`.
    loaded_at: Instant,
    /// Time of the last refresh *attempt*, successful or not — gates how often a
    /// failing DB is re-queried once at least one load has succeeded, so a
    /// post-first-success outage costs one query per TTL, not one per request.
    fetched_at: Instant,
}

pub struct DbAudienceGrantsSource {
    pool: PgPool,
    ttl: Duration,
    snapshot: tokio::sync::RwLock<Option<Snapshot>>,
    /// Unix-epoch seconds of the last refresh *attempt*, recorded outside
    /// `Snapshot` so it exists even before any load has ever succeeded —
    /// mirrors `db_api_key.rs`'s `last_logged_at`. This is what gates
    /// cold-start retries: with no `Snapshot` yet, there is nowhere else to
    /// remember "we just tried and failed a moment ago", so without this field
    /// every `current()` call during process startup against a still-coming-up
    /// DB would re-query with no throttling at all, unlike the post-success
    /// path which `fetched_at` already gates.
    last_attempt_at: Arc<AtomicI64>,
}

impl DbAudienceGrantsSource {
    pub fn new(pool: PgPool, config: DbAudienceGrantsConfig) -> Self { ... }

    /// Returns the current grant snapshot, refreshing it first if stale.
    ///
    /// Both the cold-start path (no `Snapshot` yet) and the post-success path
    /// are throttled to at most one DB query per TTL window: cold-start via
    /// `last_attempt_at` (checked-and-set the same compare-exchange way
    /// `db_api_key.rs::maybe_log_error` rate-limits its own log line), the
    /// post-success path via `Snapshot::fetched_at` as before. A `current()`
    /// call that lands inside an already-throttled window returns the last
    /// snapshot if one exists, or the prior cold-start error if not — it never
    /// skips the query silently with nothing to show for it.
    ///
    /// `Err` only when there has never been one successful load — a fresh
    /// process whose first query hits a down DB has no "last good" to serve, so
    /// it fails closed like everything else on this seam, at a rate capped by
    /// `last_attempt_at` rather than once per request. Once any load has
    /// succeeded, a later refresh failure is logged + counted
    /// (`imetric!("audience_grant_refresh_error_count", ...)`) and the last good
    /// snapshot keeps serving, unbounded — this store has no per-item TTL
    /// eviction to fall back on the way `db_api_key.rs`'s cache does, so an
    /// outage degrades to staleness for as long as it lasts, not just one TTL
    /// window. See design question 1 below for why that trade is deliberate.
    pub async fn current(&self) -> Result<AudienceGrants> { ... }
}
```

Refresh queries the whole table (`SELECT audience, axis, selector FROM audience_grants`) and
builds an `AudienceGrants` via `AudienceGrants::from_rows`. No single-flight/dedup lock: unlike
`db_api_key.rs`'s `UPDATE ... RETURNING`, this is a plain `SELECT` with no side effect to
de-duplicate, so letting a few concurrent callers each re-run it right at the TTL boundary is
strictly simpler and still cheap (the whole point of "the map is small"). This applies to both the
cold-start and post-success paths — `last_attempt_at`/`fetched_at` bound *how often* a query fires,
not how many callers race to fire the one that's due.

### 4. Wiring into `AudienceReadPolicy` / `AudienceMintPolicy`

Both gain an optional store, added as a builder method so `from_env` keeps working unchanged for
every caller that has no DB pool (disabled-auth, tests):

```rust
impl AudienceReadPolicy {
    pub fn with_store(mut self, store: Option<Arc<DbAudienceGrantsSource>>) -> Self { ... }
}
```

`AudienceMintPolicy::with_store` is built the same way for symmetry, but — unlike the read side —
this stage wires nothing to it: no production code constructs a `dyn MintPolicy` today
(`ingestion_keys.rs::mint_key` has its own unrelated, module-local `resolve_audience` and never
calls `MintPolicy::resolve_audience`), and the trait's own doc comment already defers that wiring
to Stage 6, when `mint_key` gains a real call site. This stage's mint-side scope is exactly
`AudienceMintPolicy::with_store` existing and unit-tested, with no call site — not "wiring the
store into both policies" as a completed integration.

`resolve`/`resolve_audience` consult the store when present, merging it with the env map before
matching selectors:

```rust
async fn resolve(&self, caller: &AuthContext) -> Result<ReadableAudiences> {
    let mut grants = self.grants.clone();
    if let Some(store) = &self.store {
        grants = grants.merge(&store.current().await?);   // Err only on cold-start outage
    }
    // ... unchanged matching over `grants` instead of `self.grants` ...
}
```

`default_provider.rs::ProviderBuilder::build()` only constructs authentication (`Arc<dyn
AuthProvider>`) and has no reference to `ReadPolicy`/`MintPolicy` — it is not the wiring point.
The actual construction sites are where `AudienceReadPolicy::from_env(...)` already lives today:
`rust/public/src/servers/flight_sql_server.rs::build_and_serve` and `rust/monolith/src/main.rs`.
Each constructs one `Arc<DbAudienceGrantsSource>` (when a telemetry-DB pool is configured, via
`dedicated_key_store_pool`) and calls `.with_store(...)` on the `AudienceReadPolicy` it already
builds there — one shared snapshot cache per process, not one per policy.

### 5. Admin write surface — `analytics-web-srv`

New file `rust/analytics-web-srv/src/audience_grants.rs`, directly mirroring
`ingestion_keys.rs`'s shape (`AudienceGrantsState { pool: Option<PgPool> }`, an `IntoResponse`
error enum, `AdminUser`-gated handlers). Wired into `web_server.rs::build_protected_routes`
exactly like `ingestion_keys_router`/`analytics_keys_router`, on both sides of the
`auth_state.is_some()` branch: `audience_grants_router` merged (and its `Extension` layered)
only in the `is_some()` arm, alongside the other two key-management routers, and
`/api/audience-grants` plus its `/{*rest}` wildcard added to `key_management_disabled_router`
so the route is structurally unreachable under `--disable-auth` rather than merged
unconditionally — this route is exactly as sensitive as the other two (see Security), so it
needs the same disable-auth treatment, not just the same per-request `pool: Option<PgPool>`
503.

- `POST {base_path}/api/audience-grants` — body `{audience, axis, selector}`
  (`deny_unknown_fields`), validated with `is_valid_audience`/the same selector-shape check
  `policy.rs` uses. Unlike `import_key`'s insert-then-re-`SELECT` (safe there only because that
  table never physically deletes rows), this table has a hard `DELETE`, so a concurrent delete
  between a failed insert and a re-`SELECT` could otherwise find nothing. One round trip instead,
  via a CTE that unions the just-inserted row with the pre-existing one:
  ```sql
  WITH ins AS (
      INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
      VALUES ($1, $2, $3, now(), $4)
      ON CONFLICT (audience, axis, selector) DO NOTHING
      RETURNING audience, axis, selector, created_at, created_by
  )
  SELECT audience, axis, selector, created_at, created_by, true AS created FROM ins
  UNION ALL
  SELECT audience, axis, selector, created_at, created_by, false AS created
  FROM audience_grants
  WHERE audience = $1 AND axis = $2 AND selector = $3
    AND NOT EXISTS (SELECT 1 FROM ins);
  ```
  `created = true` ⇒ `201`; `created = false` ⇒ `200`, reporting the pre-existing row — no
  re-`SELECT` after the fact, so no window for a concurrent `DELETE` to invalidate it.

  This single statement can still return **zero rows**: Postgres data-modifying CTEs share one
  statement-level snapshot with the query around them, so when two callers race to create the
  same new `(audience, axis, selector)`, the loser's `ins` branch resolves to "do nothing" (its
  `INSERT ... ON CONFLICT` finds the winner's row already committed) while its plain-`SELECT`
  branch still runs against the snapshot taken before the winner committed — neither branch sees
  the row, and the query yields nothing to build a response from. The handler must handle this:
  if the query returns zero rows, re-run the exact same statement once more (now that the
  winner's insert has definitely committed, the loser's re-`SELECT` branch will see it and return
  `created = false`); if that retry also returns zero rows, treat it as an internal error (`500`)
  rather than looping further.
- `GET {base_path}/api/audience-grants?audience=&axis=&limit=&offset=` — lists rows, optionally
  filtered, ordered by `created_at DESC`, paginated with the same `DEFAULT_LIMIT`/`MAX_LIMIT`
  clamping convention `ingestion_keys.rs::list_keys` uses. Admin-gated like the write side: this
  route reveals who can read which audience, which is itself confidentiality-sensitive.
- `DELETE {base_path}/api/audience-grants?audience=&axis=&selector=` — natural key passed as query
  parameters, not path segments: `valid_selector` places no charset restriction on a `group:<id>`
  selector (a hierarchical IdP group name can contain `/`, `?`, or other URL-significant
  characters), so encoding it as a raw path segment the way every other route's `Uuid` id does
  would be unsafe here. Query parameters avoid that without adding a new charset restriction to
  `valid_selector` itself. `404` if no such row.

### 6. `micromegas-grants` CLI

New `python/micromegas/micromegas/cli/grants.py`, `argparse` with `add_subparsers` — `create`,
`list`, `delete` — following `screens.py`'s subcommand structure, going through `WebClient` (three
new methods: `create_audience_grant`, `list_audience_grants`, `delete_audience_grant`) rather than
direct Postgres access, matching every existing CLI in this codebase. Registered as
`micromegas-grants = "micromegas.cli.grants:main"` in `python/micromegas/pyproject.toml`.

### Design questions from the issue, settled

**1. Store outage behavior — serve the last good snapshot.** Adopted as proposed: a refresh
failure after at least one success keeps serving the stale snapshot rather than propagating
`Err`. The alternative — fail closed on every refresh failure — would turn a transient DB blip
into "every query returns nothing but `public`" for every caller in the deployment simultaneously;
that is a much larger blast radius than one key's momentarily-stale validation. `resolve()` still
denies on `Err` exactly as documented (`policy.rs:314-316`) — the fail-closed guarantee is
preserved for the one case where there is no known-good state at all (cold start against a down
DB). Staleness is bounded during any *individual* outage only by how long the outage lasts, unlike
`db_api_key.rs`'s per-key TTL bound — call this out explicitly in the doc comment and in
`docs/admin/authentication.md`, alongside the existing revocation-latency note, so it isn't
mistaken for the same bounded-TTL property.

**2. Who may create a grant; first-claim.** The admin route is `is_admin`-gated, full stop — that
answers "who may create a grant" for every path this stage ships. "Minting into an audience grants
read on it" stays rejected, as the issue requires. The narrower first-claim variant (auto-create
`read: user:<caller>` iff the audience has no existing grant rows) is a real future need for Stage
6/#1374's self-mint flow, but this stage does **not** implement it — only shapes the schema so it
needs no later migration:

```sql
INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
SELECT $1, 'read', $2, now(), $2
WHERE NOT EXISTS (SELECT 1 FROM audience_grants WHERE audience = $1)
ON CONFLICT DO NOTHING
RETURNING audience;
```
(`RETURNING` empty ⇒ someone else's grant already exists ⇒ no claim happened.) The
`WHERE NOT EXISTS` combined with the table's own primary key is what makes this a single atomic
statement with no separate locking. #1374's design should scope first-claim to a configured
namespace (e.g. an audience-name prefix or regex knob), never the whole label space, per the
issue's own squatting concern — that knob is #1374's to define, since nothing in this stage calls
the helper yet. Landing the helper function itself with no call site would be dead code; leaving it
out until #1374 needs it is the better shape (no half-finished feature) and costs #1374 nothing,
since the schema already supports it.

**3. Precedence/merge semantics.** Additive union, per `AudienceGrants::merge` (§2) — a selector
present in the env map, the store, or both, grants exactly the same access either way. A duplicate
entry across the two sources is not an error and not reported specially: it's just redundant, the
same as two identical entries within one source would be.

**4. One table vs two.** One table with an `axis` column (§1) — already the issue's own proposal,
confirmed against the long-term model's "one map, kept as one only because there is no store yet
to split it" framing. Splitting into `group_read_grants`/`group_mint_grants` is left to whichever
stage introduces the group store; nothing here forecloses it, since `ReadPolicy`/`MintPolicy`
never see the table shape.

## Implementation Steps

### Phase 1 — Schema
1. Add `upgrade_data_lake_schema_v7` to `rust/ingestion/src/sql_migration.rs`; bump
   `LATEST_DATA_LAKE_SCHEMA_VERSION` to 7; add the `execute_migration` branch.

### Phase 2 — Store-backed grant source (`micromegas-auth`)
2. Add `GrantAxis`, `AudienceGrants::from_rows`, `AudienceGrants::merge` to `policy.rs`.
3. Add `rust/auth/src/db_audience_grants.rs`: `DbAudienceGrantsConfig`, `DbAudienceGrantsSource`,
   the snapshot cache with serve-stale-on-failure semantics.
4. Add `AudienceReadPolicy::with_store`/`AudienceMintPolicy::with_store`; extend `resolve`/
   `resolve_audience` to merge in the store's snapshot.
5. Wire `DbAudienceGrantsSource` construction into
   `flight_sql_server.rs::build_and_serve` and `monolith/src/main.rs`, alongside their existing
   `AudienceReadPolicy::from_env(...)` calls — one `Arc<DbAudienceGrantsSource>` per process, built
   via `dedicated_key_store_pool` when a telemetry-DB pool is configured, passed to
   `AudienceReadPolicy::with_store`.

### Phase 3 — Admin API (`analytics-web-srv`)
6. Add `rust/analytics-web-srv/src/audience_grants.rs` (state, error type, three handlers, router
   function). In `web_server.rs::build_protected_routes`: merge `audience_grants_router` and layer
   its `Extension` only in the `auth_state.is_some()` arm, beside `analytics_keys_router`/
   `ingestion_keys_router`; add `/api/audience-grants` and `/api/audience-grants/{*rest}` to
   `key_management_disabled_router` for the `--disable-auth` arm.

### Phase 4 — CLI
7. Add `create_audience_grant`/`list_audience_grants`/`delete_audience_grant` to `web_client.py`.
8. Add `python/micromegas/micromegas/cli/grants.py` (`create`/`list`/`delete` subcommands);
   register `micromegas-grants` in `pyproject.toml`.

### Phase 5 — Tests and docs
9. Unit tests (Phase 2) + integration tests (Phase 3/4) per Testing Strategy below.
10. Update `mkdocs/docs/admin/authentication.md` and `mkdocs/docs/admin/api-keys.md`.

## Files to Modify

- `rust/ingestion/src/sql_migration.rs` — migration v7.
- `rust/auth/src/policy.rs` — `GrantAxis`, `AudienceGrants::from_rows`/`merge`, `with_store` on
  both policies, `resolve`/`resolve_audience` changes.
- `rust/auth/src/db_audience_grants.rs` — new.
- `rust/public/src/servers/flight_sql_server.rs` — construct `DbAudienceGrantsSource` and wire it
  into `AudienceReadPolicy::with_store` in `build_and_serve`.
- `rust/monolith/src/main.rs` — same wiring, alongside its existing `AudienceReadPolicy::from_env`
  call.
- `rust/analytics-web-srv/src/audience_grants.rs` — new.
- `rust/analytics-web-srv/src/web_server.rs` — merge `audience_grants_router` + layer its state
  in the `auth_state.is_some()` arm of `build_protected_routes`, alongside
  `analytics_keys_router`/`ingestion_keys_router`; add `/api/audience-grants` (+ `/{*rest}`) to
  `key_management_disabled_router`.
- `python/micromegas/micromegas/web_client.py` — three new client methods.
- `python/micromegas/micromegas/cli/grants.py` — new.
- `python/micromegas/pyproject.toml` — new script entry.
- `mkdocs/docs/admin/authentication.md`, `mkdocs/docs/admin/api-keys.md` — document the store,
  the new env knob, and the CLI.
- `CHANGELOG.md` — `## Unreleased` entry for the new table/migration, the cache-TTL knob, the
  admin routes, and the CLI.

## Trade-offs

- **No `grant_id` surrogate key.** Considered, mirroring `ingestion_api_keys.key_id`, but a grant
  has no secret component to keep unlinkable from its own identity the way a key's hash does — the
  natural key `(audience, axis, selector)` already is the row's identity, so a surrogate key would
  add a second name for the same thing with no new capability. The `DELETE` route takes the three
  fields as query parameters instead (see §5) — not path segments, since a `group:<id>` selector's
  charset isn't restricted enough to make it a safe raw path segment.
- **No `revoked_at`/`revoked_by`, hard `DELETE`.** Loses the ability to answer "who removed this
  grant and when" after the fact. Accepted for this stage because the issue's own DDL sketch
  carries no such columns and the row count is expected to stay small enough that this is a minor
  operational gap, not a security one (the *current* grant set is always correct, which is what
  every enforcement path needs); an audit trail can be added later without a breaking schema
  change if it turns out to matter.
- **Unbounded staleness on a sustained outage**, vs. `db_api_key.rs`'s per-key TTL-bounded
  staleness. Accepted per design question 1 — see reasoning there. The mitigating control is the
  `imetric!` on refresh failure, which makes an operator's dashboard, not the request path, the
  place staleness surfaces.
- **First-claim mechanism designed but not built.** Keeps this stage's diff to exactly what has a
  call site today; #1374 pays no schema cost for building it later.

## Security

- The admin route is exactly as sensitive as `ingestion_keys.rs`/`analytics_keys.rs`: it can grant
  read access to any audience to any selector, so it must stay behind `AdminUser`, never a weaker
  gate. `GET` is included in that gate (see §5) — read access to *who* can read *what* is itself a
  disclosure.
- `AudienceGrants::from_rows` re-validates every row against `is_valid_audience`/selector-shape
  rules independently of the table's `CHECK` constraints, so a row inserted by any means other
  than this stage's own INSERT path (a manual `psql` fix, a future migration) still can't produce
  an unparseable or unreadable grant silently — it fails the whole snapshot load loudly instead
  (surfaced via the refresh-failure metric, same as a DB connectivity error).
- No change to the "minting into an audience implies no read escalation" property — the admin
  route and the (not-yet-built) first-claim helper are both additive grants of `read`, never
  derived from a `mint` action.

## Testing Strategy

- **Unit** (`rust/auth/tests/`): `AudienceGrants::from_rows` validation (valid/invalid audience
  names, valid/invalid selectors); `AudienceGrants::merge` (union across disjoint audiences,
  overlapping audiences, and an identical selector present in both sources); `DbAudienceGrantsSource`
  against a test Postgres — first load succeeds, a subsequent refresh failure (simulate by pointing
  at a closed pool) keeps serving the prior snapshot, and a cold-start failure (no prior snapshot)
  returns `Err`.
- **Integration** (`rust/analytics-web-srv/tests/` or equivalent, following `ingestion_keys.rs`'s
  own test file if one exists): create/list/delete round-trip through the HTTP routes; a
  non-admin request to any of the three routes is rejected before reaching a handler; a `resolve()`
  call against `AudienceReadPolicy`/`AudienceMintPolicy` picks up a grant created via the admin
  route within the cache TTL.
- **Migration**: run `execute_migration` against a v6 database, assert the resulting version is 7
  and the table + constraints exist.

## Documentation

- `mkdocs/docs/admin/authentication.md` — add the store alongside the existing
  `{prefix}_AUDIENCE_GRANTS` section, the new `MICROMEGAS_AUDIENCE_GRANT_CACHE_TTL_SECONDS` knob,
  and the staleness-on-outage note from design question 1.
- `mkdocs/docs/admin/api-keys.md` — cross-reference the new admin route and CLI the way it already
  cross-references `{prefix}_AUDIENCE_GRANTS`; update the `--disable-auth` wording at line 343
  ("both key-management route groups return a fixed 503...") to reflect three route groups, not
  two, now that the audience-grants routes are wired into the same
  `key_management_disabled_router` mechanism.
- `CHANGELOG.md` — an `## Unreleased` entry, following every prior AbAC stage's precedent: the new
  `audience_grants` table/migration (v7), the `MICROMEGAS_AUDIENCE_GRANT_CACHE_TTL_SECONDS` knob,
  the new admin routes, and the `micromegas-grants` CLI.
- `tasks/data_isolation/audience_based_access_control_plan.md` — add a dated revision note at the
  top announcing Stage 6a (#1489), cross-referencing this plan file, following the doc's existing
  revision-log convention (e.g. "Stage 5 landed (#1373)", "Long-term model recorded 2026-08-12").

## Open Questions

- Exact config-knob name and namespace-restriction shape for Stage 6/#1374's first-claim helper —
  intentionally left to that issue's own design, per "Design questions to settle in the plan" §2
  above.
