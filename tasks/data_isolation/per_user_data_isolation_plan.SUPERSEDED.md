# Per-user data isolation for privacy-sensitive users

> ⚠️ **SUPERSEDED** by [`policy_based_data_isolation_plan.md`](policy_based_data_isolation_plan.md).
> This is the earlier, simpler per-user-only design. It is retained for its confidentiality/integrity
> analysis and current-state audit, which the successor builds on. **Do not implement from this
> document** — the policy-based plan generalizes it (per-user is the default identity-policy instance)
> and reflects the codebase research. Where they differ, the successor wins.

## Overview

Goal: ensure that telemetry produced by a user can only be **seen** by that user. Target
population is **developers/users each running their own OpenTelemetry app on their own machine
or under their own OS account** — one owner per app instance. Analytics/query auth is **OIDC**;
ingestion stays credential-gated but moves from a shared static key to **per-user API keys** that
a user requests via an authenticated call.

The stack already authenticates a principal at both ends and then discards it before it touches
data. This plan wires that identity through: **stamp a trustworthy owner at ingestion, and enforce
`owner = caller` on every query**, non-bypassably.

### The load-bearing property: confidentiality vs. integrity

Reads go through the **query** path (OIDC-gated). The ingestion **key only governs writes**.
Consequences that shape the whole design:

