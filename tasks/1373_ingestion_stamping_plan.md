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
  `otel.resource.<key>` (`block.rs:448-456`), so an OTLP client cannot reach the reserved
  namespace, but nothing server-side writes into it either.

Both use `ON CONFLICT (process_id) DO NOTHING`: a re-registration of an existing `process_id` is a
silent no-op, whatever it claims. `insert_stream` (`:265-308`) binds stream properties verbatim
too; `insert_block_typed` (`:146-241`) inserts with `ON CONFLICT (block_id) DO NOTHING`
(`:186`) and writes the payload to `blobs/{process_id}/{stream_id}/{block_id}`.

`analytics/src/replication.rs:120-145` copies `processes` rows (properties included) between
lakes, so a replicated process keeps the audience it was stamped with at its origin — the correct
behavior, and no change is needed there.

### OTLP identity is a pure function of client bytes

- `process_id_from_resource` (`identity.rs:188-231`) hashes a 31-field tuple of client-supplied
  resource attributes under `NS_OTEL_PROCESS_V1`.
- `stream_id_from_process_signal` (`identity.rs:234-237`) derives from `(process_id, signal)`, so
  it inherits whatever scoping `process_id` has.
- `block_id_from_payload` (`identity.rs:241-243`) hashes **only** the encoded resource submessage.
  `split_logs_with_extra_hash_input` (`block.rs:285-329`) already folds extra bytes in for the
  webhook path; `split_metrics` (`:332-355`) and `split_traces` (`:357-379`) do not have the
  hook.
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

A newtype in `micromegas-ingestion` (`rust/ingestion/src/write_audience.rs`), so both
`micromegas-otel-ingestion` and `micromegas-public` can name it without either depending on
`micromegas-auth`:

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
trade-off `read_scope.rs:104-113` already made and documented for
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
write side and the read side cannot drift. No `micromegas.*` property exists in the tree today, so
reserving the whole prefix costs nothing and pre-empts the next reserved key needing its own
migration.

### 3. Stamping and stripping, in one place per insert path

In `web_ingestion_service.rs`, one private helper used by both process paths:

```rust
/// Drops every client-supplied reserved-namespace property, then appends the server-written
/// audience. Client input can neither assert nor suppress the stamp.
fn finalize_process_properties(client: Vec<Property>, audience: &WriteAudience) -> Vec<Property>;
```

- `insert_process(body, audience)` → `finalize_process_properties(make_properties(&info.properties), audience)`.
- `register_otel_process(..., properties, audience)` → same call on the `otel.resource.*` list.
- `insert_stream(body)` and `register_otel_stream` strip the reserved prefix as well (a second
  tiny helper, `strip_reserved_properties`). Nothing reads a stream audience today; stripping keeps
  the namespace honest so a later stage that does read one is not reading client input.
- A dropped reserved key logs at `warn!` once per process registration, naming the key — a native
  client setting `micromegas.audience` was either doing the pre-Stage-5 thing or probing, and both
  are worth seeing. Stripping (rather than rejecting the request with 400) keeps a legacy producer
  that self-stamped from losing all telemetry on upgrade.
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
- `block_id`: prepend `"aud\x1F{audience}\x1F"` to the hash input when `Some`, ahead of the
  existing `extra_hash_input` bytes, using `identity.rs`'s own `SEPARATOR` convention. Necessary
  and not redundant with `process_id`: `blocks` conflicts on `block_id` alone
  (`web_ingestion_service.rs:186`), so without this, two audiences with byte-identical payloads
  silently dedup into one row belonging to one of them.
- `stream_id` needs no change — it derives from `process_id`.
- Collapse `split_logs` / `split_logs_with_extra_hash_input` into a single
  `split_logs(req, ctx)`; give `split_metrics(req, ctx)` / `split_traces(req, ctx)` the same
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

Every handler gains `ctx: Option<Extension<AuthContext>>` (absent ⇔ no auth provider, since both
middlewares always insert) and the `Extension<Arc<StampingConfig>>` layered by `serve_ingestion`.
`serve_ingestion` takes the `StampingConfig` as a parameter; the two binaries build it with their
own prefix — `""` for `telemetry-ingestion-srv` (`main.rs:59-63`), `"MICROMEGAS_INGESTION"` for the
monolith (`main.rs:206`) — matching how they already scope `ProviderBuilder`.

**The Firehose fix** is `firehose_common.rs:98-108`: `Ok(ctx) => { …strip spoofable headers…;
req.extensions_mut().insert(ctx); next.run(req).await }`, mirroring `auth/src/axum.rs:73-83`. Both
Firehose routers then behave exactly like the Bearer routes.

