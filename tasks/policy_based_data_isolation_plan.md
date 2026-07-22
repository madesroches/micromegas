# Policy-based data isolation for telemetry

> **Supersedes** `tasks/privacy_per_user_data_isolation_plan.md`. That document analyzed the
> per-user case and remains the reference for the confidentiality/integrity analysis and the
> current-state audit; this plan generalizes its mechanism. Where they disagree, this document wins.

## Overview

Isolate telemetry so that data produced under one identity is only **readable** by principals
authorized to read it. The mechanism is a small, general **RBAC seam**; the **default configuration
makes it byte-for-byte equivalent to per-user isolation** (each user sees only their own data).
Turning on group-based access is a config flip plus two additional policy implementations — no data
migration, no API change, no rewrite of the enforcement rule.

The design rests on one structural decision: model everything as a **generic principal stamped on
data** (`audience`) plus a **set-valued read check** (`audience IN (readable principals)`), never an
equality (`owner = caller`). With the default policy the readable set is always a singleton, so the
executed query plan is identical to the simple per-user design — but the code is already the general
form and never has to be reworked.

### The three-relation model

Authorization decomposes into three independent relations:

1. **`write(subject → group)`** — checked **once, at mint time** (key issuance). Grants the right to
   mint an ingestion key that stamps `audience = group`.
2. **`read(subject → group)`** — checked **on every query**. Grants visibility of data whose
   `audience = group`.
3. **`audience`** — the label physically stamped on data by the key; the link between the two.

**Per-user isolation is the restriction of this model to singleton self-groups**: every principal is
its own group, with `write(u → u)` and `read(u → u)` the only grants, and `audience` always the
minter's own email. The general and specific cases run the *same* code; only the policy objects
differ.

### Load-bearing property preserved

Confidentiality rides entirely on **OIDC identity + `ReadPolicy`**, evaluated per-query. The write
key governs **integrity only**:

