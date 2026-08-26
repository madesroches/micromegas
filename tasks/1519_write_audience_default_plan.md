# Resolve the Deployment Default Audience at the Ingestion Write Path Plan

Issue: [#1519](https://github.com/madesroches/micromegas/issues/1519)

## Overview

An ingestion credential that carries no bound audience should behave exactly as if it were bound
to the deployment's default audience (`MICROMEGAS_DEFAULT_AUDIENCE`, `public` when unset). Exactly
two code sites resolve it that way today: `micromegas_auth::policy::default_audience_from_env` for
key mint/import, and `micromegas_analytics::audience::default_audience_from_env`, read once by
`LakehouseContext::new` and handed to the three Postgres read sites. (The query-side read scope
resolves no default at all — `rust/analytics/src/lakehouse/read_scope.rs:199-210` only rejects the
removed `MICROMEGAS_UNSTAMPED_AUDIENCE`.) The ingest-time write path resolves nothing: it keeps
"unaudienced" as a distinct third state, and as a direct consequence
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
generating this class of bug until it is gone. Concretely: **this plan must land before #1518's
write-side gate**, not after — no #1518 gate exists in the tree yet (the only guard today is
`check_process_audience_conflict`), so there is nothing to sequence against beyond keeping that
ordering.

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

`WebIngestionService::new(lake)` (`web_ingestion_service.rs:165`) — 27 call sites: 26 external
callers of `WebIngestionService::new(` plus `Self::new(lake)` inside the crate's own
`WebIngestionService::from_env()` (`web_ingestion_service.rs:243-250`). One external call site is
production (`rust/public/src/servers/ingestion.rs:147`, inside `serve_ingestion`); the rest are
integration tests. `serve_ingestion`'s two callers (`rust/monolith/src/main.rs:315`,
`rust/telemetry-ingestion-srv/src/main.rs:75`) pass no audience configuration.

`from_env()` is itself public API, used outside `serve_ingestion`'s call graph by
`rust/ingestion/tests/readiness.rs:36`, `rust/public/tests/firehose_tests.rs:243`, and
`rust/public/tests/firehose_cloudwatch_logs_tests.rs:205`. `rust/ingestion/Cargo.toml` depends on
neither `micromegas-auth` nor `micromegas-analytics`, so `from_env` cannot call
`default_audience_from_env` itself — that is exactly why default resolution lives in
`rust/public` instead. `from_env` therefore gains a required `WriteAudience` parameter rather than
a third, crate-local copy of default resolution:

```rust
pub async fn from_env(default_audience: WriteAudience) -> anyhow::Result<Arc<Self>>
```

