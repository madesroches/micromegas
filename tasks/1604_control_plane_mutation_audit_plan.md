# Control-Plane Mutation Audit Record Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1604

## Overview

Grant and group mutations are the ABAC control plane, but today's log trail can't serve as an
access audit: denied mutations emit nothing at all, two of the four group-route lines carry no
actor, and every successful line is free text under a module-path target. This adds one structured
JSON audit record for every mutation attempt that reaches a gate or handler — allowed *and*
denied — carrying the actor, the action, the target, the outcome and `client_ip`, emitted under a
single dedicated log target
`control_plane_audit`, mirroring the existing `flightsql_query_audit` record
(`rust/public/src/servers/query_audit.rs`). The record replaces the free-text mutation lines at
those sites.

This ships on the observability path that exists today. The tracing sink is fire-and-forget and
sheds load under pressure, so the trail is best-effort — the same guarantee every other log line
carries, and a large improvement over today's *nothing* for denials. Issue #1606 replaces the write
path with a durable record-then-apply contract later; when it lands, the record type and the
emission-site inventory here carry over unchanged and only the write call swaps.

## Current State

### Emission sites today

| Site | File:line | Actor? | Deny logged? |
|---|---|---|---|
| create grant | `audience_grants.rs:524` | yes (`created_by`) | no |
| delete grant | `audience_grants.rs:679` | yes (`deleted_by`) | no |
| self-service claim | `ingestion_keys.rs:689` | yes (`created_by`) | no |
| create group | `groups.rs:213` | yes (`created_by`) | no |
| delete group | `groups.rs:285` | **no** | no |
| add member | `groups.rs:453` | yes (`created_by`) | no |
| remove member | `groups.rs:579` | **no** | no |

All seven are `info!` with the target defaulting to `module_path!()`
(`rust/tracing/src/macros.rs:271`), so auditing means substring matching across three targets.

### Where denials happen

Three distinct layers, only one of which is reachable from the handler:

1. **Extractor gates, before the handler runs.**
   - `GrantGate::from_request_parts` (`audience_grants.rs:253-274`) rejects a non-admin with
     `Forbidden("self-service grant management is disabled")` when the knob is off.
   - `AdminUser` (`auth/handlers.rs:585-600`) rejects a non-admin on every group route with
     `AdminRequired`, whose `IntoResponse` (`:555-563`) logs nothing.
2. **In-handler authorization/validation**, returning `Forbidden` / `NotFound` /
   `GroupNotFound` / `NestedGroupNotFound` / `BadRequest` / `Conflict`.
3. **`IntoResponse`** (`audience_grants.rs:122-175`, `groups.rs:72-...`), which logs only
   `Database` and `Internal` — and has no access to the caller or `client_ip`, so it cannot be
   where an attributable record is built.

### The pattern to follow

`QueryAuditRecord` (`rust/public/src/servers/query_audit.rs`) is a `serde::Serialize` struct
emitted as `info!(target: "flightsql_query_audit", "{json}")`
(`flight_sql_service_impl.rs:411`), landing in `log_entries` with the JSON payload in `msg`,
documented in `mkdocs/docs/query-guide/query-audit-log.md` and unit-tested in
`rust/public/tests/query_audit_tests.rs`. `micromegas_tracing::log!` already accepts
`target: $target:expr` and stores it in a `static LogMetadata`, so a `const &'static str` works as
the target.

`client_ip` comes from `micromegas::servers::http_utils::get_client_ip(&headers, &extensions)`
(rightmost `X-Forwarded-For`, then `X-Real-IP`, then `ConnectInfo`), the same function
`axum_utils`'s middleware and `QueryAuditRecord` already use.

## Design

### New module: `rust/analytics-web-srv/src/mutation_audit.rs`

Registered in `lib.rs` alongside the other route modules.

**The target constant.**

```rust
pub const CONTROL_PLANE_AUDIT_TARGET: &str = "control_plane_audit";
```

**The record.** Flat, so every field is one `jsonb_get` away. Target fields are optional because
the grant shape (`audience`/`axis`/`selector`) and the group shape (`group`/`member`) are
disjoint, and because a gate-level denial knows neither.