- A stolen write key lets an attacker *write* data labeled with some `audience` (pollutes that
  audience's view — an integrity problem). It grants **zero** read power: reading requires the
  caller's own OIDC identity to satisfy `read(caller → audience)`.
- There is no write→read escalation: a holder of `write(caller → G)` who lacks `read(caller → G)`
  cannot read G — not even the rows they themselves wrote (audience is `G`; reading `G` needs
  `read`).

Because the write grant is frozen into the key at mint and ingestion never re-checks it, **write
keys can be eternal** (the current use case). No per-write policy lookup; no `minted_by` bookkeeping
in v1 (see Deferred / Trade-offs for when that changes).

## Current State

(Condensed from the superseded plan; verified against current code.)

### Query path — authentication real, authorization absent
- `make_session_context()` takes **no caller identity**
  (`rust/analytics/src/lakehouse/query.rs:186-228`) and executes SQL verbatim (`ctx.sql(sql)`,
  `flight_sql_service_impl.rs:389`).
- The only injected analyzer rule is `TableScanRewrite`
  (`rust/analytics/src/lakehouse/table_scan_rewrite.rs`), which adds **time-range predicates only**
  and, critically, **only rewrites `MaterializedView` table providers** — it early-returns
  `Transformed::no` for table functions (`table_scan_rewrite.rs:37-43`). Any ownership rule must
  handle the table-function case explicitly.
- Per-request caller identity **is** available: `validate_and_resolve_user_attribution_grpc` yields
  `attr.user_email` before the session context is built (`flight_sql_service_impl.rs:317`,
  used for audit at `349-366`). It is never passed to `make_session_context`.
- `SessionConfigurator` (`rust/analytics/src/lakehouse/session_configurator.rs`) is a per-**service**
  object (`self.session_configurator.clone()`), not per-request — it cannot by itself carry
  per-caller identity.

### Ingestion path — key gate, identity discarded
- API keys come from the static `MICROMEGAS_API_KEYS` env var
  (`rust/auth/src/api_key.rs`), parsed into an in-memory `HashMap<Key, name>`; constant-time compare;
  no runtime add/revoke. On match: `AuthContext { subject: name, email: None, audience: None,
  auth_type: ApiKey, is_admin: false, allow_delegation: true }` (`api_key.rs:116-127`).
- Providers compose via `MultiAuthProvider` in `default_provider::provider_with_prefix`
  (`rust/auth/src/default_provider.rs:51-119`).
- No ingestion handler reads `AuthContext`; identity gates the request and is dropped.

### Data model — no owner dimension
- `processes` table (`rust/ingestion/src/sql_telemetry_db.rs`): no owner/tenant column.
- `processes_view` is a SQL view exposing `process_id … properties`
  (`rust/analytics/src/lakehouse/processes_view.rs`). Properties are row-level queryable via
  `property_get` but cannot prune partitions.

### Naming collision to avoid
`AuthContext` **already has** an `audience` field (`rust/auth/src/types.rs:26`) — it holds the *OIDC
token audience* (API audience / client id). The data-isolation principal is a **different concept**.
To avoid confusion, this plan uses:
- `audience` — the principal **stamped on data** (process property / column). New concept.
- `bound_audience: Option<String>` — a **new** `AuthContext` field: the principal a credential is
  bound to write as. Do **not** overload the existing `audience` field.

## Design

Four seams. Two are trait objects (the policy seam); two are the mechanical stamp + enforce.

### 1. `MintPolicy` — who may stamp which audience (mint-time, ingestion side)

```rust
/// Resolves the audience a mint request is permitted to bind to a key.
#[async_trait]
pub trait MintPolicy: Send + Sync + std::fmt::Debug {
    /// `caller` is the authenticated OIDC context of the mint request.
    /// `requested` is the optional audience the caller asked for.
    /// Returns the audience to bind, or Err if not permitted.
    async fn resolve_audience(
        &self,
        caller: &AuthContext,
        requested: Option<&str>,
    ) -> anyhow::Result<String>;
}
```

Default impl (`SelfMintPolicy`) = identity:
```rust
// requested is ignored (or rejected if != caller email); audience is always the caller.
Ok(caller.email.clone().ok_or_else(|| anyhow!("mint requires an authenticated email"))?)
```

### 2. `ReadPolicy` — which audiences a caller may read (query-time, flight-sql side)

```rust
/// Resolves the set of audiences a caller is permitted to read.
#[async_trait]
pub trait ReadPolicy: Send + Sync + std::fmt::Debug {
    async fn readable_principals(&self, caller: &AuthContext) -> anyhow::Result<ReadScope>;
}

/// Result of a ReadPolicy. Explicit `All` variant models the admin/daemon bypass
/// without a magic sentinel string.
pub enum ReadScope {
    /// Unfiltered (maintenance daemon; audited admin bypass).
    All,
    /// Filter to `audience IN (principals)`. May be a singleton (per-user default).
    Principals(Vec<String>),
}
```

Default impl (`SelfReadPolicy`) = identity:
```rust
Ok(ReadScope::Principals(vec![
    caller.email.clone().ok_or_else(|| anyhow!("read requires an authenticated email"))?,
]))
```

`SelfReadPolicy` never inspects a `groups` claim, so **default mode needs no OIDC groups
extraction** and adds no attack surface.

### 3. Ingestion stamps `audience`

- Mint endpoint runs `MintPolicy::resolve_audience` **once** and records the resolved audience on the
  key (env keyring: not applicable — see key-store note; DB keyring: an `audience` column).
- Key auth sets `AuthContext.bound_audience = Some(key.audience)`.
- Ingestion handlers (native `rust/public/src/servers/ingestion.rs`, OTLP
  `rust/public/src/servers/otlp.rs`) read `AuthContext.bound_audience` (currently discarded) and
  write it onto the process. **No policy lookup at write time** — the audience is already vetted and
  frozen into the key.
- Client-supplied `process.owner` / `host.*` stay **display metadata only**, never the audience.

Storage of the stamped audience (v1 vs later) mirrors the superseded plan's open decision:
- **v1: reserved property** `micromegas.audience` on the process — zero schema migration, flows
  through existing property plumbing, row-level filter works immediately.
- **later: first-class `audience` column** on `processes` + propagate through views — enables
  partition pruning and a physical boundary.

### 4. Query enforcement — `OwnershipRewrite` analyzer rule (set-valued, mandatory)

A new mandatory `AnalyzerRule` beside `TableScanRewrite`, non-bypassable because it operates on the
logical plan below the SQL text. It is constructed with the resolved `ReadScope` for the request.

- If `ReadScope::All` → the rule is a no-op (bypass; see §5).
- If `ReadScope::Principals(ps)` → inject, per table:
  - **Tables carrying `audience` directly** (`processes`; `list_partitions`-style system tables):
    `audience IN (ps)` — as a property predicate in v1, a column predicate later.
  - **Tables keyed by `process_id`** (`streams`, `blocks`, `log_entries`, `measures`, spans, …):
    semi-join subquery, **not** a materialized id list:
    `process_id IN (SELECT process_id FROM processes WHERE audience IN (ps))`.
    No fixed ceiling on owned processes (streaming-friendly; consistent with the project's
    no-hard-limits preference).
  - **Table functions** (`view_instance('<set>', <id>)`, `list_partitions`, span table functions):
    `TableScanRewrite` **skips these today** (`table_scan_rewrite.rs:37-43`). The ownership rule
    **must not** skip them — they are the leak path. Cover via: (a) an ownership guard on
    `view_instance` so a caller cannot name another principal's `process_id`/`stream_id`, and
    (b) predicate injection / wrapping for the metadata table functions. This is the most
    delicate part of implementation and is called out in Open Questions.

With `SelfReadPolicy`, `ps` is a singleton, so every predicate reduces to `audience IN ('alice@…')`
— the exact per-user filter, same DataFusion plan.

### 5. Bypass paths (unchanged in spirit)

- **Maintenance daemon** materializing global views must run with `ReadScope::All` (internal
  materialization path, never a user session).
- **Human admins**: `is_admin` (OIDC only; API keys can never be admin, `api_key.rs:123`) may map to
  `ReadScope::All` **with an audit record** — the audit plumbing already exists and is the only
  current consumer of user attribution.

### 6. Threading identity into the session context

`make_session_context` currently takes no identity. Add the resolved `ReadScope` (or a small
`QueryAuthz { scope: ReadScope, is_admin: bool }`) as a parameter:

```
make_session_context(lakehouse, part_provider, query_range, view_factory, configurator, read_scope)
```

- `flight_sql_service_impl` already resolves `attr.user_email` per request
  (`flight_sql_service_impl.rs:317`); call `ReadPolicy::readable_principals` there and pass the
  result into both `make_session_context` call sites (`:371`, `:841`).
- The `ReadPolicy` object itself is a per-**service** dependency (like `session_configurator`),
  stored on `FlightSqlServiceImpl`; the **resolved scope** is per-request.
- Do **not** try to smuggle identity through `SessionConfigurator` — it is shared across requests.

### Config surface

One knob, defaulting to the per-user identity policy:

```
MICROMEGAS_ISOLATION_POLICY = self   # default → SelfMintPolicy + SelfReadPolicy (== per-user)
                            = rbac   # RbacMintPolicy + RbacReadPolicy (+ groups claim, policy source)
```

Wiring lives next to `default_provider::provider_with_prefix` (a `mint_policy()` / `read_policy()`
factory reading the env var). The seam permits splitting into `MINT_POLICY` / `READ_POLICY` later for
asymmetric modes (e.g. RBAC reads, self-only mint) with no code change.

## Implementation Steps

### Phase 1 — General mechanics, per-user behavior (ship this)
1. **Policy traits.** Add `MintPolicy`, `ReadPolicy`, `ReadScope` in `rust/auth/src/` (e.g.
   `policy.rs`). Add `SelfMintPolicy`, `SelfReadPolicy`.
2. **AuthContext field.** Add `bound_audience: Option<String>` to `AuthContext`
   (`rust/auth/src/types.rs`); populate `None` everywhere except the key path.
3. **Enforcement rule.** Add `OwnershipRewrite` in
   `rust/analytics/src/lakehouse/ownership_rewrite.rs`, constructed from `ReadScope`. Filter on the
   `micromegas.audience` property. Cover materialized views, `process_id`-keyed tables (semi-join),
   **and** the table functions.
4. **Thread identity.** Add `read_scope` param to `make_session_context` (`query.rs`) and register
   `OwnershipRewrite` when scope ≠ `All`. Resolve scope via `ReadPolicy` in
   `flight_sql_service_impl` and pass through both call sites. Daemon/admin → `ReadScope::All`.
5. **Config factory.** `MICROMEGAS_ISOLATION_POLICY` → default `self`; wire `Self*` impls.
6. **Test with audience stamped manually** (before ingestion stamping exists): seed processes with a
   `micromegas.audience` property and assert cross-audience queries return nothing; same-audience and
   admin/daemon return everything.

### Phase 2 — Ingestion stamping
7. Read `AuthContext.bound_audience` in native + OTLP handlers; write `micromegas.audience` onto the
   process; demote client-supplied owner fields to display metadata.

### Phase 3 — DB-backed key store + mint endpoint (enables real per-user keys)
8. `api_keys` table (telemetry DB) with an `audience` column; `DbApiKeyAuthProvider` composed via
   `MultiAuthProvider`; produces `AuthContext { bound_audience: Some(audience), email: Some(...),
   allow_delegation: false, is_admin: false }`. Supports revocation/rotation without redeploy.
9. OIDC-authenticated `POST /auth/api_keys` mint endpoint running `MintPolicy::resolve_audience`.
   `SelfMintPolicy` binds the caller's own email.
10. Setup script: OIDC device-code/loopback flow → mint → write OTLP exporter env
    (`OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS=authorization=Bearer <key>`).

### Phase 4 — RBAC mode (pure additions, no rewrites)
11. `RbacReadPolicy`: `{caller.email} ∪ {G : read(caller → G)}` from a groups claim and/or policy
    store. Requires groups-claim extraction in `rust/auth/src/oidc.rs` (add next to `get_email`).
12. `RbacMintPolicy`: permit `requested` iff `write(caller → requested)`.
13. Policy source: IdP role claims (preferred — keeps "confidentiality rides on OIDC" literally true)
    and/or a Postgres RBAC/grants table (admin-managed; those admins then join the TCB — see
    Trade-offs).
14. Flip `MICROMEGAS_ISOLATION_POLICY=rbac`. **No change** to the data model, `OwnershipRewrite`,
    ingestion stamping, or the mint endpoint API.

### Phase 5 — (optional) physical boundary
15. Promote `micromegas.audience` to a first-class `audience` column; propagate through views; enable
    partition pruning and per-audience object-storage prefixing.

## Files to Modify

- Auth: `rust/auth/src/types.rs` (`bound_audience`), `rust/auth/src/policy.rs` (new — traits +
  `Self*`), `rust/auth/src/default_provider.rs` (policy factory / config knob),
  `rust/auth/src/api_key.rs` + new `db_api_key.rs` (Phase 3), `rust/auth/src/oidc.rs` (groups claim,
  Phase 4).
- Analytics: `rust/analytics/src/lakehouse/ownership_rewrite.rs` (new),
  `rust/analytics/src/lakehouse/query.rs` (`make_session_context` signature),
  `rust/analytics/src/lakehouse/processes_view.rs` (audience exposure if promoted to column).
- Query service: `rust/public/src/servers/flight_sql_service_impl.rs` (resolve scope, pass through).
- Ingestion: `rust/public/src/servers/ingestion.rs`, `rust/public/src/servers/otlp.rs`,
  `rust/ingestion/src/sql_telemetry_db.rs` (audience storage).
- Mint endpoint + monolith wiring: `rust/public/src/servers/…`, `rust/monolith/src/main.rs`.

## Trade-offs

- **Set-valued rule from day one** vs. a per-user equality now, generalize later. Chosen: set-valued.
  The singleton `IN` costs nothing at runtime and is the one decision that prevents a rewrite; a
  boolean `owner = caller` special-case is exactly the corner to avoid.
- **`ReadScope::All` variant** vs. a wildcard principal string. Chosen: explicit enum — no sentinel
  that could collide with a real audience or be forged into a filter.
- **Eternal write keys / no `minted_by` in v1.** Accepts that revoking a subject's `write(→G)` does
  not retroactively invalidate keys already minted for G — the key *is* the frozen grant; to undo it
  you revoke the key. This matches the stated use case. If retroactive write-revocation is ever
  needed, add `minted_by` to `api_keys` and revoke by `(minted_by, audience)` — an additive change.
- **Policy source in RBAC mode.** IdP claims keep confidentiality resting solely on OIDC. A local
  grants table is more flexible but puts its editors in the TCB and weakens the confidentiality
  statement to "OIDC identity + integrity of the local read-policy." Deferred decision (Open
  Questions).
- **Reserved property vs. first-class column** for the audience (v1 vs Phase 5): row-level filter now
  with zero migration, physical pruning later.

## Security

- Confidentiality = OIDC + `ReadPolicy` per query; write-key theft is integrity-only.
- No write→read escalation (audience label ≠ read grant).
- Metadata tables/functions **must** be covered by `OwnershipRewrite` or they leak process names,
  machine names, and `otel.resource.*` properties even while log bodies are hidden — this is the
  primary correctness risk and the focus of testing.
- Admin bypass is audited; API keys can never be admin.
- RBAC mode adds a trust dependency on the IdP's group/role claims (and, if used, the local grants
  table).