Its three callers above already need mechanical updates for the `WriteAudience` type change
(Implementation step 11); they now also pass a `WriteAudience` value (any valid label — these
tests don't exercise default resolution itself) at the same call sites.

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
let default_audience = WriteAudience::new(&default_audience_from_env("")?)?;
let service = Arc::new(WebIngestionService::new(lake, default_audience));
```

The `WriteAudience::new` call here cannot actually fail: `default_audience_from_env`
(`rust/auth/src/policy.rs:73-97`) already trims and validates against the same
`[A-Za-z0-9_-]{1,255}` predicate `WriteAudience::new` applies. The `?` is redundant-by-design —
defence in depth against the two predicates drifting apart — not a reachable error path, so it
carries no `.with_context(...)` message (which would be unreachable, and
`rust/public/src/servers/ingestion.rs:1-16` imports no `anyhow::Context`).

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

`WebIngestionService::from_env()` (`web_ingestion_service.rs:243-250`) gets the same treatment as
`new`: it gains a required `default_audience: WriteAudience` parameter and passes it straight
through to `Self::new`. `micromegas-ingestion` cannot resolve the default itself — its
`Cargo.toml` depends on neither `micromegas-auth` nor `micromegas-analytics` — so the caller
resolves it and passes it in, exactly as `serve_ingestion` does for `new`.

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
appends `micromegas.audience` unconditionally, so every process registered **through the HTTP
ingestion path** carries a real audience property in Postgres. Consequences:

- **HTTP-written rows are always stamped; `bulk_ingest`/replication still writes properties
  verbatim, so `COALESCE` stays load-bearing.** That is the exact scope of the invariant, and no
  sentence anywhere in this plan or in the docs may state it more broadly. The admin FlightSQL `bulk_ingest`/`do_put_statement_ingest` replication path
  (`rust/public/src/servers/flight_sql_service_impl.rs:1281-1290`,
  `rust/analytics/src/replication.rs:120-145`) is untouched: it `INSERT`s `processes` rows with the
  source lake's properties **verbatim** — none of the stamping or stripping this plan adds — so a
  replication run from a lake holding legacy unstamped rows keeps producing NULL-audience rows
  after this change lands. The read-side `COALESCE` in `coalesced_audience_subselect` therefore
  remains load-bearing for replicated rows and for pre-existing legacy ones. `audience.rs:8-12`'s
  contract must be rewritten to say exactly this, and must not claim the write path always
  stamps.
- **Each audience gets its own id namespace, and the deployment default's namespace is the
  un-salted (legacy) one.** The five `IdentityContext` construction sites
  (`rust/otel-ingestion/src/handler.rs:161,185,220,318`, `cloudwatch_logs.rs:223`) pass `None` when
  the resolved write audience *is* the deployment default, and `Some(aud)` otherwise. All five have
  the `WebIngestionService` in scope, so the default comes from `service.default_audience()`:

  ```rust
  let aud = audience.as_str();                       // always a real label now
  let id_audience = (aud != default).then_some(aud);  // the default occupies the legacy namespace
  ```

  Consequences:

  - `rust/otel-ingestion/src/identity.rs` and `rust/otel-ingestion/src/block.rs` need **no code
    change**, and `IdentityContext.audience` stays `Option<&str>` with exactly its current
    semantics: `Some(a)` salts `NS_OTEL_PROCESS_V1` with the audience and hashes the joined field
    key under that namespace (`identity.rs:262-269`) and prefixes `block_id`'s hash input
    (`block.rs:202-206`); `None` reproduces pre-Stage-5 ids byte for byte.
  - **No re-derivation for traffic that carries no bound audience; OTLP ids *do* re-derive once
    for a key explicitly bound to the deployment default label.** A deployment leaving the knob
    unset (`public`) and one setting it to e.g. `unassigned` both keep deriving exactly today's ids
    for traffic that carries no bound audience, because that traffic resolves to the default and
    therefore keeps the legacy namespace. But today `IdentityContext.audience` is
    `Some(bound_audience)` for *any* DB-backed key, so a key explicitly bound to a label equal to
    the deployment default is salted today and moves into the un-salted legacy namespace under
    this rule — a one-time `process_id`/`stream_id`/`block_id` re-derivation for that key. This is
    not an edge case: mint falls back to the default when no audience is named
    (`rust/analytics-web-srv/src/ingestion_keys.rs:248-262`), and migration v6 backfilled every
    pre-existing key's audience to `'public'` (`rust/ingestion/src/sql_migration.rs:144-149`), so on
    a deployment with the knob unset, most DB-backed keys are bound to exactly the default label.
  - The audience-collision property the salting exists for is preserved: the mapping
    audience → namespace is still injective (the default maps to the un-salted namespace, every
    other audience to its own salted one), so two audiences sending identical resource attributes
    still never collide on one `process_id`.
  - Two doc comments change meaning and are **doc-only** edits: `process_id_from_resource`'s
    `ctx.audience` paragraph (`identity.rs:208-218`) and `block_id_with_context`'s doc
    (`block.rs:195-201`) both describe `None` as "the credential carried no audience"; `None` now
    means "the deployment default's namespace".
- **Local dev (`--disable-auth`) now stamps `public`.** Harmless — it is what every reader already
  resolved those processes to.
- **A recorded audience is permanent, exactly as it already is for every explicitly-bound
  credential.** There is no retro-stamp and no `UPDATE processes` anywhere in the tree, so a row
  that carries `micromegas.audience` keeps that label for good — that has been true since #1373 for
  every credential with a bound audience, and this change simply extends the same uniformity to
  unaudienced writes. The rows whose effective label was ever mutable were the never-stamped ones,
  and only because no label was recorded: read-time resolution from a config knob is an artifact of
  absent data, not a designed flexibility lever. The operator levers are unchanged — **access
  changes go through grants (who is granted which audiences), and new audiences come from minting
  keys bound to them** — and neither has ever involved relabeling already-written data.

**No backfill, no retro-stamp — decided.** Legacy unstamped rows are never rewritten; they keep
resolving to the deployment default on read, exactly as `insert_process`'s doc comment has always
promised and as `mkdocs/docs/admin/authentication.md:308-316` (before this plan's rewrite of it)
documents as a standing accepted gap. There is no `UPDATE processes` statement anywhere in the
codebase today and this plan does not add one for that purpose — the ones it does add (steps 9 and
12) only fabricate a legacy-shaped fixture for test coverage. Nothing about stamping new rows
changes that reasoning, since `audience` has been a physical, `COALESCE`-resolved column since
#1516 regardless of when a given row was written.

The alternative (keep an `explicit: bool` beside the label so a resolved-to-default caller stamps
nothing) is analyzed under Trade-offs.

### 7. Why the newly-real conflict arm is safe to turn on

Arm 2 becoming a 403 is a behavior change on a path a legitimate producer essentially cannot
reach:

- Native `process_id`s are client-generated random UUIDs, fresh per process run, so a native
  producer never re-registers a *previous* run's row.
- An OTLP producer that ran unaudienced before the upgrade derives the *same* `process_id` after
  it — Design §6's id-namespace rule keeps the deployment default in the legacy un-salted namespace
  — so it lands on exactly its own legacy row. That row's stored audience resolves to the same
  default the producer now resolves to, the comparison matches, and the re-registration is `Ok`.
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
   - `from_env()` gains a required `default_audience: WriteAudience` parameter, threaded straight
     into `Self::new` (Design §3).
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
6. Add `WriteAudience::id_namespace<'a>(&'a self, default: &WriteAudience) -> Option<&'a str>` to
   `rust/ingestion/src/write_audience.rs`, implementing Design §6's id-namespace rule
   (`(self.as_str() != default.as_str()).then_some(self.as_str())`) in one named place rather than
   inlining it. Update `rust/otel-ingestion/src/handler.rs:161,185,220,318` and
   `cloudwatch_logs.rs:223` to build `IdentityContext.audience` by calling
   `audience.id_namespace(service.default_audience())` at all five sites (each already has the
   `WebIngestionService` in scope). No code change in `identity.rs` or `block.rs`; both get
   **doc-comment edits only** — `identity.rs:208-218` and `block.rs:195-201` must say `None` now
   means "the deployment default's namespace", not "the credential carried no audience". This
   produces a one-time `process_id`/`stream_id`/`block_id` re-derivation for a DB-backed key
   explicitly bound to a label equal to the deployment default (Design §6) — it is not a
   zero-re-derivation change for every caller.