Rejection shape per entry point, so a rejected write is retried-or-not correctly: 403 + plain body
for native routes; `google.rpc.Status` (code 7, `PERMISSION_DENIED`) for OTLP via the existing
`OtlpHttpError` mapping; the Firehose ack shape with `errorMessage` for the Firehose routes (a 4xx
is non-retryable for the client, which is right — retrying a credential without an audience cannot
succeed).

**Prefixed-var resolution, DRY.** `auth/src/policy.rs:52-99` and `ProviderBuilder`
(`default_provider.rs:49-83`) each hand-roll `{prefix}_X`-with-fallback-to-`MICROMEGAS_X`. Extract
one `pub fn resolve_prefixed_var(prefix: &str, suffix: &str) -> String` in `micromegas-auth`, use
it for the new knob, and refactor those existing copies onto it.

### 6. One audience per process, enforced

`insert_process` / `register_otel_process` currently treat a conflicting re-registration as a
no-op. Add: when `rows_affected() == 0` **and** the request carries `Some` audience, `SELECT` the
existing row's audience and compare.

| Existing | Incoming | Outcome |
|---|---|---|
| same `Some(a)` | `Some(a)` | no-op (a retry) — today's behavior |
| `NULL` | `Some(a)` | no-op, `debug!` — a mid-migration re-registration must not lose the process; no retro-stamp |
| `Some(b)` | `Some(a)`, `a != b` | **403**, `warn!` with both audiences and the `process_id` |

This costs one indexed point query only on the conflict path (a retry or an id collision), nothing
on first insert. It is what makes Stage 2's `MAX(audience)` resolution sound rather than assumed,
and on the native path (client-chosen `process_id`) it is the only thing standing between a
deliberately-reused `process_id` and a mislabeled process.

### 7. What this stage does not close

`insert_stream` and `insert_block` accept any `process_id`/`stream_id` without checking that the
target process belongs to the caller's audience. A credential bound to audience A that knows a
`process_id`/`stream_id` belonging to audience B can append events to B's process, and those
events inherit B's audience — so B's readers see data B did not produce. It grants the attacker
**no read power** (reading B requires a read grant on B), so it stays inside the plan's
"write keys govern integrity only" framing (`audience_based_access_control_plan.md:97-111`), but it
is a real integrity gap and the plan's phrasing ("pollutes *that audience's* view") understates it.

Deliberately deferred, with a follow-up issue (Stage 5b) rather than silence:

- The fix is a write-side ownership check — `process_id → audience` (and `stream_id → process_id`)
  through the same immutable, invalidation-free `moka` caches Stage 3 already specifies for Prong B
  (`audience_based_access_control_plan.md:465-478`), so the design work is shared, not duplicated.
- `insert_block` is the hot path; a warm cache hit is an in-memory lookup, but the measurement and
  the cold-miss behavior deserve their own issue rather than riding along here.
- Until it lands, the exposure needs a known-gap doc comment on `insert_stream`/`insert_block_typed`
  in the same style as `ownership_rewrite.rs:59-75`, and a line in
  `mkdocs/docs/admin/authentication.md`.

### Config surface

| Knob | Meaning | Default | Open deployment | Privacy deployment |
|---|---|---|---|---|
| `{prefix}_REQUIRE_WRITE_AUDIENCE` (new) → `MICROMEGAS_REQUIRE_WRITE_AUDIENCE` | reject ingestion from a credential carrying no write audience | off | off (env-keyring keys keep working, data stays unstamped and `UNSTAMPED_AUDIENCE`-visible) | `true` |

Consistent with every other stage: inert until an operator configures it. Stage 7 (#1374's sibling,
step 14) is where "the operator must choose a posture" becomes a startup requirement; this stage
only supplies the switch. `{prefix}_UNSTAMPED_AUDIENCE` (analytics side, already shipped) remains
the continuity mechanism for everything written before stamping.

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
3. `micromegas-auth`: `resolve_prefixed_var`; refactor `policy.rs:52-99` and
   `default_provider.rs:49-83` onto it.
4. `firehose_common.rs:98-108`: insert the resolved `AuthContext` into request extensions.
5. Thread `&WriteAudience` through the signatures — `insert_process`, `register_otel_process`,
   `write_blocks`, `ingest_logs`/`ingest_metrics`/`ingest_traces`/`ingest_webhook`/
   `ingest_firehose_metrics`/`ingest_cloudwatch_logs_firehose` — passing `WriteAudience::none()`
   everywhere for now. Fix the call sites the compiler names (`analytics/tests/*_db_test.rs`,
   `otel-ingestion/tests/*`, `public/tests/firehose*`).

**Phase 2 — stamp and strip.**

6. `finalize_process_properties` / `strip_reserved_properties` in `web_ingestion_service.rs`, wired
   into both process paths and both stream paths.
7. `servers/write_audience.rs` in `rust/public`: `StampingConfig::from_env(prefix)`,
   `resolve_write_audience`, `WriteAudienceError` with the three per-entry-point response shapes.