```rust
#[derive(serde::Serialize)]
pub struct MutationAuditRecord {
    /// Caller email, else subject, else `"unauthenticated"` when no `AuthContext` was present.
    pub actor: String,
    pub is_admin: bool,
    pub action: &'static str,   // see Action below
    pub outcome: &'static str,  // "allowed" | "denied" | "error"
    pub client_ip: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub axis: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    /// Only on an `allowed` idempotent create: `false` when the row already existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<bool>,
    /// The denial/error message. Absent when `outcome == "allowed"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
```

**Actions**, as `&'static str` constants on a `pub mod action`: `create_grant`, `delete_grant`,
`claim_audience`, `create_group`, `delete_group`, `add_member`, `remove_member`.

**Outcome classification.** A trait, so a new error enum opts in by implementing it rather than by
editing a match in the audit module:

```rust
pub trait AuditOutcome {
    /// `("denied", Some(msg))` for a caller-attributable refusal, `("error", Some(msg))` for a
    /// server-side failure.
    fn audit_outcome(&self) -> (&'static str, Option<String>);
}
```

Implemented next to each error enum (same crate, no orphan issue):

- `AudienceGrantError` (`audience_grants.rs`): `Forbidden`/`Unauthenticated`/`NotFound`/
  `GroupNotFound`/`BadRequest` → `denied`; `Database`/`Internal`/`NotConfigured` → `error`.
- `GroupsError` (`groups.rs`): `BadRequest`/`NotFound`/`NestedGroupNotFound`/`Conflict` →
  `denied`; `Database`/`NotConfigured`/`Internal` → `error`.
- `IngestionKeyError` (`ingestion_keys.rs`): `Forbidden`/`Unauthenticated`/`NotFound`/`BadRequest` →
  `denied`; `Conflict` (claim contention) → `error` — it is transient lock contention, explicitly
  *not* a denial per `try_claim_and_mint`'s own comment; `Database`/`NotConfigured`/`Unavailable` →
  `error`.

`Database`'s message must not reach the record verbatim (`sqlx::Error` can carry SQL/connection
detail); use a fixed `"database error"` string, matching what `IntoResponse` already returns to
the client.

**The builder.** Built as the handler's first statement, emitted exactly once at the end:

```rust
pub struct MutationAudit { record: MutationAuditRecord }

impl MutationAudit {
    pub fn new(action: &'static str, actor: String, is_admin: bool, client_ip: String) -> Self;
    pub fn from_context(action: &'static str, caller: &AuthContext, client_ip: &ClientIp) -> Self;
    pub fn grant(self, audience: &str, axis: &str, selector: &str) -> Self;
    pub fn group(self, group: &str) -> Self;
    pub fn member(self, member: &str) -> Self;
    pub fn created(self, created: bool) -> Self;

    /// Classifies `result` and emits one record.
    pub fn emit<T, E: AuditOutcome>(self, result: &Result<T, E>);
    /// Emits an `allowed`/`denied`/`error` record directly -- for the gate paths, which have no
    /// `Result` to classify.
    pub fn emit_denied(self, reason: impl Into<String>);
}
```

`emit` serializes with `serde_json::to_string` and logs
`info!(target: CONTROL_PLANE_AUDIT_TARGET, "{json}")`, falling back to
`warn!("failed to serialize control-plane audit record: {e}")` on a serialization failure —
identical to `QueryAuditState::emit`.

**`ClientIp` extractor.** An infallible `FromRequestParts` in this module:

```rust
pub struct ClientIp(pub String);
```
wrapping `get_client_ip(&parts.headers, &parts.extensions)`. Added to each mutation handler's
signature. It must be declared *before* any body extractor in the handler argument list, per
axum's `FromRequestParts`-then-`FromRequest` ordering.

### Emission sites

Each handler keeps its existing body, renamed to `<name>_inner`, with a thin wrapper that owns
the audit:

```rust
async fn create_grant(
    Extension(state): Extension<AudienceGrantsState>,
    GrantGate(caller): GrantGate,
    client_ip: ClientIp,
    Json(body): Json<CreateGrantRequest>,
) -> Result<(StatusCode, Json<GrantResponse>), AudienceGrantError> {
    let audit = MutationAudit::from_context(action::CREATE_GRANT, &caller, &client_ip)
        .grant(&body.audience, &body.axis, &body.selector);
    let result = create_grant_inner(state, caller, body).await;
    // `created` is known only on success; fold it in before emitting.
    let audit = match &result {
        Ok((_, Json(resp))) => audit.created(/* from the response */),
        Err(_) => audit,
    };
    audit.emit(&result);
    result
}
```