## Testing Strategy

- **Unit:** `SelfMintPolicy` rejects non-self `requested`; `SelfReadPolicy` returns the singleton.
  `OwnershipRewrite` injects the expected predicate per table kind (snapshot the rewritten logical
  plan), including table functions and `view_instance` guard.
- **Integration (default/self mode):** two audiences seeded; assert each sees only its own rows
  across `processes`, `log_entries`, `measures`, spans, `view_instance`, `list_partitions`; assert
  the `process_id` semi-join blocks naming another audience's process directly; assert
  daemon/admin(`ReadScope::All`) sees everything.
- **Equivalence:** confirm the executed plan in `self` mode matches the intended per-user filter (a
  singleton `IN`), i.e. no behavioral difference from a hand-written per-user design.
- **RBAC mode (Phase 4):** group member reads group data; write-only producer (`write(→G)`,
  no `read(→G)`) cannot read G including its own writes; membership change reflected on next query.
- Rust: `cargo test`, `cargo clippy --workspace -- -D warnings`, `cargo fmt`; CI via
  `python3 build/rust_ci.py`.

## Documentation

- New page under `mkdocs/docs/` for the isolation model: the three-relation model, the `self` default,
  the `MICROMEGAS_ISOLATION_POLICY` knob, and the confidentiality/integrity properties.
- Update any auth/deployment docs to mention the mint endpoint, the setup script, and (Phase 4) the
  groups-claim / policy-store configuration.

## Open Questions

1. **Table-function coverage.** Exact mechanism to enforce ownership on `view_instance`,
   `list_partitions`, and span table functions, given `TableScanRewrite` currently skips non-
   `MaterializedView` providers. Guard-at-construction vs. predicate injection vs. wrapping — needs a
   spike against DataFusion's table-function plumbing. Highest-risk item.
2. **RBAC policy source.** IdP claims only, local grants table only, or both — and who administers the
   table. Determines whether the confidentiality statement stays "OIDC only."
3. **Admin read bypass.** Confirm audited `is_admin → ReadScope::All` vs. no bypass at all.
4. **Audience identifier shape.** Emails for users; what namespace/format for group audiences (to
   avoid collision between a user email and a group id in the same `audience` field). E.g. prefix
   group audiences (`group:eng`) vs. rely on email format.
5. **Audience storage for v1.** Confirm reserved property `micromegas.audience` (vs. going straight to
   a column) — property is the zero-migration choice recommended here.
