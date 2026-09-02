# Local Group Membership and the `admins` Group Plan

Issue: [#1549](https://github.com/madesroches/micromegas/issues/1549). Part of #1334; builds on #1489
(DB-backed audience grants); supersedes #1376. Implements the "Long-term model — groups, nested
membership, and grants" section of `tasks/data_isolation/audience_based_access_control_plan.md`
(lines 736-893), with the schema simplification the issue argues for (a `member` selector column
instead of a `(member_kind, member_id)` pair).

## Overview

micromegas already owns **grants** (`audience_grants`, schema v7). It still delegates
**membership** to the IdP's flat `groups` claim and **admin-ness** to a startup env var. This plan
moves both into Postgres: two new tables (`groups`, `group_members`, schema v10), a snapshot-cached
`DbGroupsSource` beside `DbAudienceGrantsSource`, a provider wrapper that resolves each caller's
transitive membership once per request, a reserved `admins` group that replaces
`MICROMEGAS_ADMINS`, admin CRUD routes plus a Groups page and a `micromegas-groups` CLI, and the
deletion of the `groups` claim, `can_grant_admin`, and `admin_principal_possible`. A
`group_members.member` row is a selector in exactly the vocabulary `audience_grants.selector` uses
(`*`, `user:<email>`, `group:<name>`), so nesting is the `group:` arm of the same predicate rather
than a special case.

## Status

**Implemented and committed** on the `group` branch (`5be2d3539` implementation, plus eight
follow-up commits from a multi-round review pass and two deliberate design changes made after
implementation). The workspace builds clean, `cargo clippy -D warnings` and `cargo fmt --check`
pass, and the full Rust/web-app/Python test suites pass. All Implementation Steps below, every
entry in Files to Modify, and the Documentation/CHANGELOG items have landed.

Two design changes were made after the initial implementation and are already folded into the
sections below, so they read as what was actually built rather than as deltas:

- **One shared cache-TTL knob.** The plan originally specified per-role `{prefix}_` TTL knobs —
  `{prefix}_AUDIENCE_GRANT_CACHE_TTL_SECONDS` for the grant store and
  `{prefix}_GROUP_CACHE_TTL_SECONDS` for the group store — mirroring the existing
  `{prefix}_API_KEY_CACHE_TTL_SECONDS` shape. All three were consolidated into one flat,
  unprefixed `MICROMEGAS_AUTH_CACHE_TTL_SECONDS` (default 60s) shared by the API-key, audience-grant,
  and group caches. The three old names are refused at startup (`reject_removed_cache_ttl_vars`,
  alongside the admin-var refusal below) rather than silently ignored.
- **Always-wildcard admin seeding.** The plan originally had the v10 migration read
  `MICROMEGAS_ANALYTICS_ADMINS`/`MICROMEGAS_ADMINS` and seed `admins` from that list when set
  (`AdminSeed::Users`), falling back to wildcard only when unset (`AdminSeed::Everyone`) — so an
  upgrade preserved who was admin. This was simplified: the migration now **always** seeds
  `admins = ['*']` unconditionally, on every fresh install and every upgrade alike, with no env-var
  read at all. `AdminSeed`, `AdminSeed::parse`, and `admin_seed_from_env` don't exist. Every upgrade
  now needs the same manual "add `user:<you>`, then remove `*`" fixup that was previously only
  needed on fresh installs. `reject_removed_admin_vars` is unrelated to this and is unchanged: the
  three admin-list env vars are still refused at startup on every boot after migration, regardless
  of value.

A multi-round review pass (five rounds of substantive fixes, a sixth round with only trivial
findings, a finalize pass, and a seventh round after the always-wildcard-seeding change) hardened
several concurrency and consistency gaps beyond the original design: transactional locking (with
deterministic lock ordering to avoid deadlocks) around `groups`/`group_members` writes that could
otherwise race `delete_group`, `add_member`, `remove_member`, and `create_grant`'s
`group:<name>` existence check; a graph-wide (not just direct-row-count) reachability check for the
`admins` lockout guard, serialized with a `pg_advisory_xact_lock` so it holds under concurrent
removals from different nested groups; startup refusal of the old per-role cache-TTL var names
alongside the admin-var refusal; and a migration-time refusal (later superseded by the
always-wildcard change above) for non-email-shaped admin entries. None of this changed the shape of
the design below — the schema, `GroupGraph`, the route table, and the CLI are all as specified.

## Current State

### Membership comes from the IdP claim

- `rust/auth/src/types.rs:71-79` — `AuthContext.groups: Vec<String>`, documented as IdP-asserted
  leaf membership. Filled from `Claims.groups` (`rust/auth/src/oidc.rs:227-233`, `:542`); hardcoded
  empty at `rust/auth/src/api_key.rs:130`, `rust/auth/src/db_api_key.rs:358`, and the
  `--disable-auth` context at `rust/analytics-web-srv/src/web_server.rs:469`.
- `rust/auth/src/policy.rs:105-143` — `valid_selector`, `caller_selectors` (`*`, `user:<email>`,
  one `group:<g>` per claim value), and `selector_matches`, whose `group:` arm scans
  `caller.groups`. `caller_selectors` has three consumers:
  `flight_sql_service_impl.rs:626` (`CallerContext::grant_selectors`),
  `analytics-web-srv/src/audience_grants.rs` (`my_audiences`, and the `caller_holds_pair` hold
  check that strips the leading `*`).

### Admin-ness comes from an env var

- `rust/auth/src/oidc.rs:271-276` (`load_admin_users`), `:331` (`admin_users` field), `:348`
  (`OidcAuthProvider::new(config, admin_var)`), `:397-401` (`is_admin`), `:541`.
- Var-name plumbing: `rust/auth/src/default_provider.rs:64-66`, `:94`, `:206`, `:240`;
  `rust/monolith/src/main.rs:301-306`, `:386`; `rust/analytics-web-srv/src/web_server.rs:58-62`,
  `:72`, `:99`; `rust/analytics-web-srv/src/auth/state.rs:26-31`, `:64-75`;
  `rust/analytics-web-srv/src/main.rs:38`; `rust/auth/src/env.rs:2`; `rust/auth/src/lib.rs:55`;
  `rust/auth/src/multi.rs:41`.
- `MICROMEGAS_INGESTION_ADMINS` is read by the monolith's ingestion chain only; the standalone
  `rust/telemetry-ingestion-srv/` never reads it, and nothing under `rust/ingestion/` or
  `rust/object-cache-srv/` reads `is_admin`.
- `is_admin` travels: `AuthContext.is_admin` → `x-auth-is-admin` gRPC header
  (`rust/auth/src/tower.rs:102-138`) → `user_attribution::is_admin(md)` (absent header ⇒ `true`,
  the `--disable-auth` convention) → `CallerContext.is_admin`. On the web side:
  `cookie_auth_middleware` → `ValidatedUser.is_admin` → `AdminUser` extractor
  (`rust/analytics-web-srv/src/auth/handlers.rs:459-579`).

### `can_grant_admin` / `admin_principal_possible`

- `AuthProvider::can_grant_admin` (`types.rs:165-175`), overridden by `OidcAuthProvider`
  (`oidc.rs:580-584`: `!admin_users.is_empty()`) and `MultiAuthProvider` (`multi.rs:80-84`).
- Derived once at startup: `flight_sql_server.rs:371`, carried on `FlightSqlServiceImpl`
  (`flight_sql_service_impl.rs:533-536`, `:547`, `:621`) and `CallerContext.admin_principal_possible`
  (`read_scope.rs:62-72`, `:99`, `:114`).
- Three compound gates `caller.is_admin || !caller.admin_principal_possible`:
  `analytics/src/lakehouse/query.rs:128` (mutating UDTF/UDF registration),
  `query_deny_list.rs:291-292` (`skip_for_admin_recovery`), and `AudienceGuard.lakehouse_admin`
  (`audience_guard.rs:361-365`, `:468`) via `query.rs:128-134`.
- Test coverage of the two-armed gate: `analytics/tests/lakehouse_admin_gate_test.rs`,
  `query_deny_list_tests.rs:492-499`, `public/tests/read_policy_threading_tests.rs:131-139`; every
  `CallerContext` literal in `analytics/tests/*.rs` sets the field.

### The grant store to mirror

- `rust/auth/src/db_audience_grants.rs` — whole-table snapshot with a TTL
  (`{prefix}_AUDIENCE_GRANT_CACHE_TTL_SECONDS`, default 60), cold-start throttling, last-good
  serving after the first success, `ProviderUnavailable` on a cold-start outage, one metric
  (`audience_grant_refresh_error_count`). ~200 lines of cache mechanics, none of them specific to
  grants except `fetch()` and the names.
- Wiring: `flight_sql_server.rs:300-345` and `monolith/src/main.rs:261-289` build the store over a
  `dedicated_key_store_pool`; `analytics-web-srv` reads the same table through
  `AudienceGrantsState.pool` (`web_server.rs:692-753`, `MICROMEGAS_SQL_CONNECTION_STRING`,
  `Option` — 503 `NOT_CONFIGURED` when unset).
- Admin surface precedent: `analytics-web-srv/src/audience_grants.rs` (routes, `GrantGate`,
  `caller_identity` for `created_by`, `insert_or_get` UPSERT, `DELETE` with the natural key as
  query params because a selector can hold URL-significant bytes), `tests/audience_grants_tests.rs`
  (extension-injected router harness, `#[ignore]`d live-DB tests), the web app's
  `AudienceAccessPage.tsx` / `lib/audience-grants-api.ts`, and the `micromegas-grants` CLI
  (`python/micromegas/micromegas/cli/grants.py`, `web_client.py:241-283`).

### Schema

`LATEST_DATA_LAKE_SCHEMA_VERSION = 9` (`rust/ingestion/src/sql_migration.rs:8`). Migrations are
`upgrade_data_lake_schema_vN(tr)` functions chained in `execute_migration` (`:369-454`);
`tests/sql_migration_test.rs` pins throwaway schemas at v(N-1) via `build_vN_schema` helpers.
`warn_if_data_lake_schema_stale` (`:30-52`) is the only signal a non-migrating process gets.

## Design

### Data model (schema v10)

```sql
CREATE TABLE groups (
  name        VARCHAR(255) PRIMARY KEY,
  description TEXT,
  created_at  TIMESTAMPTZ NOT NULL,
  created_by  VARCHAR(255) NOT NULL,
  CONSTRAINT groups_name CHECK (name ~ '^[A-Za-z0-9_-]+$')
);

CREATE TABLE group_members (
  group_name  VARCHAR(255) NOT NULL REFERENCES groups(name) ON DELETE CASCADE,
  member      VARCHAR(255) NOT NULL,
  created_at  TIMESTAMPTZ NOT NULL,
  created_by  VARCHAR(255) NOT NULL,
  PRIMARY KEY (group_name, member),
  CONSTRAINT group_members_selector_shape CHECK (member = '*' OR member ~ '^(user|group):.+$')
);
```

One row reads one way: *every principal matching `member` is a member of `group_name`*. A
`group:X` member means X nests into `group_name`. `group:X` is a selector string, not a foreign
key: a reference to a group that does not exist matches nobody and is inert, the same as a dangling
`audience_grants` row today. The admin route refuses to create one; a row inserted by other means
is tolerated.

`groups.name` shares `is_valid_audience`'s charset so it is URL-safe, and so a group name is a
distinct kind of thing from an email. Hard `DELETE`, no `revoked_*` columns, same reasoning as
`audience_grants` (a removed membership leaves no ongoing artifact).

### Migration v10

`upgrade_data_lake_schema_v10(tr)`:

1. Create both tables.
2. Insert `('admins', 'Deployment administrators', now(), 'default')`, `ON CONFLICT (name) DO
   NOTHING`.
3. Seed `admins` with a single `('admins', '*', now(), 'default')` row, unconditionally — every
   migration, upgrade or fresh install alike, ends up with `admins = ['*']`. No env var is read to
   decide this; there is no "preserve who was admin" mode. On an upgrade this widens who can reach
   the web admin routes (`AdminUser`/`require_admin`, `GrantGate`, `MintGate`), the mint-any-audience
   arm of `AudienceMintPolicy`, the FlightSQL `bulk_ingest` gate (`do_put_statement_ingest`'s plain
   `is_admin(request.metadata())` check, now satisfiable by an API-key caller, not just OIDC), and
   `list_audience_grants()`'s all-rows (`GrantVisibility::All`) branch, all the way down to "every
   authenticated caller" — the same state the SQL admin-function gate already had. The operator is
   expected to run the standard two-step fixup (`micromegas-groups add admins user:<you>` then
   `remove admins '*'`) immediately after every migration, not just on a fresh install.
4. For every distinct `X` in `audience_grants.selector = 'group:X'`, insert an empty group `X`
   (`created_by = 'migration'`, `ON CONFLICT (name) DO NOTHING` — a pre-existing `group:admins`
   selector would otherwise collide with step 2's row and fail the whole v10 transaction) when `X`
   passes the name charset; log each. A value that fails the charset (a display name with spaces)
   cannot become a group — log it at `warn!` and leave the grant row in place, inert.

`warn_if_data_lake_schema_stale`'s message gains the v10 consequence: on a v9 schema every request
fails with a retryable 503 until the migration runs, because the group store cannot load.

### `GroupGraph` and closure resolution (`rust/auth/src/groups.rs`)

```rust
pub const ADMINS_GROUP: &str = "admins";

pub struct GroupGraph {
    groups: BTreeSet<String>,                 // every group name, members or not
    members_of: BTreeMap<String, Vec<String>>, // member selector -> groups it belongs to
}

impl GroupGraph {
    pub fn from_rows(groups: impl IntoIterator<Item = String>,
                     members: impl IntoIterator<Item = (String, String)>) -> Result<Self>;
    pub fn closure(&self, email: Option<&str>) -> Vec<String>;
    pub fn has_wildcard_admin(&self) -> bool;
    pub fn nesting_would_cycle(&self, group: &str, nested: &str) -> bool;
}
```

- `from_rows` re-runs the name charset and `valid_selector` checks the way
  `AudienceGrants::from_rows` does, so a row that slipped past the `CHECK` constraints fails the
  snapshot load rather than reaching a decision.
- `closure` is a breadth-first walk **upward** from the caller: seeds are the groups listed under
  the selectors `*` and `user:<email>`; each newly reached group `g` contributes
  `members_of["group:g"]`. The visited set is what tolerates a cycle at read time. Returned sorted,
  deduplicated. Sync and infallible: the graph is an already-loaded snapshot.
- `has_wildcard_admin()` is `self.closure(None)` (seeded at `*` alone) containing `ADMINS_GROUP`,
  reusing the same upward walk as `closure` so a wildcard reached through nesting — not just a
  direct `('admins', '*')` row — trips the check.
- `nesting_would_cycle(G, X)` — adding `group:X` to `G` (X nests into G) is a cycle when `X == G`
  or when `X` is already reachable upward from `G` (a `closure`-style walk seeded at `G` reaches
  `X`). Used by the write route against a freshly queried graph — `GroupsLoader::fetch` is `pub`,
  and the `POST .../members` handler calls it directly to build one — never the TTL snapshot (a
  stale snapshot could accept a cycle another replica just refused; the read-time visited set
  covers the race that remains).

### `DbGroupsSource` and the shared snapshot cache

`db_audience_grants.rs` is refactored into a generic `rust/auth/src/db_snapshot.rs`:

```rust
#[async_trait]
pub trait SnapshotLoader: Send + Sync + 'static {
    type Snapshot: Send + Sync;
    const NAME: &'static str;         // "audience grant store", "group store" -- error/log text
    async fn fetch(pool: &PgPool) -> Result<Self::Snapshot>;
    fn count_refresh_error();         // each impl calls imetric!() with its own literal metric name
}
pub struct SnapshotSource<L: SnapshotLoader> { /* today's fields */ }
impl<L: SnapshotLoader> SnapshotSource<L> {
    pub fn new(pool: PgPool, ttl: Duration) -> Self;
    pub async fn current(&self) -> Result<Arc<L::Snapshot>>; // calls L::count_refresh_error() at the two failure sites
}
pub type DbAudienceGrantsSource = SnapshotSource<AudienceGrantsLoader>;
pub type DbGroupsSource = SnapshotSource<GroupsLoader>;
```

`current()` keeps every property documented today (cold-start throttle, last-good serving,
`ProviderUnavailable` wrapping). `GroupsLoader::fetch` runs both `SELECT`s (`groups`,
`group_members`) in one read transaction and builds a `GroupGraph`, so a concurrent `DELETE ...
CASCADE` cannot yield a snapshot with members of a group the `groups` query missed. Config:
`DbGroupsConfig::from_env_with_prefix` reads the same **shared, flat** cache-TTL knob the grant
store and the API-key store already read — `MICROMEGAS_AUTH_CACHE_TTL_SECONDS`, default 60, via
`resolve_u64("", "AUTH_CACHE_TTL_SECONDS", 60)` — a single unprefixed value process-wide, with no
role-scoped variant, rather than a `GROUP`-specific or `{prefix}_`-fallback name (`DbApiKeyConfig`/
`DbAudienceGrantsConfig` are renamed onto this same shared, flat knob as part of this plan; the
`prefix` argument each still accepts, for call-site symmetry, is not consulted for this one
field). **Membership and admin changes take effect within
this TTL per process**; that is the documented latency.

### Where the closure is resolved: `MembershipProvider`

```rust
pub struct MembershipProvider {
    inner: Arc<dyn AuthProvider>,
    groups: Arc<DbGroupsSource>,
}
#[async_trait]
impl AuthProvider for MembershipProvider {
    async fn validate_request(&self, parts: &dyn RequestParts) -> Result<AuthContext> {
        let mut ctx = self.inner.validate_request(parts).await?;
        let graph = self.groups.current().await?;   // ProviderUnavailable propagates unchanged
        ctx.memberships = graph.closure(ctx.email.as_deref()).into();
        Ok(ctx)
    }
}
```

One resolution site, upstream of every consumer.

`AuthContext` changes (`types.rs`):

- `groups: Vec<String>` is **deleted**.
- `memberships: Arc<[String]>` is added: the caller's resolved transitive local-group membership.
  Empty until a `MembershipProvider` fills it.
- `is_admin: bool` field is **replaced by a method** `fn is_admin(&self) -> bool
  { self.memberships.iter().any(|g| g == ADMINS_GROUP) }`. `--disable-auth` and maintenance
  contexts set `memberships: Arc::from([ADMINS_GROUP.to_string()])`.

`selector_matches`' `group:` arm becomes `caller.memberships.iter().any(|g| g == group)`;
`caller_selectors` emits one `group:<g>` per membership. `AudienceReadPolicy`/`AudienceMintPolicy`
need no group store of their own.

### Wiring

- `ProviderBuilder` gains `with_group_store(pool: PgPool)`. `compose()` wraps the finished
  `MultiAuthProvider` in a `MembershipProvider` when a store is attached, and `build()`/
  `build_chain()` run one eager `current()` at startup: on `Ok`, `warn!` on every boot while
  `has_wildcard_admin()` is true ("every authenticated caller is an admin; add a `user:` member to
  `admins` and remove `*`"); on `Err`, `warn!` and continue (the first request will 503, and a
  split deployment may legitimately start flight-sql before the migration runner is up, as with
  the v5 key store).
- Attached on the **analytics** chains only: `flight_sql_server.rs`'s `use_default_auth` branch and
  `monolith/src/main.rs`'s `analytics_auth`, each over its own `dedicated_key_store_pool`. The
  ingestion chains (`telemetry-ingestion-srv`, monolith `ingestion_auth`) attach no store: nothing
  on the write path reads `is_admin()` or `memberships`, and the write path should not take a
  Postgres dependency for a value it never consults; a future ingestion route reading `is_admin()`
  gets `false`, which is fail-closed.
- `analytics-web-srv`: `AuthState.auth_provider` becomes `OnceCell<Arc<dyn AuthProvider>>` holding
  `MembershipProvider { inner: OidcAuthProvider, groups }`. `AuthState` gains a
  `groups: Arc<DbGroupsSource>` field (mirroring `MembershipProvider.groups`); `build_auth_state`
  takes the analytics-keys pool that already backs `AudienceGrantsState`/`GroupsState` as a new
  parameter and uses it to fill that field, and `get_auth_provider`'s lazy init wraps
  `OidcAuthProvider` in `MembershipProvider` over it. With auth enabled,
  `MICROMEGAS_SQL_CONNECTION_STRING` becomes **required** (startup error naming it): without it no
  session can resolve admin-ness or group grants. `WebServerConfig.admin_var_name`,
  `WebCliArgs.admin_var_name`, and `AuthState.admin_var_name` are deleted. The three test files that
  build `AuthState` literals directly (`tests/auth_unit_tests.rs`, `tests/auth_integration.rs`,
  `tests/maps_tests.rs`) need an `unreachable_pool`-style lazy `DbGroupsSource` (never awaited) to
  fill the new field.
- `OidcAuthProvider::new(config)` loses `admin_var`; `load_admin_users`, `admin_users`, and
  `OidcAuthProvider::is_admin` are deleted.
- **Removed-var refusal.** A new `micromegas_auth::env::reject_removed_admin_vars()` returns `Err`
  when any of `MICROMEGAS_ADMINS`, `MICROMEGAS_ANALYTICS_ADMINS`, or `MICROMEGAS_INGESTION_ADMINS`
  is set to any value, naming the replacement ("admin membership lives in the `admins` group;
  manage it with `micromegas-groups` or the Groups admin page"). Called from
  `ProviderBuilder::compose` and `WebServerConfig`. Same posture `IsolationConfig::from_env` takes
  for `UNSTAMPED_AUDIENCE` and #1502 takes for its removals.

### `admin_principal_possible` goes away

With `admins` always populated (`*` or explicit users), an admin principal always exists, so the
"no admin possible ⇒ open to everyone" fallback has nothing left to do:

- Delete `AuthProvider::can_grant_admin`, the `OidcAuthProvider`/`MultiAuthProvider` overrides,
  `CallerContext.admin_principal_possible`, `FlightSqlServiceImpl.admin_principal_possible` and its
  `new` parameter, and the derivation at `flight_sql_server.rs:371`.
- `query.rs:128` → `let lakehouse_admin = caller.is_admin;`. `skip_for_admin_recovery(sql,
  is_admin)` loses its third parameter. `AudienceGuard` is unchanged (it already takes the boolean).
- Once `*` is removed, no API key is admin — API-key contexts carry no email for a `user:` member
  to match, so only a wildcard can make one admin (see Migration v10 step 3).

### Admin surface (`rust/analytics-web-srv/src/groups.rs`)

`GroupsState { pool: Option<PgPool> }`, layered as an `Extension` like `AudienceGrantsState`; 503
`NOT_CONFIGURED` when the pool is absent; `key_management_disabled_router` gains `/api/groups`
and `/api/groups/{*rest}`. Every write goes through one predicate,
`can_manage_group(caller: &AuthContext, group: &str) -> bool`, which is `caller.is_admin()` today.
Two-sided authorization is thereby in place from the start: editing membership requires authority
over the group (this predicate); granting an audience to `group:X` still requires authority over
the audience (the existing `caller_holds_pair` check in `audience_grants.rs`), and delegating group
ownership later means widening one function.

| Method | Path | Gate | Behavior |
| --- | --- | --- | --- |
| `GET` | `/api/groups` | admin | `[{name, description, member_count, created_at, created_by}]` |
| `POST` | `/api/groups` | admin | `{name, description?}`; 400 on charset; 409 if it exists |
| `DELETE` | `/api/groups/{name}` | admin | 204; 409 for `admins`; 409 while referenced by any `group_members.member = 'group:<name>'` or `audience_grants.selector = 'group:<name>'` row (the response lists the referrers) |
| `GET` | `/api/groups/{name}/members` | admin | `[{group_name, member, created_at, created_by}]` |
| `POST` | `/api/groups/{name}/members` | admin | `{member}`; validates `valid_selector` + 255 bytes; for `group:X`, 404 if X does not exist and 409 if `nesting_would_cycle`; 201 created / 200 already existed via the `insert_or_get` UPSERT pattern |
| `DELETE` | `/api/groups/{name}/members?member=` | admin | 204; 404 unknown; 409 when it would remove the last row of `admins` |

`created_by` uses the existing `caller_identity`. `MyAudiencesResponse` gains a trailing `groups:
Vec<String>` (the caller's closure, straight off `AuthContext`, no query), so the CLI and the
Audience Access page can show why a caller holds a `group:` grant. `audience_grants.rs`'
`create_grant` additionally returns 404 for a `group:X` selector when no group `X` exists, since
such a grant is inert.

The "last row of `admins`" guard is the only lockout protection: removing it would leave admin
reachable only through `psql`. A caller removing their own `user:` row while `*` or another user
remains is allowed.

### Web app

- `/admin/groups` (`routes/GroupsPage.tsx`, admin-only via `AuthGuard requireAdmin` and
  `ADMIN_ONLY_PATHS`): group list with member counts and a New group dialog; selecting a group shows
  its members as chips (`*` highlighted, `group:` chips linking to that group) with remove and an
  Add member dialog whose kind toggle mirrors `GrantDialog`'s `everyone | user | group` (the group
  kind offers a select of existing groups). Error shapes reuse `ErrorBanner`/`ConfirmDialog`.
  `lib/groups-api.ts` mirrors `audience-grants-api.ts`.
- Admin hub: a `Groups` card (`adminOnly: true`), and a warning `ErrorBanner variant="warning"`
  rendered while `GET /api/groups/admins/members` contains `*` — fetched only when `user.is_admin`,
  which under the wildcard is everyone, so the warning is unmissable.
- Audience Access page: a small "Your groups" line from `my_audiences().groups`, so a `group:`
  grant a user holds is explicable.

### CLI

`micromegas-groups` (`python/micromegas/micromegas/cli/groups.py`, registered in `pyproject.toml`),
same `--url`/`--profile`/auth resolution as `micromegas-grants`:

```
micromegas-groups --url URL list
micromegas-groups --url URL create <name> [--description TEXT]
micromegas-groups --url URL delete <name>
micromegas-groups --url URL members <name>
micromegas-groups --url URL add <name> <member>
micromegas-groups --url URL remove <name> <member>
```

`WebClient` gains `list_groups`, `create_group`, `delete_group`, `list_group_members`,
`add_group_member`, `remove_group_member` (member passed as a query param on `DELETE`).

### Upgrade path

1. Deploy the new binaries and run the migration once, by starting
   `telemetry-ingestion-srv` or the monolith. None of `MICROMEGAS_ADMINS`,
   `MICROMEGAS_ANALYTICS_ADMINS`, or `MICROMEGAS_INGESTION_ADMINS` should be set anywhere in the
   deployment — the migration does not read them, and every process refuses to start (the
   removed-var check) if any is still set, regardless of value.
2. The v10 migration always seeds `admins` with a single `('admins', '*')` row, upgrade or fresh
   install alike — no exception.
3. Start everything else. Flight-sql and web processes started before the migration ran answer 503
   until it has (the schema-stale warning says so).
4. Every time, on every upgrade and every fresh install: add `user:<you>` to `admins`, then remove
   `*`. The install guide makes this the first post-migration step, always.

Anyone who relied on claim-derived `group:` grants must re-add membership by hand: the v10
migration creates each such group empty and logs it, and the Groups page lists it with zero
members.

## Mockups

- `tasks/completed/1549_local_groups_mockups/groups-admin-page.html` — Admin hub with the wildcard-admin
  banner, the Groups page (list + selected group's members), and the Add member dialog showing a
  cycle rejection. One option; the page is a sibling of the Audience Access page and inherits its
  conventions rather than choosing between layouts.

## Implementation Steps

### Phase 1 — store and closure (`rust/auth`)

1. Extract `db_audience_grants.rs`'s cache into `db_snapshot.rs` (`SnapshotLoader`,
   `SnapshotSource`); make `DbAudienceGrantsSource` a type alias over `AudienceGrantsLoader`.
   `tests/db_audience_grants_tests.rs`'s constructor calls change to
   `DbAudienceGrantsSource::new(pool, Duration::from_secs(cfg.cache_ttl_secs))`;
   `DbAudienceGrantsConfig` stays as the env-knob type. No other test behavior changes. The same
   constructor-signature change applies to the three production call sites in
   `flight_sql_server.rs` and `monolith/src/main.rs` (already listed under Files to Modify).
2. Add `groups.rs`: `ADMINS_GROUP`, `GroupGraph` (`from_rows`, `closure`, `has_wildcard_admin`,
   `nesting_would_cycle`), `GroupsLoader`, `DbGroupsConfig`, `DbGroupsSource`.
3. Add `membership.rs`: `MembershipProvider`.
4. `types.rs`: delete `groups`, add `memberships`, turn `is_admin` into a method, delete
   `AuthProvider::can_grant_admin`. Fix every construction/read site the compiler reports
   (`api_key.rs`, `db_api_key.rs`, `oidc.rs`, `multi.rs`, `tower.rs`, `policy.rs`, tests).
5. `policy.rs`: `selector_matches`/`caller_selectors` read `memberships`; update the doc comments
   that describe `group:` as a claim match.
6. `oidc.rs`: delete `Claims.groups`, `load_admin_users`, `admin_users`, `is_admin`,
   `can_grant_admin`; `new(config)`.
7. `env.rs`: `reject_removed_admin_vars`. `default_provider.rs`: delete `admin_var`, add
   `with_group_store`, wrap in `compose`, eager load + wildcard warning in `build`/`build_chain`,
   call `reject_removed_admin_vars`. Update `lib.rs`/`multi.rs` doc examples.

### Phase 2 — schema (`rust/ingestion`)

8. `sql_migration.rs`: `upgrade_data_lake_schema_v10`, which always seeds `admins` with a single
   `('admins', '*')` row (no env access, no seed parameter); chain it in `execute_migration`, bump
   `LATEST_DATA_LAKE_SCHEMA_VERSION` to 10, extend `warn_if_data_lake_schema_stale`'s message.
9. No dedicated unit test for the seeding itself — it is a single hardcoded `INSERT`, covered by
   manual verification alongside the rest of this migration's behavior (see Testing Strategy).

### Phase 3 — analytics gate collapse

10. `read_scope.rs`: delete `admin_principal_possible`; reword the `MICROMEGAS_ADMINS` references at
    `:139`/`:160` (the comma-separated-vs-JSON contrast can cite `MICROMEGAS_API_KEYS` instead).
    `tests/ownership_rewrite_config_tests.rs`: update the error-text assertion that names
    "MICROMEGAS_ADMINS-style JSON array" to match the reworded text.
11. `query.rs:128`, `query_deny_list.rs:291`, `audience_guard.rs` doc comments;
    `flight_sql_service_impl.rs` (`new`, field, `caller_context`, `:799`); `flight_sql_server.rs`
    (`:371`, `with_group_store` on the `use_default_auth` branch);
    `tonic_auth_interceptor.rs:35`, whose log statement reads `auth_ctx.is_admin` and becomes a
    call to `is_admin()`.
12. Rewrite `lakehouse_admin_gate_test.rs` to the one-armed gate (drop the two
    `admin_principal_possible` tests, keep the other four); update `query_deny_list_tests.rs:492`,
    `read_policy_threading_tests.rs:131-139`, `firehose_tests.rs`, and
    `resolve_write_audience_tests.rs` (both construct `AuthContext` literals with the removed
    `is_admin`/`groups` fields), and every `CallerContext` literal in `analytics/tests/`.

### Phase 4 — monolith and services

13. `monolith/src/main.rs`: `with_group_store` on `analytics_auth`; delete `analytics_admin_var`
    and the `admin_var_name` argument. Note: `telemetry-ingestion-srv` attaches no store, but the
    builder's removed-var check applies there too, with no code change to that crate.

### Phase 5 — web server

14. `auth/state.rs` (add `AuthState.groups: Arc<DbGroupsSource>`, `get_auth_provider` wraps
    `OidcAuthProvider` in `MembershipProvider` over it), `auth/handlers.rs` (add an `AuthApiError`
    503 variant and downcast `ProviderUnavailable` to it at both `validate_request` call sites,
    `cookie_auth_middleware` and `auth_me`), `auth/claims.rs` (`is_admin()`),
    `web_server.rs` (delete `admin_var_name`, require `MICROMEGAS_SQL_CONNECTION_STRING` under
    auth, thread the analytics-keys pool into `build_auth_state` to fill `AuthState.groups`,
    `--disable-auth` contexts use `memberships: [admins]`, `GroupsState`, router merge, disabled-
    router prefixes, call `warn_if_data_lake_schema_stale(&analytics_keys_pool)` when auth is
    enabled, mirroring the flight-sql call site), `main.rs`.
15. `groups.rs` routes; `audience_grants.rs`: `groups` field on `MyAudiencesResponse`, group
    existence check on `create_grant`; `ingestion_keys.rs`/`analytics_keys.rs`: `is_admin()`.
16. `tests/groups_tests.rs` (modeled on `audience_grants_tests.rs`), `routing_tests.rs` 503
    assertions for the new prefixes, `auth_unit_tests.rs`/`auth_integration.rs`/`maps_tests.rs`
    (add the `groups` field, `unreachable_pool`-style, to each `create_test_auth_state()` literal),
    `web_server_config_tests.rs` updates for the removed var; `ingestion_keys_tests.rs`, which
    constructs `AuthContext` literals with the removed `is_admin`/`groups` fields. Delete
    `cookie_auth_middleware_inserts_auth_context_with_groups` (`auth_integration.rs`), which
    asserts membership from the removed `groups` claim; replace it with the planned
    `ProviderUnavailable` → 503 assertion from Testing Strategy (no live DB needed; the store uses
    an `unreachable_pool`-style seam).

### Phase 6 — clients

17. Web app: `lib/groups-api.ts`, `routes/GroupsPage.tsx`, `AdminPage.tsx` card + banner,
    `AppShell.tsx` `ADMIN_ONLY_PATHS`, `router.tsx`, Audience Access "Your groups" line, tests.
18. Python: `web_client.py` methods, `cli/groups.py`, `pyproject.toml` script,
    `tests/cli/test_groups.py`, `tests/test_web_client.py`; update the `bulk_ingest` docstring in
    `flightsql/client.py:600-628`.

### Phase 7 — docs and scripts

19. Documentation section below; `local_test_env/ai_scripts/start_services_with_oidc.py:20-22`,
    `:147-149` and `analytics-web-app/start_analytics_web_docker.py:245-247` drop
    `MICROMEGAS_ADMINS`; the latter script also gains a `MICROMEGAS_SQL_CONNECTION_STRING` entry in
    `env_vars`, built the same `host.docker.internal` way as `app_db_conn_string`, whenever
    `MICROMEGAS_OIDC_CONFIG` is set — otherwise the container fails the new required-var check as
    soon as auth is enabled; `CHANGELOG.md` entry; status note at the top of the AbAC plan's
    long-term section pointing here.

## Files to Modify

Rust, `rust/auth/src/`: `types.rs`, `policy.rs`, `oidc.rs`, `multi.rs`, `tower.rs`, `axum.rs`,
`api_key.rs`, `db_api_key.rs`, `db_audience_grants.rs`, `default_provider.rs`, `env.rs`, `lib.rs`;
new `db_snapshot.rs`, `groups.rs`, `membership.rs`; tests `oidc_tests.rs`, `policy_tests.rs`,
`multi_tests.rs`, `default_provider_tests.rs`, `db_audience_grants_tests.rs`, `tower_tests.rs`,
`db_api_key_tests.rs`, `test_utils.rs`; new `groups_tests.rs`, `membership_tests.rs`.

Rust, elsewhere: `rust/ingestion/src/sql_migration.rs`, `rust/ingestion/Cargo.toml`,
`rust/ingestion/tests/sql_migration_test.rs`;
`rust/analytics/src/lakehouse/{read_scope,query,query_deny_list,audience_guard}.rs`,
`rust/analytics/tests/{lakehouse_admin_gate_test,query_deny_list_tests,audience_guard_tests,
prong_b_guard_db_test,ownership_rewrite_db_test,ownership_rewrite_public_view_set_tests,
ownership_rewrite_config_tests,retire_partition_by_metadata_db_test,
list_audience_grants_db_test}.rs`;
`rust/public/src/servers/{flight_sql_server,flight_sql_service_impl,tonic_auth_interceptor}.rs`,
`rust/public/tests/{read_policy_threading_tests,firehose_tests,
resolve_write_audience_tests}.rs`; `rust/monolith/src/main.rs`;
`rust/analytics-web-srv/src/{lib,main,web_server,
audience_grants,ingestion_keys,analytics_keys}.rs`, `rust/analytics-web-srv/src/auth/{state,
handlers,claims}.rs`, new `rust/analytics-web-srv/src/groups.rs`,
`rust/analytics-web-srv/tests/{audience_grants_tests,routing_tests,auth_unit_tests,
auth_integration,web_server_config_tests,maps_tests,ingestion_keys_tests}.rs`, new `groups_tests.rs`.

Web app: `src/routes/AdminPage.tsx`, `src/routes/AudienceAccessPage.tsx`, new
`src/routes/GroupsPage.tsx`, new `src/lib/groups-api.ts`, `src/components/layout/AppShell.tsx`,
`src/router.tsx`, tests under `src/routes/__tests__/` and `src/lib/__tests__/`.

Python: `micromegas/web_client.py`, new `micromegas/cli/groups.py`, `pyproject.toml`,
`micromegas/flightsql/client.py`, `tests/test_web_client.py`, new `tests/cli/test_groups.py`.

Docs and scripts: see Documentation; `CHANGELOG.md`; `local_test_env/ai_scripts/
start_services_with_oidc.py`; `analytics-web-app/start_analytics_web_docker.py`;
`tasks/data_isolation/audience_based_access_control_plan.md`.

## Trade-offs

- **Closure on `AuthContext` via a provider wrapper vs. inside the policies.** The policy-side
  design would need the group store in both policies plus the three `caller_selectors` sites and a
  separate admin-resolution path, and would sit behind `OidcAuthProvider`'s token cache unless
  admin-ness were pulled out separately. One wrapper resolves everything once; the cost is that
  `AuthContext` carries a resolved authorization fact, which `is_admin` and `read_audiences`
  already do.
- **`is_admin` as a method, not a field.** A field is one more thing every construction site must
  set consistently with `memberships`. The method makes inconsistency unrepresentable, and changing
  field to method is exactly the kind of Rust break `CLAUDE.md` prefers when it makes the compiler
  enumerate the call sites.
- **Whole-table snapshot vs. per-subject `moka` closure cache.** The AbAC plan sketched a bounded
  per-subject cache. A whole-table snapshot is what `audience_grants` already uses, the graph is
  small, and BFS over an in-memory graph per request is cheaper than a cache lookup plus its
  eviction machinery. Invalidation semantics are also simpler to document: one TTL.
- **Generic `SnapshotSource` vs. a copied `DbGroupsSource`.** Copying ~200 lines of throttling
  logic would leave two implementations to keep in step. The generic costs one trait with one
  associated const and two fns.
- **No `list_groups()` SQL UDTF.** `micromegas-grants` has no `list` because `list_audience_grants()`
  exists; groups get REST list routes and a CLI `list`/`members` instead, since admin-only data
  has no self-service SQL audience. A UDTF can follow if a SQL surface is wanted.
- **Group reads admin-only.** Exposing every group's member emails to any authenticated user is
  wider than what the Audience Access page exposes today (held pairs only). The caller's own
  closure is exposed through `my_audiences().groups`, which is all the self-service UI needs.

## Decisions

- Migration-time seeding reads no env var at all: `admins` always gets a single `('admins', '*')`
  row, on every v10 migration, upgrade or fresh install alike. There is no "preserve who was admin"
  mode — simpler to reason about and to document than a conditional seed, at the cost of requiring
  the operator to run the add-user/remove-wildcard fixup after every upgrade, not just a fresh
  install. The removed-var refusal (`reject_removed_admin_vars`) is unrelated to seeding and still
  refuses startup, on every boot, whenever any of the three vars is set to any value.
- A `group:` selector counts as an identity selector in `caller_holds_pair` even when the group's
  membership is `*`. An operator who grants an audience to a wildcard-membered group has chosen
  to make that audience shareable by everyone; the check is not second-guessing that.
- `user:` selectors keep matching `AuthContext.email` only.
- Legacy `group:X` selectors whose `X` fails the name charset are left as inert grant rows with a
  migration warning, not deleted and not force-renamed.
- `MICROMEGAS_SQL_CONNECTION_STRING` is required by `analytics-web-srv` whenever auth is enabled.
- `micromegas-groups` does not get a `bootstrap` convenience command; the two-command sequence
  (`add admins user:<me>` then `remove admins '*'`) stays documented as-is.
- `member` is a selector string, not a `(member_kind, member_id)` pair — per the issue.
- No directory sync — per the issue.
- No live-DB tests for this feature; coverage is the no-DB unit tests in Testing Strategy plus
  manual verification — starting the local stack against a v9 database and reading the migration
  logs, and exercising groups CRUD through the Groups page and `micromegas-groups`.

## Documentation

- New `mkdocs/docs/admin/groups.md`, in `mkdocs.yml` nav under Administration after
  Authentication: the model, the selector table, nesting and cycles, the `admins` group, the
  wildcard warning and first-login step, the routes table, `micromegas-groups`, the TTL knob and
  latency, outage behavior, the v10 upgrade path — including the wildcard widening enumerated in
  Migration v10 step 3, which now applies on every upgrade, not just a fresh install.
- `mkdocs/docs/admin/authentication.md`: selector table at `:415-421` (`group:<g>` → "members of
  local group `g`, transitively"), the `MICROMEGAS_ADMINS` lines at `:90`, `:104`, `### Admin
  Privileges` at `:1196-1211`, and the `GrantGate` create/delete wording at `:673-693`.
- `mkdocs/docs/admin/{flight-sql,ingestion,monolith,api-keys}.md`: remove the `MICROMEGAS_ADMINS`
  / `_ANALYTICS_ADMINS` / `_INGESTION_ADMINS` rows and prose (`flight-sql.md:30,58,73`,
  `ingestion.md:31`, `monolith.md:111-113`, `api-keys.md:93-95,338-341,362-365`); document the
  group store's cache-TTL knob as the same shared, flat `MICROMEGAS_AUTH_CACHE_TTL_SECONDS` the
  API-key and grant stores already read (`DbApiKeyConfig`/`DbAudienceGrantsConfig` rename onto
  this shared, flat knob too — a minor breaking env-var rename, not additive; unlike every other
  knob in this crate it has no `{prefix}_`-scoped variant, since one value governs all three
  caches process-wide); add `groups`/`group_members` to the grant recipe in `api-keys.md`.
- `mkdocs/docs/admin/web-app.md`: Groups page, hub banner, `MICROMEGAS_SQL_CONNECTION_STRING` now
  required under auth. `mkdocs/docs/admin/functions-reference.md:452-462`: "no email, no groups" →
  "no email, no memberships". `mkdocs/docs/query-guide/python-api.md`: `micromegas-groups` section
  beside `micromegas-grants`, `bulk_ingest` admonition at `:519`.
- `CHANGELOG.md` (`## Unreleased`, **Auth**): operator-facing break (`MICROMEGAS_ADMINS` family
  removed and refused; `groups` claim no longer read; existing `group:` grants now mean local
  groups), the v10 migration and seeding rules — document the wildcard widening enumerated in
  Migration v10 step 3; call out that this reopens `bulk_ingest` to analytics API-key callers
  specifically until `*` is removed, contradicting the earlier claim that API keys can never be
  admin (`CHANGELOG.md:270`, the five mutating SQL functions) and the `bulk_ingest` admin notes in
  `mkdocs/docs/admin/flight-sql.md:73`, `mkdocs/docs/query-guide/python-api.md:519-523`, and
  `python/micromegas/micromegas/flightsql/client.py:600-628` — and a **Minor breaking change**
  clause listing
  `AuthContext.groups` → `memberships`, `is_admin` field → method, `AuthProvider::can_grant_admin`
  removed, `OidcAuthProvider::new` signature, `CallerContext.admin_principal_possible` removed,
  `FlightSqlServiceImpl::new` and `skip_for_admin_recovery` parameters, `DbAudienceGrantsSource`
  now a type alias.

## Testing Strategy

- `rust/auth/tests/groups_tests.rs` (no DB): closure for a direct `user:` member; via `*`; a
  three-level chain `alice ∈ a`, `group:a ∈ b`, `group:b ∈ c` resolves `{a, b, c}` and a member of
  `c` alone resolves `{c}` (edge direction); a diamond; a cycle `group:a ∈ b`, `group:b ∈ a`
  terminates; `has_wildcard_admin`; `nesting_would_cycle` for self-nesting and a two-step loop;
  `from_rows` rejects a bad name and a bad selector.
- `membership_tests.rs`: wrapper fills `memberships` and `is_admin()`; inner `Err` passes through
  unchanged; store `ProviderUnavailable` propagates as `ProviderUnavailable`.
- `policy_tests.rs`: `group:` selector matches a membership and not a same-named email.
- `db_audience_grants_tests.rs`: constructor calls updated to
  `DbAudienceGrantsSource::new(pool, Duration::from_secs(cfg.cache_ttl_secs))`, behavior otherwise
  unchanged; one smoke test for `DbGroupsSource` over a `connect_lazy` pool hitting the cold-start
  error path.
- `default_provider_tests.rs`: `MICROMEGAS_ADMINS` set ⇒ `Err` naming the replacement.
- `sql_migration_test.rs`: no dedicated unit test for the wildcard seed itself — it is a single
  hardcoded `INSERT`, not conditional logic. v10's seeding, backfill, and `CHECK`-constraint
  behavior is verified manually (see the Manual bullet below).
- Analytics: `lakehouse_admin_gate_test.rs` asserts non-admin cannot plan the mutating functions
  and admin can, with no second arm; `query_deny_list_tests.rs` and the `prong_b_guard_db_test.rs`
  `'global'`-row test updated to the one-armed gate.
- `analytics-web-srv/tests/groups_tests.rs`: 403 for non-admin on every route; 400 bad name/
  selector; 409 deleting `admins`. The CRUD round trip, the cycle/delete-while-referenced/
  last-admins-row conflict responses, and the missing-group 404 are verified manually (see the
  Manual bullet below). `routing_tests.rs`: `--disable-auth` 503 for `/api/groups*` — this
  exercises the disabled router's `AUTH_DISABLED` 503, not `GroupsState`'s `NOT_CONFIGURED` branch,
  which is kept only for shape parity with `AudienceGrantsState` and is unreachable in either mode.
  `auth_integration.rs`: `ProviderUnavailable` from the group store → 503 (no live DB needed; the
  store uses an `unreachable_pool`-style seam).
- Web app: `GroupsPage.test.tsx` (list, add member, cycle error surfaced, remove), `AdminPage.test.tsx`
  (banner shown iff `*` present and admin), `groups-api.test.ts`.
- Python: `tests/cli/test_groups.py` dispatch tests mirroring `test_grants.py`;
  `test_web_client.py` payload/query-param shapes.
- Manual: the v10 migration is verified by starting the local stack against a v9 database and
  reading the startup logs — schema version bump, the wildcard admin seed, and any `group:X`
  backfill/warning lines. `start_services_with_oidc.py` against that same fresh
  DB shows the wildcard warning at boot and the hub banner; `micromegas-groups add admins
  user:<me>` then `remove admins '*'` clears both within the TTL; a non-admin then loses
  `retire_partitions` and the Groups page. The groups CRUD round trip, cycle/conflict responses,
  and the missing-group 404 are exercised the same way, through the Groups page and
  `micromegas-groups`.