8. `serve_ingestion` takes `StampingConfig`, layers it as an extension; handlers in `ingestion.rs`,
   `otlp.rs`, `webhook.rs`, `firehose.rs`, `firehose_cloudwatch_logs.rs` gain
   `Option<Extension<AuthContext>>` + `Extension<Arc<StampingConfig>>` and resolve before
   ingesting. Update `telemetry-ingestion-srv/src/main.rs` and `monolith/src/main.rs` with their
   prefixes.

**Phase 3 — audience-scoped OTLP identity.**

9. `identity.rs`: `IdentityContext`; `process_id_from_resource(resource, ctx)` (append-only when
   `Some`); audience-prefixed `block_id` input.
10. `block.rs`: `build_prepared_block(.., ctx)`; collapse the `split_logs` pair into
    `split_logs(req, ctx)`; add the parameter to `split_metrics` / `split_traces`. Update
    `handler.rs` call sites and `otel-ingestion/tests/{identity,split,webhook,firehose,cloudwatch_*}_tests.rs`.

**Phase 4 — one audience per process.**

11. Conflict guard in both process-insert paths per §6, with a new
    `IngestionServiceError::AudienceConflict` variant (a caller must branch on it to answer 403 —
    the `rust/CLAUDE.md` bar for a typed error) mapped to 403 in each entry point's error mapping.

**Phase 5 — docs, changelog, tests.**