- **A stolen ingestion key is not a confidentiality breach.** With Bob's key, Alice can *write*
  data tagged as Bob (pollutes Bob's view — an integrity problem) but cannot *see* Bob's data:
  querying as Bob needs Bob's OIDC login, and she cannot retag Bob's already-ingested data.
- **Confidentiality rides entirely on OIDC + the query filter.** The per-user ingestion key exists
  only to make *attribution* trustworthy at write time (so the query filter has an honest tag to
  filter on). With a shared key, owner could only come from client-supplied `process.owner`, which
  is forgeable — that is the hole this design closes.

The join that makes it line up: **mint-time owner email == query-time OIDC email**, both from the
same identity provider, so they match by construction.

### Out of scope

- A single shared process ingesting on behalf of many end-users (multi-tenant backend). One key =
  one owner, so this design cannot split end-users behind shared infrastructure. Confirmed out of
  scope: each user runs their own app.
- Write-side pollution hardening beyond key custody (Alice writing garbage as Bob). It is an
  integrity concern, not a confidentiality one; not defended here.
- What a user's own process can observe about the host ("a process knowing more than it should").

## Threat model

Defended:
- **Casual/accidental cross-visibility** — a user must never see another user's telemetry through
  normal query use.
- **Deliberate insider read** — a user who knows another's identity must not be able to read that
  user's data. This is why attribution must be non-forgeable (per-user key), not self-asserted
  `(username, machine)`.

Not defended: stolen-key write pollution; shared-infra per-end-user split; host-level observation.

Non-goal turned advantage: because reads are OIDC-gated independently of the ingestion key, key
custody protects *integrity*, not *confidentiality* — so key storage does not need password-grade
fortification (the key is high-entropy random).

## Current State

### Query path — authentication real, authorization absent

- `flight-sql-srv` authenticates by default (`rust/flight-sql-srv/src/flight_sql_srv.rs:31-33`);
  fail-fast if no provider configured (`rust/public/src/servers/flight_sql_server.rs:221-237`).
- OIDC JWTs validated (issuer/audience/expiry/JWKS) in `rust/auth/src/oidc.rs`
  (`validate_jwt_token` ~428-495). Claims extracted: `iss`, `sub`, `aud`, `exp`, and **email** via
  a priority chain (`get_email()` ~232-240). **No `groups`/roles claim** is extracted.
- The tower auth layer (`rust/auth/src/tower.rs:76-149`) strips client-supplied `x-auth-*` headers
  (anti-spoof, 105-110) and injects the trusted identity into metadata + request extensions
  (112-135).
- **After authentication there is no authorization.** Any authenticated caller runs arbitrary SQL
  over the entire lakehouse. `make_session_context()` takes **no identity**
  (`rust/analytics/src/lakehouse/query.rs:186-228`); SQL is executed verbatim (`ctx.sql(sql)`,
  service impl ~389). The only injected analyzer rule is `TableScanRewrite`, which adds
  **time-range predicates only** (`rust/analytics/src/lakehouse/table_scan_rewrite.rs:11-15`).
- `UserAttribution` is resolved (`rust/auth/src/user_attribution.rs:108-187`) but feeds **only**
  audit logging + impersonation-prevention (service impl ~317, 349-366) — never query filtering.
  `is_admin` is never enforced on the query path.

### Ingestion path — key gate, identity discarded

- All ingestion + OTLP + webhook routes are behind bearer-auth middleware
  (`rust/public/src/servers/ingestion.rs:134-149`); `auth_middleware`
  (`rust/auth/src/axum.rs:39-79`) validates and injects an `AuthContext`.
- API keys come from the **static** `MICROMEGAS_API_KEYS` env var, parsed once into an in-memory
  `HashMap<Key, name>` (`rust/auth/src/api_key.rs:43-133`). Constant-time compare; no runtime
  add/revoke. `AuthContext` for a key: `subject = name, email = None, auth_type = ApiKey,
  is_admin = false, allow_delegation = true`.
- **No ingestion handler reads `AuthContext`.** Native (`ingestion.rs:54-82`) and OTLP
  (`rust/public/src/servers/otlp.rs:141-184`) handlers extract only the service + body. The
  identity gates the request and is dropped — functionally a shared secret.

### Data model — no owner dimension

- `processes` table (`rust/ingestion/src/sql_telemetry_db.rs:24-48`): `process_id`, `exe`,
  `username`, `realname`, `computer`, `distro`, `cpu_brand`, `start_time`, `parent_process_id`,
  `properties micromegas_property[]`. `username`/`computer` are **self-reported and unauthenticated**
  (for OTel, derived from `process.owner`/`host.name` resource attributes,
  `rust/otel-ingestion/src/block.rs:427-488`, `identity.rs:127-140`).
- No `owner`/`tenant`/`org` column anywhere (confirmed by grep across `rust/`).
- Lakehouse partitions keyed by `(view_set_name, view_instance_id, time)`
  (`rust/analytics/src/lakehouse/migration.rs:109-127`). `view_instance_id` is `"global"` |
  `process_id` | `stream_id` (`rust/analytics/src/lakehouse/view.rs:56-57`). Object layout
  `views/<view_set>/<view_instance_id>/<date>/<time>_<uuid>.parquet`
  (`rust/analytics/src/lakehouse/write_partition.rs:545-552`) — single shared namespace, no
  per-owner prefix.
- Properties are queryable at **row level** (`property_get`,
  `rust/datafusion-extensions/src/properties/property_get.rs:82`) but **cannot prune partitions** —
  they are row data inside parquet, not part of the partition key.

## Design

Four components. Owner is stamped server-side from the authenticated key at ingestion and enforced
against the caller's OIDC email at query time.

### 1. Dynamic, DB-backed key store (the one new subsystem)

The static env keyring cannot issue or revoke at runtime. Add a Postgres-backed keyring next to
the telemetry metadata (ingestion already holds a PG pool).

Table (telemetry DB):
```
api_keys(
  key_id        UUID  PRIMARY KEY,   -- non-secret; also the visible prefix
  key_hash      BYTEA NOT NULL,      -- SHA-256 of the random secret (key is high-entropy)
  owner_email   TEXT  NOT NULL,      -- bound at mint time == OIDC email
  label         TEXT,
  created_at    TIMESTAMPTZ NOT NULL,
  expires_at    TIMESTAMPTZ,         -- optional
  revoked_at    TIMESTAMPTZ          -- soft-delete for revocation
)
```
- Key material: `mm_<key_id>_<secret>`. Look up by non-secret `key_id`, then verify `secret`
  against `key_hash`. SHA-256 suffices — no bcrypt (256-bit random, not a password).
- New `DbApiKeyAuthProvider` alongside the env-based one (compose via the existing
  `MultiAuthProvider`). On match, produce `AuthContext { subject: key_id, email: Some(owner_email),
  auth_type: ApiKey, is_admin: false, allow_delegation: false }`.
  - **`allow_delegation: false`** for user keys (current API keys are `true`) so a user key cannot
    impersonate. Delegation stays reserved for admin/service accounts.
  - **`email` is populated** with the bound owner — this is what ingestion stamps and what the
    query filter later matches.
- Big side benefit: **revocation and rotation without redeploy** (impossible with the env keyring).

### 2. Mint endpoint (authenticated call)

`POST /auth/api_keys` (or on the monolith / ingestion server — wherever the OIDC provider is built).
- **OIDC-authenticated** via the existing provider. The setup script performs a device-code or
  loopback-redirect OAuth flow (like `gh`/`claude`), gets a token, calls this endpoint.
- **Iron rule: `owner_email` = the authenticated token's email, always.** Never client-specified.
  A user mints keys only for themselves; minting for another identity is a separate privileged
  admin/service-account path.
- Returns the plaintext key **once**; server stores only the hash.

Setup script flow (the enrollment ceremony):
1. User runs script → OIDC auth → token (email).
2. Script calls the mint endpoint → receives a per-user key.
3. Script writes the OTLP exporter config:
   `OTEL_EXPORTER_OTLP_ENDPOINT=<ingestion>`,
   `OTEL_EXPORTER_OTLP_HEADERS=authorization=Bearer <key>`.

`username`/`computer` are now irrelevant to security; no `(email → username, machine)` mapping
table, no OTel resource-detector dependency, no claim-conflict/generation logic.

### 3. Ingestion stamps owner

The key is gone by query time, so the owner **must** be persisted at ingestion. Read the
`AuthContext` the middleware already builds (currently discarded) in the OTLP + native handlers and
write `owner_email` onto the process. **Ignore/demote client-supplied `process.owner`** (display
metadata only) — mirror the header-stripping the auth layer already does.

Storage decision (see Open Decisions):
- **v1: reserved property** `micromegas.owner` on the process — no schema migration, flows through
  existing property plumbing, row-level filter works immediately.
- **later: first-class `owner_email` column** on `processes` + propagate through views — enables
  partition pruning and a physical boundary.

### 4. Query enforcement (mandatory analyzer rule)

A new mandatory analyzer rule beside `TableScanRewrite`, non-bypassable because it operates on the
logical plan below the SQL text.

- Thread the caller's OIDC email from `AuthContext` into `make_session_context()` (it currently
  takes no identity — required plumbing change).