One `emit` per handler, on both arms — no per-`return Err` sprinkling, so a new early return can't
silently skip the audit.

The `created` flag needs the `UpsertedRow.created` value, which `GrantResponse`/`MemberRow` don't
carry. Rather than widening the JSON response bodies (a web-app-visible change), have
`create_grant_inner`/`add_member_inner` return `(StatusCode, Json<...>)` as they already do and
derive `created` from the status: `StatusCode::CREATED` → `true`, `OK` → `false`. That is exactly
what the handlers already encode.

Sites, in full:

| Handler | File | Action | Target fields |
|---|---|---|---|
| `create_grant` | `audience_grants.rs` | `create_grant` | audience, axis, selector, created |
| `delete_grant` | `audience_grants.rs` | `delete_grant` | audience, axis, selector |
| `create_group` | `groups.rs` | `create_group` | group |
| `delete_group` | `groups.rs` | `delete_group` | group |
| `add_member` | `groups.rs` | `add_member` | group, member, created |
| `remove_member` | `groups.rs` | `remove_member` | group, member |
| claim, in `mint_key` | `ingestion_keys.rs` | `claim_audience` | audience, selector |

The claim is audited at `try_claim_and_mint`'s **call site** in `mint_key`
(`ingestion_keys.rs:448`), not inside the function, so the wrapper sees the `Result`. Only the
claim branch is audited: `mint_key`'s plain path writes no grant row, and its pre-claim
`Forbidden` ("not in the caller's mintable set") is a mint denial, not a claim attempt. The
`selector` is `user:<caller email>`; `axis` is left absent since a claim always writes both axes,
which the `claim_audience` action already implies.

### Deny paths in the gates

**`GrantGate`** (`audience_grants.rs`) already runs in `from_request_parts` with `parts` in hand,
so it emits its own record before rejecting. The body has not parsed yet, so the target fields are
absent; the action comes from `parts.method` — `POST` → `create_grant`, `DELETE` → `delete_grant`
— the only two methods routed through this gate. An unexpected method is unreachable through the
router; map it to `create_grant` rather than adding an error path, and say so in a comment.

It emits for both its rejections: the knob-off `Forbidden`, and the `AuthenticatedUser` fallback
`Unauthenticated` (`actor: "unauthenticated"`).

**Group routes** get a new module-local gate in `groups.rs`, mirroring `GrantGate`'s shape:

```rust
struct GroupAdminGate(ValidatedUser);
```

Its `from_request_parts` delegates to `AdminUser`; on rejection it emits a `denied` record
(actor from the `ValidatedUser` extension when present, else `"unauthenticated"`) and returns
`AdminRequired` unchanged, so the HTTP response is byte-identical to today. The action is derived
from `parts.method` and the route template in `axum::extract::MatchedPath`, read out of
`parts.extensions` — not the raw `parts.uri.path()`, which would misclassify a group literally
named `members` (`is_valid_group_name` allows it):

| method | route template | action |
|---|---|---|
| POST | `.../{name}` | `create_group` |
| DELETE | `.../{name}` | `delete_group` |
| POST | `.../{name}/members` | `add_member` |
| DELETE | `.../{name}/members` | `remove_member` |

All four group mutation handlers swap `AdminUser(user)` for `GroupAdminGate(user)`. The three
read routes (`list_groups`, `list_members`) keep plain `AdminUser` — reads are out of scope.

This gate deliberately does **not** live on `AdminUser` itself: that extractor gates every
admin route in the crate (data sources, screens, folders, analytics keys, ingestion keys), and
auditing all of them under a control-plane target is not what this record is for.

### Free-text lines replaced

The seven `info!` lines in the table above are removed — the structured record carries strictly
more (actor on all seven, `client_ip`, outcome) and is the whole point of defect 3. Two lines
nearby are **kept**, because they are not grant/group mutations: `ingestion_keys.rs:685` (the
mint line, `key_id`-bearing), `:814` (revoke) and `:931` (import).

## Implementation Steps

1. **`rust/analytics-web-srv/src/mutation_audit.rs`** — new module: `CONTROL_PLANE_AUDIT_TARGET`,
   `action` constants, `MutationAuditRecord`, `AuditOutcome`, `MutationAudit`, `ClientIp`.
   Register in `lib.rs`.
