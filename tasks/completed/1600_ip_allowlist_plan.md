# IP Allowlisting for API Keys Plan (#1600)

## Overview

Let an API key be pinned to a set of source IP addresses/CIDR ranges. This covers both the
DB-backed keys used by ingestion, flight-sql and analytics-web-srv (`DbApiKeyAuthProvider`), and
the env/keyring-based keys used by `object-cache-srv` (`ApiKeyAuthProvider` — the only remaining
non-test construction site of the env keyring; ingestion/flight-sql/analytics-web-srv compose only
OIDC → `DbApiKeyAuthProvider`, per `rust/auth/src/default_provider.rs`). A request presenting a
valid key from an IP outside its allowlist is rejected exactly like an invalid key. An
empty/absent allowlist means "no restriction" — the backward-compatible default for every key
that exists today.

## Current State

### Credential validation has no request-metadata input

- `AuthProvider::validate_request(&self, parts: &dyn RequestParts) -> Result<AuthContext>`
  (`rust/auth/src/types.rs:168-171`) is the only entry point every provider implements. `RequestParts`
  (`rust/auth/src/types.rs:90-108`) exposes headers/method/uri — no client IP.
- `ApiKeyAuthProvider::validate_request` (`rust/auth/src/api_key.rs:92-138`) does a constant-time
  scan of the keyring (`KeyRing = HashMap<Key, String>`, name only) and returns an `AuthContext` on
  match.
- `DbApiKeyAuthProvider::validate_request` (`rust/auth/src/db_api_key.rs:257-383`) hashes the
  bearer token, checks a short-TTL `moka` cache, and on a miss runs
  `UPDATE {table} SET last_used_at = now() WHERE key_hash = $1 AND revoked_at IS NULL RETURNING
  key_id, name[, audience]` (`db_api_key.rs:290-296`). The returned `KeyRow` (`db_api_key.rs:170-176`)
  is what would need to grow an allowlist field.

### The client IP is already resolved, but only for logging