### Phase 4 — tests

7. `rust/ingestion/tests/write_audience_tests.rs`: drop the two `none()` constructor tests and the
   two `finalize_process_properties`-with-`none()` tests; add one asserting the resolved default is
   stamped like any other audience, and keep the client-`micromegas.*`-stripping coverage under a
   real audience. Also add a unit test for `WriteAudience::id_namespace` covering: a label equal to
   the default returns `None`; a label different from the default returns `Some` of itself; and
   both cases hold again when the default itself is a non-`public` label (e.g. `unassigned`), so
   the rule is verified independent of which label happens to be the default.
8. `rust/ingestion/tests/process_audience_cache_test.rs`: `make_test_service` passes a default;
   delete `no_incoming_audience_skips_the_database` (no such state left). No test is added for arm 3
   (`remember_process_audience`'s dropped `is_some()` guard): with `WriteAudience` single-state a
   resolved-default caller is byte-for-byte indistinguishable from any other labelled caller, so
   there is nothing label-specific left to assert.
9. `rust/ingestion/tests/audience_stamping_db_test.rs`: rewrite
   `existing_null_audience_reregistration_is_ok_and_stays_unstamped` into two tests over a
   fabricated legacy row. Fabricate by inserting via `insert_process`, then stripping the audience
   property with an `UPDATE processes SET properties = ...` helper local to this file (step 12 has
   the `micromegas-analytics` copy). Construct the test service with `public` as its default. The
   fabricating label must differ from **both** the default and the label the tests re-register
   under — e.g. fabricate under `seed-only`, re-register under `team-a` — because `insert_process`
   calls `remember_process_audience` on a fresh insert
   (`web_ingestion_service.rs:546-550`) and the 60s TTL cannot expire within a test, so a shared
   label would short-circuit on the cache-hit arm and never reach the resolved comparison. The two
   cases: re-registering the stripped row under `team-a` is now `AudienceConflict`; re-registering
   it under the deployment default is `Ok` **and leaves the row unstamped**. No separate arm-1 test
   — `different_audience_reregistration_is_a_conflict` (`:76-107`) already asserts
   stamped-`team-a` → re-register-`team-b` → `AudienceConflict` over a live DB, and once
   `WriteAudience` is single-state a "carries no bound audience" caller is that same code path
   under a different string literal. No arm-3 test either, per step 8.
10. `rust/public/tests/resolve_write_audience_tests.rs`: `make_test_service` (`:29-38`) passes a
    default; replace the `none()` expectations with
    default-resolution ones (no `bound_audience`, `ctx: None`, and a malformed `bound_audience`
    all resolve to the supplied default); keep the HTTP-level pass-through cases.
11. Mechanical `WebIngestionService::new` / `WriteAudience::new` updates in the remaining test
    files: `rust/ingestion/tests/{insert_block_dedup_db_test,readiness}.rs`,
    `rust/public/tests/{firehose_tests,firehose_cloudwatch_logs_tests}.rs`,
    `rust/analytics/tests/{thread_spans_ordering_db_test,jit_process_batch_db_test}.rs`. Three of
    these files (`readiness.rs:36`, `firehose_tests.rs:243`, `firehose_cloudwatch_logs_tests.rs:205`)
    also call `WebIngestionService::from_env()` — pass a `WriteAudience` default there too (Design §3).
12. `rust/analytics/tests/{ownership_rewrite_db_test,prong_b_guard_db_test}.rs` are **not**
    mechanical: both call a local `seed_process(.., audience: Option<&str>)` whose `None` arm
    produces a `processes` row with no `micromegas.audience` property, and both use it for a core
    fixture (`process_c`, asserted against the read-side default at
    `ownership_rewrite_db_test.rs:560-646` and `prong_b_guard_db_test.rs:373`).
    - **Keep** `seed_process`'s `audience: Option<&str>` parameter — do not narrow it to `&str`.
      Only the `None` arm's meaning changes: insert under the deployment default, then strip
      `micromegas.audience` back off with an `UPDATE processes SET properties = ...`, run *inside*
      `seed_process` so no call site changes. `Some(label)` and all downstream assertions are
      unchanged.
    - `prong_b_guard_db_test.rs`'s `seed_process(ingestion, pool, audience)` (`:75-79`) already
      takes a `&sqlx::Pool<sqlx::Postgres>`; `ownership_rewrite_db_test.rs`'s (`:93-96`) does not —
      add one, threaded from the caller's `lake.db_pool` (in scope at every call site).
    - The `UPDATE` helper goes in `rust/analytics/tests/common/` (already shared by both files).
      It cannot be shared with step 9's copy — different crate, no shared test-support crate under
      `rust/` — so there is one copy per crate.
    - Update the doc comments the insert-then-strip rewrite invalidates: both `seed_process` docs
      (`ownership_rewrite_db_test.rs:87-92`, `prong_b_guard_db_test.rs:67-75`) and
      `ownership_rewrite_db_test.rs`'s module doc (`:26`) describe the `None` arm as a never-stamped
      row produced "through the real `insert_process(body, &WriteAudience)` parameter, exactly the
      path a real client hits" — that mechanism no longer exists.

### Phase 5 — docs and changelog

13. Docs per the Documentation section.
14. `CHANGELOG.md`, under `## Unreleased`:
    - Add a new **Ingestion** entry, with a **Minor breaking change** clause (Design §1/§3 API
      breaks) and an **Upgrade note** covering the new **third code reader** of
      `MICROMEGAS_DEFAULT_AUDIENCE` — the counting convention throughout this plan is code call
      sites of a `default_audience_from_env` function (two today, see Overview), not deployment
      roles — and stating explicitly that traffic carrying no bound audience derives the same
      `process_id`/`stream_id`/`block_id` as before, but a DB-backed key explicitly bound to a
      label equal to the deployment default re-derives its ids once, moving from the salted
      namespace it occupies today into the un-salted legacy one (Design §6's id-namespace rule).
    - Reconcile the existing #1373 **Ingestion** entry (`CHANGELOG.md:35-50`) rather than leaving
      it to contradict the new one. Its "**Amended (#1482, still `## Unreleased`)**" paragraph
      (`:42`) currently claims `WriteAudience::none()` is already removed, that a credential with
      no bound audience is already stamped with the resolved deployment default, that "an
      idempotent backfill runs at every ingestion-service startup", and that an existing-`NULL`
      row is now "rejected as a database error" — none of that is true in the tree today, and the
      backfill and rejected-as-a-database-error claims never become true (Design §6 explicitly
      rejects a backfill; the existing-`NULL` arm becomes a resolved comparison, not a rejection).
      Delete or rewrite that paragraph's backfill and rejected-as-a-database-error sentences so it
      states only what actually ships, or fold its accurate content into the new entry and drop
      the stale paragraph outright. The "**Unchanged by #1482**" paragraph (`:46`) — which asserts
      the write path still stamps nothing, `WriteAudience` is still `Option<Arc<str>>` with
      `none()`, and there is no startup backfill — is retired at the same time, since after this
      plan lands none of those claims hold either.

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
- `rust/otel-ingestion/src/identity.rs` (doc comment only — `:208-218`'s `ctx.audience` paragraph;
  `None` now means "the deployment default's namespace". **No code change**, per Design §6)
- `rust/otel-ingestion/src/block.rs` (doc comment only — `block_id_with_context`'s doc,
  `:195-201`; same correction, and no code change)
- `rust/analytics/src/audience.rs` (module doc only — the write-side contract it states changes)
- `rust/analytics/src/lakehouse/ownership_rewrite.rs` (module doc only — `:78-80` asserts "A
  credential carrying no audience stamps nothing", which becomes false for the HTTP write path)
- `rust/analytics/src/metadata.rs` (doc comment only — `:53-58` asserts "A process whose credential
  carried no audience keeps no property in Postgres; every producer of this struct resolves that
  to `MICROMEGAS_DEFAULT_AUDIENCE`", which becomes false for the HTTP write path)
- `rust/analytics/src/lakehouse/audience_guard.rs` (module doc only — the same "keeps no property
  in Postgres" sentence appears in the fail-closed module doc, `:27-29`)
- `rust/monolith/src/main.rs` (comment only — `:249-255` carries the same "stamps nothing" framing
  around `analytics_read_policy`'s construction)

Tests: `rust/ingestion/tests/{write_audience_tests,audience_stamping_db_test,process_audience_cache_test,insert_block_dedup_db_test,readiness}.rs`,
`rust/public/tests/{resolve_write_audience_tests,firehose_tests,firehose_cloudwatch_logs_tests}.rs`,
`rust/analytics/tests/{thread_spans_ordering_db_test,jit_process_batch_db_test}.rs` (mechanical),
`rust/analytics/tests/{ownership_rewrite_db_test,prong_b_guard_db_test,common/db_fixtures}.rs` (not
mechanical — see step 12: their unstamped `process_c` fixture must be fabricated via a post-insert
`UPDATE` helper added to `tests/common/`, and `ownership_rewrite_db_test.rs`'s `seed_process` gains
a pool parameter)

Docs: `CHANGELOG.md`, `mkdocs/docs/admin/{ingestion,authentication,monolith,flight-sql,maintenance,api-keys,web-app}.md`,
`mkdocs/docs/otlp/index.md`, `mkdocs/docs/query-guide/schema-reference.md`,
`tasks/data_isolation/audience_based_access_control_plan.md`

## Trade-offs

**Resolve at the edge (chosen) vs. resolve inside the guard.** Resolving only inside
`check_process_audience_conflict` would fix the comparison with a much smaller diff, but leaves
`WriteAudience` three-state — so the next surface added on the write path (#1518's gate) starts
from the same broken premise. The issue is explicit that the model, not just
the two arms, is the defect.

**Stamp the resolved default (chosen) vs. `WriteAudience { label, explicit: bool }`.** The
alternative keeps the stamp on the *explicit* audience only, so no new property appears on
existing-shape processes and `audience.rs:8-12`'s contract survives verbatim. Rejected on two
grounds. First, it keeps a two-headed type whose users must pick the right accessor — `label` for
comparisons, `explicit` for stamping — which is the same class of mistake as today's `Option`, just
relocated. Second, it makes a resolved-to-default OTLP producer and an explicitly-default-bound one
derive *different* `process_id`s for the same resource attributes while both resolve to the same
audience: two rows for one logical process, and a permanent asymmetry between two callers the whole
point of this change is to make indistinguishable. Design §6's id-namespace rule gives both of those
callers the *same* `process_id`, which strengthens the second ground — and it also removes the
alternative's only remaining pitch, since under that rule neither design re-derives any id.

**Unprefixed only (chosen) vs. `{prefix}_DEFAULT_AUDIENCE`.** The knob is a property of the lake's
contents, not a per-role setting, and the lakehouse roles read it unprefixed. Giving the ingestion
edge a prefixed override would create a way to configure the write side and the read side to
disagree — the exact failure this change exists to remove. `default_audience_from_env("")` reads
`MICROMEGAS_DEFAULT_AUDIENCE` and nothing else.

**Threading the default through `WebIngestionService::new` (27 call sites, counting `from_env`'s
internal `Self::new`)** rather than a `Default`/env fallback inside the service: per
`rust/CLAUDE.md`, a signature the compiler forces every caller to answer beats a silently
defaulted value, and all but one (`serve_ingestion`) are test constructions.

**Malformed `bound_audience` degrades to the default rather than rejecting.** Failing closed would
be defensible, but it would require plumbing a `Result` back through six handlers with three
distinct error-response shapes, and it is a *widening* of behavior relative to today only in
appearance: `none()` already resolved to the default on every read. Left as-is, with the `warn!`
kept and its message updated. Unreachable for a DB-backed key.

**The #1482 addendum reverted this exact write-side design; re-adding it is deliberate.** This
design already landed on this same unmerged `audience` branch and was then reverted —
`tasks/completed/1482_audience_column_plan.md:1508-1620` ("Addendum: one default audience, resolved
where the audience is read", *Status: implemented*), reverted in `c84b7daae` / `8b674f942` /
`993c07b23`. What this plan **re-adds** from that revert: `WriteAudience` single-state with `none()`
gone and `default_from_env()`'s job moved to the caller, `WebIngestionService`'s `default_audience`
field and `new(lake, default_audience)`, the conflict guard's existing-`NULL` arm becoming a real
comparison, `resolve_write_audience` taking the default, and its five HTTP-edge callers passing it.
What **stays reverted** and this plan does not bring back: `rust/ingestion/src/audience_backfill.rs`
with its Postgres mutation on every ingestion-role startup, and `replication.rs`'s
reject-unstamped-source-process check. Also staying reverted: `IdentityContext.audience` keeps its
`Option<&str>` shape and `block.rs` keeps its "domain-separate only when `Some`" short-circuit —
Design §6's id-namespace rule depends on both, which is also why the addendum's "the ~94-site test
churn does not happen" concern remains moot here.

Why the addendum's reasoning does not settle this plan: it was arguing about what the *physical
`audience` column* needs — "the column only needs the value it materializes to be non-null, not the
row it was extracted from" — which is correct and unchanged here (Design §6 keeps `COALESCE`
load-bearing for legacy and replicated rows, and adds no backfill). This plan's motivation is a
mechanism the addendum never weighed: the fail-open authorization *comparison* in
`check_process_audience_conflict`, which is not a materialization concern and is not fixed by
resolving at read time. And the addendum's "more moving parts — and a real Postgres mutation on
every ingestion-role startup — than the column actually requires" objection lands squarely on the
backfill, which is exactly the piece left reverted.

**`IdentityContext.audience` stays `Option<&str>` — deliberately, not as a deferred cleanup.**
Narrowing it would be positively undesirable under Design §6's id-namespace rule: `None` carries
real meaning there — the deployment default's legacy un-salted namespace — and every caller,
including the five HTTP-edge sites, must be able to express it. The `Option` is load-bearing rather
than a leftover of the three-state write path, so no follow-up is proposed and
`IdentityContext::default()` stays as it is.

## Documentation

- `mkdocs/docs/admin/ingestion.md:32` — the `MICROMEGAS_DEFAULT_AUDIENCE` row currently reads "The
  ingestion role itself does **not** read it". That inverts: the ingestion role now reads it, and
  it must be set identically here and on every lakehouse role.
- `mkdocs/docs/admin/ingestion.md:72-101` (*What gets stamped*) — env-keyring keys, OIDC, and
  `--disable-auth` are now "stamped with the deployment default", not "stamped with nothing". The
  "Ingestion writes no audience of its own, and there is no startup backfill" paragraph becomes:
  new processes are stamped with the resolved default; pre-existing unstamped rows are never
  retro-stamped and keep resolving to the default on read. The existing `process_id`-churn
  paragraph (`:94-101`) needs new content, not "no change": (1) traffic carrying no bound audience
  keeps deriving today's ids across this upgrade, because it resolves to the default and keeps the
  legacy un-salted namespace (Design §6); (2) a DB-backed key explicitly bound to a label equal to
  the deployment default re-derives its `process_id`/`stream_id`/`block_id` once at upgrade time,
  moving from the salted namespace it occupies today into the un-salted one; and (3) after this
  change ships, flipping `MICROMEGAS_DEFAULT_AUDIENCE` itself becomes a new churn trigger — it
  re-derives ids for every key bound to the old default label and every key bound to the new one,
  since the un-salted namespace moves with the knob.
- `mkdocs/docs/admin/authentication.md:230-262` (*Audience stamping and the default*) — "A
  credential with none stamps nothing" and "without anything being written back to `processes`"
  both change. Add the third reader to "Set it on **every role that builds a lakehouse**": the
  ingestion role now needs it too, and a deployment that sets it on only some roles gets new
  processes stamped with one label while legacy rows read as another. The "Changing the default is
  not a routine operation" warning block in this range needs a **scoping** correction, not a new
  warning: its "regenerate the six views" advice is about read-side resolution of rows that carry no
  stamp, so after this change it applies to legacy rows and rows from the admin replication path.
  Rows that carry a stamp were never relabelled by regeneration — that has always been true of every
  explicitly-bound credential's rows (Design §6) — so do not present this as a lost capability. In
  the same warning block, add a new consequence this change introduces (additive to the scoping
  correction above, not a replacement for it): flipping the knob now also re-derives OTLP
  `process_id`/`stream_id`/`block_id` for any DB-backed key explicitly bound to the *old* or the
  *new* default label, since the un-salted namespace moves with the knob (Design §6) — today
  changing this knob has zero id consequences.
- `mkdocs/docs/admin/authentication.md:281-307` (residual-gap warning, squatting paragraph) — can
  drop its implicit "only when the squatted row is already stamped" qualifier; the registration
  guard now also rejects a claim against a legacy unstamped row. The `insert_stream`/`insert_block`
  write-injection gap is untouched and stays.
- `mkdocs/docs/admin/authentication.md:383-398` (the "Worked profiles" privacy-deployment bash
  block) — "Set it on every role that builds a lakehouse -- FlightSQL, maintenance, monolith --"
  omits ingestion; add it, since the ingestion role now reads `MICROMEGAS_DEFAULT_AUDIENCE` too
  (the third code reader — step 14 fixes the counting convention), and an operator who sets the knob
  only on the three listed roles gets new processes physically stamped `public` while legacy rows
  still read as the intended `unassigned` label. Also **rescope** "regenerate the six views if you
  ever change it": it relabels rows that carry no stamp (legacy rows, and rows from the admin
  replication path), which is all it ever did — a stamped row's label has never been regeneration's
  to change. Scoping correction only; do not frame it as a limitation. Also add the same id-churn
  consequence noted for the `:230-262` warning block: changing the knob now re-derives OTLP
  `process_id`/`stream_id`/`block_id` for any key explicitly bound to the old or new default label.
- `mkdocs/docs/admin/authentication.md:308-316` ("Known gap — no retro-stamp") — its core claim
  ("a credential with no bound audience can still pre-register a victim's future `process_id` and
  have it stay unstamped") is now false: the squatter's registration is stamped with the resolved
  default at write time, and the victim's later registration under its own audience is rejected
  with a 403 — a different residual shape (registration denial, already covered by the paragraph
  above) rather than an unlabelled row. Rewrite or delete this paragraph.
- `mkdocs/docs/admin/authentication.md:242-247` — the "three places the audience is read out of
  Postgres" list is unchanged, but the sentence framing the default as read-side-only needs the
  write-side resolution added.
- `mkdocs/docs/admin/authentication.md:184-187` — inside the physical-column admonition, "a process
  whose credential carried no audience keeps no property at all ... every site that reads an
  audience out of Postgres resolves that to `MICROMEGAS_DEFAULT_AUDIENCE`" carries the same
  now-false read-side-only framing; correct it alongside the `:230-262` edit above.
- `mkdocs/docs/admin/monolith.md:52,60-70` — the "one prefix asymmetry" note: ingestion joins the
  unprefixed readers of `MICROMEGAS_DEFAULT_AUDIENCE`.
- `mkdocs/docs/admin/{flight-sql,maintenance}.md` — each `MICROMEGAS_DEFAULT_AUDIENCE` row says
  "set it identically on every role that builds a lakehouse"; add ingestion to that set. Each row
  also describes the knob as "the audience a process whose ingestion credential carried no
  audience is **read** as" — correct that read-side-only wording too, since the HTTP ingestion
  path now stamps the resolved default at write time (legacy rows and the admin replication path
  are still read-side-resolved only).
- `mkdocs/docs/admin/api-keys.md:265-268` — "the same value data written by a credential with no
  bound audience is *read* as, applied where the audience is resolved rather than stamped onto the
  data itself" becomes false for the HTTP ingestion path; rewrite to say the value is now stamped
  onto the data at write time (still read-side-resolved only for legacy rows and the admin
  replication path).
- `mkdocs/docs/admin/api-keys.md:306-311` — "Data ingested through the env keyring
  (`MICROMEGAS_API_KEYS`) ... its processes are stamped with nothing and are *read* as the
  deployment's `MICROMEGAS_DEFAULT_AUDIENCE`" is now wrong: env-keyring ingestion resolves to no
  bound audience, which the HTTP edge now stamps as the default explicitly. Rewrite to say these
  processes are now stamped with the resolved default rather than left unstamped.
- `mkdocs/docs/admin/web-app.md:71-77` — the `MICROMEGAS_DEFAULT_AUDIENCE` comment's "nothing on
  the write side stamps this default" is now false; rewrite to say the ingestion HTTP edge stamps
  it explicitly on any credential with no bound audience.
- `mkdocs/docs/query-guide/schema-reference.md:620-628` — "the default is applied where the
  audience is read out of the metadata database, so a process that was never stamped still
  materializes under a real label" is now true only of legacy rows and the admin replication path;
  rewrite to say new rows are stamped with the resolved default at write time.
- `mkdocs/docs/otlp/index.md:85-115` — the two-arm `process_id` formula stays correct and both arms
  stay reachable over HTTP; only the meaning of the "no write audience" arm needs restating, from
  "the credential carried no audience" to "the resolved write audience is the deployment default",
  which is the arm every unaudienced credential takes (Design §6's id-namespace rule). Same
  restatement for the `block_id` audience-prefix description at `:244` and the webhook note at
  `:443-446`. The `:108-115` churn paragraph needs a new trigger added: a DB-backed key explicitly
  bound to a label equal to the deployment default re-derives its ids once at upgrade time, moving
  from the salted namespace it occupies today into the un-salted legacy one; traffic carrying no
  bound audience is unaffected. After this change ships, flipping `MICROMEGAS_DEFAULT_AUDIENCE`
  becomes a further churn trigger of its own, re-deriving ids for keys bound to the old or new
  default label.
- `rust/analytics/src/lakehouse/ownership_rewrite.rs:78-80` — "A credential carrying no audience
  stamps nothing; the resulting missing property is resolved to the deployment's
  `MICROMEGAS_DEFAULT_AUDIENCE` where the audience is *read*" is now true only of legacy rows and
  the admin replication path (Design §6); rewrite to say the HTTP write path resolves and stamps
  the default itself.
- `rust/analytics/src/metadata.rs:53-58` — "A process whose credential carried no audience keeps
  no property in Postgres; every producer of this struct resolves that to
  `MICROMEGAS_DEFAULT_AUDIENCE`" needs the identical correction: true only of legacy rows and the
  admin replication path now, not of the HTTP write path.
- `rust/analytics/src/lakehouse/audience_guard.rs:27-29` — the same sentence, in the fail-closed
  module doc's "Fail-closed" section; identical correction.
- `rust/monolith/src/main.rs:249-255` — the same "stamps nothing" framing around
  `analytics_read_policy`'s construction needs the identical correction.
- `rust/analytics/src/audience.rs:8-12` — the module doc's "the default is applied where the
  audience is read, not where the process is written" is now true of *legacy rows and admin
  `bulk_ingest`/replication rows only*; rewrite it to say the HTTP write path resolves the default
  too, and that `COALESCE` remains load-bearing both for rows written before it did and for rows
  written through the verbatim-write admin replication path
  (`rust/public/src/servers/flight_sql_service_impl.rs:1281-1290`), which this change does not
  touch.
- `tasks/data_isolation/audience_based_access_control_plan.md` (Stage 5 status update,
  `:1340-1353`) — this is the epic tracker's shipped-status record for #1373, and it currently
  states, as fact, exactly the contract this plan inverts: "`resolve_write_audience` is infallible
  now: a bound audience always stamps, an audience-less credential stays unstamped" and a
  malformed `bound_audience` "now warns and resolves to `WriteAudience::none()`". Both become false
  once this plan lands. Follow this doc's own convention for a landed-then-changed claim (the
  `*(Superseded by #1482: ...)*` parenthetical at `:1271-1272`) and append a matching note to the
  Stage 5 status update: an audience-less credential (and a malformed `bound_audience`) now
  resolves to the deployment default at the HTTP edge and is stamped explicitly, rather than
  staying unstamped / resolving to a `none()` state that no longer exists — see #1519.
- `CHANGELOG.md` — **Ingestion** entry as described in step 14. Breaking-change surface:
  `WriteAudience::none` removed; `WriteAudience::new` takes `&str` and `as_str` returns `&str`;
  `WebIngestionService::new` gains a required `WriteAudience`;
  `WebIngestionService::from_env` gains a required `WriteAudience`;
  `micromegas::servers::write_audience::resolve_write_audience` gains a required
  `&WriteAudience`.

## Testing Strategy

- `cargo test -p micromegas-ingestion -p micromegas -p micromegas-otel-ingestion` for the unit and
  no-database tests (`write_audience_tests` — including the new `WriteAudience::id_namespace` unit
  test, step 7, which is the only automated coverage of the id-namespace rule; the manual
  end-to-end check below is a supplement, not the primary coverage — `process_audience_cache_test`,
  `resolve_write_audience_tests`, firehose HTTP tests).
- `cargo clippy --workspace --all-targets` — the `WriteAudience::new` signature change is what
  enumerates the remaining call sites.
- DB-backed tests (`#[ignore]`, need `MICROMEGAS_SQL_CONNECTION_STRING` /
  `MICROMEGAS_OBJECT_STORE_URI`): `cargo test -p micromegas-ingestion --test audience_stamping_db_test -- --ignored`,
  plus the `micromegas-analytics` DB tests touched in steps 11-12.
- End-to-end against a local stack (`python3 local_test_env/ai_scripts/start_services.py`,
  `--disable-auth`): ingest with the Python client, then
  `micromegas-query "SELECT process_id, audience, properties FROM processes ORDER BY insert_time DESC LIMIT 5"`
  and confirm a `micromegas.audience=public` property is present on newly registered processes and
  the `audience` column matches. Repeat with `MICROMEGAS_DEFAULT_AUDIENCE=unassigned` exported for
  *all* roles and confirm both sides read `unassigned`.
- Id-stability check (Design §6's id-namespace rule): POST the same OTLP payload with an
  unaudienced credential before and after the change, under both the unset default and
  `MICROMEGAS_DEFAULT_AUDIENCE=unassigned`, and confirm the derived `process_id` / `stream_id` /
  `block_id` are identical in all four cases; then confirm a credential bound to a *non-default*
  audience still derives different ids from those. Then confirm the case that *does* churn: a
  DB-backed credential explicitly bound to a label equal to the deployment default derives a
  *different* `process_id`/`stream_id`/`block_id` after the change than before — it moves from the
  salted namespace it occupied today into the un-salted legacy one.
- Manual squatting check against the local stack, mirroring the new DB tests: register a process
  under `team-a` via a DB-backed ingestion key, then re-register the same `process_id` with an
  unaudienced credential and confirm a 403 where today it is a silent 200.

## Open Questions

None. The backfill question is settled (Design §6: no backfill, no retro-stamp), the
`IdentityContext.audience` typing question is settled (§6's id-namespace rule makes `Option<&str>`
the correct long-term type rather than a deferred cleanup — see Trade-offs), and the #1518 ordering
is settled (Overview: this plan lands first).