2. **`audience_grants.rs`** — `impl AuditOutcome for AudienceGrantError`; emit from `GrantGate`'s
   two rejections; split `create_grant`/`delete_grant` into wrapper + `_inner`; remove the two
   `info!` lines.
3. **`groups.rs`** — `impl AuditOutcome for GroupsError`; add `GroupAdminGate`; swap it into the
   four mutation handlers; split each into wrapper + `_inner`; remove the four `info!` lines.
   Closes the `delete_group`/`remove_member` actor gap.
4. **`ingestion_keys.rs`** — `impl AuditOutcome for IngestionKeyError`; add `ClientIp` to
   `mint_key`; wrap the `try_claim_and_mint` call site; remove the claim `info!` line.
5. **Tests** — `rust/analytics-web-srv/tests/mutation_audit_tests.rs` (see Testing Strategy).
6. **Docs** — new `mkdocs/docs/admin/control-plane-audit-log.md`, nav entry in `mkdocs.yml`,
   cross-link from `admin/authorization.md` and `admin/groups.md`.
7. **`CHANGELOG.md`** — new record, plus the removal of the seven free-text lines as a behavior
   change for anyone grepping them.

## Files to Modify

- `rust/analytics-web-srv/src/mutation_audit.rs` (new)
- `rust/analytics-web-srv/src/lib.rs`
- `rust/analytics-web-srv/src/audience_grants.rs`
- `rust/analytics-web-srv/src/groups.rs`
- `rust/analytics-web-srv/src/ingestion_keys.rs`
- `rust/analytics-web-srv/tests/mutation_audit_tests.rs` (new)
- `mkdocs/docs/admin/control-plane-audit-log.md` (new)
- `mkdocs/mkdocs.yml`
- `mkdocs/docs/admin/authorization.md`
- `mkdocs/docs/admin/groups.md`
- `CHANGELOG.md`

## Trade-offs

**One record type across grants and groups, vs. one per module.** A single flat record with
optional target fields means one target to query and one place to add a field, at the cost of
`audience`/`axis`/`selector` and `group`/`member` never being populated together. Two record types
would be tighter per-shape but would put auditing back to "match across several targets", which is
the defect being fixed.

**Emitting in the gate vs. threading the caller into `IntoResponse`.** `IntoResponse` is the one
place every error already funnels through, but it has neither the caller nor the headers, so
making it the emission point would mean widening every error variant to carry
actor + `client_ip` + action. Emitting where the caller is known — the gate for pre-handler
denials, the handler wrapper for everything else — touches far less and keeps the error enums as
they are.

**Replacing the free-text lines vs. keeping both.** `flightsql_query_audit` kept its start-of-query
`info!` because that line carries in-flight visibility the completion record cannot. Here the free
text is a strict subset of the record at the same instant, so keeping it is pure duplication.

## Decisions

- Ship on the existing tracing-sink path rather than waiting on #1606's durable write path
  (user decision) — the record shape and emission inventory are designed to survive that swap.
- Gate-level denials carry no target fields. The body/path is unparsed at that point, and the
  generic middleware line (`axum_utils.rs:38`) already records method + URI + `client_ip` at the
  same instant, so the URI is recoverable by correlation.
- `Database` errors record a fixed `"database error"` reason, never the `sqlx::Error` text.
- `IngestionKeyError::Conflict` on the claim path classifies as `error`, not `denied` — it is
  advisory-lock contention, which `try_claim_and_mint` already documents as explicitly not a
  denial.
- Read routes (`/visible`, `/my-audiences`, `list_groups`, `list_members`) are out of scope; this
  record is for mutations.
- Extractor-level malformed-input rejections (`Json`/`Query` returning 400/422) after the gate
  emit no record: no mutation target is parseable at that point.

## Documentation

- **New** `mkdocs/docs/admin/control-plane-audit-log.md`, modeled on
  `query-guide/query-audit-log.md`: what the record is, the field table, the best-effort caveat,
  and worked `log_entries` queries —
  - every mutation by one actor in a window,
  - all denials grouped by actor and `client_ip` (the privilege-probing query),
  - every mutation touching one audience or group.
  Each example filters `target = 'control_plane_audit'` with a bounded time range and parses `msg`
  with `jsonb_parse`/`jsonb_get`, as the query-audit page does.
