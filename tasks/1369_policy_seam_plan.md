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

`grep -rn make_session_context rust --include="*.rs"` finds **13 production call sites** (plus the
public `query()` wrapper at `query.rs:259-280`, which itself still takes `is_admin: bool` and forwards
it into `make_session_context` at `:269`) and **4 test files** that pass `is_admin` positionally into
`make_session_context` directly, plus **3 more test files** — `histo_view_test.rs:199`,
`sql_view_test.rs:421,447` and `thread_spans_ordering_db_test.rs` (already among the 4) — that call
the public `query()` wrapper with a bare positional `false`. **6 distinct test files** in total need
the `CallerContext` change. Neither the wrapper nor the test files were in the original inventory
below; both need the same `CallerContext` change as the production sites, which is why Testing
Strategy's "existing `cargo test` suites pass untouched" is softened in Phase 2, step 7.

Call sites of `make_session_context`:

| Site | `is_admin` | Needs |
|---|---|---|
| `flight_sql_service_impl.rs:661` (execute path) | from metadata | caller's scope |
| `flight_sql_service_impl.rs:1149` (prepared stmt) | from metadata | caller's scope |
| `analytics/src/metadata.rs:182`, `:283` (`find_stream_from_view`, `find_process_with_latest_timing`) | `false` | **caller's scope** — user-reachable via `jit_update` (`thread_spans_view.rs:343,352`, `net_spans_view.rs:326`, `async_events_view.rs:130`), itself invoked while planning a user's `view_instance(...)` query |
| `analytics/src/lakehouse/export_log_view.rs:118`, `:172` | `true` | `All` (maintenance) |
| `analytics/src/lakehouse/merge.rs:254` | `true` | `All` (maintenance) |
| `analytics/src/lakehouse/batch_partition_merger.rs:133` | `true` | `All` (maintenance) |
| `analytics/src/lakehouse/sql_batch_view.rs:106`, `:255` | `true` | `All` (maintenance) |
| `analytics/src/lakehouse/perfetto_trace_execution_plan.rs:254` | `false` | **caller's scope** — user-reachable |
| `analytics/src/lakehouse/parse_block_table_function.rs:82` | `false` | **caller's scope** — user-reachable (`parse_block` UDTF, registered for every caller, `query.rs:96-176`) |
| `analytics/src/lakehouse/process_spans_table_function.rs:254` | `false` | **caller's scope** — user-reachable (`process_spans` UDTF, same registration) |
| `analytics/src/lakehouse/query.rs:269` (inside public `query()`) | passed through | caller-supplied — `query()`'s own signature changes too |

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

#[async_trait]
pub trait ReadPolicy: Send + Sync + Debug {
    async fn resolve(&self, caller: &AuthContext) -> Result<ReadableAudiences>;
}
#[async_trait]
pub trait MintPolicy: Send + Sync + Debug {
    async fn resolve_audience(&self, caller: &AuthContext, requested: Option<&str>) -> Result<String>;
}
pub struct AudienceReadPolicy { implicit_groups: Vec<String> }
pub struct AudienceMintPolicy { implicit_groups: Vec<String> }

// rust/analytics/src/lakehouse/read_scope.rs
pub enum ReadScope { All, Audiences(Arc<[String]>) }
```

**Both traits are `async` and fallible, and both call sites deny on `Err`.** The AbAC plan specs
`#[async_trait]` (its §1–§2) and this is not cosmetic: the recorded target state resolves grants from a
store — nested groups plus group→audience grants — which cannot live behind a sync infallible signature
(AbAC plan, *Long-term model*, property 1). `async_trait` is already used across this crate
(`types.rs:139`, `multi.rs:78`) and both call sites are already async, so it costs nothing today.
Prong B still receives a resolved `ReadScope` *value*, not the policy object, so nothing downstream
becomes async.

The fallible half needs an explicit branch, not just a `?`: a resolution failure must become
`Status::unavailable` (store outage) or a denial, **never** an empty, defaulted, or `All` scope. Write it
in this issue, with a test that stubs `Err` — `AudienceReadPolicy` cannot fail, so a permissive fallback
here would stay invisible until the day a store-backed policy lands, and then be a silent fail-open.

`ReadableAudiences` is a newtype so a policy result cannot be confused with any other string list on
a security path. `ReadScope::All` is deliberately **not** a policy output — no `ReadPolicy` can
return it. It is the marker internal (non-request) callers pass, which is what makes
"who granted themselves `All`?" a greppable question with a small, auditable answer set.