- `get_client_ip(headers: &http::HeaderMap, extensions: &http::Extensions) -> String`
  (`rust/public/src/servers/http_utils.rs:32-69`) picks, in order: the rightmost
  `X-Forwarded-For` entry (the ALB's own observation), then `X-Real-IP`, then a `SocketAddr`
  extension (axum's `ConnectInfo` or tonic's transport-level connect info via
  `rust/public/src/servers/connect_info_layer.rs`). It lives in the `public` crate and is called
  from `axum_utils.rs`, `http_gateway.rs`, `log_uri_service.rs`, and twice in
  `flight_sql_service_impl.rs` (`:1187`, `:1357`) — all for audit/request logging
  (`query_audit.rs:87`), never for an authorization decision.
- **Circular-dependency constraint**: `micromegas-auth` (the `auth` crate) does not depend on
  `micromegas` (the `public` crate) — it's the other way around. The four places that turn a raw
  request into `RequestParts` — `auth_middleware` (`rust/auth/src/axum.rs:40-89`), `AuthService`
  (`rust/auth/src/tower.rs:58-161`), `check_auth` (`rust/public/src/servers/tonic_auth_interceptor.rs:10-44`,
  living in `public` but calling into `auth` — currently uncalled anywhere in the workspace), and
  `firehose_auth_middleware` (`rust/public/src/servers/firehose_common.rs:92`) — live inside or are
  called by `auth`, so `get_client_ip`'s logic can't be reused from `public` as-is; it has to move
  down.
- For gRPC, the `SocketAddr` extension is already present on `req.extensions()` by the time
  `AuthService::call` runs — `ConnectedIncoming`/`ConnectedStream` (`connect_info_layer.rs:15-108`)
  attach it at TCP-accept time, below every custom tower layer, and `flight_sql_service_impl.rs`
  already reads it successfully after the same `AuthService` layer has run. For HTTP, axum's
  `into_make_service_with_connect_info::<SocketAddr>()` (`rust/public/src/servers/ingestion.rs:214`)
  does the same for `auth_middleware`.

### Admin routes that mint keys

- `rust/analytics-web-srv/src/ingestion_keys.rs` and `analytics_keys.rs` are near-identical (by
  design — see that module's own "Duplication, accepted" doc comment) REST surfaces:
  `mint_key`, `list_keys`, `revoke_key`, each with its own request/response struct
  and a single `INSERT`/`UPDATE`.
- There is no import path: every key is generated server-side by its mint route, so an allowlist
  can only arrive at mint time or through the new `PATCH` route below.
- The analytics-web-app (`analytics-web-app/src/lib/api-keys-shared.ts`,
  `ingestion-api-keys-api.ts`, `MintIngestionKeyDialog.tsx`, `IngestionApiKeysPage.tsx`, and the
  analytics counterparts) is the browser admin UI for the same routes.

### No CIDR-matching dependency yet

No crate in the workspace parses/matches IP CIDR ranges today (checked every `Cargo.toml`).

## Design

### 1. A typed, allowlist-only IP allowlist type (`rust/auth/src/ip_allowlist.rs`, new)

Add the `ipnet` crate (`IpNet`, parses `a.b.c.d/n` / `xxxx::/n` into a network+mask; already in
`rust/Cargo.lock` at `2.12.0`, pulled in transitively by `hyper-util`, so this adds no new crate to
the build graph) as a `micromegas-auth` dependency, pinned to `2.12` in the root `Cargo.toml` to
match.

```rust
pub struct IpAllowlist(Vec<IpNet>);

impl IpAllowlist {
    /// Empty input -> empty allowlist -> "no restriction" (`allows` always true).
    pub fn parse(entries: &[String]) -> Result<Self>; // fails on the first unparseable entry
    pub fn allows(&self, ip: Option<IpAddr>) -> bool; // empty -> true; ip == None && !empty -> false
}
```

`allows` is the single decision point both providers call — no restriction when the stored list
is empty, deny-if-unresolved once it isn't (an "unknown" client IP must never satisfy a
restriction). `IpNet::from_str` doesn't accept a bare IP without a prefix, so `parse` falls back
to it explicitly: `s.parse::<IpNet>().or_else(|_| s.parse::<IpAddr>().map(IpNet::from))`, giving
bare IPs like `"203.0.113.7"` a `/32` or `/128`.

### 2. Threading the client IP into `RequestParts`

Move `get_client_ip`'s decision logic into the `auth` crate as the typed primitive everything
else is built from:

```rust
// rust/auth/src/client_ip.rs (new)
pub fn resolve_client_ip(headers: &http::HeaderMap, extensions: &http::Extensions) -> Option<IpAddr>;
```

`resolve_client_ip` returns `ip.to_canonical()` (stable since Rust 1.75) on whichever address it
resolves, so an IPv4-mapped IPv6 form (`::ffff:a.b.c.d`) arriving from a dual-stack listener
unmaps to `a.b.c.d` right here, the single normalization point — independent of listener
configuration, and before `IpAllowlist::allows` ever sees the address, so an allowlist entry of
`a.b.c.d/32` matches either arrival form. Same priority order (rightmost `X-Forwarded-For`, then
`X-Real-IP`, then the `SocketAddr` extension) and the same parse-or-fall-through behavior as
today's `get_client_ip`, plus the `to_canonical()` call above — a deliberate behavior change from
today's code, not a verbatim copy, that also normalizes the form logged for audit purposes.
Because header sources take priority over the socket peer, the resolved IP is only
trustworthy for a request that actually traversed the load balancer — which is the deployment
shape this feature targets (see `## Decisions`). No trusted-proxy configuration is added.

`rust/public/src/servers/http_utils.rs::get_client_ip` becomes a thin wrapper:

```rust
pub fn get_client_ip(headers: &http::HeaderMap, extensions: &http::Extensions) -> String {
    micromegas_auth::client_ip::resolve_client_ip(headers, extensions)
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
```

No existing caller of `get_client_ip` changes (`axum_utils.rs`, `http_gateway.rs`,
`log_uri_service.rs`, `flight_sql_service_impl.rs`, `firehose_common.rs`); its existing unit
tests (`rust/public/tests/http_utils_tests.rs`) move to `rust/auth/tests/client_ip_tests.rs`
against `resolve_client_ip`, and `http_utils_tests.rs` shrinks to a couple of smoke tests that the
wrapper still formats `None` as `"unknown"`.

Extend `RequestParts` (`rust/auth/src/types.rs`) with a required method:

```rust
pub trait RequestParts: Send + Sync {
    ...
    /// The resolved client IP, `None` when nothing resolves.
    fn client_ip(&self) -> Option<std::net::IpAddr>;
}
```

`HttpRequestParts`/`GrpcRequestParts` each gain a `pub client_ip: Option<IpAddr>` field and
implement `client_ip()` by returning it. `CookieTokenRequestParts`
(`rust/analytics-web-srv/src/auth/claims.rs`), the one other implementer, implements `client_ip()`
by returning `None`, matching every other method on that adapter. Every construction site
computes it up front:

- `rust/auth/src/axum.rs::auth_middleware` — `resolve_client_ip(req.headers(), req.extensions())`
  before building `HttpRequestParts` (axum's `ConnectInfo<SocketAddr>` extension is already on
  `req` for every service that enables it, same as `get_client_ip`'s existing HTTP callers rely
  on).
- `rust/auth/src/tower.rs::AuthService::call` — `resolve_client_ip(&parts.headers, &parts.extensions)`
  right after `req.into_parts()`, before headers are cloned into `GrpcRequestParts`.
- `rust/public/src/servers/tonic_auth_interceptor.rs::check_auth` — same call, using
  `req.metadata().as_ref()` and `req.extensions()`, mirroring `flight_sql_service_impl.rs`'s
  existing two call sites.
- `rust/public/src/servers/firehose_common.rs::firehose_auth_middleware` — same call before
  building its own `HttpRequestParts`.

Every other `HttpRequestParts { .. }` / `GrpcRequestParts { .. }` literal (mostly in
`rust/auth/tests/*`) gains `client_ip: None` — deliberate: those tests exercise auth logic
unrelated to IP, and `None` is a no-op for the allowlist check. A handful of tests for the new
allowlist behavior override it explicitly (see Testing Strategy).

### 3. `ApiKeyAuthProvider` (env/keyring, `object-cache-srv`'s auth path) — `rust/auth/src/api_key.rs`

`KeyRingEntry` gains an optional field:

```rust
#[derive(Deserialize)]
pub struct KeyRingEntry {
    pub name: String,
    #[serde(deserialize_with = "key_from_string")]
    pub key: Key,
    /// CIDR ranges or bare IPs this key may be used from. Absent/empty = unrestricted.
    #[serde(default)]
    pub allowed_cidrs: Vec<String>,
}
```

`KeyRing`'s value type changes from `String` (name) to a small struct so the parsed allowlist
travels with the name:

```rust
pub struct KeyRingValue {
    pub name: String,
    pub allowlist: IpAllowlist,
}
pub type KeyRing = HashMap<Key, KeyRingValue>;
```

`parse_key_ring` calls `IpAllowlist::parse(&entry.allowed_cidrs)` per entry and fails the whole
parse (fail-fast at startup, same as every other keyring-shape error today) on a malformed entry.

`validate_request` keeps the same constant-time full-keyring scan (unchanged — the allowlist
check happens on public data, so there is no timing side-channel to protect for it specifically,
and it runs after the loop, on `found` only, so the loop's constant-time property over the
*key comparison* is untouched). After the loop:

```rust
let matched = found.ok_or_else(|| anyhow!("invalid API token"))?;
if !matched.allowlist.allows(parts.client_ip()) {
    anyhow::bail!("invalid API token: source IP not permitted");
}
Ok(matched.context)
```

(`found` becomes `Option<(KeyRingValue-derived AuthContext, IpAllowlist)>`, or the allowlist is
looked up a second time from `stored_key`/`name` — implementation detail, either shape works;
the point is the IP check happens once, after the scan, not inside the hot loop.)

### 4. `DbApiKeyAuthProvider` — `rust/auth/src/db_api_key.rs`

`KeyRow` (`:170-176`) gains an `allowlist: IpAllowlist` field parsed from the `allowed_cidrs`
column read (every pre-v11 row reads back `NULL`, which sqlx decodes as `None`, not an empty
`Vec` — see §5) once per cache fill (not per request) — added alongside `audience` as part of the
same `RETURNING` clause and the same `try_get_with` loader closure:

```rust
let returning = if table.has_audience() {
    "key_id, name, audience, allowed_cidrs"
} else {
    "key_id, name, allowed_cidrs"
};
```

Store the parsed `IpAllowlist` (not the raw `Option<Vec<String>>`) in the cached `KeyRow` so a hot
cache hit never re-parses CIDR strings — `IpAllowlist::parse` is fed `&[]` when the column read
back `None`, giving the same "no restriction" allowlist as an empty stored array. After a cache
hit or a fresh load, before returning `Ok(AuthContext { .. })`:

```rust
if !row.allowlist.allows(parts.client_ip()) {
    return Err(anyhow!("invalid API token: source IP not permitted"));
}
```

Allowlist changes take effect within `cache_ttl_secs`, same as revocation.

An IP-allowlist rejection is **not** cached in `self.unknown` (that cache means "no such live
key", not "this key exists but this caller may not use it from here") and must not increment
`db_api_key_error_count` (that metric means "the DB was unreachable", not "a credential was
rejected") — both existing invariants, unchanged, just confirmed not to trigger for the new
rejection path since it happens after the `try_get_with` closure returns successfully.

### 5. Schema — migration v11 (`rust/ingestion/src/sql_migration.rs`)

Adds one nullable `TEXT[]` column to each table, following the exact shape of migration v6's
`audience` column add (and the existing `TEXT[]` precedent — `lakehouse_partitions.sort_order`,
`blocks.tags`):

```sql
ALTER TABLE ingestion_api_keys ADD COLUMN allowed_cidrs TEXT[];
ALTER TABLE analytics_api_keys ADD COLUMN allowed_cidrs TEXT[];
UPDATE migration SET version=11;
```

No backfill needed (unlike `audience`, which had to become `NOT NULL`): `NULL`/absent means "no
restriction", which is exactly the correct value for every pre-existing row. `LATEST_DATA_LAKE_SCHEMA_VERSION`
becomes `11`, which requires defining `upgrade_data_lake_schema_v11` (running the two `ALTER
TABLE`s above) and adding its `if 10 == current_version { ... }` arm to `execute_migration`
(`rust/ingestion/src/sql_migration.rs`), following the same per-version dispatch pattern as every
prior version bump — otherwise `execute_migration`'s closing `assert_eq!` panics at startup for
any service running the migration against a pre-v11 database.

Reading the column back: a nullable `TEXT[]` must decode into `Option<Vec<String>>`, not a bare
`Vec<String>` — sqlx returns an `UnexpectedNull` decode error on a bare `Vec<String>` target when
the column is `NULL`, which every pre-v11 row is. Decode as `Option<Vec<String>>` and treat `None`
as an empty slice before handing it to `IpAllowlist::parse` (equivalently, `SELECT
COALESCE(allowed_cidrs, '{}')` in the `RETURNING`/`SELECT` clauses and decode straight into
`Vec<String>`) — either way, every existing row must parse without error.

**Deploy ordering**: `migrate_db` (which runs this migration) is only invoked by
`telemetry-ingestion-srv` and `monolith`; `flight-sql-srv` and `analytics-web-srv` never run it, so
a new build of either can start against a pre-v11 database. Its `RETURNING ... allowed_cidrs`
query would then fail with "column allowed_cidrs does not exist" on every DB-API-key request. The
service that runs the migration (`telemetry-ingestion-srv`/`monolith`) must therefore be deployed
— and the migration must have completed — before any new `flight-sql-srv`/`analytics-web-srv`
build reaches production, the same ordering constraint migration v6 already documents for
`audience`.

### 6. Admin routes — `ingestion_keys.rs` / `analytics_keys.rs`

`MintRequest` gains `allowed_cidrs: Option<Vec<String>>` (`#[serde(default)]`, additive field,
existing callers omitting it keep working). `mint_key` validates with `IpAllowlist::parse(...)`
up front (same place `validate_name`/`resolve_audience` already run) and returns the existing
`BadRequest` variant on a malformed entry — no new error variant needed. The validated (but not re-normalized — store what the caller wrote, same as
`audience`'s literal-string convention) list is bound into the `INSERT` alongside the existing
columns, in every branch that currently issues one (`insert_key`, both `INSERT`s inside
`try_claim_and_mint`).

`KeyListEntry` gains `allowed_cidrs: Vec<String>` (decoded via the same `Option`-to-empty mapping
as §4/§5, since `KeyListEntry` derives `sqlx::FromRow` directly off the row) so `list_keys`
surfaces the restriction (empty = unrestricted, matching every other list-response convention in
this API). `list_keys`'s `SELECT` statements enumerate columns explicitly, so each one must add
`allowed_cidrs` too, or the `FromRow` decode fails at runtime (see Implementation Steps Phase 3).

New route, one per table, admin-only (`AdminUser`, same gate as `revoke_key`/`list_keys` — this
is not a self-service action, unlike `mint_key`):

```
PATCH {base_path}/api/ingestion-api-keys/{key_id}/allowlist
PATCH {base_path}/api/analytics-api-keys/{key_id}/allowlist
Body: { "allowed_cidrs": [...] }   // [] clears the restriction
```

`UPDATE {table} SET allowed_cidrs = $2 WHERE key_id = $1 RETURNING allowed_cidrs`, `NotFound` on
no row — same shape as `revoke_key`. This is the only way to change an existing key's allowlist
without revoking and re-minting it, and is what closes the "keys can be updated with an
allowlist" half of the issue's rough idea that mint alone doesn't cover.

## Mockups

None — this is a backend/API/CLI feature with no new screens or layout changes. The
analytics-web-app surfacing is tracked separately in #1611.

## Implementation Steps

### Phase 1 — Client IP plumbing (no behavior change yet)
1. Add `rust/auth/src/client_ip.rs` with `resolve_client_ip`; move
   `rust/public/tests/http_utils_tests.rs`'s cases to `rust/auth/tests/client_ip_tests.rs` against it.
2. Turn `rust/public/src/servers/http_utils.rs::get_client_ip` into the thin wrapper described
   above; trim its test file to the `None -> "unknown"` formatting case.
3. Add `client_ip()` as a required method on the `RequestParts` trait (`rust/auth/src/types.rs`).
4. Add the `client_ip` field to `HttpRequestParts`/`GrpcRequestParts`; update every construction
   site (`axum.rs`, `tower.rs`, `tonic_auth_interceptor.rs`, `firehose_common.rs`, and every test
   literal — set `None` in tests unrelated to this feature), including the two compiled doctests in
   `rust/auth/src/lib.rs` (the ```rust example and the ```rust,no_run example),
   both of which build a `HttpRequestParts { .. }` struct literal and must add `client_ip: None` to
   keep compiling under `cargo test --doc`. Add an explicit `client_ip()` returning `None` to
   `CookieTokenRequestParts` (`rust/analytics-web-srv/src/auth/claims.rs`), the trait's one other
   implementer, since the method is now required.
5. `cargo build --workspace` — this phase changes no auth *decisions*, only threads data through;
   every existing test should pass unmodified except for the moved/renamed IP-resolution tests.

### Phase 2 — `IpAllowlist` and provider changes
1. Add `ipnet = "2.12"` to `rust/auth/Cargo.toml` (alphabetical, per `rust/CLAUDE.md`) and the
   workspace root, matching the version already resolved in `Cargo.lock`.
2. Add `rust/auth/src/ip_allowlist.rs` (`IpAllowlist::parse`/`allows`) with full unit
   coverage (Testing Strategy).
3. Update `KeyRing`/`KeyRingEntry`/`parse_key_ring` and `ApiKeyAuthProvider::validate_request`
   (`rust/auth/src/api_key.rs`).
4. Migration v11 (`rust/ingestion/src/sql_migration.rs`): add `allowed_cidrs TEXT[]` to both
   tables, define `upgrade_data_lake_schema_v11`, add its `if 10 == current_version` arm to
   `execute_migration`, and bump `LATEST_DATA_LAKE_SCHEMA_VERSION`.
5. Update `KeyRow`/`DbApiKeyAuthProvider::validate_request` (`rust/auth/src/db_api_key.rs`) to load,
   cache, and check the new column.

### Phase 3 — Admin HTTP routes
1. `ingestion_keys.rs`: `MintRequest`/`KeyListEntry` fields, validation, `INSERT`
   binds (`insert_key`, both `try_claim_and_mint` inserts), new `PATCH .../{key_id}/allowlist`
   route + handler.
2. `analytics_keys.rs`: the same set of changes, mirroring `ingestion_keys.rs` (per that module's
   own "duplication, accepted" precedent — no shared helper introduced).
3. `ingestion_keys.rs` and `analytics_keys.rs`: extend all four `list_keys` `SELECT` column lists
   (the `include_revoked` on/off variant in each module) with
   `COALESCE(allowed_cidrs, '{}') AS allowed_cidrs`, since each `SELECT` enumerates columns
   explicitly and `KeyListEntry` decodes straight off the row.

### Phase 4 — Python client
1. `python/micromegas/micromegas/web_client.py`: new
   `set_ingestion_api_key_allowlist`/`set_analytics_api_key_allowlist` methods wrapping the new
   `PATCH` routes.
2. Add `allowed_cidrs=None` to `mint_ingestion_api_key` (and its analytics counterpart), passed
   through in the request body only when set — same omit-when-`None` convention already used for
   `audience`. Without this, a non-admin caller minting their own key via `mint_ingestion_api_key`
   would have no way to attach an allowlist at all, since the new `PATCH` route is `AdminUser`-gated.

## Files to Modify

- `rust/auth/src/types.rs` — `RequestParts::client_ip()`, `HttpRequestParts`/`GrpcRequestParts` field
- `rust/analytics-web-srv/src/auth/claims.rs` — `CookieTokenRequestParts::client_ip()` impl
- `rust/auth/src/client_ip.rs` (new) — moved IP-resolution logic
- `rust/auth/src/ip_allowlist.rs` (new) — `IpAllowlist`
- `rust/auth/src/api_key.rs` — `KeyRing`/`KeyRingEntry`/`parse_key_ring`/`validate_request`
- `rust/auth/src/db_api_key.rs` — `KeyRow`/`validate_request`
- `rust/auth/src/axum.rs`, `rust/auth/src/tower.rs` — resolve+pass `client_ip`
- `rust/auth/Cargo.toml`, root `Cargo.toml` — `ipnet` dependency
- `rust/public/src/servers/http_utils.rs` — thin wrapper over `client_ip::resolve_client_ip`
- `rust/public/src/servers/tonic_auth_interceptor.rs`, `firehose_common.rs` — resolve+pass `client_ip`
- `rust/ingestion/src/sql_migration.rs` — migration v11
- `rust/analytics-web-srv/src/ingestion_keys.rs`, `analytics_keys.rs` — request/response fields, new
  `PATCH` route
- `python/micromegas/micromegas/web_client.py`
- `rust/auth/tests/api_key_tests.rs`, `db_api_key_tests.rs`, new `client_ip_tests.rs`,
  `ip_allowlist_tests.rs`, and every other test constructing `HttpRequestParts`/`GrpcRequestParts`

## Trade-offs

- **`get_client_ip` moves down into `auth`, rather than passing a `SocketAddr`/`HeaderMap` pair
  through `RequestParts` for each provider to resolve itself.** Resolving once, at the same three
  call sites that already build `RequestParts`, means every `AuthProvider` (including third-party
  ones per `multi.rs`'s own doc comment) gets a ready-made `Option<IpAddr>` instead of
  reimplementing the `X-Forwarded-For`/`X-Real-IP`/socket priority order. The cost is the `auth`
  crate now owns logic that used to live in `public`; mitigated by making `public`'s
  `get_client_ip` a one-line wrapper so no external behavior changes.
- **The allowlist check lives inline in `ApiKeyAuthProvider`/`DbApiKeyAuthProvider`, not as a
  separate wrapping provider (the `MembershipProvider` pattern).** `MembershipProvider` wraps
  *any* inner `AuthProvider` because group membership is resolved from the caller's *identity*
  (email), independent of credential kind. An IP allowlist is metadata *on the credential itself*
  (the keyring entry / DB row) — exactly where `bound_audience`/`read_audiences` already live
  inline in these same two providers — so a wrapper would need the same per-key data plumbed back
  out to it anyway, with no benefit.
- **`ipnet` over hand-rolled CIDR parsing.** IPv6 mask arithmetic is easy to get subtly wrong
  (off-by-one prefix lengths, mixed v4/v6 comparison); a small, well-established crate is worth
  using, and it's a better fit than `ipnetwork`, which is not currently a dependency of anything in
  this workspace (see §1 for the build-graph and bare-IP-fallback detail).
- **Rejection is a generic `"invalid API token"`, not a distinguishable `403`/error code.** A
  distinct client-visible signal ("your key is valid but your IP isn't allowed") would help a
  legitimate caller debug faster, but also confirms to anyone holding a leaked key that it's
  live and merely IP-restricted — narrowing their remaining search space. Matching the existing
  "wrong key" response exactly costs debuggability but not security; the mismatch is still visible
  server-side in logs for the operator who controls the allowlist.
- **`TEXT[]` column over a JSON/JSONB column.** `TEXT[]` matches the two existing array columns in
  this schema (`blocks.tags`, `lakehouse_partitions.sort_order`) and needs no serde round-trip;
  JSONB would only pay off if entries ever needed structure beyond a bare string, which CIDR
  notation doesn't.

## Decisions

- IP allowlisting is for deployments behind a load balancer. `resolve_client_ip` keeps
  `get_client_ip`'s header-first priority order, so a caller that reaches a service directly,
  bypassing the load balancer, can name any source IP via `X-Forwarded-For`. This is documented as
  a deployment requirement rather than enforced with a trusted-proxy setting.

- The analytics-web-app UI is out of scope here and tracked in #1611. This plan covers Phases 1-4
  (client-IP plumbing, the `IpAllowlist` type and provider enforcement, the admin routes, and the
  Python client), which are independently useful and testable without a browser surface.

- The API-key import path (the `micromegas-import-keys` CLI and both
  `POST .../{table}-api-keys/import` routes) was removed in a separate change earlier on this
  branch (#1609), so this plan carries no allowlist plumbing for it — mint and the new `PATCH`
  route are the only two ways an allowlist reaches a row.

## Documentation

- `rust/auth/src/lib.rs`'s two compiled doctests construct `HttpRequestParts` struct literals —
  updating them is a required Phase 1 step 4 edit (see Implementation Steps), not a follow-up
  check.
- `mkdocs/docs/admin/api-keys.md`: its route/request-body table (`POST
  {base_path}/api/ingestion-api-keys` etc.) needs the new `allowed_cidrs` field and the new
  `PATCH .../allowlist` route added; its `## Schema` DDL blocks for both `ingestion_api_keys` and
  `analytics_api_keys` need the new `allowed_cidrs TEXT[]` column added; its existing "Deploy
  ordering matters in the other direction too" paragraph needs a second case added for migration
  v11 — the migration-running service (`telemetry-ingestion-srv`/`monolith`) must deploy, and the
  migration must complete, before any new `flight-sql-srv`/`analytics-web-srv` build reaches
  production (see Design §5).
- `mkdocs/docs/admin/api-keys.md` (allowlist section): must state that an IP allowlist is only
  enforceable when every request reaches the service through a load balancer that sets or
  overwrites `X-Forwarded-For`. A deployment where a client can connect to the service directly
  can present any source IP by sending that header, so the allowlist restricts nothing there.
- `mkdocs/docs/query-guide/python-api.md`: its `mint_ingestion_api_key(name, audience=None)`
  signature and `WebClient` method list need the new `allowed_cidrs` mint parameter and the two
  `set_*_allowlist` methods added.
- `mkdocs/docs/admin/object-cache.md` and `object-cache-srv`'s `--api-keys` doc: need the new
  `allowed_cidrs` keyring-entry field documented (see Overview/§3 on `object-cache-srv` being the
  env-keyring's sole consumer).
- `CHANGELOG.md`: new column + new provider-side behavior is additive at the SQL/API layer (no
  breaking change there), but `KeyRing`'s value type change (`String` -> `KeyRingValue`) is a
  **Minor breaking change** to the Rust API surface — record it per the Interface Stability policy.

## Testing Strategy

Per this repo's verification-tier rule, most of this is covered by no-DB unit tests;
`DbApiKeyAuthProvider`'s existing `#[ignore]`d live section is for a different purpose (DB
wiring/latency), not new-feature acceptance. The route-level round-trip through a real table
(mint -> `list_keys` -> `PATCH .../allowlist`) does need a live Postgres and is covered under
Manual Verification instead, per the same rule and this file's own module-doc precedent (see
below).

- `rust/auth/tests/ip_allowlist_tests.rs` (new): empty list allows any IP including `None`;
  single bare-IP entry matches only that exact address (v4 and v6); a `/24`/`/64` entry matches
  every address in range and rejects one outside it; a non-empty list rejects `None` (unresolved
  client IP); malformed entries in `parse` return `Err` (bad CIDR syntax, out-of-range prefix
  length, empty string).
- `rust/auth/tests/client_ip_tests.rs` (moved from `rust/public/tests/http_utils_tests.rs`): same
  cases as today, retargeted at `resolve_client_ip` and asserting `Option<IpAddr>` instead of a
  formatted string; plus a new case that an IPv4-mapped IPv6 socket address (`::ffff:a.b.c.d`)
  resolves to the canonical `a.b.c.d`.
- `rust/auth/tests/api_key_tests.rs`: a keyring entry with a restrictive `allowed_cidrs` accepts a
  request whose `HttpRequestParts.client_ip` is inside it and rejects one outside it (including a
  `client_ip: None` request against a restricted key); an entry with no `allowed_cidrs` is
  unaffected (regression check that the existing `test_valid_api_key`/`test_invalid_api_key` keep
  passing unchanged).
- `rust/auth/tests/db_api_key_tests.rs`: extend the existing no-DB pattern — a canned `KeyRow`
  (constructed directly, no `try_get_with`/DB round trip) with a populated `allowed_cidrs` is
  checked against in-range and out-of-range `parts.client_ip()` values. Confirms an IP-allowlist
  rejection is not written into `self.unknown` and does not increment `db_api_key_error_count`
  (both by constructing the scenario directly against a `DbApiKeyAuthProvider` backed by a lazy,
  never-queried pool wherever the existing tests already use that pattern — e.g.
  `missing_bearer_token_fails_before_any_db_access`).
- `rust/analytics-web-srv` route tests (`ingestion_keys_tests.rs`/`analytics_keys_tests.rs`): only
  the malformed-`allowed_cidrs` -> `400` case is no-DB reachable (`IpAllowlist::parse` runs before
  `require_pool`'s first query, so the existing `lazy_pool()` fixture — the same one
  `missing_bearer_token_fails_before_any_db_access`-style tests already use — gets past mint
  validation without ever connecting). The round-trip-through-`list_keys`, `PATCH .../allowlist`
  update/clear, and 404-on-unknown-`key_id` cases all reach an `INSERT`/`UPDATE` and move to
  Manual Verification instead, per this file's own module-doc rule that every route-level test
  reaching an `INSERT` is `#[ignore]`d and run manually against a real DB.
- `rust/auth/tests/axum_tests.rs` (extend): a restricted keyring entry, layered through the real
  `Router`/`auth_middleware` stack this file already builds, accepts a request whose resolved
  `SocketAddr`/header-derived IP is in range, and rejects one from an out-of-range IP and one with
  no resolvable IP at all — a no-DB, no-network seam that directly exercises auth enforcement
  end-to-end without needing Manual Verification for it.
- `rust/public/tests/read_policy_threading_tests.rs`-style tonic test (new or extended): unlike that
  file's existing `start_server`, which serves raw `TcpStream`s and never attaches a `SocketAddr`
  extension, this test must wrap its listener in `ConnectedIncoming::new(listener)` (see
  `rust/public/src/servers/connect_info_layer.rs`) so the extension `resolve_client_ip` reads is
  actually present; then serve a restricted key's `AuthService` over it and confirm a connection
  from an in-range peer address is accepted and an out-of-range one is rejected.
- Python: `python/micromegas/tests/test_web_client.py` — the two new `set_*_allowlist` methods
  build the expected `PATCH` URL and body, following `TestAudienceGrants`'s style.

## Manual Verification

1. Start services (`python3 local_test_env/ai_scripts/start_services.py`), mint an ingestion key
   with `allowed_cidrs: ["127.0.0.1/32"]` via the admin route, then send an ingestion request
   through a real ALB-shaped `X-Forwarded-For` header set to a different address (expect the same
   generic auth failure a wrong key would get). The `axum_tests.rs` case already covers enforcement
   itself; this step is only to eyeball the real `X-Forwarded-For` priority logic against actual
   proxy header shapes, which that in-process test approximates but doesn't confirm.
2. Repeat against `flight-sql-srv` (gRPC) with an analytics key and a real proxy in front of it, to
   eyeball the real transport-level `Connected::connect_info()` header shape — the
   `read_policy_threading_tests.rs`-style case already covers enforcement itself.
3. Mint a key with `allowed_cidrs`, confirm it round-trips through `list_keys`, then exercise the
   `PATCH .../allowlist` route: it updates the column, clears it back to unrestricted, and 404s on
   an unknown `key_id` — the DB-backed coverage moved out of the automated route tests above.
