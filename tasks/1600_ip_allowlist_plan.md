# IP Allowlisting for API Keys Plan (#1600)

## Overview

Let an API key (env/keyring-based or DB-backed, ingestion or analytics) be pinned to a set of
source IP addresses/CIDR ranges. A request presenting a valid key from an IP outside its
allowlist is rejected exactly like an invalid key. An empty/absent allowlist means "no
restriction" — the backward-compatible default for every key that exists today.

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
  `micromegas` (the `public` crate) — it's the other way around. The three places that turn a raw
  request into `RequestParts` — `auth_middleware` (`rust/auth/src/axum.rs:40-89`), `AuthService`
  (`rust/auth/src/tower.rs:58-161`), and `check_auth` (`rust/public/src/servers/tonic_auth_interceptor.rs:10-44`,
  the last one living in `public` but calling into `auth`) — live inside or are called by `auth`,
  so `get_client_ip`'s logic can't be reused from `public` as-is; it has to move down.
- For gRPC, the `SocketAddr` extension is already present on `req.extensions()` by the time
  `AuthService::call` runs — `ConnectedIncoming`/`ConnectedStream` (`connect_info_layer.rs:15-108`)
  attach it at TCP-accept time, below every custom tower layer, and `flight_sql_service_impl.rs`
  already reads it successfully after the same `AuthService` layer has run. For HTTP, axum's
  `into_make_service_with_connect_info::<SocketAddr>()` (`rust/public/src/servers/ingestion.rs:214`)
  does the same for `auth_middleware`.

### Admin routes and CLI that mint/import keys

- `rust/analytics-web-srv/src/ingestion_keys.rs` and `analytics_keys.rs` are near-identical (by
  design — see that module's own "Duplication, accepted" doc comment) REST surfaces:
  `mint_key`, `list_keys`, `revoke_key`, `import_key`, each with its own request/response struct
  and a single `INSERT`/`UPDATE`.
- `python/micromegas/micromegas/cli/import_keys.py` reads a legacy JSON keyring (optionally
  carrying a per-entry `"audience"` field for `--table ingestion`) and calls
  `WebClient.import_ingestion_api_key`/`import_analytics_api_key`
  (`python/micromegas/micromegas/web_client.py:216`, `:376`).
- The analytics-web-app (`analytics-web-app/src/lib/api-keys-shared.ts`,
  `ingestion-api-keys-api.ts`, `MintIngestionKeyDialog.tsx`, `IngestionApiKeysPage.tsx`, and the
  analytics counterparts) is the browser admin UI for the same routes.

### No CIDR-matching dependency yet

No crate in the workspace parses/matches IP CIDR ranges today (checked every `Cargo.toml`).

## Design

### 1. A typed, allowlist-only IP allowlist type (`rust/auth/src/ip_allowlist.rs`, new)

Add the `ipnetwork` crate (`IpNetwork`, parses both `IpAddr` and `a.b.c.d/n` /
`xxxx::/n` into a network+mask) as a `micromegas-auth` dependency.

```rust
pub struct IpAllowlist(Vec<IpNetwork>);

impl IpAllowlist {
    /// Empty input -> empty allowlist -> "no restriction" (`allows` always true).
    pub fn parse(entries: &[String]) -> Result<Self>; // fails on the first unparseable entry
    pub fn is_empty(&self) -> bool;
    pub fn allows(&self, ip: Option<IpAddr>) -> bool; // empty -> true; ip == None && !empty -> false
}
```

`allows` is the single decision point both providers call — no restriction when the stored list
is empty, deny-if-unresolved once it isn't (an "unknown" client IP must never satisfy a
restriction). Bare IPs (`"203.0.113.7"`) parse as `/32` or `/128` via `IpNetwork::from_str`.

### 2. Threading the client IP into `RequestParts`

Move `get_client_ip`'s decision logic into the `auth` crate as the typed primitive everything
else is built from:

```rust
// rust/auth/src/client_ip.rs (new)
pub fn resolve_client_ip(headers: &http::HeaderMap, extensions: &http::Extensions) -> Option<IpAddr>;
```

Same priority order (rightmost `X-Forwarded-For`, then `X-Real-IP`, then the `SocketAddr`
extension) and the same parse-or-fall-through behavior as today's `get_client_ip` — copied
verbatim, just returning `Option<IpAddr>` instead of formatting to `String`.
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

Extend `RequestParts` (`rust/auth/src/types.rs`) with a defaulted method:

```rust
pub trait RequestParts: Send + Sync {
    ...
    /// The resolved client IP, `None` when nothing resolves. Default `None` so no existing
    /// non-HTTP/gRPC implementer of this trait needs to change.
    fn client_ip(&self) -> Option<std::net::IpAddr> { None }
}
```