- Inject `owner = caller_email` on **every** table scan:
  - Tables carrying owner directly (`processes`, and `list_partitions`-style system tables): direct
    predicate. Metadata tables **must** be covered or they leak other users' process names,
    machine names, and `otel.resource.*` properties even while log bodies are hidden.
  - Tables keyed by `process_id` (`streams`, `blocks`, `log_entries`, `measures`, spans, …):
    **semi-join subquery**, not a materialized id list —
    `process_id IN (SELECT process_id FROM processes WHERE owner = caller_email)`.
    Avoids any fixed ceiling on the number of owned processes (streaming-friendly; no hard limit).
  - `view_instance('<set>', <id>)`: an **ownership guard** so a user cannot name another user's
    `process_id`/`stream_id` directly.
- **Admin bypass:** the maintenance daemon that materializes global views must run **unfiltered**
  (enforcement applies to user FlightSQL sessions only, never the internal materialization path).
  For human admins, `is_admin` may bypass the filter **with an audit record** (the audit plumbing
  already exists and is the only current consumer of `UserAttribution`).

## Properties gained

- **"All my data across all my machines"** is trivial — `owner = me`. The `(username, machine)`
  approach fragmented per machine; this matches intent better.
- No OTel resource-detector dependency; no null filter-key failure mode.
- The forgery / squatting / claim-conflict problem **disappears** rather than being managed
  (no mapping table, no generation/epoch windows needed as a security mechanism).
- Key theft ≠ read breach (confidentiality rides on OIDC).

## Open decisions

1. **Owner storage: reserved property vs. first-class column.** Property = zero-migration v1,
   row-level filter only. Column = schema migration through the views, but enables partition
   pruning and a physical wall. Recommend property for v1, promote if a physical boundary is wanted.
2. **Key lifecycle policy.** Expiry default (or non-expiring + explicit revoke); rotation via
   re-running the setup script; who can revoke (self + admin). All cheap once the store is
   DB-backed.
3. **Admin read bypass.** Confirm admins bypass-with-audit vs. no bypass at all. (Currently
   `is_admin` means nothing on the query path.)

## Phased plan

1. **Enforcement skeleton (correctness-first).** Thread caller email into `make_session_context`;
   add the analyzer rule filtering on a `micromegas.owner` property (covering all tables +
   `view_instance` guard + admin/daemon bypass). Test with owner stamped manually.
2. **Ingestion stamping.** Read `AuthContext` in OTLP + native handlers; write `micromegas.owner`;
   demote client `process.owner`.
3. **DB-backed key store + mint endpoint.** `api_keys` table, `DbApiKeyAuthProvider`,
   OIDC-authenticated `POST /auth/api_keys`, revocation.
4. **Setup script.** OIDC device-code flow → mint → write OTLP env vars.
5. **(Optional) promote owner to a first-class column** + partition pruning / object-storage prefix
   for a physical boundary.

## Key files

- Auth: `rust/auth/src/api_key.rs`, `rust/auth/src/oidc.rs`, `rust/auth/src/types.rs`,
  `rust/auth/src/default_provider.rs`, `rust/auth/src/tower.rs`, `rust/auth/src/axum.rs`,
  `rust/auth/src/user_attribution.rs`
- Ingestion: `rust/public/src/servers/ingestion.rs`, `rust/public/src/servers/otlp.rs`,
  `rust/otel-ingestion/src/{handler,block,identity}.rs`,
  `rust/ingestion/src/web_ingestion_service.rs`, `rust/ingestion/src/sql_telemetry_db.rs`
- Query: `rust/public/src/servers/flight_sql_service_impl.rs`,
  `rust/analytics/src/lakehouse/query.rs`, `rust/analytics/src/lakehouse/table_scan_rewrite.rs`
- Monolith: `rust/monolith/src/main.rs`