12. Known-gap doc comments on `insert_stream` / `insert_block_typed` (§7); file the Stage 5b issue.
13. Docs and `CHANGELOG.md` (see [Documentation](#documentation)).
14. Update `tasks/data_isolation/audience_based_access_control_plan.md`: mark Stage 5 landed,
    correct step 11's stale auth premise, record the identity-scoping decision and the
    write-ownership residual, and update `ownership_rewrite.rs:59-75`'s "neither stage has landed"
    note.

## Files to Modify

- `rust/ingestion/src/write_audience.rs` (new), `rust/ingestion/src/lib.rs`
- `rust/ingestion/src/web_ingestion_service.rs` (stamp/strip helpers, both process paths, both
  stream paths, conflict guard, `AudienceConflict` variant)
- `rust/telemetry/src/property.rs` (constants)
- `rust/auth/src/default_provider.rs`, `rust/auth/src/policy.rs` (`resolve_prefixed_var`)
- `rust/public/src/servers/write_audience.rs` (new), `mod.rs`
- `rust/public/src/servers/ingestion.rs`, `otlp.rs`, `webhook.rs`, `firehose.rs`,
  `firehose_cloudwatch_logs.rs`, `firehose_common.rs`
- `rust/otel-ingestion/src/identity.rs`, `block.rs`, `handler.rs`, `cloudwatch_logs.rs`
- `rust/analytics/src/lakehouse/ownership_rewrite.rs` (constant + stale-gap note)
- `rust/telemetry-ingestion-srv/src/main.rs`, `rust/monolith/src/main.rs`
- Tests: `rust/ingestion/tests/`, `rust/otel-ingestion/tests/`, `rust/public/tests/`,
  `rust/analytics/tests/ownership_rewrite_db_test.rs`
- Docs: `mkdocs/docs/admin/ingestion.md`, `authentication.md`, `api-keys.md`, `monolith.md`,
  `mkdocs/docs/otlp/index.md`, `CHANGELOG.md`
- `tasks/data_isolation/audience_based_access_control_plan.md`

## Trade-offs

- **Strip client `micromegas.*` vs. 400 on it.** Stripping keeps a producer that self-stamped
  pre-Stage-5 alive on upgrade; a 400 would drop all of its telemetry to punish a key that is now
  simply ignored. The `warn!` preserves the signal.
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

## Security

- The property `OwnershipRewrite` filters on becomes server-written and unspoofable from the
  payload: the reserved namespace is stripped on the way in and re-written from the authenticated
  credential. This is the fix `ownership_rewrite.rs:59-75` points at.
- A stamped process's audience is immutable: no `UPDATE` path, and a conflicting re-registration is
  a 403, so Stage 2's `MAX(audience)` collapse cannot be gamed by a later, narrower stamp.
- Audience-scoped OTLP identity removes cross-audience process collapse and cross-audience block
  dedup.
- Both spoofable-header strips (`x-auth-*`) now happen on the Firehose path too, alongside the
  extension insert.
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
  its id-churn consequence; the §7 residual.
- `mkdocs/docs/admin/api-keys.md:208-226`: drop "Stage 5, not yet shipped".
- `mkdocs/docs/admin/monolith.md`: prefixed knob row.
- `mkdocs/docs/otlp/index.md`: process ids are audience-scoped when the credential carries an
  audience; existing ids re-derive once stamping starts.
- `CHANGELOG.md` **Unreleased**: an `Ingestion:` entry in the established AbAC style, with the
  **Minor breaking change** clause for the Rust signature changes (`insert_process`,
  `register_otel_process`, `split_*`, `process_id_from_resource`, `serve_ingestion`) and an upgrade
  note covering OTLP `process_id` re-derivation.

## Testing Strategy

Unit (no DB):

- `WriteAudience::new` accepts `[A-Za-z0-9_-]{1,255}`, rejects empty / `:` / 256 bytes / non-ASCII.
- `finalize_process_properties`: client `micromegas.audience` dropped and replaced; other
  `micromegas.*` dropped; `otel.resource.*` and arbitrary client keys untouched; `None` audience
  writes no property at all (asserted as *absent*, not empty).
- `strip_reserved_properties` on stream properties.
- `resolve_write_audience`: the full 3×2 table of §5, including "no extension + require ⇒ 403".
- Identity: `process_id_from_resource(r, ctx{audience: None})` equals a golden pre-change value
  (the no-churn guarantee); two audiences over the same resource differ; same for `block_id`;
  webhook `extra_hash_input` still influences `block_id` with and without an audience.
- `StampingConfig::from_env` prefixed/unprefixed resolution.

HTTP level (`tower::ServiceExt::oneshot`, in-memory object store + lazy pool, per
`public/tests/firehose_tests.rs:1-40`):

- Firehose: an authenticated request reaches the handler with an `AuthContext` extension carrying
  the expected `bound_audience` (the regression test for the discarded-context bug); an
  audience-less key under `require_write_audience` gets the Firehose ack shape with a 4xx and an
  `errorMessage`.
- OTLP: audience-less credential under `require_write_audience` ⇒ `google.rpc.Status` code 7 in the
  request's own encoding (JSON in → JSON out).
- Native: 403 body shape; unstamped-and-allowed passes through when the knob is off.

DB-backed (`#[ignore]` + `MICROMEGAS_SQL_CONNECTION_STRING`/`MICROMEGAS_OBJECT_STORE_URI`, per
`analytics/tests/ownership_rewrite_db_test.rs`):

- `rust/ingestion/tests/audience_stamping_db_test.rs`: native + OTLP insert with `Some("team-a")`
  lands `micromegas.audience = team-a` on the row; a client-supplied `micromegas.audience = team-b`
  in the same request does not survive; `None` leaves the property absent.
- Conflict guard: re-register the same `process_id` with the same audience ⇒ ok; with a different
  audience ⇒ `AudienceConflict`; existing `NULL` + incoming `Some` ⇒ ok, still `NULL`.
- Two audiences posting **identical** OTLP resources produce two distinct `process_id`s and both
  blocks persist (the collision/dedup regression test).
- End-to-end acceptance, reusing `ownership_rewrite_db_test`'s materialize-then-query harness:
  ingest through the real path under audience A, materialize, then assert a `ReadScope` granting
  only B sees nothing, only A sees the rows, and `ReadScope::All` sees everything — i.e. Stage 5's
  stamp actually drives Stage 2's filter, with no hand-stamped property anywhere in the test.
  That file's own hand-stamping (`ownership_rewrite_db_test.rs:144-161`) switches to the new
  parameter, and its "no stamping exists yet" preamble is corrected.

Manual: `local_test_env/ai_scripts/start_services.py` uses `MICROMEGAS_API_KEYS`
(`start_services.py:135`), i.e. an audience-less key — so the default local flow must keep working
unstamped. Exercise stamping locally by importing a DB ingestion key with an audience through
`analytics-web-srv`'s import route and pointing a producer at it.

## Open Questions

1. **Is the Stage 5b split acceptable** (stamping now; cross-audience `insert_stream`/
   `insert_block` ownership checks in a follow-up issue with the Prong B caches), or should the
   write-side ownership check land inside #1373? The recommendation is to split: it keeps the hot
   path untouched here, and the cache design is shared with Stage 3.
2. **Knob name.** `{prefix}_REQUIRE_WRITE_AUDIENCE` reads as the fail-closed switch it is; an
   alternative is folding it into a future single `{prefix}_ISOLATION_REQUIRED` posture flag at
   Stage 7 and shipping no knob now (privacy deployments would then have no way to fail closed
   until Stage 7). Recommendation: ship the knob.
3. **Should `MICROMEGAS_DEFAULT_KEY_AUDIENCE`'s ingestion-side analogue exist** — i.e. a configured
   fallback audience stamped when the credential carries none? It would ease migration but
   re-introduces exactly the "silent audience nobody chose" failure #1372 rejected for `mint`
   (`audience_based_access_control_plan.md:1154-1157`). Recommendation: no; `UNSTAMPED_AUDIENCE` on
   the read side already covers the continuity case without writing a guess into the data.
