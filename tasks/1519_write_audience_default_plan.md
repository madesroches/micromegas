# Resolve the Deployment Default Audience at the Ingestion Write Path Plan

Issue: [#1519](https://github.com/madesroches/micromegas/issues/1519)

## Overview

An ingestion credential that carries no bound audience should behave exactly as if it were bound
to the deployment's default audience (`MICROMEGAS_DEFAULT_AUDIENCE`, `public` when unset). Three
of the four surfaces that touch an audience already resolve it that way — key mint/import, the
three Postgres read sites, and the query-side read scope. The ingest-time write path does not: it
keeps "unaudienced" as a distinct third state, and as a direct consequence
`check_process_audience_conflict` has two fail-open arms that skip the registration conflict guard
(#1373 §6) instead of comparing.

This plan collapses that third state. `WriteAudience` becomes a single-state type holding a real
audience label, resolved once at startup from the environment and applied at the HTTP edge by
`resolve_write_audience`. Both fail-open arms in the guard then disappear: the early return has no
`None` case left, and the existing-`NULL`-row arm becomes a real comparison against the resolved
default — which is what closes the process-squatting hole
(`mkdocs/docs/admin/authentication.md:279-285` presents it as closed by #1373; today it is closed
only when the squatted row is already *stamped*). A resolved-to-default caller stamps
`micromegas.audience` explicitly, so the write path and the read path agree by construction rather
than by two independent conventions.

Severity is fail-open asymmetry / defense-in-depth, not a demonstrated live confidentiality break
(see the issue's *Severity* section). The reason to do it now is that #1518's proposed write-side
gate must compare resolved-to-resolved to be correct, and the three-state model will keep
generating this class of bug until it is gone.

## Current State

### The third state

- `rust/ingestion/src/write_audience.rs` — `WriteAudience(Option<Arc<str>>)`.
  `WriteAudience::new(Option<&str>)` validates the charset; `WriteAudience::none()` (`:47-52`) is
  the third state; `as_str() -> Option<&str>`.
- `rust/public/src/servers/write_audience.rs:16-27` — `resolve_write_audience(ctx)` reads
  `AuthContext.bound_audience`, and produces `none()` both when the credential carries no audience
  and when a carried audience fails `WriteAudience::new` validation (warn-and-degrade, since
  #1507 removed the `REQUIRE_WRITE_AUDIENCE` gate that used to 403 there).
- `rust/analytics/src/audience.rs:8-12` states the intended contract: *"The default is applied
  where the audience is read, not where the process is written."* That is coherent for the
  **stamp**; it is not coherent for an **authorization comparison**.

### Who produces an unaudienced credential

Not an edge case — two of the three `AuthContext` producers hardcode `bound_audience: None`:

- `rust/auth/src/api_key.rs:128` — the env API keyring (`MICROMEGAS_API_KEYS`). Removing it is
  #1502, still open.
- `rust/auth/src/oidc.rs:553-555` — OIDC tokens never carry a write-side `bound_audience`.
- `rust/auth/src/db_api_key.rs:358` — DB-backed keys *do* carry one, and mint resolves it to the
  deployment default (`rust/analytics-web-srv/src/ingestion_keys.rs:248-262`), so these are fine.

Plus `ctx: None` (no auth provider, e.g. `--disable-auth`), and any `bound_audience` that fails
`WriteAudience::new`.

### The two fail-open arms

`rust/ingestion/src/web_ingestion_service.rs:566-641`:

1. **`:571-574`** — early return before the `SELECT`:
   ```rust
   let Some(incoming) = audience.as_str() else {
       debug!("duplicate process_id={process_id} skipped (already exists)");
       return Ok(());
   };
   ```
   An unaudienced credential re-registering a process stamped `B` gets no `AudienceConflict`.
2. **`:625-631`**, the `None` arm — returns `Ok` ("already exists, unstamped -- no retro-stamp").
   But every reader resolves that unstamped row to the deployment default, so a producer claiming
   audience `B` may attach to a process all readers see as `public`; `B`'s subsequent
   streams/blocks land there, readable by anyone with a `public` read grant.
3. **`:643-648`** — `remember_process_audience` no-ops on `none()`, so `process_audience_cache`
   cannot memoize unaudienced callers at all (minor, same root cause).

`rust/ingestion/tests/audience_stamping_db_test.rs:113`
(`existing_null_audience_reregistration_is_ok_and_stays_unstamped`) encodes arm 2's behavior.

### Where the value is stamped, and where it feeds identity

- `finalize_process_properties` (`web_ingestion_service.rs:116-128`) strips every client-supplied
  `micromegas.*` property and appends `micromegas.audience` **only when** `audience.as_str()` is
  `Some`.
- `IdentityContext.audience: Option<&'a str>` (`rust/otel-ingestion/src/identity.rs:52-59`) is
  fed from `audience.as_str()` at five sites — `handler.rs:161,185,220,318` and
  `cloudwatch_logs.rs:223`. `Some` domain-separates the OTLP `process_id` hash under a
  per-audience namespace UUID and prefixes `block_id`'s hash input; `None` reproduces
  pre-Stage-5 ids byte for byte (`identity.rs:220-268`, `block.rs:202-230`).

### Where the default is read today

- `micromegas_auth::policy::default_audience_from_env(prefix)` (`rust/auth/src/policy.rs:73-97`) —
  the key-minting role. Accepts a `{prefix}_DEFAULT_AUDIENCE` override falling back to the
  unprefixed name (`rust/auth/src/env.rs:16-27`).
- `micromegas_analytics::audience::default_audience_from_env()` (`analytics/src/audience.rs:83`) —
  unprefixed only; read once by `LakehouseContext::new`
  (`analytics/src/lakehouse/lakehouse_context.rs:98`) and handed to the three read sites from
  there.

Both trim-then-validate identically and default to `public`, so one env value can never be
accepted by one role and rejected by another. The ingestion role reads neither today.

### Service construction

`WebIngestionService::new(lake)` (`web_ingestion_service.rs:165`) — 26 call sites, one of which is
production (`rust/public/src/servers/ingestion.rs:147`, inside `serve_ingestion`); the rest are
integration tests. `serve_ingestion`'s two callers (`rust/monolith/src/main.rs:315`,
`rust/telemetry-ingestion-srv/src/main.rs:75`) pass no audience configuration.

All six write handlers that call `resolve_write_audience` already have
`Extension<Arc<WebIngestionService>>` in scope: `ingestion.rs:70`, `otlp.rs:153,170,187`,
`firehose.rs:46`, `firehose_cloudwatch_logs.rs:39`, `webhook.rs:126`.

## Design

### 1. `WriteAudience` becomes single-state

```rust
/// The authenticated write audience a request ingests under. Always a real audience: a
/// credential that carries none resolves to the deployment default at the HTTP edge
/// (`micromegas::servers::write_audience::resolve_write_audience`), the same resolution every
/// other surface that touches an audience already performs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteAudience(Arc<str>);

impl WriteAudience {
    /// Rejects a malformed label -- `ingestion_api_keys.audience` is already `CHECK`-constrained,
    /// so this is defence in depth against a future producer of `bound_audience` that doesn't go
    /// through that column. Also the validating constructor for the deployment default itself,
    /// called once at startup.
    pub fn new(audience: &str) -> anyhow::Result<Self>;

    /// The audience label. Never absent.
    pub fn as_str(&self) -> &str;
}
```

`WriteAudience::none()` is removed. `new` takes `&str` rather than `Option<&str>`, so the compiler
enumerates every call site (`rust/CLAUDE.md`'s Rust-API stance: a silently defaulted value is the
more expensive failure). The local `is_valid_audience` copy and its "keep the three copies in
step" comment stay as they are.

### 2. Resolve once at startup, in `serve_ingestion`

```rust
// rust/public/src/servers/ingestion.rs, in serve_ingestion
let default_audience = WriteAudience::new(&default_audience_from_env("")?)
    .with_context(|| "MICROMEGAS_DEFAULT_AUDIENCE")?;
let service = Arc::new(WebIngestionService::new(lake, default_audience));
```

Using `micromegas_auth::policy::default_audience_from_env("")`: `rust/public` is the crate that
sees both `micromegas-auth` and `micromegas-ingestion` (`micromegas_auth::types` is already
imported in this file), so `micromegas-ingestion` acquires no new dependency. The **empty prefix
is deliberate** — `resolve_prefixed_var("", ...)` yields `MICROMEGAS_DEFAULT_AUDIENCE` and nothing
else, so the ingestion edge reads exactly the same unprefixed name the lakehouse roles read. There
is no `{prefix}_DEFAULT_AUDIENCE` override on this path.

`serve_ingestion` already returns `anyhow::Result<()>`, so a malformed knob fails startup rather
than surfacing per-request — matching `LakehouseContext::new`'s handling of the same variable.
Neither of `serve_ingestion`'s callers changes. In the monolith the variable is read twice (once
here, once by `LakehouseContext::new`), which is consistent by construction since both read the
same unprefixed name in the same process; the only visible effect is a duplicated `info!` line.

### 3. The service carries the resolved default

`WebIngestionService` gains a `default_audience: WriteAudience` field, set by
`WebIngestionService::new(lake, default_audience)`, exposed as
`pub fn default_audience(&self) -> &WriteAudience`. The guard needs it to resolve an existing
**unstamped** row — the incoming side is resolved at the edge, and a comparison is only sound when
both sides are resolved the same way. Storing an already-validated `WriteAudience` (rather than a
`String`) keeps per-request validation out of the picture entirely and gives
`resolve_write_audience` a value it can clone.

This mirrors how `LakehouseContext` holds the resolved default and hands it to `BlocksView` /
`AudienceIndex` / `metadata::find_process` rather than each re-reading the environment.

### 4. `resolve_write_audience` takes the default

```rust
pub fn resolve_write_audience(
    ctx: Option<&Extension<AuthContext>>,
    default_audience: &WriteAudience,
) -> WriteAudience {
    let Some(bound) = ctx.and_then(|Extension(c)| c.bound_audience.as_deref()) else {
        return default_audience.clone();
    };
    match WriteAudience::new(bound) {
        Ok(w) => w,
        Err(e) => {
            warn!("bound_audience failed WriteAudience validation, using the deployment default: {e:#}");
            default_audience.clone()
        }
    }
}
```

Each of the six handlers becomes
`resolve_write_audience(ctx.as_ref(), service.default_audience())`, evaluated before `service` is
moved into the downstream call.

Note the malformed-`bound_audience` arm is **not** a new fail-open: today it degrades to
`none()`, which every reader already resolves to the deployment default. Degrading to the default
explicitly produces the identical audience resolution, now written down instead of implied. (It
stays unreachable in practice for a DB-backed key, whose column is `CHECK`-constrained.)

### 5. Both fail-open arms disappear

```rust
async fn check_process_audience_conflict(
    &self,
    process_id: Uuid,
    audience: &WriteAudience,
) -> Result<(), IngestionServiceError> {
    let incoming = audience.as_str();
    if let Some(cached) = self.process_audience_cache.get(&process_id)
        && cached.as_str() == incoming
    { /* ... unchanged ... */ return Ok(()); }

    // ... unchanged SELECT + concurrently-deleted-row early return ...

    // An existing row with no `micromegas.audience` property is legacy data written before the
    // write path resolved the default. Resolve it the same way every reader does, rather than
    // treating "unstamped" as a state that matches anything.
    let existing = properties
        .iter()
        .find(|p| p.key_str() == PROPERTY_AUDIENCE)
        .map(|p| p.value_str())
        .unwrap_or_else(|| self.default_audience.as_str());

    if existing != incoming {
        warn!(...);
        return Err(IngestionServiceError::AudienceConflict {
            process_id,
            existing: existing.to_string(),
            incoming: incoming.to_string(),
        });
    }
    self.remember_process_audience(process_id, audience);
    Ok(())
}
```

The three-way `match` collapses to one comparison. `remember_process_audience` drops its
`is_some()` guard and becomes an unconditional `insert` (arm 3 of the issue's consequences).

Caching the resolved value for a legacy unstamped row is correct and deliberate — that *is* the
audience the row resolves to, the deployment default cannot change within a process lifetime, and
there is no `UPDATE processes` path anywhere in the codebase that could retro-stamp the row out
from under the entry. The old comment about "recording a value the row never held" no longer
applies once the resolved value is the only value enforcement ever uses.

**No retro-stamp, still.** A matching re-registration of an unstamped row returns `Ok` and leaves
the row unstamped — only the *comparison* changes. The `AudienceConflict` error reports the
resolved value as `existing`, which is what enforcement acts on.

### 6. A resolved-to-default caller stamps explicitly

This is the issue's §3 decision. With `WriteAudience` single-state, `finalize_process_properties`
appends `micromegas.audience` unconditionally, so every newly registered process carries a real
audience property in Postgres. Consequences, all accepted:

- **`micromegas.audience` becomes genuinely non-null in Postgres for new rows.** The read-side
  `COALESCE` in `coalesced_audience_subselect` becomes a legacy-data concern only, not a live
  convention the write path depends on. `audience.rs:8-12`'s contract must be rewritten to say so.
- **OTLP/webhook/Firehose `process_id` (and therefore `stream_id` and `block_id`) re-derive once**
  in any deployment whose ingestion credentials carry no audience today, because
  `IdentityContext.audience` flips from `None` to `Some(default)` and the hash moves into the
  per-audience namespace. `identity.rs:203-206` states outright that "long-term stability of
  `process_id` values is not a design goal; re-deriving existing ids is always acceptable", and
  this is the same churn shape `mkdocs/docs/admin/ingestion.md:96-101` and
  `mkdocs/docs/otlp/index.md:108-115` already document for a deployment that starts binding
  audiences. Pre-upgrade data keeps its old ids, resolved to the same default on read; the same
  logical process appears as a new row going forward. A retried Firehose/OTLP POST that straddles
  the deploy can also store one duplicate block (old `block_id` and new both present) — a
  one-time window, already counted as `block_object_duplicate`.
- **The five `IdentityContext` sites pass `Some(audience.as_str())`.** `IdentityContext.audience`
  stays `Option<&str>`: `IdentityContext::default()` and the `None` arm remain reachable from
  `otel-ingestion`'s own unit tests and any non-HTTP caller, and narrowing that type is a separate
  cleanup with its own test churn. Worth a comment noting no HTTP path produces `None` any more.
- **Local dev (`--disable-auth`) now stamps `public`.** Harmless — it is what every reader already
  resolved those processes to.

The alternative (keep an `explicit: bool` beside the label so a resolved-to-default caller stamps
nothing) is analyzed under Trade-offs.

### 7. Why the newly-real conflict arm is safe to turn on

Arm 2 becoming a 403 is a behavior change on a path a legitimate producer essentially cannot
reach:

- Native `process_id`s are client-generated random UUIDs, fresh per process run, so a native
  producer never re-registers a *previous* run's row.
- An OTLP producer that ran unaudienced before the upgrade derives a *different* (default-salted)
  `process_id` after it, so it lands on a fresh row rather than colliding with its own legacy
  unstamped one.
- What does now get rejected is a credential claiming audience `B` re-registering a legacy
  unstamped row — i.e. exactly the squatting shape the guard exists to reject.

## Implementation Steps

### Phase 1 — the type

1. `rust/ingestion/src/write_audience.rs`: `WriteAudience(Arc<str>)`; `new(&str) -> Result<Self>`;
   `as_str(&self) -> &str`; delete `none()`. Rewrite the module and type doc comments — the
   "`None` means the credential carries no audience" framing goes away and is replaced by "always
   a real audience, resolved at the HTTP edge"; keep the charset/three-copies note.

### Phase 2 — the write path

2. `rust/ingestion/src/web_ingestion_service.rs`:
   - `WebIngestionService` gains `default_audience: WriteAudience`; `new(lake, default_audience)`;
     `pub fn default_audience(&self) -> &WriteAudience`.
   - `finalize_process_properties`: stamp unconditionally; update its doc comment.
   - `check_process_audience_conflict`: delete the early return, replace the three-way `match`
     with the single resolved comparison (Design §5). Update the method's doc comment — it
     currently documents both fail-open arms as intended behavior.
   - `remember_process_audience`: unconditional insert; update its doc comment.
   - Update `insert_process`'s doc comment (`:501-507`), which promises "an existing `NULL`
     audience is left alone".

### Phase 3 — the HTTP edge

3. `rust/public/src/servers/write_audience.rs`: `resolve_write_audience(ctx, default_audience)`
   per Design §4; rewrite the module/function docs.
4. `rust/public/src/servers/ingestion.rs`: resolve the default in `serve_ingestion` and pass it to
   `WebIngestionService::new`; update the `insert_process_request` handler call.
5. Update the five remaining handler call sites: `otlp.rs:153,170,187`, `firehose.rs:46`,
   `firehose_cloudwatch_logs.rs:39`, `webhook.rs:126`.
6. `rust/otel-ingestion/src/handler.rs:161,185,220,318` and `cloudwatch_logs.rs:223`: pass
   `Some(audience.as_str())`.

### Phase 4 — tests

7. `rust/ingestion/tests/write_audience_tests.rs`: drop the two `none()` constructor tests and the
   two `finalize_process_properties`-with-`none()` tests; add one asserting the resolved default is
   stamped like any other audience, and keep the client-`micromegas.*`-stripping coverage under a
   real audience.
8. `rust/ingestion/tests/process_audience_cache_test.rs`: `make_test_service` passes a default;
   delete `no_incoming_audience_skips_the_database` (no such state left); add a case proving a
   resolved-default caller is memoized (arm 3), i.e. a primed default-audience entry skips the
   unreachable database.
9. `rust/ingestion/tests/audience_stamping_db_test.rs`:
   - Rewrite `existing_null_audience_reregistration_is_ok_and_stays_unstamped` into two tests over
     a fabricated legacy row (insert via `insert_process`, then
     `UPDATE processes SET properties = ...` stripping the audience property): re-registering it
     under a *different* audience is now `AudienceConflict`; re-registering it under the
     deployment default is `Ok` **and leaves the row unstamped**.
   - Add the arm-1 case: an existing row stamped `team-b`, re-registered by a caller carrying no
     bound audience (i.e. the resolved default), is now `AudienceConflict` — previously a silent
     `Ok`.
10. `rust/public/tests/resolve_write_audience_tests.rs`: replace the `none()` expectations with
    default-resolution ones (no `bound_audience`, `ctx: None`, and a malformed `bound_audience`
    all resolve to the supplied default); keep the HTTP-level pass-through cases.
11. Mechanical `WebIngestionService::new` / `WriteAudience::new` updates in the remaining test
    files: `rust/ingestion/tests/{insert_block_dedup_db_test,readiness}.rs`,
    `rust/public/tests/{firehose_tests,firehose_cloudwatch_logs_tests}.rs`,
    `rust/analytics/tests/{thread_spans_ordering_db_test,jit_process_batch_db_test,ownership_rewrite_db_test,prong_b_guard_db_test}.rs`.

### Phase 5 — docs and changelog

12. Docs per the Documentation section.
13. `CHANGELOG.md` **Ingestion** entry under `## Unreleased`, with a **Minor breaking change**
    clause (Design §1/§3 API breaks) and an **Upgrade note** covering the id re-derivation and the
    new third reader of `MICROMEGAS_DEFAULT_AUDIENCE`.

No migration and no `SCHEMA_VERSION` bump: the queryable Arrow schema is unchanged, and the
`audience` column's materialized values are identical (`COALESCE(NULL, default)` and a stamped
`default` produce the same string).

## Files to Modify

Code:

- `rust/ingestion/src/write_audience.rs`
- `rust/ingestion/src/web_ingestion_service.rs`
- `rust/public/src/servers/write_audience.rs`
- `rust/public/src/servers/ingestion.rs`
- `rust/public/src/servers/otlp.rs`
- `rust/public/src/servers/firehose.rs`
- `rust/public/src/servers/firehose_cloudwatch_logs.rs`
- `rust/public/src/servers/webhook.rs`
- `rust/otel-ingestion/src/handler.rs`
- `rust/otel-ingestion/src/cloudwatch_logs.rs`
- `rust/analytics/src/audience.rs` (module doc only — the write-side contract it states changes)

Tests: `rust/ingestion/tests/{write_audience_tests,audience_stamping_db_test,process_audience_cache_test,insert_block_dedup_db_test,readiness}.rs`,
`rust/public/tests/{resolve_write_audience_tests,firehose_tests,firehose_cloudwatch_logs_tests}.rs`,
`rust/analytics/tests/{thread_spans_ordering_db_test,jit_process_batch_db_test,ownership_rewrite_db_test,prong_b_guard_db_test}.rs`

Docs: `CHANGELOG.md`, `mkdocs/docs/admin/{ingestion,authentication,monolith,flight-sql,maintenance}.md`,
`mkdocs/docs/otlp/index.md`

## Trade-offs

**Resolve at the edge (chosen) vs. resolve inside the guard.** Resolving only inside
`check_process_audience_conflict` would fix the comparison with a much smaller diff and zero id
churn, but leaves `WriteAudience` three-state — so the next surface added on the write path
(#1518's gate) starts from the same broken premise. The issue is explicit that the model, not just
the two arms, is the defect.

**Stamp the resolved default (chosen) vs. `WriteAudience { label, explicit: bool }`.** The
alternative keeps the stamp and the id derivation on the *explicit* audience only, so nothing
churns: no new property on existing-shape processes, no `process_id` re-derivation, and
`audience.rs:8-12`'s contract survives verbatim. It was rejected on two grounds. First, it keeps a
two-headed type whose users must pick the right accessor — `label` for comparisons, `explicit` for
stamping — which is the same class of mistake as today's `Option`, just relocated. Second, it makes
a resolved-to-default OTLP producer and an explicitly-default-bound one derive *different*
`process_id`s for the same resource attributes while both resolve to the same audience: two rows
for one logical process, and a permanent asymmetry between two callers the whole point of this
change is to make indistinguishable. The one-time id churn is bounded, documented, and explicitly
sanctioned by `identity.rs`'s stability note.

**Unprefixed only (chosen) vs. `{prefix}_DEFAULT_AUDIENCE`.** The knob is a property of the lake's
contents, not a per-role setting, and the lakehouse roles read it unprefixed. Giving the ingestion
edge a prefixed override would create a way to configure the write side and the read side to
disagree — the exact failure this change exists to remove. `default_audience_from_env("")` reads
`MICROMEGAS_DEFAULT_AUDIENCE` and nothing else.

**Threading the default through `WebIngestionService::new` (26 call sites)** rather than a
`Default`/env fallback inside the service: per `rust/CLAUDE.md`, a signature the compiler forces
every caller to answer beats a silently defaulted value, and 24 of the 26 are test constructions.

**Malformed `bound_audience` degrades to the default rather than rejecting.** Failing closed would
be defensible, but it would require plumbing a `Result` back through six handlers with three
distinct error-response shapes, and it is a *widening* of behavior relative to today only in
appearance: `none()` already resolved to the default on every read. Left as-is, with the `warn!`
kept and its message updated. Unreachable for a DB-backed key.

## Documentation

- `mkdocs/docs/admin/ingestion.md:32` — the `MICROMEGAS_DEFAULT_AUDIENCE` row currently reads "The
  ingestion role itself does **not** read it". That inverts: the ingestion role now reads it, and
  it must be set identically here and on every lakehouse role.
- `mkdocs/docs/admin/ingestion.md:72-101` (*What gets stamped*) — env-keyring keys, OIDC, and
  `--disable-auth` are now "stamped with the deployment default", not "stamped with nothing". The
  "Ingestion writes no audience of its own, and there is no startup backfill" paragraph becomes:
  new processes are stamped with the resolved default; pre-existing unstamped rows are never
  retro-stamped and keep resolving to the default on read. The existing `process_id`-churn
  paragraph (`:94-101`) gains this upgrade as another trigger — note that its claim about "simply
  adopting a non-`public` default" causing churn only becomes true with this change.
- `mkdocs/docs/admin/authentication.md:230-262` (*Audience stamping and the default*) — "A
  credential with none stamps nothing" and "without anything being written back to `processes`"
  both change. Add the third reader to "Set it on **every role that builds a lakehouse**": the
  ingestion role now needs it too, and a deployment that sets it on only some roles gets new
  processes stamped with one label while legacy rows read as another.
- `mkdocs/docs/admin/authentication.md:279-300` (residual-gap warning) — the process-squatting
  paragraph can drop its implicit "only when the squatted row is already stamped" qualifier; the
  registration guard now also rejects a claim against a legacy unstamped row. The
  `insert_stream`/`insert_block` write-injection gap is untouched and stays.
- `mkdocs/docs/admin/authentication.md:242-247` — the "three places the audience is read out of
  Postgres" list is unchanged, but the sentence framing the default as read-side-only needs the
  write-side resolution added.
- `mkdocs/docs/admin/monolith.md:52,60-70` — the "one prefix asymmetry" note: ingestion joins the
  unprefixed readers of `MICROMEGAS_DEFAULT_AUDIENCE`.
- `mkdocs/docs/admin/{flight-sql,maintenance}.md` — each `MICROMEGAS_DEFAULT_AUDIENCE` row says
  "set it identically on every role that builds a lakehouse"; add ingestion to that set.
- `mkdocs/docs/otlp/index.md:85-115` — the two-arm `process_id` formula: the "no write audience"
  arm is no longer reachable over HTTP, since the credential's audience or the deployment default
  always applies. Same for the `block_id` audience-prefix description at `:244` and the webhook
  note at `:443-446`.
- `rust/analytics/src/audience.rs:8-12` — the module doc's "the default is applied where the
  audience is read, not where the process is written" is now true of *legacy rows only*; rewrite
  it to say the write path resolves the default too, and that `COALESCE` exists for rows written
  before it did.
- `CHANGELOG.md` — **Ingestion** entry as described in step 13. Breaking-change surface:
  `WriteAudience::none` removed; `WriteAudience::new` takes `&str` and `as_str` returns `&str`;
  `WebIngestionService::new` gains a required `WriteAudience`;
  `micromegas::servers::write_audience::resolve_write_audience` gains a required
  `&WriteAudience`.

## Testing Strategy

- `cargo test -p micromegas-ingestion -p micromegas -p micromegas-otel-ingestion` for the unit and
  no-database tests (`write_audience_tests`, `process_audience_cache_test`,
  `resolve_write_audience_tests`, firehose HTTP tests).
- `cargo clippy --workspace --all-targets` — the `WriteAudience::new` signature change is what
  enumerates the remaining call sites.
- DB-backed tests (`#[ignore]`, need `MICROMEGAS_SQL_CONNECTION_STRING` /
  `MICROMEGAS_OBJECT_STORE_URI`): `cargo test -p micromegas-ingestion --test audience_stamping_db_test -- --ignored`,
  plus the `micromegas-analytics` DB tests touched in step 11.
- End-to-end against a local stack (`python3 local_test_env/ai_scripts/start_services.py`,
  `--disable-auth`): ingest with the Python client, then
  `micromegas-query "SELECT process_id, audience, properties FROM processes ORDER BY insert_time DESC LIMIT 5"`
  and confirm a `micromegas.audience=public` property is present on newly registered processes and
  the `audience` column matches. Repeat with `MICROMEGAS_DEFAULT_AUDIENCE=unassigned` exported for
  *all* roles and confirm both sides read `unassigned`.
- Manual squatting check against the local stack, mirroring the new DB tests: register a process
  under `team-a` via a DB-backed ingestion key, then re-register the same `process_id` with an
  unaudienced credential and confirm a 403 where today it is a silent 200.

## Open Questions

- **Do we want a one-off backfill for legacy unstamped rows?** This plan says no: the read side
  already resolves them correctly, and a retro-stamp would rewrite rows whose original audience
  was genuinely unknown. Left explicitly out of scope, as `insert_process`'s doc comment has
  always promised. Worth confirming that stays the intent now that new rows are always stamped.
- **Should `IdentityContext.audience` become non-optional in a follow-up?** After this change no
  HTTP path produces `None`; the remaining `None` users are `otel-ingestion`'s own tests and
  `IdentityContext::default()`. Narrowing it would remove the last write-side three-state
  remnant, at the cost of churn in that crate's test suite.
- **#1518 sequencing.** That issue's write-side gate depends on this comparison being
  resolved-to-resolved. Nothing here blocks on it, but the gate should not land first.
