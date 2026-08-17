# Ingestion Stamping of `micromegas.audience` Plan (#1373)

## Overview

Stage 5 of the AbAC rollout (epic #1334, design in
`tasks/data_isolation/audience_based_access_control_plan.md`). Stage 2 (#1370) already filters
every query by the `micromegas.audience` property on the process, and Stage 4 (#1372) already
binds one immutable write audience to every `ingestion_api_keys` row and carries it authenticated
into `AuthContext.bound_audience`. Nothing connects the two: ingestion discards the auth context,
so the property the query filter trusts is still whatever the instrumented client happened to send
(`ownership_rewrite.rs:59-75` documents this as the known Stage 2 gap). This stage makes the
property **server-written from the authenticated credential** — the trust anchor moves from the
client payload to the ingestion key — and closes the two structural gaps that stamping exposes on
the OTLP side: an authenticated identity that is validated then thrown away (Firehose), and a
`process_id`/`block_id` derivation that is a pure function of client-supplied bytes and therefore
collapses two audiences onto one process row.

## Scope

In scope:

1. Resolve the write audience from `AuthContext.bound_audience` at every ingestion entry point and
   thread it to the two process-insert sites.
2. Write it as the reserved `micromegas.audience` property; strip any client-supplied
   `micromegas.*` property so the reserved namespace cannot be asserted from the payload.
3. Make OTLP-derived `process_id` / `block_id` audience-scoped, so two audiences sending
   identical resources/payloads do not collapse onto one process row (silent cross-audience
   mislabeling + silent data loss).
4. Enforce one audience per process on re-registration (the invariant Stage 2's
   `MAX(audience)` resolution already assumes).
5. The OTLP / Firehose auth story: propagate the authenticated context Firehose already resolves
   but discards; document what each entry point authenticates with.
6. A fail-closed knob (`{prefix}_REQUIRE_WRITE_AUDIENCE`) rejecting writes from credentials that
   carry no audience, off by default.

Out of scope (see [What this stage does not close](#7-what-this-stage-does-not-close)):
cross-audience `insert_stream` / `insert_block` injection into another audience's existing process
(an integrity-only gap; needs its own issue), and promoting the property to a first-class column
(the plan's "Later" step 15).

## Current State

Verified against the tree at `5298a1ca9`.

### Ingestion entry points and what each authenticates with

`serve_ingestion` (`rust/public/src/servers/ingestion.rs:115-185`) assembles four groups:

| Routes | Auth today | `AuthContext` reaches the handler? |
|---|---|---|
| `/ingestion/insert_process`, `insert_stream`, `insert_block` (`ingestion.rs:96-101`) | global `auth_middleware`, applied to `protected_app` (`ingestion.rs:141-156`) | yes, as a request extension — but no handler reads it |
| `/ingestion/otlp/v1/{logs,metrics,traces}` (`otlp.rs:188-195`) | same — `otlp_router()` is merged **into** `protected_app` (`ingestion.rs:142`) | yes, unread |
| `/ingestion/webhook` (`webhook.rs:158`) | same — merged into `protected_app` (`ingestion.rs:143`) | yes, unread |
| `/ingestion/otlp/v1/metrics/firehose` (`firehose.rs:71-87`), `.../logs/firehose` (`firehose_cloudwatch_logs.rs:66-75`) | own `firehose_auth_middleware` (`firehose_common.rs:71-132`), synthesizing `Authorization: Bearer` from `X-Amz-Firehose-Access-Key` and validating through the **same** `AuthProvider` | **no** — `Ok(_ctx) => { … next.run(req) }` (`firehose_common.rs:98-108`) drops it |

The issue's premise ("OTLP handlers currently have no auth wiring at all, and Firehose routes are
merged outside the protected router") is **stale on the first half and misleading on the second**:
OTLP and webhook sit inside `protected_app` and are covered by `auth_middleware`; Firehose is
indeed merged outside it (`ingestion.rs:158-168`) but carries equivalent auth of its own, because
Firehose can only send a credential in a non-standard header. So the OTLP/Firehose auth story is
not "add auth" — it is **one missing `extensions_mut().insert(ctx)`** plus a decision about
credentials that authenticate with no audience.

With no auth provider (`--disable-auth`, `--disable-ingestion-auth`) no middleware runs at all, so
no extension exists — the dev-mode path.

### Where `bound_audience` comes from

- `DbApiKeyAuthProvider` → `bound_audience: row.audience.clone()`, `Some(..)` for every ingestion
  key (`db_api_key.rs:370`; the column is `NOT NULL` with a
  `CHECK (audience ~ '^[A-Za-z0-9_-]+$')` constraint since migration v6).
- Env keyring `ApiKeyAuthProvider` → `None` (`api_key.rs:128`).
- OIDC → `None` (`oidc.rs:553-555`).

So a deployment that has not migrated to the DB key store, or that authenticates producers with
OIDC, has no audience to stamp. That rules out "reject when absent" as a Stage 5 default.

### The two process-insert sites

- `WebIngestionService::insert_process` (`web_ingestion_service.rs:359-396`) — parses `ProcessInfo`
  from CBOR and binds `make_properties(&process_info.properties)` verbatim
  (`telemetry/src/property.rs:84-88`). No reserved-key filtering: a native client can send
  `micromegas.audience` and today the query filter believes it.
- `WebIngestionService::register_otel_process` (`web_ingestion_service.rs:405-451`) — takes an
  already-built `Vec<Property>`; every OTLP resource attribute lands namespaced
  `otel.resource.<key>` (`block.rs:449-457`), so an OTLP client cannot reach the reserved
  namespace, but nothing server-side writes into it either.

Both use `ON CONFLICT (process_id) DO NOTHING`: a re-registration of an existing `process_id` is a
silent no-op, whatever it claims. `insert_stream` (`:265-308`) binds stream properties verbatim
too; `insert_block_typed` (`:146-261`) inserts with `ON CONFLICT (block_id) DO NOTHING`
(`:186`) and writes the payload to `blobs/{process_id}/{stream_id}/{block_id}`.

`analytics/src/replication.rs:120-145` copies `processes` rows (properties included) between
lakes, so a replicated process keeps the audience it was stamped with at its origin — the correct
behavior, and no change is needed there.

### OTLP identity is a pure function of client bytes

- `process_id_from_resource` (`identity.rs:188-231`) hashes a 31-field tuple of client-supplied
  resource attributes under `NS_OTEL_PROCESS_V1`.
- `stream_id_from_process_signal` (`identity.rs:234-237`) derives from `(process_id, signal)`, so
  it inherits whatever scoping `process_id` has.
- `block_id_from_payload` (`identity.rs:241-243`) hashes the bytes it is handed — its doc comment
  says "the encoded resource submessage", already stale.
  `split_logs_with_extra_hash_input` (`block.rs:285-329`) folds extra bytes in ahead of them for the
  webhook path (`:309-312`); `split_metrics` (`:332-354`) and `split_traces` (`:357-379`) do not have
  the hook.
- `is_degenerate_resource` (`identity.rs:162-167`) exists precisely because resources with none of
  `host.id`/`host.name`/`process.pid`/`service.instance.id` collapse onto one id — it only
  `debug!`s (`block.rs:219-228`).

Consequence once stamping exists: two audiences sending the same resource attributes (the same
containerized app in two tenants, a degenerate resource, a CloudWatch namespace) derive the same
`process_id`. The first one there owns the process row and its audience; the second's blocks land
under the first's audience (mislabeled, and invisible to their own owner), and byte-identical
payloads additionally dedup on `block_id` so the second write is silently dropped. This is not
only an attack — it is the ordinary multi-tenant case, so it must be fixed in this stage.

### The read side that consumes the stamp

`OwnershipRewrite::audience_col` (`ownership_rewrite.rs:140-151`) reads
`property_get(properties, 'micromegas.audience')` off the raw `processes` partitions, collapsed to
one row per process by `MAX(audience)` (`:153+`). `OwnershipRewriteConfig.unstamped_audience`
(`read_scope.rs:88-96`) coalesces a `NULL` audience to a configured label. Two facts follow:

- **`MAX(audience)` assumes at most one distinct audience per process** over its lifetime. Nothing
  enforces that today.
- **The stamp must be present before the process's blocks are materialized**, because
  `BlocksView::data_sql` snapshots `processes.properties` into the `blocks` partitions and the
  `processes` view reads from those partitions, not from Postgres
  (`ownership_rewrite_db_test.rs:14-23`). Stamping at process-insert time — before any block for
  that process exists — satisfies this by construction, and is another reason never to
  retro-`UPDATE` a stamp.

## Design

### 1. `WriteAudience` — the value threaded through the write path

A newtype in `micromegas-ingestion` (`rust/ingestion/src/write_audience.rs`), so
`micromegas-otel-ingestion` can name it without depending on `micromegas-auth` — unlike
`micromegas-public`, which already pulls in `micromegas-auth` (`dep:micromegas-auth` under its
`server` feature), `micromegas-otel-ingestion` is auth-free today and should stay that way:

```rust
/// The authenticated write audience a request ingests under (AbAC Stage 5, #1373).
/// `None` means the credential carries no audience -- data stays unstamped, exactly as
/// before this stage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteAudience(Option<Arc<str>>);

impl WriteAudience {
    /// Rejects a malformed label rather than stamping it: `ingestion_api_keys.audience` is
    /// already `CHECK`-constrained, so this is defence in depth against a future producer of
    /// `bound_audience`.
    pub fn new(audience: Option<&str>) -> anyhow::Result<Self>;
    pub fn none() -> Self;
    pub fn as_str(&self) -> Option<&str>;
}
```

The charset check duplicates `micromegas_auth::policy::is_valid_audience` (`policy.rs:44-50`)
rather than depending on `micromegas-auth` from `micromegas-ingestion` — the same crate-boundary
trade-off `read_scope.rs:108-114` already made, documented at `:102-107`, for
`is_well_formed_audience`. Keep that cross-reference in the doc comment so the three copies stay
discoverable.

Both process-insert signatures take it by reference, and so do the OTLP entry points. Per
`rust/CLAUDE.md`'s Rust-API stance, no defaulted parameter: every call site states its audience,
so the compiler enumerates the internal/test sites that must pass `WriteAudience::none()`.

### 2. Reserved property namespace

Two constants in `micromegas-telemetry` (`telemetry/src/property.rs`), the lowest crate common to
the writer (`micromegas-ingestion`) and the reader (`micromegas-analytics`):

```rust
/// Reserved, server-written property namespace. A client-supplied property whose key starts
/// with this prefix is dropped at ingestion (AbAC Stage 5, #1373).
pub const RESERVED_PROPERTY_PREFIX: &str = "micromegas.";
/// The audience a process's data belongs to -- written server-side from the authenticated
/// ingestion credential, read by `OwnershipRewrite`.
pub const PROPERTY_AUDIENCE: &str = "micromegas.audience";
```

`ownership_rewrite.rs:148`'s `lit("micromegas.audience")` becomes `lit(PROPERTY_AUDIENCE)`, so the
write side and the read side cannot drift. The only `micromegas.*` property key in the tree today is
`micromegas.audience` itself — read at `ownership_rewrite.rs:148`, hand-written by
`ownership_rewrite_db_test.rs:154` — and no client (`python/`, `unreal/`, the Rust sink) sets any
other one, so reserving the whole prefix costs nothing and pre-empts the next reserved key needing
its own migration. Note `property.rs` is behind `micromegas-telemetry`'s non-default `server`
feature (`telemetry/src/lib.rs:9-10`), which every crate here already enables; the constants are
therefore invisible to `micromegas-telemetry-sink`, which is fine since only server code needs them.

### 3. Stamping and stripping, in one place per insert path

In `web_ingestion_service.rs`, one helper used by both process paths:

```rust
/// Drops every client-supplied reserved-namespace property, then appends the server-written
/// audience. Client input can neither assert nor suppress the stamp.
pub fn finalize_process_properties(client: Vec<Property>, audience: &WriteAudience) -> Vec<Property>;
```

`pub`, not private, and the same for `strip_reserved_properties` below: `rust/CLAUDE.md` puts unit
tests under the crate's `tests/` folder, which cannot reach a private item, and the Testing
Strategy asserts both helpers directly (reserved keys dropped, `None` writing no property at all).
That is exactly why `handler::build_webhook_request` is public — "so `tests/webhook_tests.rs` can
assert its shape directly" (`handler.rs:234-235`). Both are pure functions of their arguments, so
exposing them widens no invariant.

- `insert_process(body, audience)` → `finalize_process_properties(make_properties(&info.properties), audience)`.
- `register_otel_process(..., properties, audience)` → same call on the `otel.resource.*` list.
- `insert_stream(body)` strips the reserved prefix as well (a second tiny helper,
  `pub fn strip_reserved_properties`). Nothing reads a stream audience today; stripping keeps the
  namespace honest so a later stage that does read one is not reading client input.
  `register_otel_stream` has no client-supplied stream properties to strip — it binds
  `Vec::<Property>::new()` unconditionally (`web_ingestion_service.rs:341`) — so it needs no call.
- A dropped reserved key logs at `warn!` once per process registration, naming the key — a native
  client setting `micromegas.audience` was either doing the pre-Stage-5 thing or probing, and both
  are worth seeing. Stripping (rather than rejecting the request with 400) keeps a legacy producer
  that self-stamped from losing all telemetry on upgrade. The self-stamp itself, though, stops
  taking effect: today it is the *only* mechanism that stamps a process at all
  (`ownership_rewrite.rs:59-75`; `ownership_rewrite_db_test.rs:144-161` hand-stamps exactly this
  way), and it is replaced by the credential's `bound_audience`, which is `None` for both
  env-keyring keys (`api_key.rs:128`) and OIDC (`oidc.rs:553-555`). A producer that self-stamped
  `team-a` while authenticating with such a credential silently becomes **unstamped** on upgrade:
  invisible to every `ReadScope::Audiences` caller when `{prefix}_UNSTAMPED_AUDIENCE` is unset, and
  widened to the unstamped label (visible to whoever that label is granted to) when it is set.
  Carrying the producer's history forward under its own label requires switching it to a DB
  ingestion key bound to that audience.
- `WriteAudience::none()` stamps nothing at all — the property is absent, not empty. `NULL` is what
  `OwnershipRewriteConfig.unstamped_audience` already coalesces; an empty string would silently
  fail every `IN` comparison instead.

No `UPDATE` path, ever: an existing process is never retro-stamped (see the materialization
snapshot in Current State).

### 4. Audience-scoped OTLP identity

Fold the audience into `process_id` and `block_id` derivation, so identity is per-audience.

```rust
/// Identity inputs beyond the OTLP payload itself (AbAC Stage 5, #1373).
#[derive(Debug, Clone, Copy, Default)]
pub struct IdentityContext<'a> {
    /// Authenticated write audience. Folded into `process_id` and `block_id` so two audiences
    /// posting identical resources/payloads never collapse onto one process or dedup against
    /// each other. `None` reproduces pre-Stage-5 ids byte for byte.
    pub audience: Option<&'a str>,
    /// Webhook-only: canonicalized incoming header bytes (today's `extra_hash_input`).
    pub extra_hash_input: &'a [u8],
}
```

- `process_id_from_resource(resource, ctx)` appends the audience as a **32nd field, only when
  `Some`** — appending unconditionally would add a trailing `\x1F` and re-derive every existing id
  even for unstamped deployments. With `None` the joined key is byte-identical to today.
  `identity.rs:169-187` already licenses in-place field addition under the same namespace UUID
  ("Long-term stability of `process_id` values is not a design goal").
- `block_id`: prepend `aud\x1F{audience}\x1F` to the hash input when `Some`, ahead of the
  existing `extra_hash_input` bytes, reusing `identity.rs`'s own separator const rather than a
  `\x1F` string literal — so the prefix is `format!("aud{SEPARATOR}{audience}{SEPARATOR}")`. Both
  `SEPARATOR: char` and `SEPARATOR_STR: &str` (`identity.rs:39-40`) are **module-private** today,
  and the prepend happens in `block.rs`, a sibling module that cannot name them: step 9 therefore
  widens both to `pub(crate)`. (The alternative — a `block_id_from_payload_with_audience` helper
  inside `identity.rs` — keeps them private but splits the hash-input assembly across two modules,
  where the webhook path already assembles it in `block.rs`.)
  `block_id_from_payload(payload: &[u8])` (`identity.rs:241`) needs no signature change at all —
  the prepend is caller-side concatenation in `block.rs`, exactly as the webhook path already does
  at `block.rs:309-312`. Necessary and not redundant with `process_id`: `blocks` conflicts on
  `block_id` alone (`web_ingestion_service.rs:186`), so without this, two audiences with
  byte-identical payloads silently dedup into one row belonging to one of them.
- While in there, fix `identity.rs:239-240`'s doc comment. It claims `block_id` derives "from the
  re-encoded protobuf bytes of one Resource submessage", which the webhook path already falsified
  when it started folding `extra_hash_input` in; the audience makes it a third input.
- `stream_id` needs no change — it derives from `process_id`.
- Collapse `split_logs` (`block.rs:273-275`, today a thin `split_logs_with_extra_hash_input(req, &[])`
  wrapper) and `split_logs_with_extra_hash_input` (`:285-329`) into a single `split_logs(req, ctx)`;
  give `split_metrics(req, ctx)` (`:332-354`) / `split_traces(req, ctx)` (`:357-379`) the same
  parameter (they have no hook today). One shared code path for both identity inputs instead of a
  logs-only special case.
- `is_degenerate_resource`'s `debug!` (`block.rs:219-228`) stays as-is: after this change a
  degenerate resource collapses processes *within* one audience only, which is the pre-existing,
  documented behavior.

**Cost, stated plainly.** A deployment that starts stamping re-derives its OTLP `process_id`s: the
same logical process appears as a new row, and its pre-upgrade data keeps the old id and stays
unstamped (visible via `{prefix}_UNSTAMPED_AUDIENCE`, hidden without it). Rotating an ingestion key
to a different audience likewise splits a long-lived producer's history across two process ids —
semantically right (the data belongs to two audiences) but worth a docs sentence. Unstamped
deployments see no churn at all.

**Alternative rejected:** keep ids stable and reject a colliding write. That permanently locks the
second tenant out of ingestion whenever two tenants genuinely share resource attributes — worse
than id churn, and it converts a labeling problem into an outage.

**Alternative rejected:** fold the *ingestion key* into the identity instead of the audience. The
audience is the right granularity for three reasons. (1) Key rotation is routine and an audience
change is not: keying on the credential would fork a long-lived producer's `process_id` on every
rotation, while the audience only changes when the data genuinely changes owner — which is exactly
when a new process id *is* required, since one `processes` row cannot carry two audiences (§6). (2)
It buys no ownership separation: the read side scopes by audience (`ownership_rewrite.rs:140-151`,
collapsed with `MAX(audience)` per process), so two keys of the same audience landing on one
`process_id` is an intra-tenant merge — the same behavior a single key has today — not a
cross-audience leak. (3) There is no stable key identity to hash anyway: `AuthContext` carries no
`key_id`, `DbApiKeyAuthProvider` sets `subject: row.name.clone()` (`db_api_key.rs:349`) and `name` has no
unique index (`sql_migration.rs:104-113`; the only unique index is on `key_hash`, `:117`, and the v6
migration at `:152-169` adds none), while env-keyring keys have no row at all
(`api_key.rs:116-131`) — the input would be non-unique and provider-dependent.

### 5. Resolving the audience at the HTTP edge

Resolution lives in `rust/public` (the only crate that sees both `AuthContext` and the ingestion
service), in a new `servers/write_audience.rs`:

```rust
/// Per-service stamping config, resolved once at startup.
pub struct StampingConfig { require_write_audience: bool }
impl StampingConfig { pub fn from_env(prefix: &str) -> anyhow::Result<Self>; }

/// Resolves the audience for one request. `Err` is a 403 -- never a silent unstamped write.
pub fn resolve_write_audience(
    ctx: Option<&AuthContext>,
    cfg: &StampingConfig,
) -> Result<WriteAudience, WriteAudienceError>;
```

Rules:

| Credential | `require_write_audience` off (default) | on |
|---|---|---|
| DB ingestion key (`bound_audience: Some`) | stamp it | stamp it |
| Env-keyring key / OIDC (`None`) | unstamped + `warn!` (rate-limited) | **403**, body `write audience required` |
| No auth provider (no extension) | unstamped | **403** |

Every handler gains `ctx: Option<Extension<AuthContext>>`. Post-condition, once step 4 lands: absent
⇔ no auth provider, since `auth_middleware` inserts unconditionally on its success path
(`auth/src/axum.rs:82`, every failure short-circuiting at `:56-64`) and `firehose_auth_middleware`
then does the same. Before step 4 the Firehose path is the one exception, which is why step 4 is
ordered ahead of the handlers. For the native/OTLP/webhook routes — all merged into `serve_ingestion`'s
own router tree — the `StampingConfig` reaches handlers the same way: an `Extension<Arc<StampingConfig>>`
layered by `serve_ingestion`, which takes the `StampingConfig` as a parameter; the two binaries build
it with their own prefix — `""` for `telemetry-ingestion-srv` (`main.rs:59-63`),
`"MICROMEGAS_INGESTION"` for the monolith (`main.rs:206`) — matching how they already scope
`ProviderBuilder`.

The two Firehose routers are different: `firehose_router(service, auth_provider)` is built and
layered standalone by `serve_ingestion` (`ingestion.rs:161-163`), but `rust/public/tests/firehose_tests.rs`
and `firehose_cloudwatch_logs_tests.rs` (11 call sites total) construct it directly with no parent
router to supply an extension — a required `Extension<Arc<StampingConfig>>` extractor would 500 at
those sites with no compiler signal. So both `firehose_router` variants instead take an explicit
`stamping: Arc<StampingConfig>` parameter and layer it themselves (`.layer(Extension(stamping))`,
alongside the existing `Extension(service)`), the same way they already take `auth_provider`
explicitly rather than expecting it as an ambient extension. `serve_ingestion` passes its
`StampingConfig` through to both calls; every existing direct-call test site fails to compile until
updated, per §1's stance.

This asymmetry — `otlp_router`/`webhook_router`/`register_routes` keep the ambient
`Extension<Arc<StampingConfig>>` rather than taking it as an explicit parameter — is justified only
by in-tree call sites: `servers/mod.rs` exports all four routers alike, but only `firehose_router`
has call sites outside `serve_ingestion` (the 11 tests above); the other three have none today. An
embedder that mounts `otlp_router`/`webhook_router`/`register_routes` directly, outside
`serve_ingestion`'s own tree, must layer the `StampingConfig` extension itself or hit the same
missing-extension 500 that motivates `firehose_router`'s explicit-parameter design.

**The Firehose fix** is a two-token change at `firehose_common.rs:98-108`: bind the context
(`Ok(_ctx)` → `Ok(ctx)`) and `req.extensions_mut().insert(ctx)` before `next.run(req).await`,
mirroring `auth/src/axum.rs:82`. Nothing else on that arm moves — the five spoofable-header strips
(`firehose_common.rs:99-106`) are already there and stay as-is. Both Firehose routers then behave
exactly like the Bearer routes.

Rejection shape per entry point, so a rejected write is retried-or-not correctly:

- **Native routes** (`insert_process`/`insert_stream`/`insert_block`): 403 + plain body, via a new
  `IngestionError::Forbidden` variant alongside the existing `BadRequest`/`Internal`
  (`servers/ingestion.rs`).
- **OTLP** (`/otlp/v1/*`): `google.rpc.Status` (code 7, `PERMISSION_DENIED`), via a new
  `OtelError::Denied { signal: Signal, message: String }` variant (`otel-ingestion/src/error.rs`).
  It carries a `signal` like every other variant because `OtelError::signal()` (`error.rs:55-61`)
  returns `Signal` unconditionally, with no fallback to give a signal-less variant. Five exhaustive
  matches in that file need the new arm — `signal()`, `grpc_code()` → `7`, `http_status()` → `403`,
  `public_message()` → a sanitized `"write audience required"` (no internal detail), and
  `with_context()` (`:122-137`), which is easy to miss. `is_retryable()` (`:84`) uses `matches!`, so
  `Denied` is non-retryable by default with no edit. **No `OtlpHttpError` arm is needed**:
  `OtlpHttpError` is just `{ WrongContentType, Otel(OtelError) }` (`otlp.rs:61-64`) and
  `into_otlp_response` (`:67-95`) already maps an arbitrary `http_status()` through
  `other => StatusCode::from_u16(other)`, so 403 + code 7 + the sanitized message flow through
  untouched. `otlp.rs` still changes, but only for step 8's extractors.
- **Webhook** (`/ingestion/webhook`): reuses `OtelError::Denied` (the same variant OTLP uses) but
  renders it through `webhook.rs`'s own `build_error_response(status, message, retryable)`
  (`webhook.rs:99-112`) rather than the OTLP shape — 403, `text/plain`, and `retryable == false`
  (no `Retry-After` header), since a denied write is not a transient condition a webhook sender
  should retry.
- **Firehose**: the Firehose ack shape with `errorMessage`, still a non-2xx status — but this is
  *not* a clean rejection the way the other two are. Firehose does not distinguish 4xx from 5xx;
  it retries any non-200 for its configured retry duration and then spills to the configured S3
  backup bucket (`firehose_common.rs:66-70` already documents "retries/spills" as the intended
  behavior for a Firehose-shape error). Enabling `require_write_audience` against an audience-less
  Firehose delivery stream therefore produces a retry-then-spill, not an immediate rejection — an
  operator note for `mkdocs/docs/admin/ingestion.md`'s "What gets stamped" section.

**Prefixed-var resolution, DRY.** The helper already exists in shape — copy it rather than invent
it: `read_scope.rs:148-162`'s `fn resolved_var(prefix: &str, suffix: &str) -> String` resolves
`{prefix}_{suffix}`, falling back to `MICROMEGAS_{suffix}` when unset *or* whenever `prefix` is
empty. Promote that exact signature to `pub fn resolve_prefixed_var` in a new
`rust/auth/src/env.rs` (declared in `lib.rs`) — three sibling modules consume it (`policy.rs`,
`default_provider.rs`, `db_api_key.rs`), so parking it in any one of them would make the other two
import an unrelated module for a pure env concern. Use it for the new knob, and refactor the four
hand-rolled copies in that crate onto it:
`policy.rs:55-66` (`audience_grants_var`), `policy.rs:70-81` (`default_key_audience_var`),
`default_provider.rs:63-71` (`oidc_config_var`), `default_provider.rs:75-86` (`admin_var`).
Two adjacent call sites also fit and should move: `default_provider.rs:51-59` (`api_keys_json`,
which returns the *value* — becomes `std::env::var(resolve_prefixed_var(..)).ok()`) and
`db_api_key.rs:80-103` (`resolve_u64`, which wants the resolved *name* for its `warn!`). Contract to
state explicitly, since every caller depends on it: suffixes are passed **without** the
`MICROMEGAS_` prefix (`"API_KEYS"`, `"ADMINS"`, `"OIDC_CONFIG"`, `"DEFAULT_KEY_AUDIENCE"`,
`"REQUIRE_WRITE_AUDIENCE"`), and an empty prefix resolves straight to `MICROMEGAS_{suffix}`.
`read_scope.rs`'s own copy stays where it is — `micromegas-analytics` deliberately does not depend
on `micromegas-auth` (`read_scope.rs:102-107`), the same trade-off §1 makes for the charset check.

### 6. One audience per process, enforced

Scoped to the **native `insert_process` path only** — `process_id` there is client-chosen, so a
conflicting re-registration under a different audience is a real, reachable case. `register_otel_process`
needs no guard: once step 9 folds the audience into `process_id_from_resource` (§4), a given
`process_id` can only ever have been derived under one audience, so a same-`process_id`,
different-audience conflict on that path is unreachable by construction — the query would run on
every OTLP resource in every export request (`write_blocks`, `handler.rs:95-145`, calls
`register_otel_process` per `PreparedBlock` at `:108` — its only caller in the workspace) and could
never fire. Its existing
`ON CONFLICT (process_id) DO NOTHING` plus `debug!` on `rows_affected() == 0` stays as-is.

`insert_process` currently treats a conflicting re-registration as a no-op. Add: when
`rows_affected() == 0` **and** the request carries `Some` audience, `SELECT` the existing row's
audience and compare.

| Existing | Incoming | Outcome |
|---|---|---|
| same `Some(a)` | `Some(a)` | no-op (a retry) — today's behavior |
| `NULL` | `Some(a)` | no-op, `debug!` — a mid-migration re-registration must not lose the process; no retro-stamp |
| `Some(b)` | `Some(a)`, `a != b` | **403**, `warn!` with both audiences and the `process_id` |

This costs one indexed point query on the conflict path in `insert_process` only — a retry or a
genuine `process_id` collision, not the steady state, since native clients are expected to pick
their own `process_id` once — nothing on first insert, and nothing at all on the OTLP path. It is
what makes Stage 2's `MAX(audience)` resolution sound rather than assumed on the one path where
`process_id` is not itself audience-derived, and it is the only thing standing between a
deliberately-reused `process_id` and a mislabeled process there.

### 7. What this stage does not close

**Two different operations, and only one of them is in this stage.** `insert_process` does not
*check* an audience — it **derives** one from the authenticated credential and writes it, which is
exactly what §3 specifies. `insert_stream` and `insert_block` are the opposite shape: because the
audience is recorded only on the process row and streams/blocks inherit it through `process_id`,
they need no audience value at all — no `WriteAudience` parameter belongs on their signatures, and
§1 accordingly threads it to the two process-insert sites only. What they need instead is an
**authorization decision**: the ingestion auth layer must approve this credential writing to *this*
process before the insert proceeds. That decision is what this stage does not yet make.

Today it makes none: `insert_stream` and `insert_block` accept any `process_id`/`stream_id`
unconditionally. A credential bound to audience A that knows a `process_id`/`stream_id` belonging to
audience B can append events to B's process, and those events inherit B's audience — so B's readers
see data B did not produce. It grants the attacker **no read power** (reading B requires a read
grant on B), so it stays inside the plan's "write keys govern integrity only" framing
(`audience_based_access_control_plan.md:97-111`), but it is a real integrity gap and the plan's
phrasing ("pollutes *that audience's* view") understates it.

**Decision: it ships as Stage 5b, a follow-up issue, not inside #1373.** The tree settles this
rather than taste. Landing the gate here would mean designing and building Stage 3's cache layer
inside this stage — the reasons follow — and every prior stage of this epic landed as its own issue.
Deliberately deferred, with a follow-up issue rather than silence:

- The fix is a write-side authorization gate, not an extra parameter: resolve the target's owning
  audience (`process_id → audience`, and `stream_id → process_id` for blocks) and let the auth layer
  decide, keeping the ingestion service's insert signatures audience-free. The lookup rides the same
  immutable, invalidation-free `moka` caches Stage 3 already specifies for Prong B
  (`audience_based_access_control_plan.md:465-478`), so the design work is shared, not duplicated.
- Prong B is verifiably unimplemented today, which is why that cache design cannot simply be reused
  here: `auth/src/policy.rs:8-9` / `read_scope.rs:13-14` record Prong B (the UDTF/UDF guards) as
  still pending (#1371, Stage 3), and `rust/ingestion/Cargo.toml` has no `moka` dependency, though
  it is a workspace dep (`rust/Cargo.toml:66`) already used by `analytics`, `auth` and
  `analytics-web-srv`. Landing the
  authorization gate inside #1373 would mean designing and building Stage 3's cache layer inside
  this stage instead. Stages 1, 2 and 4 each landed as their own issue
  (`d0364c950`, `5dcb74026`, `5298a1ca9`) — the epic's own cadence is the in-tree precedent for
  splitting this off the same way.
- `insert_block` is the hot path; a warm cache hit is an in-memory lookup, but the measurement and
  the cold-miss behavior deserve their own issue rather than riding along here.
- Until it lands, the exposure needs a known-gap doc comment on `insert_stream`/`insert_block_typed`
  in the same style as `ownership_rewrite.rs:59-75`, and a line in
  `mkdocs/docs/admin/authentication.md`.

**The reserved property stays the carrier; the physical column is #1482.** Promoting
`micromegas.audience` to a first-class `audience` column on the six global-instance views
(`blocks`, `processes`, `streams`, `log_entries`, `measures`, `log_stats`) is tracked separately as
#1482, and is deliberately not in this stage. (That issue↔step mapping is this plan's own: #1482 is
the work the AbAC plan describes as its step 15, `audience_based_access_control_plan.md:1257`, which
predates the issue and cites no number.) The dependency runs one
way: #1482 sources that column from the resolved process property, so it needs this stage's
authenticated stamp to exist first, and merging them would bundle a write-path/auth change with a
`SCHEMA_VERSION` bump on every global view (a full partition rebuild) plus a rewrite of
`OwnershipRewrite`'s §3/§4 semi-join branches into a column predicate — two blast radii that could
then no longer be reverted independently. Nothing here becomes throwaway: the property remains the
carrier from the Postgres `processes` row into the lakehouse, and remains what the landed Stage 2
rule reads for unstamped rows. What this stage owes #1482 is the invariant that makes a scalar
column representable at all — §6's one-audience-per-process enforcement, since a process holding two
audiences has no valid column value.

### Config surface

| Knob | Meaning | Default | Open deployment | Privacy deployment |
|---|---|---|---|---|
| `{prefix}_REQUIRE_WRITE_AUDIENCE` (new) → `MICROMEGAS_REQUIRE_WRITE_AUDIENCE` | reject ingestion from a credential carrying no write audience | off | off (env-keyring keys keep working, data stays unstamped and `UNSTAMPED_AUDIENCE`-visible) | `true` |

Consistent with every other stage: inert until an operator configures it. Ship
`{prefix}_REQUIRE_WRITE_AUDIENCE` now, matching the per-stage knob convention every landed AbAC
stage already follows — `{prefix}_UNSTAMPED_AUDIENCE` / `{prefix}_PUBLIC_VIEW_SETS`
(`read_scope.rs:148-192`), `{prefix}_AUDIENCE_GRANTS` / `{prefix}_DEFAULT_KEY_AUDIENCE`
(`auth/src/policy.rs:52-81`) — none of which waited for a consolidated posture flag. Stage 7 (the
AbAC plan's step 14, `audience_based_access_control_plan.md:1231`, which carries no issue number of
its own) is where "the operator must choose a posture" becomes a startup requirement over the knobs
that already exist, this one included; this stage only supplies the switch.

One asymmetry to record rather than fix here: inside the monolith, this knob resolves under
`MICROMEGAS_INGESTION_*` (the ingestion role's prefix), while the sibling mint-side default is
resolved unprefixed — `analytics-web-srv/src/web_server.rs:649` calls
`default_key_audience_from_env("")` even in-process. So one monolith reads
`MICROMEGAS_INGESTION_REQUIRE_WRITE_AUDIENCE` for stamping but only `MICROMEGAS_DEFAULT_KEY_AUDIENCE`
for mint defaults. Pre-existing, out of scope, worth a docs sentence so an operator is not surprised. `{prefix}_UNSTAMPED_AUDIENCE` (analytics side, already shipped) remains the continuity
mechanism for everything written before stamping.

### Flow after this stage

```
POST /ingestion/insert_process            POST /ingestion/otlp/v1/logs        POST .../firehose
  Authorization: Bearer <key>               Authorization: Bearer <key>         X-Amz-...-Access-Key
        |                                          |                                  |
   auth_middleware  ------------------------  auth_middleware              firehose_auth_middleware
   inserts AuthContext                        inserts AuthContext          NOW inserts AuthContext
        |                                          |                                  |
        +---------------- resolve_write_audience(ctx, StampingConfig) ------------------+
                                        |
                     WriteAudience(Some("team-alpha")) | None | 403
                                        |
        +-------------------------------+-----------------------------------+
        |                                                                   |
  insert_process(body, &aud)                            split_*(req, IdentityContext{aud,..})
  strip micromegas.* from client props                  process_id = H(resource-tuple, aud)
  append micromegas.audience = team-alpha               block_id   = H(aud, extra, payload)
  conflict + different audience -> 403                            |
        |                                             register_otel_process(.., &aud)
        |                                                         |
        +--------------------- processes.properties ---------------+
                                        |
                      OwnershipRewrite (Stage 2) filters on it -- now authenticated
```

## Implementation Steps

**Phase 1 — plumbing, no behavior change.**

1. `rust/ingestion/src/write_audience.rs`: `WriteAudience` + charset validation + unit tests;
   export from `lib.rs`.
2. `telemetry/src/property.rs`: `RESERVED_PROPERTY_PREFIX`, `PROPERTY_AUDIENCE`. Point
   `ownership_rewrite.rs:148` at `PROPERTY_AUDIENCE`.
3. `micromegas-auth`: `resolve_prefixed_var` in a new `auth/src/env.rs` (+ `lib.rs` declaration);
   refactor `policy.rs:52-99`, `default_provider.rs:49-83` and `db_api_key.rs:80-103`
   (`resolve_u64`, which wants the resolved *name* for its `warn!`) onto it. Unit-test it in the
   existing `auth/tests/default_provider_tests.rs` — no new test file needed there.
4. `firehose_common.rs:98-108`: insert the resolved `AuthContext` into request extensions.
5. Thread `&WriteAudience` through the signatures — `insert_process`, `register_otel_process`,
   `write_blocks` (`handler.rs:95-145`), `ingest_logs`/`ingest_metrics`/`ingest_traces`/
   `ingest_webhook`/`ingest_firehose_metrics` (all in `handler.rs`) and
   `ingest_cloudwatch_logs_firehose` (in `cloudwatch_logs.rs:214`, **not** `handler.rs`) — passing
   `WriteAudience::none()` everywhere for now. The private `ingest_parsed_metrics`
   (`handler.rs:169`) needs it too: it is the shared `split_metrics` wrapper behind both
   `ingest_metrics` and `ingest_firehose_metrics`. Fix the call sites the compiler names
   (`analytics/tests/*_db_test.rs`, `otel-ingestion/tests/*`, `public/tests/firehose*`, and
   `public/src/servers/{otlp.rs:150,165,180, webhook.rs:139, firehose.rs:53,
   firehose_cloudwatch_logs.rs:47}`).

**Phases 2 and 3 must ship in the same release, never separately.** Phase 2 starts stamping
processes with the authenticated audience; Phase 3 (§4) is what makes OTLP `process_id`/`block_id`
audience-scoped so two audiences sending identical resources/payloads stop colliding. §4 already
calls that collision "not only an attack — it is the ordinary multi-tenant case." A deploy cut
between step 8 and step 9 would stamp OTLP processes whose ids still collapse across audiences,
and because §3 forbids any retro-`UPDATE` of the stamp (blocks snapshot `processes.properties` at
materialization), that mislabeling is permanent and unrepairable. Land steps 6-10 as one deploy.

**Phase 2 — stamp and strip.**

6. `finalize_process_properties` / `strip_reserved_properties` in `web_ingestion_service.rs`, wired
   into both process paths and `insert_stream` (`register_otel_stream` has no client-supplied
   stream properties, so it needs no call — see §3).
7. `servers/write_audience.rs` in `rust/public`: `StampingConfig::from_env(prefix)`,
   `resolve_write_audience`, `WriteAudienceError` with the three per-entry-point response shapes.
8. `serve_ingestion` takes `StampingConfig`, layers it as an extension over the native/OTLP/webhook
   router tree; handlers in `ingestion.rs`, `otlp.rs`, `webhook.rs` gain
   `Option<Extension<AuthContext>>` + `Extension<Arc<StampingConfig>>` and resolve before ingesting,
   with `webhook.rs` rendering a denial through its own `build_error_response` (§5). `firehose.rs`
   and `firehose_cloudwatch_logs.rs` instead gain an explicit `stamping: Arc<StampingConfig>`
   parameter on `firehose_router` that each layers itself, since those routers are built directly
   (with no parent extension) by `serve_ingestion` and by the 11 existing test call sites in
   `public/tests/firehose_tests.rs` / `firehose_cloudwatch_logs_tests.rs` — update those call sites.
   Update `telemetry-ingestion-srv/src/main.rs` and `monolith/src/main.rs` with their prefixes.

**Phase 3 — audience-scoped OTLP identity.**

9. `identity.rs`: `IdentityContext`; `process_id_from_resource(resource, ctx)` (append-only when
   `Some`); audience-prefixed `block_id` input.
10. `block.rs`: `build_prepared_block(.., ctx)`; collapse the `split_logs` pair into
    `split_logs(req, ctx)`; add the parameter to `split_metrics` / `split_traces`. Update the
    production call sites — `handler.rs:157,176,205,299` **and `cloudwatch_logs.rs:225`
    (`split_logs`) / `:229` (`write_blocks`)**, the second production `split_*` caller, easy to miss
    because it lives outside `handler.rs` — then
    `otel-ingestion/tests/{identity,split,webhook,firehose,cloudwatch_*,block,json}_tests.rs`
    (all eight `*_tests.rs` files call one of the changed functions; `block_tests.rs` and
    `json_tests.rs` call `split_logs`/`split_metrics`/`split_traces` directly, and
    `cloudwatch_*_tests.rs`/`webhook_tests.rs` call `process_id_from_resource` directly.
    `fixtures.rs` builds only proto fixtures and needs no change).
    `identity_tests.rs:236-254` (`process_id_is_stable_with_new_fields`, asserting the literal
    `92267645-021b-5d0f-960b-c74719552658`) is the acceptance lock for §4's no-churn guarantee: it
    must keep passing **verbatim** under `IdentityContext::default()`, so update its call to pass the
    default context and leave the expected UUID untouched.

**Phase 4 — one audience per process.**

11. Conflict guard in the native `insert_process` path per §6, with a new
    `IngestionServiceError::AudienceConflict` variant (a caller must branch on it to answer 403 —
    the `rust/CLAUDE.md` bar for a typed error). `IngestionServiceError` has exactly two
    out-of-crate consumers, both exhaustive with no `_` arm, so add the arm to each:
    `From<IngestionServiceError> for IngestionError` (`servers/ingestion.rs:41-49`) →
    `IngestionError::Forbidden`, and `OtelError::from_ingestion` (`otel-ingestion/src/error.rs:111-117`)
    → `OtelError::Denied` (reusing the variant introduced in §5; `from_ingestion` already has the
    `signal` in scope to fill it). The latter is unreachable in practice since
    `register_otel_process` never produces `AudienceConflict` (§6), but the match still needs the arm
    to compile. Nothing else breaks: the remaining out-of-crate references are
    `map_err(|e| anyhow!(..))` in tests, which use `Display`, not a match.

**Phase 5 — docs, changelog, tests.**

12. Known-gap doc comments on `insert_stream` / `insert_block_typed` (§7); file the Stage 5b issue.
13. Docs and `CHANGELOG.md` (see [Documentation](#documentation)).
14. Update `tasks/data_isolation/audience_based_access_control_plan.md`: mark Stage 5 landed,
    correct step 11's stale auth premise, record the identity-scoping decision and the
    write-ownership residual, and update `ownership_rewrite.rs:59-75`'s "neither stage has landed"
    note.

## Files to Modify

- `rust/ingestion/src/write_audience.rs` (new), `rust/ingestion/src/lib.rs`
- `rust/ingestion/src/web_ingestion_service.rs` (stamp/strip helpers, both process paths,
  `insert_stream`, conflict guard scoped to `insert_process`, `AudienceConflict` variant)
- `rust/telemetry/src/property.rs` (constants)
- `rust/auth/src/env.rs` (new, `resolve_prefixed_var`), `rust/auth/src/lib.rs` (module
  declaration), `rust/auth/src/default_provider.rs`, `rust/auth/src/policy.rs`,
  `rust/auth/src/db_api_key.rs` (all three refactored onto it — see §5)
- `rust/public/src/servers/write_audience.rs` (new), `mod.rs`
- `rust/public/src/servers/ingestion.rs` (`IngestionError::Forbidden`), `otlp.rs` (resolve +
  render an `OtelError::Denied`; **no** new `OtlpHttpError` arm — see §5), `webhook.rs`,
  `firehose.rs`, `firehose_cloudwatch_logs.rs`, `firehose_common.rs`
- `rust/otel-ingestion/src/identity.rs`, `block.rs`, `handler.rs`, `cloudwatch_logs.rs`,
  `error.rs` (`OtelError::Denied` variant and its exhaustive-match arm for `AudienceConflict`)
- `rust/analytics/src/lakehouse/ownership_rewrite.rs` (constant + stale-gap note)
- `rust/telemetry-ingestion-srv/src/main.rs`, `rust/monolith/src/main.rs`
- Tests:
  - `rust/ingestion/tests/write_audience_tests.rs` (new — `WriteAudience` + the two `pub` property
    helpers) and `rust/ingestion/tests/audience_stamping_db_test.rs` (new — conflict guard + one
    stamp round-trip). That manifest declares no `[[test]]` entries, so both are autodiscovered.
  - `rust/auth/tests/default_provider_tests.rs` (`resolve_prefixed_var`)
  - `rust/otel-ingestion/tests/` (all eight `*_tests.rs`, incl. the `identity_tests.rs:236-254`
    golden lock)
  - `rust/public/tests/firehose_tests.rs` + `firehose_cloudwatch_logs_tests.rs` (the 11
    `firehose_router` call-site updates, plus the differential Firehose case) and a **new**
    `rust/public/tests/ingestion_stamping_tests.rs` for the native/OTLP denial cases — no existing
    file in that directory exercises those routers. It needs a matching `[[test]]` entry in
    `public/Cargo.toml` with `required-features = ["server"]`: `default = []` there, and all 13
    existing test files are declared explicitly, so an autodiscovered file would compile without
    the `server` feature and fail.
  - `rust/analytics/tests/ownership_rewrite_db_test.rs` (hand-stamping → the new parameter, plus
    the end-to-end acceptance case), and the two other `insert_process` callers the new parameter
    breaks: `thread_spans_ordering_db_test.rs` (9 call sites) and `jit_process_batch_db_test.rs`
    (1 call site) — mechanical `WriteAudience::none()` updates.
- Docs: `mkdocs/docs/admin/ingestion.md`, `authentication.md`, `api-keys.md`, `monolith.md`,
  `flight-sql.md`, `mkdocs/docs/otlp/index.md`, `CHANGELOG.md` (both the new entry **and** the two
  stale sentences at `CHANGELOG.md:8`)
- `tasks/data_isolation/audience_based_access_control_plan.md`

## Trade-offs

- **Strip client `micromegas.*` vs. 400 on it.** Stripping keeps a producer that self-stamped
  pre-Stage-5 alive on upgrade; a 400 would drop all of its telemetry to punish a key that is now
  simply ignored. The `warn!` preserves the signal. What stripping does *not* preserve is the
  self-stamp's effect: a producer authenticating with an audience-less credential (env-keyring,
  OIDC) goes from self-stamped to unstamped — invisible to `ReadScope::Audiences` callers when
  `{prefix}_UNSTAMPED_AUDIENCE` is unset, folded into the unstamped label when it is set — unless it
  is moved onto a DB ingestion key bound to that audience.
- **Knob defaulting to off.** Rejecting audience-less writes by default would break every
  env-keyring (`MICROMEGAS_API_KEYS`) and OIDC producer the moment they upgrade — a data-loss
  default in a stage that is supposed to be inert until configured. The cost is that a privacy
  deployment must remember one more knob, which Stage 7's required-config step converts into a
  startup check.
- **Audience in the identity hash vs. stable ids.** Discussed in §4: id churn for deployments that
  start stamping, versus silent cross-tenant mislabeling and dedup-driven data loss. Churn is
  already sanctioned by `identity.rs`'s own contract.
- **`WriteAudience` newtype vs. `Option<&str>`.** The newtype costs a file and buys the charset
  check at construction plus a name that says which of the several audience-ish values this is
  (`AuthContext.audience` — the OIDC token audience — is right next to `bound_audience`, and the
  plan already calls that collision out at `audience_based_access_control_plan.md:209-215`).
- **Deferring write-side ownership checks** (§7) keeps the hot path untouched in this stage; the
  price is an explicitly documented integrity gap and a second issue.
- **No ingestion-side default/fallback audience.** A configured fallback stamped onto
  otherwise-unstamped writes (an ingestion-side analogue of `MICROMEGAS_DEFAULT_KEY_AUDIENCE`)
  would ease migration but reintroduces exactly the "silent audience nobody chose" failure #1372
  already rejected for `mint`: `auth/src/policy.rs:83-113`'s `default_key_audience_from_env` lets
  `import` fall back but never `mint`, because "an unresolved mint is a 400, never a silent
  `public`" — a silent default there "would publish a new credential's entire future ingestion
  history" (`audience_based_access_control_plan.md:1154-1157`). Stamping a guessed audience onto
  data is the same failure one layer down, permanent once blocks materialize (Current State,
  "The read side that consumes the stamp"). `OwnershipRewriteConfig.unstamped_audience`
  (`read_scope.rs:88-96`) already gives the read side the same continuity without ever writing a
  guess into the data, so this stage adds no ingestion-side fallback.

## Security

- The property `OwnershipRewrite` filters on becomes server-written and unspoofable from the
  payload: the reserved namespace is stripped on the way in and re-written from the authenticated
  credential. This is the fix `ownership_rewrite.rs:59-75` points at.
- A stamped process's audience is immutable: no `UPDATE` path, and a conflicting re-registration is
  a 403, so Stage 2's `MAX(audience)` collapse cannot be gamed by a later, narrower stamp.
- Audience-scoped OTLP identity removes cross-audience process collapse and cross-audience block
  dedup.
- Unchanged on the Firehose path: it already strips all five spoofable headers (`x-auth-subject`,
  `x-auth-email`, `x-auth-issuer`, `x-allow-delegation`, `x-auth-is-admin`) on the success path
  (`firehose_common.rs:99-106`), mirroring `auth/src/axum.rs:75-79`. The only delta this stage makes
  there is that the validated `AuthContext` stops being discarded.
- Residual, tracked, integrity-only: `insert_stream` / `insert_block` cross-audience injection
  (§7). No write→read escalation exists in any of these cases — reading an audience still requires
  a read grant.
- Unchanged: `otel.resource.process.owner` / `host.*` remain display metadata; nothing in the
  authorization path reads them.

## Documentation

- `mkdocs/docs/admin/ingestion.md`: `{prefix}_REQUIRE_WRITE_AUDIENCE` in the env table; a
  "What gets stamped" section (which credentials carry an audience, what unstamped means, the
  reserved `micromegas.*` namespace).
- `mkdocs/docs/admin/authentication.md`: extend the audiences/grants material with the write side —
  stamping is what makes the read filter trustworthy; the OTLP-identity-is-audience-scoped note and
  its id-churn consequence; the §7 residual; and that client self-stamping of `micromegas.audience`
  stops taking effect — a process relying on it falls back to unstamped (unless
  `{prefix}_UNSTAMPED_AUDIENCE` widens it) unless its credential is a DB ingestion key bound to that
  audience.
- `mkdocs/docs/admin/api-keys.md:212`: drop "Stage 5, not yet shipped" (the passage runs `:208-226`).
- `mkdocs/docs/admin/monolith.md:38-55`: prefixed knob row, alongside the existing
  `MICROMEGAS_ANALYTICS_*` AbAC rows.
- `mkdocs/docs/admin/flight-sql.md:33`: the same `MICROMEGAS_UNSTAMPED_AUDIENCE` row as
  `monolith.md:51`, whose "a process with no `micromegas.audience` property" framing describes
  never-stamped legacy data once stamping ships. Keep the two copies in step.
- `mkdocs/docs/otlp/index.md`: this file **restates both derivation formulas literally**, so it is a
  correctness fix, not a note — three sites: `:73-98` (the full 31-field `process_id` field list,
  which gains the audience as a conditional 32nd), `:224`
  (`block_id = uuid_v5(NS_OTEL_BLOCK_V1, payload_bytes)`), and `:414-417` (the webhook header-hash
  input, now one of three `block_id` inputs). Also state that existing ids re-derive once stamping
  starts — `:93-95` already establishes the re-derivation precedent this leans on. `:17` and `:36`
  ("the OTLP routes share the same auth chain as the rest of the ingestion service") are the in-tree
  evidence for step 14's correction of the AbAC plan's stale step-11 premise.
- `CHANGELOG.md:8` (Stage 2's Unreleased entry) carries **two** sentences this stage falsifies, both
  to be amended in the same commit, the same way step 14 handles `ownership_rewrite.rs:59-75`:
  1. its known-limitation close — "Stage 5 (#1373) … has not landed yet, so operators should not
     treat this stage's enforcement as a hard security boundary against a malicious or
     misconfigured instrumented client until it does."
  2. earlier on the same line, inside the `MICROMEGAS_UNSTAMPED_AUDIENCE` upgrade note — "No
     `micromegas.audience` stamping exists yet (ingestion stamping is Stage 5, #1373), so this knob
     is required for every legacy-data deployment until then." This one is load-bearing operator
     guidance: post-stage the knob covers *pre-stamping* data only, not everything.
- `CHANGELOG.md` **Unreleased**: a new `* **Ingestion:**` group (Unreleased already carries two
  separate `**Analytics:**` groups, so append the new group after the existing `**Auth:**` one rather
  than assuming a canonical slot) with an entry in the established AbAC style —
  `(#1373, Stage 5 of the epic tracked at #1334)` — with the
  **Minor breaking change** clause covering every published Rust item this stage moves —
  `WebIngestionService::insert_process` and `register_otel_process` (`&WriteAudience`);
  `serve_ingestion` (a `StampingConfig` parameter); both `firehose_router`s
  (`micromegas::servers::firehose` and `::firehose_cloudwatch_logs`, an
  `Arc<StampingConfig>` parameter); `micromegas_otel_ingestion::identity::process_id_from_resource`
  and `block::{split_logs, split_metrics, split_traces}` (an `IdentityContext`);
  `block::split_logs_with_extra_hash_input` **removed outright**, collapsed into `split_logs` (§4);
  and `handler::{ingest_logs, ingest_metrics, ingest_traces, ingest_webhook,
  ingest_firehose_metrics}` plus `cloudwatch_logs::ingest_cloudwatch_logs_firehose`
  (`&WriteAudience`) — and an upgrade
  note covering both OTLP `process_id` re-derivation and the visibility change for client
  self-stamped audiences: a process that previously self-stamped `micromegas.audience` while
  authenticating with an audience-less credential (env-keyring, OIDC) silently becomes unstamped —
  it must move to a DB ingestion key bound to that audience to keep its own label.

## Testing Strategy

Unit (no DB) — hosts, since `rust/CLAUDE.md` puts unit tests under each crate's `tests/` folder:
a new `rust/ingestion/tests/write_audience_tests.rs` (`WriteAudience` + the two property helpers,
which §3 makes `pub` for exactly this reason; that manifest declares no `[[test]]` entries, so
Cargo autodiscovers it), the existing `rust/otel-ingestion/tests/identity_tests.rs` for §4's
identity cases, the existing `rust/auth/tests/default_provider_tests.rs` for `resolve_prefixed_var`,
and the new `rust/public/tests/ingestion_stamping_tests.rs` below for `resolve_write_audience` /
`StampingConfig::from_env` alongside its HTTP cases.

- `WriteAudience::new` accepts `[A-Za-z0-9_-]{1,255}`, rejects empty / `:` / 256 bytes / non-ASCII.
- `finalize_process_properties`: client `micromegas.audience` dropped and replaced; other
  `micromegas.*` dropped; `otel.resource.*` and arbitrary client keys untouched; `None` audience
  writes no property at all (asserted as *absent*, not empty).
- `strip_reserved_properties` on stream properties.
- `resolve_write_audience`: the full 3×2 table of §5, including "no extension + require ⇒ 403".
- Identity — this is where the whole §4 collision story is asserted, because every input is a pure
  function of its arguments and needs no database to exercise:
  `process_id_from_resource(r, IdentityContext::default())` still equals the golden value already
  locked in `identity_tests.rs:236-254` (the no-churn guarantee); two audiences over the **same**
  resource derive two distinct `process_id`s; same for `block_id`; webhook `extra_hash_input` still
  influences `block_id` with and without an audience; and `stream_id` inherits the split for free
  because it derives from `process_id`.
- `StampingConfig::from_env` prefixed/unprefixed resolution, and `resolve_prefixed_var`'s
  empty-prefix and fallback rules.

HTTP level (`tower::ServiceExt::oneshot`, in-memory object store + lazy pool, per
`public/tests/firehose_tests.rs:1-40`). The lazy pool points at an unreachable database, so every
case below must be a request that either stops at the gate or does zero database work — the same
constraint `firehose_tests.rs:1-7` already records for itself:

- Firehose: `firehose_tests.rs:1-40`'s own `make_auth_provider()` builds an `ApiKeyAuthProvider`
  from an env keyring, and every env-keyring key hard-codes `bound_audience: None`
  (`auth/src/api_key.rs:128`) — only `DbApiKeyAuthProvider` (live Postgres) ever produces `Some(..)`,
  so the discarded-context regression cannot be asserted on that harness as it stands. Add a test
  `impl AuthProvider` (`async-trait` is already a `micromegas-public` dev-dependency) returning an
  `AuthContext` with `bound_audience: Some("team-a")`, following the existing precedent at
  `public/tests/read_policy_threading_tests.rs:247-269`.
  Assert it **differentially**, with `require_write_audience` on and a zero-record body: the
  `Some("team-a")` provider gets a clean ack (no `errorMessage`), while the audience-less
  env-keyring key gets the Firehose ack shape with a 4xx and an `errorMessage`. The passing case is
  precisely what proves the context is no longer dropped — if `firehose_common.rs` still discarded
  it, the audience-carrying key would be rejected too. Asserting the extension *directly* is not
  available here: this harness never touches the DB ("every case … either fails auth before the
  handler or sends zero records", `firehose_tests.rs:1-7`), and a layer added outside
  `firehose_router` cannot observe an extension inserted by middleware inside it.
- OTLP: audience-less credential under `require_write_audience` ⇒ `google.rpc.Status` code 7 in the
  request's own encoding (JSON in → JSON out). The knob-off counterpart uses an **empty
  `resource_logs`** body, which `ingest_logs` (`handler.rs:154-156`) returns `Ok` on before touching
  the database — a 200 there is proof the gate let the request through.
- Native: 403 body shape with the knob on. The knob-off counterpart cannot assert a successful
  insert on this harness (`insert_process` would reach the unreachable pool and 500), so assert it
  **differentially against the parse boundary instead**: a deliberately malformed CBOR body returns
  **400** (`IngestionServiceError::ParseError`, `web_ingestion_service.rs:360-361`) with the knob
  off and **403** with it on — the 400 is reached only after `resolve_write_audience` returns `Ok`,
  which is exactly the pass-through being asserted, and it needs no database.

DB-backed (`#[ignore]` + `MICROMEGAS_SQL_CONNECTION_STRING`/`MICROMEGAS_OBJECT_STORE_URI`, per
`analytics/tests/ownership_rewrite_db_test.rs`):

Scoped deliberately: a live PG/object-store test earns its place only where the assertion is about
*Postgres semantics* — `ON CONFLICT` outcomes, `rows_affected`, what a row actually holds. Everything
that is a pure function of its inputs (`finalize_process_properties`, all of §4's identity
derivation, `resolve_write_audience`) is asserted in the unit tests above and is **not** re-asserted
against a database.

- `rust/ingestion/tests/audience_stamping_db_test.rs` (new; the crate already has this harness —
  `insert_block_dedup_db_test.rs` — and `sqlx`/`tokio`/`object_store` are already deps):
  the **conflict guard**, which is irreducibly about `ON CONFLICT (process_id) DO NOTHING` +
  `rows_affected() == 0` + the follow-up `SELECT`, and cannot be unit-tested. Re-register the same
  `process_id` with the same audience ⇒ ok; with a different audience ⇒ `AudienceConflict`; existing
  `NULL` + incoming `Some` ⇒ ok, row still `NULL` (no retro-stamp). Plus **one** round-trip case
  proving the stamp survives the `sqlx` bind into `processes.properties` and reads back — that is
  the only thing the DB adds over the `finalize_process_properties` unit tests, so it is one case,
  not four.
- **No new `rust/public` *DB* test.** An earlier draft put the OTLP identity/collision regression there
  ("two audiences posting identical resources produce two distinct `process_id`s and both blocks
  persist"). Both halves are pure functions already covered by the identity unit tests above, and the
  persistence half adds nothing on top: `insert_block_typed`'s create-only behavior under
  `ON CONFLICT (block_id) DO NOTHING` is already locked against live PG by
  `rust/ingestion/tests/insert_block_dedup_db_test.rs`, which is indifferent to how a `block_id` was
  derived (it builds ids with `Uuid::new_v4()`). Distinct ids ⇒ distinct rows needs no second
  DB-backed proof, so nothing in `rust/public` needs a live Postgres. The one new file there is the
  DB-less `ingestion_stamping_tests.rs` above.
- End-to-end acceptance, reusing `ownership_rewrite_db_test`'s materialize-then-query harness:
  ingest through the real path under audience A, materialize, then assert a `ReadScope` granting
  only B sees nothing, only A sees the rows, and `ReadScope::All` sees everything — i.e. Stage 5's
  stamp actually drives Stage 2's filter, with no hand-stamped property anywhere in the test.
  That file's own hand-stamping (`ownership_rewrite_db_test.rs:144-161`) switches to the new
  parameter, and its "no stamping exists yet" preamble is corrected.

Manual: `local_test_env/ai_scripts/start_services.py` launches ingestion with `--disable-auth`
(split mode, `:174`) and the monolith with `--disable-ingestion-auth`/`--disable-auth` (`:290`) —
no auth provider runs, so no `AuthContext` extension exists at all. (`MICROMEGAS_API_KEYS` at
`:135` only configures the object-cache server, not ingestion.) So the default local flow exercises
the no-extension branch of `resolve_write_audience` — it must keep working unstamped. Exercise
stamping locally by importing a DB ingestion key with an audience through `analytics-web-srv`'s
import route and pointing a producer at it.

## Open Questions

None. The one question this plan carried — whether the cross-audience `insert_stream`/`insert_block`
ownership check belongs inside #1373 or in a follow-up — is settled by the tree and recorded as a
decision in [§7](#7-what-this-stage-does-not-close): it ships as Stage 5b, because the gate depends
on Stage 3's Prong B cache layer, which is verifiably unimplemented (`policy.rs:8-9`,
`read_scope.rs:13-14`, and no `moka` dependency in `rust/ingestion/Cargo.toml`), and because stages
1, 2 and 4 each landed as their own issue (`d0364c950`, `5dcb74026`, `5298a1ca9`).