`AudienceReadPolicy` and `AudienceMintPolicy` compute **related but distinct** sets — the plan's
§1–§2 formula, plus one addition settled during planning (**service-account grants**), applied to
*read* only. Folding the two into one shared formula would fold the Stage 4b read grant into mint
authority the moment `read_audiences` is populated, which the AbAC plan's *Read and write finally
separate* section calls a security regression.

The **mintable** set (`AudienceMintPolicy::resolve_audience`'s non-admin arm — what a caller may
stamp onto a newly minted key) is:

```
{user:<email>}       if email present      // human OIDC caller
  ∪ {group:<g>}       for g in groups claim
  ∪ {group:<g>}       for g in MICROMEGAS_IMPLICIT_GROUPS
```

The **readable** set (`AudienceReadPolicy::resolve`'s output) is the same union **plus** the
caller's read grant:

```
caller.read_audiences                        // grant carried by an analytics service-account key
  ∪ {user:<email>}     if email present      // human OIDC caller
  ∪ {group:<g>}        for g in groups claim
  ∪ {group:<g>}        for g in MICROMEGAS_IMPLICIT_GROUPS
```

`read_audiences` is deliberately absent from the mintable set: it is a Stage 4b **read** grant, and
a service-account key's ability to *see* an audience must not imply it can *stamp new keys* with
that audience — mint is integrity, read is confidentiality, the same distinction the admin-arm
rationale below draws in the other direction. Every element in both formulas is `user:`- or
`group:`-prefixed. Both unions are **branch-free — no `auth_type` check anywhere**: an OIDC caller
carries no `read_audiences`, and a key carries no email and no groups claim (`api_key.rs:116-127`,
`db_api_key.rs:318-328`). With no groups claim and no implicit groups a human caller's set is the
singleton `{user:<email>}` in both formulas — the per-user case, with no separate `Self*`
implementation.

`AuthContext.read_audiences` is empty for every principal in Stage 1; the analytics key provider
populates it in the AbAC plan's new **Stage 4b**, where analytics keys become service-account credentials
with a configurable read grant. Until then a key-authenticated query resolves implicit-groups-only —
empty in a privacy deployment, i.e. fail-closed. Carrying the field now is what keeps the formula
branch-free and keeps Stage 4b from reshaping the policy.

**`AudienceMintPolicy` needs an explicit admin arm:** `caller.is_admin` ⇒ any well-formed
(`user:`/`group:`-prefixed) audience; otherwise the mintable-set formula. Without it the only shipped
impl cannot express the mint flow that exists today — `mint_key` is `AdminUser`-gated
(`analytics-web-srv/src/ingestion_keys.rs:170-172`), so `requested = "user:bob@…"` is unrepresentable and
Stage 6 would stamp every fleet key with the minting admin's own audience. The arm grants no power the
route's gate does not already grant. It is deliberately asymmetric to the read path — `is_admin` is never
a read bypass (AbAC plan §5) — because mint is integrity and reads are confidentiality; say that in the
doc comment or it reads as a contradiction.

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

**Failure convention.** The absent-extension case is the *only* permissive branch in the resolver. A
`ReadPolicy::resolve` that returns `Err` is a hard failure, and the crate already has the discriminator for
which status to return: `e.downcast_ref::<ProviderUnavailable>()` (`auth/src/types.rs:22-24`), the same
check `tower.rs:146` uses to pick between `Status::unavailable` and its other error status. Mirror it here
— `Status::unavailable` when the error downcasts to `ProviderUnavailable` (a store/provider outage),
`Status::permission_denied` otherwise. No `unwrap_or_default()`, no "empty scope on error" — an empty
`Audiences([])` would read as a legitimate fail-closed decision to Stage 2/3 and hide the outage, and an
`All` fallback would be a fail-open bypass. The two branches (absent extension ⇒ `All`, `Err` ⇒ fail) look
adjacent and mean opposite things, so both get a doc comment and a test.

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
readable at a glance, and means Stage 2/3 add no further parameters. It touches the same call sites
the `read_scope` parameter would touch anyway (the full inventory is in Current State above), so it
is not extra work — just a better landing spot. `register_functions` / `register_lakehouse_functions`
take the same struct.

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

**Doc-comment framing matters here.** Describe the field as *IdP-asserted **leaf** membership — an input
to policy resolution, possibly incomplete* — not as "the caller's groups". In the AbAC plan's recorded
target state the IdP supplies direct memberships only, while nesting (group-in-group) and
group→audience grants live in a micromegas-owned store, so the caller's *effective* groups are the
transitive closure the policy computes, not this vector (AbAC plan, *Long-term model*, property 3). A
future reader who takes this field for the answer will inline the flat formula somewhere the policy seam
cannot reach.

### 5. Config factory

Add `AudienceReadPolicy::from_env(prefix: &str) -> Result<Self>` (`rust/auth/src/policy.rs`) — a plain,
synchronous, fallible constructor, deliberately **not** a method on `ProviderBuilder`: reading the
implicit-groups env var needs no DB pool, no async, and no `self`, and folding it into
`ProviderBuilder::build()` would change that method's return type (see below for why that is the wrong
trade). Reads the implicit-groups env var (comma-separated; the AbAC plan's Config surface table). Unset
⇒ empty implicit groups ⇒ readable set is the caller's singleton ⇒ enforcement inactive because nothing
consumes it yet. A malformed entry (the *Encoding* rule below) is `Err`, not an empty/defaulted set —
unlike `load_admin_users` (`oidc.rs:263-269`), which swallows a parse failure to `vec![]`. Both real call
sites (`flight_sql_server.rs`'s `use_default_auth` branch and the monolith's `main.rs`) already return
`Result` from their surrounding function, so surfacing `from_env`'s error with `?` is a startup failure,
never a silently dropped knob on a security path.

**Env var: prefix-scoped, following the existing `{prefix}_*`-with-fallback convention** —
`{prefix}_IMPLICIT_GROUPS` (e.g. `MICROMEGAS_ANALYTICS_IMPLICIT_GROUPS`) falling back to the
unprefixed `MICROMEGAS_IMPLICIT_GROUPS`, resolved by a new `implicit_groups_var(prefix: &str)` free
function in `default_provider.rs` mirroring `admin_var()`'s prefix-with-fallback logic
(`default_provider.rs:71-81`), but taking `prefix` directly rather than reading `self.prefix`, so
`AudienceReadPolicy::from_env` can call it without constructing a `ProviderBuilder`. `from_env` is
called with the same prefix a `ProviderBuilder` for that service would use
(`MICROMEGAS_INGESTION`/`MICROMEGAS_ANALYTICS` in the monolith, `main.rs:204,227`) — a prefix-blind
knob would be ambiguous the moment a second prefixed caller also resolves implicit groups, and
`MICROMEGAS_ADMINS` is exactly the precedent for *config-sourced authorization data* needing this
treatment. In practice only the analytics-facing call sites call `AudienceReadPolicy::from_env` (see
below), but the resolver follows the convention regardless, for the same reason
`admin_var()`/`api_keys_json()`/`oidc_config_var()` do.

**Encoding: comma-separated, and reject any entry containing `[`, `]`, or `"`, or that is empty after
trimming** (naming the offending entry). A plain "reject entries containing a comma" check is vacuous —
the value is split on commas first, so no resulting entry can ever contain one — and would not catch the
mistake it exists to catch. Note in the doc comment that this deliberately differs from
`MICROMEGAS_ADMINS`, which is a JSON array — that variable is the precedent for *config-sourced
authorization data* and for the `from_env`-when-unset pattern, not for an encoding. An operator copying the
`MICROMEGAS_ADMINS` shape (`serde_json::from_str::<Vec<String>>`, `oidc.rs:263-269`) would otherwise
silently configure one implicit group literally named `["everyone"]` (single entry, no comma) or two
groups `["a"` / `"b"]` (comma-free after the split) — both must be rejected by name.

Only the implicit-groups knob is parsed in Stage 1 — it is the only one the policies themselves
need. `MICROMEGAS_UNSTAMPED_AUDIENCE`, `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS` and
`MICROMEGAS_PUBLIC_VIEW_SETS` are consumed by Stages 2/3 and are parsed there; parsing them now
would create config that reads as active and is not.

Startup log line naming the resolved implicit groups (or "none") — the operator's only feedback that
the knob took effect, since there is no behavior to observe yet.

**The policy does not travel through `ProviderBuilder::build()` at all.** `build(self)` keeps its
current signature, `Result<Option<Arc<dyn AuthProvider>>>` (`default_provider.rs:114`), unchanged.
Folding the policy into that return type would ripple well past this issue's call sites: `build()`
backs the crate's two documented public entry points, `provider()` and `provider_with_prefix()`
(`default_provider.rs:214,227`, the latter calling `.build()` directly at `:228`), which are called
from `rust/telemetry-ingestion-srv/src/main.rs:59` (matching `Some(p)`/`None`) and from four sites in
`rust/auth/tests/default_provider_tests.rs` (`:100`, `:138`, `:219`, `:284`) — none of which have
anything to do with a read policy. A policy-carrying `build()` would force every one of those
call sites to unpack a tuple it has no use for. Because `AudienceReadPolicy::from_env` needs neither
`self` nor `.await`, there is no reason to route it through the builder at all: callers that want a
policy call `AudienceReadPolicy::from_env(prefix)` directly, alongside (not through) whatever
`ProviderBuilder` call they already make.

`FlightSqlServerBuilder` gains `with_read_policy(policy: Arc<dyn ReadPolicy>)`, mirroring
`with_auth_provider`. `flight_sql_server.rs`'s three auth branches (`:219-248`) each now have an
explicit policy source:
- `self.auth_provider` set (the monolith's injected-provider path, `main.rs:290`): the monolith calls
  `AudienceReadPolicy::from_env("MICROMEGAS_ANALYTICS")` alongside its existing
  `ProviderBuilder::new("MICROMEGAS_ANALYTICS")...build()` call and passes both halves —
  `.with_auth_provider(provider).with_read_policy(policy)`.
- `use_default_auth`: `FlightSqlServer` calls `AudienceReadPolicy::from_env("")` alongside its own
  `ProviderBuilder::new("").build()` call (`flight_sql_server.rs:~232-235`) and supplies both directly.
- neither set (`--disable-auth`, and any caller that never calls `with_read_policy`): default to
  `AudienceReadPolicy` with empty implicit groups — the same policy `AudienceReadPolicy::from_env`
  produces when its env var is unset. **Not** `ReadScope::All` — the absent-`AuthContext`-extension convention
  in §2 already supplies `All` when no provider is configured, so the policy field must not duplicate
  that decision; it only ever resolves a scope when an `AuthContext` is present to resolve one from.

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

1. `rust/auth/src/types.rs` — add `bound_audience: Option<String>`, `read_audiences: Vec<String>` and
   `groups: Vec<String>` to `AuthContext`. Update the three construction sites (`api_key.rs`,
   `db_api_key.rs`, `oidc.rs`) to `None`/`vec![]`, plus `auth/tests/tower_tests.rs`. Document
   `bound_audience` as Stage 4's field (write label, ingestion key provider only) and `read_audiences` as
   Stage 4b's (read grant, analytics key provider only) — one field per direction, neither populated here.
2. `rust/auth/src/policy.rs` (new) + `pub mod policy;` in `lib.rs` — `ReadableAudiences`,
   `MintPolicy`, `ReadPolicy`, `AudienceMintPolicy`, `AudienceReadPolicy`, both traits
   `#[async_trait]`. Doc comments state the readable-set formula, that `ReadPolicy` can never return
   "all", the mint admin arm and its asymmetry with the read path, and that `Err` must never be
   softened into a scope by any caller.
3. `rust/auth/src/oidc.rs` — `groups` claim on `Claims`; populate `AuthContext.groups` at `:536-545`.
4. `rust/auth/src/default_provider.rs` — `implicit_groups_var(prefix: &str)` free function
   (prefix-with-fallback, mirroring `admin_var()`'s logic but taking `prefix` directly rather than
   `self.prefix`), resolving `{prefix}_IMPLICIT_GROUPS` with fallback to `MICROMEGAS_IMPLICIT_GROUPS`.
   `rust/auth/src/policy.rs` — `AudienceReadPolicy::from_env(prefix: &str) -> Result<Self>` built on top
   of it, plus the startup log. `ProviderBuilder::build()`'s signature is unchanged — the policy is
   resolved separately, not returned from `build()`.

### Phase 2 — the analytics-side scope type and signatures

5. `rust/analytics/src/lakehouse/read_scope.rs` (new) — `ReadScope`, `CallerContext`,
   `CallerContext::internal()` / `::maintenance()`. Registered in `lakehouse/mod.rs`.
6. `rust/analytics/src/lakehouse/query.rs` — replace `is_admin: bool` with
   `caller: CallerContext` on `make_session_context`, `register_functions`,
   `register_lakehouse_functions`, **and on the public `query()` wrapper** (`:259-280`, whose callers
   are `rust/analytics/tests/thread_spans_ordering_db_test.rs`, `histo_view_test.rs` and
   `sql_view_test.rs`), which forwards `caller` into `make_session_context` at `:269` exactly as it
   forwards `is_admin` today. Registration logic reads `caller.is_admin`; `caller.read_scope` is
   stored/ignored for now (Stage 2/3 consume it).
7. Update the remaining call sites — all now require an explicit `CallerContext`:
   - `CallerContext::maintenance()` (background/materialization paths, `is_admin: true` today, never a
     user session): `export_log_view.rs:118,172`, `merge.rs:254`, `batch_partition_merger.rs:133`,
     `sql_batch_view.rs:106,255`.
   - `CallerContext::internal()` **with a named `TODO(#1371)`** on each — every one of these is
     reachable from a live user query, so `internal()`'s `ReadScope::All` is a latent bypass that
     Stage 3 must replace with the caller's inherited scope; a named TODO beats a silent `All`:
     `perfetto_trace_execution_plan.rs:254`; `parse_block_table_function.rs:82` and
     `process_spans_table_function.rs:254` (the `parse_block`/`process_spans` UDTFs, registered for
     every caller — `query.rs:96-176`); and `metadata.rs:182,283` (`find_stream_from_view` /
     `find_process_with_latest_timing`, called from `jit_update` at `thread_spans_view.rs:343,352`,
     `net_spans_view.rs:326`, `async_events_view.rs:130` while planning a user's
     `view_instance(...)` query — not maintenance, despite today's `is_admin: false` reading as
     "internal").
   - Six test files pass `is_admin` positionally today and must pass an explicit `CallerContext`
     instead: four calling `make_session_context` directly —
     `rust/analytics/tests/lakehouse_admin_gate_test.rs:37`, `log_stats_ordering_tests.rs:189`,
     `sql_partition_spec_sort_order_tests.rs:133`, `thread_spans_ordering_db_test.rs:381` — plus three
     calling the public `query()` wrapper (`thread_spans_ordering_db_test.rs` again, at `:338,349,406,559,572,606,633`,
     and `histo_view_test.rs:199`, `sql_view_test.rs:421,447`). No assertion or fixture changes — only
     the literal `true`/`false` argument becomes `CallerContext::maintenance()`/`::internal()`.

### Phase 3 — threading and hole-closing

8. `rust/public/src/servers/flight_sql_service_impl.rs` — add a `read_policy: Arc<dyn ReadPolicy>`
   field to `FlightSqlServiceImpl` and a matching parameter to its public constructor,
   `FlightSqlServiceImpl::new(...)` (`:489-503`, the struct's only constructor, re-exported via
   `pub mod flight_sql_service_impl` at `rust/public/src/servers/mod.rs:42` and reachable as
   `micromegas::servers::flight_sql_service_impl::FlightSqlServiceImpl` — a public-API signature
   change, called out in Trade-offs). Add a resolver helper
   (`async fn caller_context(&self, ext: &http::Extensions, md: &MetadataMap) -> Result<CallerContext, Status>`)
   implementing §2's absent-extension convention **and its failure convention** — `Err` maps to
   `Status::unavailable` when it downcasts to `ProviderUnavailable` (mirroring `tower.rs:146`) and to
   `Status::permission_denied` otherwise, never to a scope. Thread `&Extensions` into `execute_query`
   (two callers: `:800`, `:963`) and use the helper at `:661`. The `read_policy` argument does not
   exist yet when `flight_sql_server.rs`'s call to `FlightSqlServiceImpl::new(...)` (`:219-224`) runs
   today — see step 12, which moves the auth/policy resolution above it.
9. Same helper at `:1149` — closes hole #1.
10. `rust/auth/src/user_attribution.rs` — no code change; add a doc-comment warning that
    `UserAttribution` is audit-only and must never feed a `ReadScope`, naming the fallback at
    `:145-152` as the reason. Closes hole #2 by making the constraint explicit at the definition.
11. `rust/analytics-web-srv/src/auth/handlers.rs:~509` — insert `AuthContext` into request
    extensions. Closes hole #3.
12. Wire the policy at service construction (§5): add `with_read_policy()` to
    `FlightSqlServerBuilder`; in `flight_sql_server.rs`'s `use_default_auth` branch (~`:227-248`),
    call `AudienceReadPolicy::from_env("")?` alongside the existing `ProviderBuilder::new("")...build()`
    call and set both the provider and the policy; in the monolith (`main.rs:227,290`), do the same
    with `AudienceReadPolicy::from_env("MICROMEGAS_ANALYTICS")?` alongside its
    `ProviderBuilder::new("MICROMEGAS_ANALYTICS")...build()` call, and pass the resulting policy
    alongside `with_auth_provider`. Both call sites already return `Result` from their enclosing
    function, so a malformed knob fails startup via `?` rather than being silently dropped. Neither
    the injected-provider branch nor `--disable-auth` resolves a policy from env, so both default
    `read_policy` to `AudienceReadPolicy` with empty implicit groups when `with_read_policy` is never
    called. Unset config ⇒ a policy that resolves the caller singleton. This step must land *before*
    step 8's `FlightSqlServiceImpl::new(...)` call
    (`flight_sql_server.rs:219`) in execution order, since the constructed service now takes the
    resolved policy as a constructor argument — move the auth/policy resolution block
    (`flight_sql_server.rs:227-249`) above the `FlightServiceServer::new(FlightSqlServiceImpl::new(...))`
    call at `:219-224`.

## Files to Modify

- `rust/auth/src/types.rs` — `bound_audience`, `read_audiences`, `groups` on `AuthContext`
- `rust/auth/src/policy.rs` — **new**; traits + `Audience*` impls + `ReadableAudiences`
- `rust/auth/src/lib.rs` — `pub mod policy;`
- `rust/auth/src/oidc.rs` — `groups` claim (`:194-227`, `:536-545`)
- `rust/auth/src/default_provider.rs` — `implicit_groups_var(prefix: &str)` free function;
  `build()`'s signature is unchanged
- `rust/auth/src/api_key.rs`, `rust/auth/src/db_api_key.rs` — new `AuthContext` fields
- `rust/auth/src/user_attribution.rs` — doc-comment constraint only
- `rust/analytics/src/lakehouse/read_scope.rs` — **new**; `ReadScope`, `CallerContext`
- `rust/analytics/src/lakehouse/mod.rs` — register the module
- `rust/analytics/src/lakehouse/query.rs` — `CallerContext` on `make_session_context`,
  `register_functions`, `register_lakehouse_functions`, and the public `query()` wrapper
- `rust/analytics/src/metadata.rs`, `lakehouse/export_log_view.rs`,
  `lakehouse/perfetto_trace_execution_plan.rs`, `lakehouse/parse_block_table_function.rs`,
  `lakehouse/process_spans_table_function.rs`, `lakehouse/merge.rs`,
  `lakehouse/batch_partition_merger.rs`, `lakehouse/sql_batch_view.rs` — call sites
- `rust/analytics/tests/lakehouse_admin_gate_test.rs`, `log_stats_ordering_tests.rs`,
  `sql_partition_spec_sort_order_tests.rs`, `thread_spans_ordering_db_test.rs`, `histo_view_test.rs`,
  `sql_view_test.rs` — positional `is_admin` argument becomes an explicit `CallerContext`
- `rust/public/src/servers/flight_sql_service_impl.rs` — resolver, both call sites, struct field,
  `new()`'s new `read_policy` parameter (public API)
- `rust/public/src/servers/flight_sql_server.rs` — `with_read_policy()`, policy construction on the
  `use_default_auth` branch, default policy on the other two branches, and reordering the auth/policy
  resolution above the `FlightSqlServiceImpl::new(...)` call
- `rust/monolith/src/main.rs` — same wiring on the `MICROMEGAS_ANALYTICS` builder
- `rust/analytics-web-srv/src/auth/handlers.rs` — `AuthContext` into extensions
- `rust/auth/tests/` — new `policy_tests.rs`; update `tower_tests.rs`, `test_utils.rs` (`groups`
  field on `TestClaims` plus a token helper that sets it), and `oidc_tests.rs` (groups-claim unit
  tests)
- `rust/public/tests/` — new `read_policy_threading_tests.rs`, exercising `FlightSqlServiceImpl`
  through the real `AuthService`/tonic stack: fail-closed resolution (`unavailable` /
  `permission_denied`), the hole-#2 attribution-vs-`AuthContext` assertion, prepared-statement vs.
  `do_get` scope equality, and the extension-survives-the-stack integration test
- `rust/analytics-web-srv/tests/auth_integration.rs` — the `cookie_auth_middleware` test asserting
  `AuthContext` (with `groups`) lands in extensions (hole #3)

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
- **`FlightSqlServiceImpl::new` gains a required parameter vs. threading the policy some other way.**
  `new()` is public API (re-exported as `micromegas::servers::flight_sql_service_impl`), so this is a
  breaking change for any external caller who constructs the service directly. Chosen anyway: the
  struct's only other option — a `with_read_policy()` setter defaulting to a permissive policy when
  unset — is exactly the "defaulting parameter" shape rejected two bullets above, for the same reason.
  The signature change also forces `flight_sql_server.rs`'s auth/policy resolution to move above the
  `FlightSqlServiceImpl::new(...)` call (step 12), which is a net simplification: the service can no
  longer be constructed before its auth state is known. This is not the only public-API break in this
  plan: `micromegas-analytics` is itself a published crate (§1), re-exported wholesale as
  `micromegas::analytics` (`rust/public/src/lib.rs:152-153`, `pub use micromegas_analytics::*;`), and
  neither its `Cargo.toml` nor the workspace sets `publish = false`. Step 6's `CallerContext` change
  to `make_session_context` (`query.rs:207`), the public `query()` wrapper (`:259`),
  `register_functions` (`:187`) and `register_lakehouse_functions` (`:96`) — all four `pub` — breaks
  any external embedder calling them directly, for the identical "required parameter forces every call
  site to state its scope" reason argued above for `new()`.
- **`AudienceReadPolicy::from_env(prefix)` as a free-standing constructor vs. folding the policy into
  `ProviderBuilder::build()`.** Folding it in would change `build()`'s return type, which backs the
  public `provider()`/`provider_with_prefix()` entry points and would ripple into
  `telemetry-ingestion-srv/src/main.rs` and four sites in `default_provider_tests.rs` that have no use
  for a `ReadPolicy`. Chosen: a separate, synchronous constructor — the implicit-groups env var needs
  no DB pool and no `.await`, so there was never a reason to route it through the async builder.
- **Parsing only the implicit-groups knob now.** Parsing all four knobs would produce config
  that appears active and is not. Chosen: parse what Stage 1 consumes; the rest land with their
  consumers.
- **Groups stored raw in `AuthContext`, prefixed by the policy.** Keeps the AbAC `group:` convention
  out of a general-purpose auth type. Cost: the prefix is applied in two policies rather than once at
  the source.
- **`async` + fallible policy traits** vs. sync infallible ones. Sync would be simpler and sufficient for
  every v1 policy (claims + env, no I/O, cannot fail). Chosen: `async` and `Result`, because the AbAC
  plan's recorded target state resolves nested groups and grants from a store, and retrofitting the
  signature would migrate every call site *and* every impl at once. Cost today: `#[async_trait]` on two
  traits and one `.await` at two call sites, plus a deny-on-`Err` branch that no v1 policy can exercise —
  which is exactly why it ships with a stub-`Err` test.
- **No group vocabulary in `micromegas-analytics`.** `ReadScope` carries resolved audiences only, so the
  entire group/grant model — closure computation, nesting, caching, store outages — stays behind the
  policy seam in `micromegas-auth`. This started as a dependency-driven split (§1) and is now also the
  property that lets the long-term model land without touching the query planner.

## Documentation

Stage 1 ships no operator-visible behavior, so no mkdocs page yet (the isolation page is Stage 7).
What does need writing:

- Doc comments carrying the load-bearing arguments, since there is no behavior to observe: why
  `UserAttribution` may not feed a `ReadScope`; why an absent `AuthContext` extension means `All`
  while an `Err` from the policy must not; why `ReadPolicy` cannot return `All`; the mint admin arm and
  its asymmetry with the read path; that `bound_audience` stays `None` until Stage 4 and
  `read_audiences` stays empty until Stage 4b; that `groups` is IdP-asserted leaf membership.
- `tasks/data_isolation/audience_based_access_control_plan.md` — **updated 2026-08-12** with: the
  `ReadScope` placement and extension-over-header decisions (Stage 1); the service-account decision and
  its new Stage 4b; the `MintPolicy` admin arm; async/fallible traits and deny-on-`Err`; the
  `MintPolicy`-takes-`AuthContext` resolution of the third identity boundary; the knob encoding; and a
  new *Long-term model* section for groups/nesting/grants. Stale assertions that read scope never comes
  from a key (Stage 0 rationale, Stage 4 step 9, Security, Resolved Decisions) were rewritten rather
  than left to contradict Stage 4b.
- `CHANGELOG.md` — per the `pr` skill's convention.

## Testing Strategy

- **Unit — `AudienceReadPolicy`** (`rust/auth/tests/policy_tests.rs`): returns `{user:<email>}` when
  the groups claim and implicit groups are both empty; returns `{user:} ∪ group:claim ∪
  group:implicit` when both are present; every element carries a `user:`/`group:` prefix; a caller
  with no email and no implicit groups resolves to the **empty** set, not to something permissive.
- **Unit — service-account grants** (`AudienceReadPolicy`): a caller with `read_audiences = [a, b]`
  and no email/groups resolves exactly `{a, b} ∪ implicit`, with **no** `user:` element; the same
  caller with an empty grant resolves implicit-only. This is Stage 4b's data flowing through the read
  formula, and it is what proves the union needed no `auth_type` branch.
- **Unit — `AudienceMintPolicy`**: defaults to `user:<email>` when `requested` is `None`; permits a
  `requested` value inside the mintable set; rejects one outside it; **read grant confers no mint
  authority** — a caller whose `read_audiences` includes an audience outside `{user:<email>} ∪
  groups ∪ implicit` is refused that audience by a non-admin mint, proving the two formulas stay
  separate; **admin arm** — an `is_admin` caller is permitted an arbitrary well-formed audience
  (including another user's `user:` value) that a non-admin caller is refused; a malformed,
  unprefixed audience is refused for both.
- **Fail-closed resolution**: a stub `ReadPolicy` returning `Err` makes the request fail; cover both arms
  of the `ProviderUnavailable` discriminator — an `Err(ProviderUnavailable(..))` resolves
  `Status::unavailable`, any other `Err` resolves `Status::permission_denied` — and assert that no
  `CallerContext` is built either way, specifically that the outcome is neither `Audiences([])` (which
  would read as a legitimate decision) nor `All`. The shipped policy cannot fail, so this test is the only
  thing holding the convention in place.
- **Unit — groups claim** (`rust/auth/tests/oidc_tests.rs`, minting tokens via a `groups` field added
  to `test_utils.rs`'s `TestClaims`): a token with a flat `groups` array populates `AuthContext.groups`;
  a token **without** the claim still deserializes and yields `vec![]` — the backward-compatibility
  guarantee, so it deserves its own test.
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
- **No behavior change**: existing `cargo test` suites pass once the six test files that pass
  `is_admin` positionally are updated to pass an explicit `CallerContext` (Phase 2, step 7) — no
  assertion or fixture in those tests changes, only the literal argument; explicitly assert an
  unconfigured deployment (implicit-groups env var unset) resolves a scope and changes no query
  result.
- `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `python3 build/rust_ci.py`.

## Resolved Questions

Both open questions closed 2026-08-12; the AbAC plan carries the decisions in its Resolved Decisions
section.

1. **`ReadScope` for key-authenticated analytics queries — resolved: analytics keys are service accounts
   with a configurable read grant.** Research reframed the question. It is not key-specific: Grafana's
   only two auth modes are a static analytics key and **OAuth 2.0 client credentials**
   (`grafana/pkg/flightsql/oauth.go`), and an M2M token takes the OIDC path with no `email`
   (`oidc.rs:535`), so it resolves to the empty set too — a key-specific `ReadPolicy` branch would have
   fixed half the problem. And key-only flight-sql is a documented, supported deployment ("a non-empty
   `analytics_api_keys` table counts as auth configured on its own",
   `mkdocs/docs/grafana/authentication.md`), so attrition was never available.
   **Decision:** `analytics_api_keys` gains a set-valued `read_audiences` grant per key (AbAC plan Stage
   4b); Stage 1 carries the `AuthContext.read_audiences` field and the branch-free union that consumes
   it. Blast radius until 4b lands is bounded — open deployments resolve `{group:everyone}` through
   implicit groups, so only privacy deployments see a key with no grant, and that case is fail-closed.
   **Rejected:** an env subject→groups map (a redeploy per new service account — the operational problem
   Stage 0 exists to kill); delegation-derived scope (client-claimed identity, hole #2 verbatim — keys
   carry `allow_delegation: true` and Grafana already sends `x-user-*`); deprecating analytics keys (a
   documented mode needs a deprecation cycle, not a stage decision).
2. **`MintPolicy` in Stage 1 — resolved: yes, and it needs the admin arm.** Defining the trait now keeps
   the seam symmetric and costs little, as originally recommended; the wiring still waits for Stage 6. But
   the shipped `AudienceMintPolicy` must carry the `is_admin` arm (§1), or the only impl cannot express
   the only mint flow that exists today. That narrows Stage 6's remaining question to *who may call the
   route* — a route-authorization decision that no longer changes the policy's shape. The trait takes
   `&AuthContext`, which §6's one-line extension insert on `analytics-web-srv` is what makes possible: that
   is the answer to the AbAC plan's "either carry `groups` into `ValidatedUser` or leave `MintPolicy`
   taking an `AuthContext`".

## Long-term Context

The AbAC plan now records a **target state** this seam must not foreclose (its *Long-term model*
section): users belong to groups, groups nest, and a group is granted a set of audiences it may read —
with today's flat rule as the degenerate identity-grant case (`read_grants(G) = {group:G}`), so adopting
it changes no caller's readable set. Four properties of this issue are what keep it reachable, all folded
into the design above:

1. `ReadPolicy` / `MintPolicy` are `async` and fallible (§1 types).
2. The resolver denies on `Err` (§2 failure convention, step 8, and its test).
3. `AuthContext.groups` is documented as IdP-asserted leaf membership, an input to resolution (§4).
4. No group vocabulary crosses into `micromegas-analytics` — `ReadScope` carries resolved audiences only
   (§1, Trade-offs).

Nothing else in this issue changes for the group model; the store, closure computation, cycle handling,
grant-latency caching and admin surface all land behind `ReadPolicy` later.
