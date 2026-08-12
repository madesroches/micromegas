# Policy Seam and Identity Threading Plan (#1369 — AbAC Stage 1)

## Overview

Stage 1 of the AbAC rollout (`tasks/data_isolation/audience_based_access_control_plan.md`, epic
#1334). It introduces the authorization seam — `MintPolicy`, `ReadPolicy`, `ReadScope` and their
audience-based implementations — threads a resolved `ReadScope` from the authenticated caller down
into the DataFusion session context, and closes the identity holes that would each be a full
enforcement bypass once Stages 2–3 add the actual filters.

**No enforcement and no behavior change.** Nothing consumes `ReadScope` yet; the config knobs are
unset by default and inactive when unset. The deliverable is a seam that Stages 2–3 (query
enforcement) and Stages 4/6 (audience on keys, mint-time resolution) can attach to without
re-plumbing anything.

Blocker #1368 closed 2026-07-30. Stage 0 (#1383) and the key-admin relocation (#1411/#1458) have
landed. #1369 is the only unblocked issue left in the epic — #1370–#1375 are all transitively
blocked by it.

## Current State

Verified against `213ed3b31` (`main`). Nothing from the AbAC vocabulary (`ReadScope`, `MintPolicy`,
`bound_audience`, `micromegas.audience`) exists in `rust/` yet.

### Identity is resolved, then dropped

`AuthContext` (`rust/auth/src/types.rs:37-59`) carries `subject`, `email`, `issuer`, `audience`
(the OIDC token audience, not ours), `expires_at`, `auth_type`, `is_admin`, `allow_delegation`.
No groups, no bound audience.

The gRPC tower layer (`rust/auth/src/tower.rs:76-160`) validates the request and then does **two**
things with the result:

1. Projects it into stringified metadata headers — `x-auth-subject`, `x-auth-email`,
   `x-auth-issuer`, `x-allow-delegation`, `x-auth-is-admin` (`:114-139`), first stripping any
   client-supplied copies (`:107-111`).
2. **Inserts the whole `AuthContext` into the request extensions** (`:141`).

Only (1) is consumed today. `validate_and_resolve_user_attribution_grpc`
(`rust/auth/src/user_attribution.rs:127`) and `is_admin` (`:72`) both read the headers.

### The session context already threads one authorization input

`make_session_context` (`rust/analytics/src/lakehouse/query.rs:207-214`) takes `is_admin: bool` and
passes it through `register_functions` (`:187`) into `register_lakehouse_functions` (`:96-176`),
where it gates registration of the five mutating UDTFs/UDFs (`:150-175`). `ReadScope` follows
exactly this path — the plumbing precedent already exists and works.

Call sites of `make_session_context`:

| Site | `is_admin` | Needs |
|---|---|---|
| `flight_sql_service_impl.rs:661` (execute path) | from metadata | caller's scope |
| `flight_sql_service_impl.rs:1149` (prepared stmt) | from metadata | caller's scope |
| `analytics/src/metadata.rs:182`, `:283` | `false` | `All` (internal) |
| `analytics/src/lakehouse/export_log_view.rs:118`, `:172` | `true` | `All` (internal) |
| `analytics/src/lakehouse/perfetto_trace_execution_plan.rs:254` | `false` | **caller's scope** — user-reachable |

### The three identity holes

1. **Prepared statements.** `do_action_create_prepared_statement`
   (`flight_sql_service_impl.rs:1142-1158`) builds its session context with `query_range = None` and
   no user identity at all. It does pass `is_admin(request.metadata())`, so it reads *some* identity
   — but no subject/email resolution. A caller who plans through
   `CreatePreparedStatement` would bypass any filter derived on the `do_get` path.
2. **Client-claimed attribution.** `validate_and_resolve_user_attribution_grpc`
   (`user_attribution.rs:145-152`) returns client-supplied `x-user-id`/`x-user-email` verbatim when
   `x-auth-subject` is absent. That is acceptable for audit attribution — it is what the function is
   for — but `ReadScope` must never derive from it.
3. **`analytics-web-srv` drops `AuthContext` fields.** `impl From<&AuthContext> for ValidatedUser`
   (`rust/analytics-web-srv/src/auth/claims.rs:40-48`) keeps only
   `subject`/`email`/`issuer`/`is_admin`. Any new `AuthContext` field is silently lost there. This is
   the **mint** path's identity source since #1458 moved key management onto that service, so a
   `MintPolicy` consulting `groups` would see an empty set and refuse every group mint. Found
   2026-08-12; not in the original issue text.

### Config precedents

- `ProviderBuilder` (`rust/auth/src/default_provider.rs:19-44`) — its own doc comment says the env
  factory "is a builder so that adding the DB store — **and later a policy** — does not re-break the
  signature."
- `load_admin_users` (`oidc.rs:264-269`) — env var → JSON array, empty on absent/unparseable.
- `StaticTablesConfigurator::from_env` (`static_tables_configurator.rs:44-54`) — env var → no-op
  implementation when unset.

## Design

### 1. Where the types live — `ReadScope` does not go in `micromegas-auth`

The AbAC plan says "Add `MintPolicy`, `ReadPolicy`, `ReadScope` in `rust/auth/src/`". For the two
traits that is right. For `ReadScope` it is not, and the reason is measurable:

**`micromegas-analytics` does not depend on `micromegas-auth` today, and adding that edge pulls in
50 new crates** — `jsonwebtoken`, `openidconnect`, `oauth2`, `rsa`, `curve25519-dalek`, `ed25519`,
`p256`/`p384`, `elliptic-curve`, `der`, `pkcs1`/`pkcs8`, and the rest of the JWT/OIDC stack
(measured with `cargo tree -e normal` on both crates at `213ed3b31`). `micromegas-analytics` is a
published library crate whose consumers include the maintenance daemon and embedders that never
authenticate anything. Putting one enum behind the entire OIDC dependency tree is the wrong trade.

There is no shared leaf crate to put it in either: the only crates both already depend on are
`micromegas-tracing` and `micromegas-transit`, and an authorization type does not belong in the
tracing crate.

So the seam splits by layer, which is also the honest description of what these things are:

```
micromegas-auth                 micromegas-analytics            micromegas (rust/public)
─────────────────               ────────────────────            ───────────────────────
MintPolicy                      ReadScope                       flight_sql_service_impl
ReadPolicy      ─── resolves ──▶ (what the query planner        bridges the two: calls
AudienceMintPolicy               filters on; consumed by        ReadPolicy on the
AudienceReadPolicy               Stage 2 OwnershipRewrite       AuthContext, builds a
ReadableAudiences                and Stage 3 UDTF guards)       ReadScope, passes it to
(policy output)                                                 make_session_context
```

`rust/public` already depends on both (`flight_sql_service_impl.rs:40` imports
`micromegas_analytics::lakehouse::query::make_session_context`, `:47` imports
`micromegas_auth::user_attribution::*`), so the bridge costs no new dependency anywhere.

**Types:**

```
// rust/auth/src/policy.rs
pub struct ReadableAudiences(Arc<[String]>);        // newtype, not a bare Vec<String>
pub trait ReadPolicy:  Send + Sync + Debug { fn resolve(&self, caller: &AuthContext) -> Result<ReadableAudiences>; }
pub trait MintPolicy:  Send + Sync + Debug { fn resolve_audience(&self, caller: &AuthContext, requested: Option<&str>) -> Result<String>; }
pub struct AudienceReadPolicy { implicit_groups: Vec<String> }
pub struct AudienceMintPolicy { implicit_groups: Vec<String> }

// rust/analytics/src/lakehouse/read_scope.rs
pub enum ReadScope { All, Audiences(Arc<[String]>) }
```

`ReadableAudiences` is a newtype so a policy result cannot be confused with any other string list on
a security path. `ReadScope::All` is deliberately **not** a policy output — no `ReadPolicy` can
return it. It is the marker internal (non-request) callers pass, which is what makes
"who granted themselves `All`?" a greppable question with a small, auditable answer set.

Both `Audience*` impls compute the same set per the plan's §1–§2: `{user:<email>} ∪ groups claim ∪
MICROMEGAS_IMPLICIT_GROUPS`, every element `user:`- or `group:`-prefixed. With no groups claim and
no implicit groups the readable set is the singleton `{user:<email>}` — the per-user case, with no
separate `Self*` implementation. A caller with no email (an API-key-authenticated analytics query)
resolves to implicit groups only, which is empty in a privacy deployment — fail-closed, and exactly
the "analytics keys may be transitional" consequence the AbAC plan already records.

### 2. Identity threading uses the existing extension, not a new header

The issue text says "the `groups` claim must also cross the tower `AuthService` boundary," implying
a new `x-auth-groups` header alongside the existing five. **That is not needed, and a header would
be the worse mechanism.** `AuthService` already inserts the whole `AuthContext` into the request
extensions (`tower.rs:141`), tonic propagates request extensions into the handler
(`tonic-0.14.6/src/request.rs:164`: `extensions: parts.extensions`), and the codebase already reads
them in exactly these handlers — `get_client_ip(request.metadata().as_ref(), request.extensions())`
at `flight_sql_service_impl.rs:797` and `:962`.

Three reasons to use it rather than add a header:

- **It cannot be forged.** A client can send arbitrary HTTP headers; it cannot inject a typed Rust
  extension. The header projection is only safe because `AuthService` strips client copies first
  (`:107-111`) — and it does that *only when a provider is configured*; the no-provider branch
  (`:156-158`) passes headers through untouched. The extension has no such caveat.
- **It is lossless and stays lossless.** `groups: Vec<String>` and `bound_audience` need no encoding
  decisions, no comma-escaping, no length limits. Stage 4's `bound_audience` rides along for free.
- **No new surface.** Zero changes to `tower.rs`.

So: read `request.extensions().get::<AuthContext>()`, resolve `ReadScope` from that, and never from
`UserAttribution`. Hole #2 closes by construction — `validate_and_resolve_user_attribution_grpc`
keeps its current behavior and stays audit-only, and the plan records why (its fallback is
deliberate, its output is not an authorization input).

**Absent-extension convention.** When no auth provider is configured (`--disable-auth`), no
extension is inserted. Mirror `is_admin`'s documented convention (`user_attribution.rs:64-71`:
absent header ⇒ trusted): absent `AuthContext` ⇒ `ReadScope::All`. The safety argument is the same
one `is_admin` already relies on — when a provider *is* configured, `AuthService::call` rejects the
request before the inner service runs, so the extension is always present. Put that argument in the
doc comment on the resolver rather than leaving it implicit.

### 3. Bundle the caller inputs instead of growing the parameter list

`make_session_context` is already six positional parameters ending in `is_admin: bool`. Adding
`read_scope` as a seventh gives it two adjacent authorization inputs that are easy to transpose at a
call site and easy to get wrong silently. Introduce one struct in `micromegas-analytics` and pass it
in place of `is_admin`:

```
// rust/analytics/src/lakehouse/read_scope.rs
pub struct CallerContext { pub read_scope: ReadScope, pub is_admin: bool }
impl CallerContext {
    pub fn internal() -> Self  // ReadScope::All, is_admin: false
    pub fn maintenance() -> Self  // ReadScope::All, is_admin: true
}
```

This keeps the two orthogonal axes the epic deliberately separated (audience scope vs. the
`is_admin` capability axis, #1376/#1377) in one place, makes each internal call site's intent
readable at a glance, and means Stage 2/3 add no further parameters. It touches the same six call
sites the `read_scope` parameter would touch anyway, so it is not extra work — just a better landing
spot. `register_functions` / `register_lakehouse_functions` take the same struct.

### 4. Groups claim (OIDC)

Add `groups: Option<Vec<String>>` to `Claims` (`oidc.rs:194-227`) and populate `AuthContext.groups`
at the construction site (`:536-545`). The struct has no `#[serde(deny_unknown_fields)]`, so this is
additive and absent-claim-safe — a token with no `groups` claim deserializes exactly as today.

Flat top-level array only, covering Auth0 / Azure AD / Google. Keycloak's nested
`realm_access.roles` is out of scope; note it in the doc comment so the next person does not read
the absence as an oversight.

Group values from the IdP are **namespaced on ingest into the policy**, not stored pre-prefixed:
`AuthContext.groups` holds raw claim values, and `AudienceReadPolicy` maps them to `group:<id>`.
Keeping the raw claim in `AuthContext` avoids baking an AbAC-specific convention into a general
auth type.

### 5. Config factory

Add `.with_policy_from_env()` to `ProviderBuilder` (`default_provider.rs`) — the builder its own doc
comment says exists for this. Reads `MICROMEGAS_IMPLICIT_GROUPS` (comma-separated; the AbAC plan's
Config surface table). Unset ⇒ empty implicit groups ⇒ readable set is the caller's singleton ⇒
enforcement inactive because nothing consumes it yet.

Only `MICROMEGAS_IMPLICIT_GROUPS` is parsed in Stage 1 — it is the only knob the policies
themselves need. `MICROMEGAS_UNSTAMPED_AUDIENCE`, `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS` and
`MICROMEGAS_PUBLIC_VIEW_SETS` are consumed by Stages 2/3 and are parsed there; parsing them now
would create config that reads as active and is not.

Startup log line naming the resolved implicit groups (or "none") — the operator's only feedback that
the knob took effect, since there is no behavior to observe yet.

### 6. Closing hole #1 (prepared statements) and hole #3 (web-srv)

**Prepared statements.** `do_action_create_prepared_statement` gets the same resolution as the
execute path: read `AuthContext` from `request.extensions()`, resolve `ReadScope`, pass the
`CallerContext` to `make_session_context`. This is the whole fix — the handler already receives
`request`.

Note the *other* half of that call site: `query_range = None` means `TableScanRewrite` is not
registered for prepared statements either (`query.rs:227-229` gates it on `query_range.is_some()`).
That asymmetry is out of scope here, but it is the same shape of bug and Stage 2 must register
`OwnershipRewrite` **unconditionally**, not under the `query_range` gate.

**`analytics-web-srv`.** The cookie middleware revalidates the token per request and builds a fresh
`AuthContext` before converting it (`handlers.rs:~500-511`), so there is no serialized session
format to migrate. Insert the `AuthContext` into request extensions next to the two things already
inserted there:

```
req.extensions_mut().insert(AuthToken(id_token));
req.extensions_mut().insert(ValidatedUser::from(&auth_context));
req.extensions_mut().insert(auth_context);          // new
```

`ValidatedUser` stays as-is — it is the browser-session view and does not need groups. Stage 6's
`mint_key` reaches for `Extension<AuthContext>` when it needs `MintPolicy`. This mirrors the gRPC
side exactly: same pattern, same reasoning, one line.

## Implementation Steps

### Phase 1 — types and policies (no call-site churn)

1. `rust/auth/src/types.rs` — add `bound_audience: Option<String>` and `groups: Vec<String>` to
   `AuthContext`. Update the three construction sites (`api_key.rs`, `db_api_key.rs`, `oidc.rs`) to
   `None`/`vec![]`, plus `auth/tests/tower_tests.rs`. Document `bound_audience` as Stage 4's field,
   populated only by the ingestion key provider.
2. `rust/auth/src/policy.rs` (new) + `pub mod policy;` in `lib.rs` — `ReadableAudiences`,
   `MintPolicy`, `ReadPolicy`, `AudienceMintPolicy`, `AudienceReadPolicy`. Doc comments state the
   readable-set formula and that `ReadPolicy` can never return "all".
3. `rust/auth/src/oidc.rs` — `groups` claim on `Claims`; populate `AuthContext.groups` at `:536-545`.
4. `rust/auth/src/default_provider.rs` — `with_policy_from_env()` on `ProviderBuilder`;
   `MICROMEGAS_IMPLICIT_GROUPS` parsing; startup log.

### Phase 2 — the analytics-side scope type and signatures

5. `rust/analytics/src/lakehouse/read_scope.rs` (new) — `ReadScope`, `CallerContext`,
   `CallerContext::internal()` / `::maintenance()`. Registered in `lakehouse/mod.rs`.
6. `rust/analytics/src/lakehouse/query.rs` — replace `is_admin: bool` with
   `caller: CallerContext` on `make_session_context`, `register_functions`,
   `register_lakehouse_functions`. Registration logic reads `caller.is_admin`; `caller.read_scope` is
   stored/ignored for now (Stage 2/3 consume it).
7. Update the four internal call sites: `metadata.rs:182,283` and
   `perfetto_trace_execution_plan.rs:254` → `CallerContext::internal()`;
   `export_log_view.rs:118,172` → `CallerContext::maintenance()`.
   **Leave a `TODO(#1371)` on `perfetto_trace_execution_plan.rs:254`** — it is reachable from user
   queries, so `internal()` there is a latent bypass that Stage 3 must replace with the caller's
   inherited scope. Better a named TODO than a silent `All`.

### Phase 3 — threading and hole-closing

8. `rust/public/src/servers/flight_sql_service_impl.rs` — add a `read_policy: Arc<dyn ReadPolicy>`
   field to `FlightSqlServiceImpl` and a resolver helper
   (`fn caller_context(&self, ext: &http::Extensions, md: &MetadataMap) -> Result<CallerContext, Status>`)
   implementing §2's absent-extension convention. Thread `&Extensions` into `execute_query`
   (two callers: `:800`, `:963`) and use the helper at `:661`.
9. Same helper at `:1149` — closes hole #1.
10. `rust/auth/src/user_attribution.rs` — no code change; add a doc-comment warning that
    `UserAttribution` is audit-only and must never feed a `ReadScope`, naming the fallback at
    `:145-152` as the reason. Closes hole #2 by making the constraint explicit at the definition.
11. `rust/analytics-web-srv/src/auth/handlers.rs:~509` — insert `AuthContext` into request
    extensions. Closes hole #3.
12. Wire the policy at service construction: `flight_sql_server.rs` (~`:219-258`) and the monolith,
    from `ProviderBuilder`. Unset config ⇒ a policy that resolves the caller singleton.

## Files to Modify

- `rust/auth/src/types.rs` — `bound_audience`, `groups` on `AuthContext`
- `rust/auth/src/policy.rs` — **new**; traits + `Audience*` impls + `ReadableAudiences`
- `rust/auth/src/lib.rs` — `pub mod policy;`
- `rust/auth/src/oidc.rs` — `groups` claim (`:194-227`, `:536-545`)
- `rust/auth/src/default_provider.rs` — `with_policy_from_env()`, `MICROMEGAS_IMPLICIT_GROUPS`
- `rust/auth/src/api_key.rs`, `rust/auth/src/db_api_key.rs` — new `AuthContext` fields
- `rust/auth/src/user_attribution.rs` — doc-comment constraint only
- `rust/analytics/src/lakehouse/read_scope.rs` — **new**; `ReadScope`, `CallerContext`
- `rust/analytics/src/lakehouse/mod.rs` — register the module
- `rust/analytics/src/lakehouse/query.rs` — `CallerContext` on the three fns
- `rust/analytics/src/metadata.rs`, `lakehouse/export_log_view.rs`,
  `lakehouse/perfetto_trace_execution_plan.rs` — call sites
- `rust/public/src/servers/flight_sql_service_impl.rs` — resolver, both call sites, struct field
- `rust/public/src/servers/flight_sql_server.rs` — policy construction
- `rust/monolith/src/main.rs` — same wiring
- `rust/analytics-web-srv/src/auth/handlers.rs` — `AuthContext` into extensions
- `rust/auth/tests/` — new `policy_tests.rs`; update `tower_tests.rs`

**Not** `rust/auth/src/tower.rs` — the extension it already inserts is the mechanism.
**Not** `rust/analytics/Cargo.toml` — the whole point of §1 is that no auth dependency is added.

## Trade-offs

- **`ReadScope` in analytics vs. in auth.** The plan's original placement costs
  `micromegas-analytics` 50 transitive crates and an OIDC dependency it has never had. Chosen: split
  by layer, bridge in `rust/public`. Cost: two types (`ReadableAudiences`, `ReadScope`) for what
  reads like one concept, and a conversion. Accepted — the conversion lives in exactly one place, and
  the types genuinely belong to different layers (a policy result vs. a planner input).
- **Extension vs. `x-auth-groups` header.** The issue text implies a header. Chosen: the existing
  extension — unforgeable, lossless, zero new surface. The headers stay for the attribution path,
  which is what they were built for.
- **`CallerContext` struct vs. a seventh positional parameter.** Chosen: struct. Same call sites
  touched either way; avoids two adjacent transposable authorization inputs and pre-absorbs Stage
  2/3's additions.
- **Required `CallerContext` vs. `Option`/default.** Chosen: required. A new required parameter
  forces every call site to be visited and to state its scope explicitly — a defaulting parameter
  would let a future call site inherit `All` by omission, which is the exact failure this seam
  exists to prevent.
- **Parsing only `MICROMEGAS_IMPLICIT_GROUPS` now.** Parsing all four knobs would produce config
  that appears active and is not. Chosen: parse what Stage 1 consumes; the rest land with their
  consumers.
- **Groups stored raw in `AuthContext`, prefixed by the policy.** Keeps the AbAC `group:` convention
  out of a general-purpose auth type. Cost: the prefix is applied in two policies rather than once at
  the source.

## Documentation

Stage 1 ships no operator-visible behavior, so no mkdocs page yet (the isolation page is Stage 7).
What does need writing:

- Doc comments carrying the load-bearing arguments, since there is no behavior to observe: why
  `UserAttribution` may not feed a `ReadScope`; why an absent `AuthContext` extension means `All`;
  why `ReadPolicy` cannot return `All`; that `bound_audience` stays `None` until Stage 4.
- `tasks/data_isolation/audience_based_access_control_plan.md` — record the `ReadScope` placement
  decision and the extension-over-header decision in Stage 1 and Resolved Decisions, so Stages 2–3
  build on the actual seam.
- `CHANGELOG.md` — per the `pr` skill's convention.

## Testing Strategy

- **Unit — `AudienceReadPolicy`** (`rust/auth/tests/policy_tests.rs`): returns `{user:<email>}` when
  the groups claim and implicit groups are both empty; returns `{user:} ∪ group:claim ∪
  group:implicit` when both are present; every element carries a `user:`/`group:` prefix; a caller
  with no email and no implicit groups resolves to the **empty** set, not to something permissive.
- **Unit — `AudienceMintPolicy`**: defaults to `user:<email>` when `requested` is `None`; permits a
  `requested` value inside the mintable set; rejects one outside it.
- **Unit — groups claim** (`rust/auth/tests/` alongside existing OIDC tests): a token with a flat
  `groups` array populates `AuthContext.groups`; a token **without** the claim still deserializes and
  yields `vec![]` — the backward-compatibility guarantee, so it deserves its own test.
- **Threading — the assertion that actually matters**: a request whose client-supplied
  `x-user-id`/`x-user-email` name a different principal than the authenticated `AuthContext` resolves
  a `ReadScope` derived from the **`AuthContext`**, not from the claimed attribution. This is hole #2
  as an executable check rather than a doc comment.
- **Threading — prepared statements**: `do_action_create_prepared_statement` resolves the same
  `CallerContext` as `do_get` for the same credentials. Assert equality of the resolved scope across
  the two paths rather than merely asserting it is non-empty — the bug being prevented is
  *divergence*.
- **Threading — extension survives the stack**: an integration test through the real `AuthService`
  layer asserting the handler observes `AuthContext` (with `groups`) from `request.extensions()`.
  This one is load-bearing: the design rests on tonic propagating extensions, and a tonic upgrade
  that broke it would otherwise fail silently and fail *open*.
- **`analytics-web-srv`**: a request through `cookie_auth_middleware` leaves `AuthContext` in
  extensions with `groups` populated (hole #3).
- **No behavior change**: existing `cargo test` suites pass untouched; explicitly assert an
  unconfigured deployment (`MICROMEGAS_IMPLICIT_GROUPS` unset) resolves a scope and changes no query
  result.
- `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `python3 build/rust_ci.py`.

## Open Questions

1. **`ReadScope` for API-key-authenticated analytics queries.** An analytics key carries no OIDC
   claim, so `AudienceReadPolicy` resolves implicit-groups-only — empty in a privacy deployment,
   i.e. that key can read nothing once Stage 2 lands. The AbAC plan already anticipates this
   ("`analytics_api_keys` may be transitional") but does not decide it. Stage 1 does not have to
   decide either — nothing enforces yet — but Stage 2 cannot ship without an answer, and the answer
   affects whether `ReadPolicy` needs a key-specific branch. Flagging now so it is not discovered at
   enforcement time.
2. **Whether `MintPolicy` belongs in Stage 1 at all.** Its only consumer is Stage 6, on a service
   (`analytics-web-srv`) whose mint route is admin-gated today — and the AbAC plan now carries an
   open question about whether self-service minting needs a non-admin path. Defining the trait now
   costs little and keeps the seam symmetric; deferring it would avoid designing against a call site
   whose shape is not settled. Recommendation: define it in Stage 1 as the issue specifies, but do
   not implement `AudienceMintPolicy`'s wiring anywhere until Stage 6.