`HttpRequestParts`/`GrpcRequestParts` each gain a `pub client_ip: Option<IpAddr>` field and
implement `client_ip()` by returning it. Every construction site computes it up front:

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
unrelated to IP, and `None` behaves exactly like today's untouched trait default. A handful of
tests for the new allowlist behavior override it explicitly (see Testing Strategy).

### 3. `ApiKeyAuthProvider` (env/keyring) — `rust/auth/src/api_key.rs`

`KeyRingEntry` gains an optional field:

```rust
#[derive(Deserialize)]
pub struct KeyRingEntry {
    pub name: String,
    #[serde(deserialize_with = "key_from_string")]
    pub key: Key,
    /// CIDR ranges or bare IPs this key may be used from. Absent/empty = unrestricted.
    #[serde(default)]
    pub allowed_ips: Vec<String>,
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

`parse_key_ring` calls `IpAllowlist::parse(&entry.allowed_ips)` per entry and fails the whole
parse (fail-fast at startup, same as every other keyring-shape error today) on a malformed entry.

`validate_request` keeps the same constant-time full-keyring scan (unchanged — the allowlist
check happens on public data, so there is no timing side-channel to protect for it specifically,
and it runs after the loop, on `found` only, so the loop's constant-time property over the
*key comparison* is untouched). After the loop:

```rust
let matched = found.ok_or_else(|| anyhow!("invalid API token"))?;
if !matched.allowlist.allows(parts.client_ip()) {
    anyhow::bail!("invalid API token"); // deliberately the same message as a wrong key
}
Ok(matched.context)
```

(`found` becomes `Option<(KeyRingValue-derived AuthContext, IpAllowlist)>`, or the allowlist is
looked up a second time from `stored_key`/`name` — implementation detail, either shape works;
the point is the IP check happens once, after the scan, not inside the hot loop.)

The rejection message is identical to a wrong-key rejection (`"invalid API token"`) — this only
ever reaches server-side logs (`rust/auth/src/axum.rs`/`tower.rs` already collapse every non-
`ProviderUnavailable` error to a generic `401`/`"Invalid token"` response body), so keeping the
text distinguishable in logs (e.g. `"invalid API token: source IP not permitted"`) is safe and
useful for operators; the *response* stays generic either way.

### 4. `DbApiKeyAuthProvider` — `rust/auth/src/db_api_key.rs`

`KeyRow` (`:170-176`) gains an `allowed_cidrs: Vec<String>` field, parsed into an `IpAllowlist`
once per cache fill (not per request) — added alongside `audience` as part of the same
`RETURNING` clause and the same `try_get_with` loader closure:

```rust
let returning = match (table.has_audience(), /* always true, both tables get this column */) {
    ... "key_id, name, audience, allowed_cidrs" | "key_id, name, allowed_cidrs" ...
};
```

Store the parsed `IpAllowlist` (not the raw `Vec<String>`) in the cached `KeyRow` so a hot cache
hit never re-parses CIDR strings. After a cache hit or a fresh load, before returning `Ok(AuthContext
{ .. })`:

```rust
if !row.allowlist.allows(parts.client_ip()) {
    return Err(anyhow!("invalid API token: source IP not permitted"));
}
```

This inherits the same staleness bound the module's doc comment already documents for
`revoked_at`: a changed allowlist takes up to `cache_ttl_secs` to take effect on a given process,
exactly like revocation latency today — no new caveat, same one, restated for this field too.

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
becomes `11`.

Reading the column back: `sqlx`'s `TEXT[]` support maps directly to `Option<Vec<String>>` /
`Vec<String>` (already used for `blocks.tags`, `lakehouse_partitions.sort_order` elsewhere in the
codebase) — no custom decode needed. `NULL` reads as an empty `Vec` (or `None` mapped to empty)
before being handed to `IpAllowlist::parse`.

### 6. Admin routes — `ingestion_keys.rs` / `analytics_keys.rs`

Each of `MintRequest`/`ImportRequest` gains `allowed_cidrs: Option<Vec<String>>` (`#[serde(default)]`,
additive field, existing callers omitting it keep working). Both `mint_key` and `import_key`
validate with `IpAllowlist::parse(...)` up front (same place `validate_name`/`resolve_audience`
already run) and return the existing `BadRequest` variant on a malformed entry — no new error
variant needed. The validated (but not re-normalized — store what the caller wrote, same as
`audience`'s literal-string convention) list is bound into the `INSERT` alongside the existing
columns, in every branch that currently issues one (`insert_key`, both `INSERT`s inside
`try_claim_and_mint`).

`KeyListEntry` gains `allowed_cidrs: Vec<String>` so `list_keys` surfaces the restriction
(empty = unrestricted, matching every other list-response convention in this API).

New route, one per table, admin-only (`AdminUser`, same gate as `revoke_key`/`list_keys` — this
is not a self-service action, unlike `mint_key`):

```
PATCH {base_path}/api/ingestion-api-keys/{key_id}/allowlist
PATCH {base_path}/api/analytics-api-keys/{key_id}/allowlist
Body: { "allowed_cidrs": [...] }   // [] clears the restriction
```

`UPDATE {table} SET allowed_cidrs = $2 WHERE key_id = $1 RETURNING allowed_cidrs`, `NotFound` on
no row — same shape as `revoke_key`. This is the only way to change an existing key's allowlist
without revoking and re-minting it, and is what closes the "keys can be created/updated with an
allowlist" half of the issue's rough idea that mint/import alone don't cover.

### 7. CLI — `python/micromegas/micromegas/cli/import_keys.py`

- `read_keyring` accepts an optional per-entry `"allowed_ips"` array (same shape/validation style
  as the existing `"audience"` field — a list-of-strings check, no CIDR parsing client-side; the
  server is the single source of truth for whether a CIDR string is valid).
- New `--allowed-ips` CLI flag (`nargs="+"`), same precedence as `--audience`: a per-entry value
  wins, the flag is the fallback, neither given means unrestricted (omit the field, server default
  applies — which for a *new* row is "no restriction", not "inherit"). Valid for both `--table`
  values (unlike `--audience`, which analytics rows don't carry).
- `import_one`/`run_import` thread the resolved list through to
  `WebClient.import_ingestion_api_key`/`import_analytics_api_key`, which both gain an
  `allowed_ips=None` keyword parameter appended last (additive, existing call sites unaffected).

## Mockups

None — this is a backend/API/CLI feature with no new screens or layout changes. The
analytics-web-app surfacing (Implementation Steps, Phase 5) reuses existing list/dialog
components with one new field each; see Open Questions for why that phase is left flexible on
scope.

## Implementation Steps

### Phase 1 — Client IP plumbing (no behavior change yet)
1. Add `rust/auth/src/client_ip.rs` with `resolve_client_ip`; move
   `rust/public/tests/http_utils_tests.rs`'s cases to `rust/auth/tests/client_ip_tests.rs` against it.
2. Turn `rust/public/src/servers/http_utils.rs::get_client_ip` into the thin wrapper described
   above; trim its test file to the `None -> "unknown"` formatting case.
3. Add `client_ip()` to the `RequestParts` trait (`rust/auth/src/types.rs`), default `None`.
4. Add the `client_ip` field to `HttpRequestParts`/`GrpcRequestParts`; update every construction
   site (`axum.rs`, `tower.rs`, `tonic_auth_interceptor.rs`, `firehose_common.rs`, and every test
   literal — set `None` in tests unrelated to this feature).
5. `cargo build --workspace` — this phase changes no auth *decisions*, only threads data through;
   every existing test should pass unmodified except for the moved/renamed IP-resolution tests.

### Phase 2 — `IpAllowlist` and provider changes
1. Add `ipnetwork` to `rust/auth/Cargo.toml` (alphabetical, per `rust/CLAUDE.md`) and the
   workspace root.
2. Add `rust/auth/src/ip_allowlist.rs` (`IpAllowlist::parse`/`is_empty`/`allows`) with full unit
   coverage (Testing Strategy).
3. Update `KeyRing`/`KeyRingEntry`/`parse_key_ring` and `ApiKeyAuthProvider::validate_request`
   (`rust/auth/src/api_key.rs`).
4. Migration v11 (`rust/ingestion/src/sql_migration.rs`): add `allowed_cidrs TEXT[]` to both
   tables, bump `LATEST_DATA_LAKE_SCHEMA_VERSION`.
5. Update `KeyRow`/`DbApiKeyAuthProvider::validate_request` (`rust/auth/src/db_api_key.rs`) to load,
   cache, and check the new column.

### Phase 3 — Admin HTTP routes
1. `ingestion_keys.rs`: `MintRequest`/`ImportRequest`/`KeyListEntry` fields, validation, `INSERT`
   binds (`insert_key`, both `try_claim_and_mint` inserts), new `PATCH .../{key_id}/allowlist`
   route + handler.
2. `analytics_keys.rs`: the same set of changes, mirroring `ingestion_keys.rs` (per that module's
   own "duplication, accepted" precedent — no shared helper introduced).

### Phase 4 — CLI and Python client
1. `python/micromegas/micromegas/web_client.py`: `allowed_ips` parameter on
   `import_ingestion_api_key`/`import_analytics_api_key`, plus new
   `set_ingestion_api_key_allowlist`/`set_analytics_api_key_allowlist` methods wrapping the new
   `PATCH` routes.
2. `python/micromegas/micromegas/cli/import_keys.py`: `"allowed_ips"` keyring field, `--allowed-ips`
   flag, threaded through `read_keyright`/`import_one`/`run_import`.

### Phase 5 — analytics-web-app surfacing (see Open Questions on scope)
1. `api-keys-shared.ts`: `allowed_cidrs?: string[]` on `ApiKeyListEntry`; `mint()` gains an
   optional param.
2. `MintIngestionKeyDialog.tsx` (+ the analytics equivalent): an optional CIDR-list input.
3. `IngestionApiKeysPage.tsx` / `AnalyticsApiKeysPage.tsx`: show the restriction in the list, add
   an edit action calling the new `PATCH` route.

## Files to Modify

- `rust/auth/src/types.rs` — `RequestParts::client_ip()`, `HttpRequestParts`/`GrpcRequestParts` field
- `rust/auth/src/client_ip.rs` (new) — moved IP-resolution logic
- `rust/auth/src/ip_allowlist.rs` (new) — `IpAllowlist`
- `rust/auth/src/api_key.rs` — `KeyRing`/`KeyRingEntry`/`parse_key_ring`/`validate_request`
- `rust/auth/src/db_api_key.rs` — `KeyRow`/`validate_request`
- `rust/auth/src/axum.rs`, `rust/auth/src/tower.rs` — resolve+pass `client_ip`
- `rust/auth/Cargo.toml`, root `Cargo.toml` — `ipnetwork` dependency
- `rust/public/src/servers/http_utils.rs` — thin wrapper over `client_ip::resolve_client_ip`
- `rust/public/src/servers/tonic_auth_interceptor.rs`, `firehose_common.rs` — resolve+pass `client_ip`
- `rust/ingestion/src/sql_migration.rs` — migration v11
- `rust/analytics-web-srv/src/ingestion_keys.rs`, `analytics_keys.rs` — request/response fields, new
  `PATCH` route
- `python/micromegas/micromegas/web_client.py`, `cli/import_keys.py`
- `rust/auth/tests/api_key_tests.rs`, `db_api_key_tests.rs`, new `client_ip_tests.rs`,
  `ip_allowlist_tests.rs`, and every other test constructing `HttpRequestParts`/`GrpcRequestParts`
- (Phase 5, if in scope) `analytics-web-app/src/lib/api-keys-shared.ts`, `ingestion-api-keys-api.ts`,
  `analytics-api-keys-api.ts`, `MintIngestionKeyDialog.tsx`, `IngestionApiKeysPage.tsx`,
  `AnalyticsApiKeysPage.tsx`, plus their `__tests__`

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
- **`ipnetwork` over hand-rolled CIDR parsing.** IPv6 mask arithmetic is easy to get subtly wrong
  (off-by-one prefix lengths, mixed v4/v6 comparison); a small, widely-used crate is worth the one
  new dependency. `ipnet`/`cidr-utils` are the alternatives — `ipnetwork` was picked for its
  simpler, more ergonomic `IpNetwork::from_str`/`contains` API doing exactly what's needed here.
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

(none yet — open questions below are unresolved)

## Documentation

- `rust/auth/src/lib.rs`'s module-level examples don't construct `HttpRequestParts`/`GrpcRequestParts`
  with every field spelled out in a way that would go stale, but double check after Phase 1 that
  the doctested examples still compile with the new `client_ip` field.
- Any admin runbook/mkdocs page documenting `mint`/`import`/`revoke` for the API-key routes (check
  `mkdocs/` for an existing page — grep for `ingestion-api-keys` or `analytics_api_keys`) needs the
  new `allowed_cidrs` field and the new `PATCH .../allowlist` route added.
- `CHANGELOG.md`: new column + new provider-side behavior is additive at the SQL/API layer (no
  breaking change there), but `KeyRing`'s value type change (`String` -> `KeyRingValue`) is a
  **Minor breaking change** to the Rust API surface — record it per the Interface Stability policy.

## Testing Strategy

All no-DB unit tests, per this repo's verification-tier rule — nothing here needs a live
Postgres; `DbApiKeyAuthProvider`'s existing `#[ignore]`d live section is for a different purpose
(DB wiring/latency), not new-feature acceptance.

- `rust/auth/tests/ip_allowlist_tests.rs` (new): empty list allows any IP including `None`;
  single bare-IP entry matches only that exact address (v4 and v6); a `/24`/`/64` entry matches
  every address in range and rejects one outside it; a non-empty list rejects `None` (unresolved
  client IP); malformed entries in `parse` return `Err` (bad CIDR syntax, out-of-range prefix
  length, empty string).
- `rust/auth/tests/client_ip_tests.rs` (moved from `rust/public/tests/http_utils_tests.rs`): same
  cases as today, retargeted at `resolve_client_ip` and asserting `Option<IpAddr>` instead of a
  formatted string.
- `rust/auth/tests/api_key_tests.rs`: a keyring entry with a restrictive `allowed_ips` accepts a
  request whose `HttpRequestParts.client_ip` is inside it and rejects one outside it (including a
  `client_ip: None` request against a restricted key); an entry with no `allowed_ips` is
  unaffected (regression check that the existing `test_valid_api_key`/`test_invalid_api_key` keep
  passing unchanged).
- `rust/auth/tests/db_api_key_tests.rs`: extend the existing no-DB pattern — a canned `KeyRow`
  (constructed directly, no `try_get_with`/DB round trip) with a populated `allowed_cidrs` is
  checked against in-range and out-of-range `parts.client_ip()` values. Confirms an IP-allowlist
  rejection is not written into `self.unknown` and does not increment `db_api_key_error_count`
  (both by constructing the scenario directly against a `DbApiKeyAuthProvider` backed by a lazy,
  never-queried pool wherever the existing tests already use that pattern — e.g.
  `missing_bearer_token_fails_before_any_db_access`).
- `rust/analytics-web-srv` route tests (wherever `ingestion_keys.rs`/`analytics_keys.rs` are
  already tested — check for an existing `mint_key`/`import_key` test module): a mint/import with
  a malformed `allowed_cidrs` entry returns `400`; a well-formed one round-trips through
  `list_keys`; the new `PATCH .../allowlist` route updates and clears the column, and 404s on an
  unknown `key_id`.
- Python: `python/micromegas/tests/` — `read_keyring` accepts/rejects `"allowed_ips"` shapes the
  same way it already does for `"audience"`; `import_one` forwards the resolved list.

## Manual Verification

1. Start services (`python3 local_test_env/ai_scripts/start_services.py`), mint an ingestion key
   with `allowed_cidrs: ["127.0.0.1/32"]` via the admin route, then send an ingestion request from
   `127.0.0.1` (expect success) and confirm the same key is rejected once `X-Forwarded-For` is set
   to a different address in the request (expect the same generic auth failure a wrong key would
   get). This exercises the full HTTP `axum_middleware` -> `ApiKeyAuthProvider`/
   `DbApiKeyAuthProvider` -> `IpAllowlist` path end-to-end, including the `X-Forwarded-For`
   priority logic, which a unit test can approximate but not confirm against the real ALB-shaped
   header handling in `get_client_ip`'s existing ingestion path.
2. Repeat against `flight-sql-srv` (gRPC) with an analytics key, confirming the tonic
   `ConnectedIncoming`/`AuthService` path also carries `client_ip` correctly — the one path where
   IP resolution comes from a transport-level `Connected::connect_info()` rather than an axum
   extension, which is worth eyeballing once since it's a different code path than step 1.

## Open Questions

1. **Is the analytics-web-app UI (Phase 5) in scope for this PR, or a follow-up?** The GitHub
   issue's "Rough idea" only calls out admin *routes* and `import_keys.py`; the browser UI is not
   mentioned. Recommend shipping Phases 1-4 first (the full backend + CLI surface, independently
   useful and testable) and opening a small follow-up issue for the UI once the API shape is
   settled — but this plan includes Phase 5 in case the intent was to cover it in one PR.
2. **Should `allowed_cidrs` be inheritable/copyable when importing a key that already has one in
   the source keyring's JSON, but the target row already exists (the `imported: false` path)?**
   `audience` is immutable on that path (`import_key`'s doc comment: "the binding is immutable, so
   an import never rewrites it"). Recommend the same rule for `allowed_cidrs` — an import never
   changes an existing row's allowlist, only the new `PATCH` route does — for consistency, but
   confirm this matches intent before implementing.
3. **IPv6-mapped IPv4 addresses (`::ffff:a.b.c.d`) arriving via a dual-stack listener**: should an
   allowlist entry of `a.b.c.d/32` match a client IP that arrived as its IPv6-mapped form?
   `ipnetwork`'s `IpNetwork::contains` does not normalize this by default. Needs a decision (and
   a unit test either way) once the target deployment's listener configuration (dual-stack or
   v4-only) is confirmed.