- `mkdocs/mkdocs.yml` — nav entry under Administration, next to Authorization/Groups.
- `mkdocs/docs/admin/authorization.md` — link from `## The grant store` (which today points only
  at `list_audience_grants()`, i.e. current state, never history).
- `mkdocs/docs/admin/groups.md` — same link.

## Testing Strategy

New `rust/analytics-web-srv/tests/mutation_audit_tests.rs`. No live-DB test: nothing here is a bug
witnessed in the wild, and every behavior is reachable without a pool.

**Serialization** (mirrors `rust/public/tests/query_audit_tests.rs`):
- A fully-populated grant record serializes every required field.
- Absent optionals are omitted, so a group record carries no `audience`/`axis`/`selector` key and
  a gate-denial record carries no target keys at all.
- `reason` is omitted when `outcome == "allowed"`.
- A selector containing `{`/`}`/quotes (a hierarchical `group:` name) round-trips.

**Outcome classification** — the highest-value unit tests here, since a variant classified
`error` instead of `denied` is a silent audit hole. One test per error enum walking every variant
through `audit_outcome`, asserting the class and that `Database`'s reason is the fixed string
rather than the `sqlx::Error` text.

**`emit`'s `Ok` path** — `emit<T, E: AuditOutcome>(&Result<T, E>)` takes its input by reference and
is generic over `T`, so it is callable with no pool: build a `MutationAudit`, call
`.created(true)`, then `.emit(&Ok::<(), AudienceGrantError>(()))`, and assert the resulting
`MutationAuditRecord` has `outcome: "allowed"`, `reason: None`, and `created: Some(true)`
preserved. This is the only automated coverage of the `Ok` → `"allowed"` classification; everything
else in Outcome classification walks `E`'s variants, never the `Ok` arm.

**Emission, end to end in-process** — using `init_in_memory_tracing()` +
`micromegas_tracing::event::in_memory_sink::InMemorySink`, the pattern
`rust/public/tests/auth_observability_tests.rs` already uses, with a collector that filters on
`evt.desc.target == "control_plane_audit"` (`LogStringEvent::desc` is the `&'static LogMetadata`).
Drive a `Router` with `tower::ServiceExt::oneshot`, `AudienceGrantsState`/`GroupsState` carrying
`pool: None`. Tests must be `#[serial]` — the dispatch is global.

The gate denials are fully covered this way with no DB, because both gates reject before
`require_pool` ever runs:
- Non-admin + `self_service_mint_enabled: false` → `POST /api/audience-grants` emits one record
  with `action: "create_grant"`, `outcome: "denied"`, the actor, and no target fields; the
  response is still 403.
- Same for `DELETE` → `action: "delete_grant"`.
- Non-admin on each of the four group mutation routes → one record with the right action derived
  from method + route template, `outcome: "denied"`, actor present; the 403 body is unchanged from
  `AdminRequired`'s.
- No `AuthContext` extension → `actor: "unauthenticated"`.
- A request with `X-Forwarded-For: 203.0.113.7, 198.51.100.1` records
  `client_ip: "198.51.100.1"` (rightmost entry), pinning that the record uses `get_client_ip` and
  not the socket address.
- An admin request with `pool: None` reaches the handler and emits `outcome: "error"` with reason
  `"audience grant store not configured…"` — proving the handler wrapper emits on the `Err` arm.

**Regression guard on the actor gap** — the `delete_group`/`remove_member` records assert `actor`
is present and non-empty, which is the defect this issue names.

## Manual Verification

Only the happy-path DB writes are left to hand-check, since covering them automatically would mean
a live-DB test this repo reserves for witnessed bugs.

1. `python3 local_test_env/ai_scripts/start_services.py --monolith`
2. As an admin, create a group, add a member, remove it, delete the group through the web app's
   Groups page.
3. `grep control_plane_audit /tmp/monolith.log` — expect four JSON lines, in order, each with the
   admin's email in `actor`, the right `action`, `outcome: "allowed"`, and the group/member
   target. **Why manual:** it exercises the real Postgres transactions and the real
   header/`ConnectInfo` chain, which the in-process router test with `pool: None` cannot reach.
4. Query it back:
   `micromegas-query "SELECT time, msg FROM log_entries WHERE target = 'control_plane_audit' ORDER BY time DESC LIMIT 10" --begin 1h`
   — confirms the dedicated target survives ingestion and is filterable, which no in-process test
   covers.
