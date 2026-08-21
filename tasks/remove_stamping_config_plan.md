# Remove `StampingConfig` Plan

## Overview

Delete the `REQUIRE_WRITE_AUDIENCE` enforcement gate (`StampingConfig`, `WriteAudienceError`, the
403-on-missing-audience branch of `resolve_write_audience`) added by AbAC Stage 5 (#1373). The gate
was designed for a future where some ingestion credentials are bound to an audience and others
aren't, so an operator could eventually force every writer onto a bound credential. In practice the
knob has never been enabled in any deployment, and even where it is, it only gates the credentials
that carry no bound audience in the first place — env-keyring keys (`MICROMEGAS_API_KEYS`), OIDC,
and `--disable-auth`. Every DB-backed `ingestion_api_keys` row already carries a bound audience
(migration v6's `NOT NULL audience` column, backfilled to `'public'`) and would sail through
unaffected, so flipping the knob on would reject 0% of a DB-key-only fleet's traffic, not 100%. An
always-off knob that only ever narrows a small, non-default credential category is not exercising
real behavior, just complexity held in reserve. Remove it; `resolve_write_audience` becomes
infallible: a credential with a bound audience keeps getting stamped, and one without stays
unstamped, exactly like the `require_write_audience: false` behavior today.

This does **not** change the read-side or the stamping mechanism itself: a credential with
`bound_audience: Some(_)` still gets stamped, `AudienceConflict` rejection on re-registration is
unrelated and stays, and `WriteAudience`/`micromegas.audience` are untouched. Only the enforcement
knob goes away.

## Current State

- `StampingConfig` (`rust/public/src/servers/write_audience.rs:19`) — one field,
  `require_write_audience: bool`, resolved via `StampingConfig::from_env(prefix)` from
  `{prefix}_REQUIRE_WRITE_AUDIENCE` / `MICROMEGAS_REQUIRE_WRITE_AUDIENCE`.
- `resolve_write_audience(ctx, cfg)` (`write_audience.rs:78`) — `Err(WriteAudienceError)` when
  `cfg.require_write_audience` is `true` and the credential has no `bound_audience` (or there's no
  auth provider at all). Otherwise always `Ok`.
- Constructed at both ingestion entry points and threaded through as `Extension<Arc<StampingConfig>>`:
  - `rust/monolith/src/main.rs:230` → `serve_ingestion(...)`
  - `rust/telemetry-ingestion-srv/src/main.rs:75` → `serve_ingestion(...)`
  - `rust/public/src/servers/ingestion.rs:169-186` layers it once over `protected_app` (native +
    OTLP + webhook routes) and passes it explicitly into the two Firehose sub-routers
    (`firehose.rs:203-209`, `firehose_cloudwatch_logs.rs:205-209`), since those aren't under the
    layered extension.
- Consumed at every write-path handler, each mapping `Err` to a 403 with its own local wrapper:
  - `ingestion.rs:65-119` (native `insert_process`/`insert_stream`/`insert_block`, via
    `resolve_native_write_audience`)
  - `otlp.rs:147-220` (`logs`/`metrics`/`traces` handlers, via `resolve_otlp_write_audience` →
    `OtelError::Denied`)
  - `webhook.rs:130-135`
  - `firehose.rs:50-59`
  - `firehose_cloudwatch_logs.rs:40-49`
- Tests: `rust/public/tests/ingestion_stamping_tests.rs` (355 lines, almost entirely about the gate:
  the `require_write_audience` truth table, `from_env` parsing, and HTTP-level 403 assertions for
  OTLP/native/webhook), plus `stamping_off()` helpers in `firehose_tests.rs:49-51` and
  `firehose_cloudwatch_logs_tests.rs:49-50`, and one on-branch 403 test in `firehose_tests.rs:199-243`.
- Docs: `mkdocs/docs/admin/authentication.md:206-286`, `mkdocs/docs/admin/ingestion.md:32,93`,
  `mkdocs/docs/admin/monolith.md:50,62-71`, `mkdocs/docs/otlp/index.md:235,720` all describe the
  knob and/or the "residual gap...closed by `REQUIRE_WRITE_AUDIENCE=true`" story (see Design below).
- Original design doc: `tasks/completed/1373_ingestion_stamping_plan.md`.

## Design

### `resolve_write_audience` becomes infallible

```rust
pub fn resolve_write_audience(ctx: Option<&Extension<AuthContext>>) -> WriteAudience {
    let audience = ctx.and_then(|Extension(c)| c.bound_audience.as_deref());
    match WriteAudience::new(audience) {
        Ok(w) => w,
        Err(e) => {
            warn!("bound_audience failed WriteAudience validation, ignoring: {e:#}");
            WriteAudience::none()
        }
    }
}
```

`bound_audience` is already `CHECK`-constrained in the `ingestion_api_keys` table
(`WriteAudience::new`'s doc comment, `rust/ingestion/src/write_audience.rs:24-28`), so the malformed
case is defence-in-depth that was already unreachable in practice for a DB-backed key; a future
non-DB producer of `bound_audience` could still trip it. Note this is a deliberate fail-**open**
flip, not a wash: `WriteAudience::new`'s own doc frames the malformed-input rejection as "defence in
depth ... rather than stamping it" — i.e. today a malformed value is a 403, and warn-and-degrade
instead resolves it to `WriteAudience::none()`, which is effectively public under the commonly
recommended `MICROMEGAS_UNSTAMPED_AUDIENCE=public`. It is accepted here only because
`resolve_write_audience` no longer has a `Result` to propagate, and reintroducing one solely for
this already-unreachable input would reopen the exact enforcement surface this plan removes. Call
this out explicitly as a deliberate behavior narrowing in the changelog entry below, rather than
leaving it implied by "infallible." `WriteAudienceError` and `StampingConfig` are deleted outright —
no deprecation shim, per this repo's Rust-API-churn stance (`rust/CLAUDE.md` § Interface stability).

### Call sites

Every handler drops its `Extension<Arc<StampingConfig>>` parameter and its `Result`-returning
wrapper (`resolve_native_write_audience`, `resolve_otlp_write_audience`) collapses into a direct
call: `let audience = resolve_write_audience(ctx.as_ref());` with no `?`/`match`. The two Firehose
`firehose_router` constructors drop their `stamping: Arc<StampingConfig>` parameter entirely (no
replacement needed — there's nothing left to layer). `serve_ingestion` drops its `stamping:
StampingConfig` parameter and the `Extension(stamping)` layer.

`IngestionError::Forbidden` stays (it's still used for `AudienceConflict`), but its doc comment
(`ingestion.rs:27-31`), which currently says "either `REQUIRE_WRITE_AUDIENCE` is set... or a
conflicting re-registration," narrows to just the conflict case. Same narrowing for the `403`
line in `otlp/index.md:235,720` (see Documentation below).

### Residual-gap doc claim needs correcting, not just deleting

`mkdocs/docs/admin/authentication.md:256-286` documents a confidentiality gap ("unstamped
pre-registration") whose *only* stated closure is `{prefix}_REQUIRE_WRITE_AUDIENCE=true`
(also referenced from `ownership_rewrite.rs:85` and the design doc). Once the knob is gone, this
gap has no mitigation at all — the doc must say so plainly rather than quietly drop the sentence
and leave a stale promise of protection. Every other deferred gap in these docs cites a concrete
tracker (Stage 5b, referenced from `ownership_rewrite.rs:75-78` and `web_ingestion_service.rs:266,396`);
a bare "tracked as a follow-up" with no issue number would just replace one stale promise with
another, so Implementation Step 7 below has the implementer file a tracking issue for this gap and
cite its number in both docs — if that step is skipped, the doc must instead say plainly that no
mitigation exists and nothing tracks it yet, rather than claiming a follow-up that doesn't exist. This gap
is not merely theoretical: migration v6 already makes every DB-backed ingestion key audience-bound,
so any deployment mixing DB-backed keys with an env-keyring/OIDC credential (which stay
audience-less) has this gap live today — the latter can still pre-register a victim's future
`process_id` unstamped. Keep the admonition at full strength; only the false closure claim goes
away, not the warning itself.

## Implementation Steps

1. **Delete the gate in `rust/public/src/servers/write_audience.rs`**: remove `StampingConfig` and
   `WriteAudienceError`; rewrite `resolve_write_audience` to take only `ctx` and return
   `WriteAudience` (infallible), per Design above. Trim the module doc comment's description of the
   config/knob.
2. **Update the five write-path modules** (`ingestion.rs`, `otlp.rs`, `webhook.rs`, `firehose.rs`,
   `firehose_cloudwatch_logs.rs`):
   - Drop the `Extension<Arc<StampingConfig>>` param from every handler.
   - Drop `resolve_native_write_audience`/`resolve_otlp_write_audience` wrappers. For handlers that
     consume the resolved `WriteAudience` (`insert_process_request`, `logs_handler`,
     `metrics_handler`, `traces_handler`, `webhook_handler`, `firehose_handler`,
     `cloudwatch_logs_firehose_handler`), call `resolve_write_audience(ctx.as_ref())` directly with
     no `?`/`match Err` branch. `insert_stream_request` and `insert_block_request` never took a
     `&WriteAudience` themselves — the resolve call there existed only to run the gate — so for
     those two, delete the call entirely along with the now-unused `ctx: Option<Extension<AuthContext>>`
     parameter (otherwise an unused-variable warning under `-D warnings`).
   - `firehose_router()` in both firehose modules: drop the `stamping: Arc<StampingConfig>`
     parameter and the `.layer(Extension(stamping))` call; update the doc comments that currently
     justify `stamping` as an explicit (non-ambient) parameter.
   - `ingestion.rs::serve_ingestion`: drop the `stamping: StampingConfig` parameter, the
     `Arc::new(stamping)` binding, the `.layer(Extension(stamping.clone()))` on `protected_app`, and
     the two `stamping.clone()` args passed into the firehose router constructors.
   - Narrow `IngestionError::Forbidden`'s doc comment to the `AudienceConflict`-only case.
   - `insert_stream_request`/`insert_block_request`'s doc comments (`ingestion.rs:89-91,104-106`)
     currently promise "Returns 403 when the write audience gate rejects the request (§5)" — drop
     that clause entirely, since neither handler can produce a 403 any more. `insert_process_request`'s
     doc comment narrows the same way `IngestionError::Forbidden`'s does, to the `AudienceConflict`
     case only.
3. **Update the two service entry points**:
   - `rust/monolith/src/main.rs`: delete the `ingestion_stamping` binding (`main.rs:226-230`) and its
     doc comment, and the argument passed to `serve_ingestion`.
   - `rust/telemetry-ingestion-srv/src/main.rs`: delete the `stamping` binding (`main.rs:75`) and the
     argument passed to `serve_ingestion`.
4. **Rewrite and rename `rust/public/tests/ingestion_stamping_tests.rs` →
   `rust/public/tests/write_audience_tests.rs`**: delete every test about the `require_write_audience`
   truth table and `from_env` parsing (the whole point of the file). What survives, trimmed to the
   new infallible signature: the "bound audience always stamps" cases and the "no bound audience →
   unstamped" case, since those still exercise real behavior (`resolve_write_audience`'s only
   remaining branch), plus one surviving HTTP-level pass-through test each for OTLP and native.
   Each knob_off/knob_on pair here reduces to the same behavior once the gate is gone —
   `otlp_knob_off_empty_resource_logs_passes_the_gate_and_returns_ok` and
   `otlp_knob_on_bound_audience_credential_passes_the_gate` both become "empty `resourceLogs` →
   200"; `native_knob_off_malformed_body_reaches_the_parse_boundary_as_400` and
   `native_knob_on_bound_audience_credential_reaches_the_parse_boundary` both become "malformed CBOR
   → 400" — so delete the `knob_off` test in each pair and keep only the `knob_on` one (renamed to
   drop the "knob"/"gate" framing and trimmed of its now-gone `StampingConfig` argument), since the
   remaining ctx-present/ctx-absent difference is already covered by the surviving unit tests.
   Webhook has no such pass-through case today —
   its only test in this file, `webhook_knob_on_no_extension_is_denied_with_plain_text_403`, is a
   gate test and gets deleted along with the rest of the truth table, and there is no other webhook
   HTTP test anywhere in the repo (`rust/otel-ingestion/tests/webhook_tests.rs` asserts
   payload/identity shape, not the HTTP route). Write one new webhook HTTP case here, but keep it
   zero-DB per this file's own invariant (`ingestion_stamping_tests.rs:4-7`, "every case here must
   be a request that either stops at the gate or does zero database work"): assert that an
   empty-body request through `webhook_router()` gets the 400 it already returns before any DB work
   (`webhook.rs:123-125`). Do not resolve the audience and reach for the harness's
   lazily-connected `postgres://localhost/unused` pool instead — that 503 boundary is only reachable
   within a useful test runtime by overriding sqlx's default 30s `acquire_timeout` (see the
   `acquire_timeout(50ms)` pattern in `rust/ingestion/tests/process_audience_cache_test.rs` and
   `rust/auth/tests/db_api_key_tests.rs`), and even then a bound-audience and an audience-less
   credential would hit the identical 503, so it wouldn't be differential anyway. Keep the surviving tests under `rust/public/tests/` — matching
   every other test file in that directory and `rust/CLAUDE.md`'s rule that unit tests live under a
   crate's `tests/` folder, not alongside the lib implementation — with the new filename replacing
   the "stamping" framing, since "gate" tests no longer exist to justify it. Also rename the
   matching `[[test]]` block in `rust/public/Cargo.toml` (its `name` and `path` from
   `ingestion_stamping_tests`/`tests/ingestion_stamping_tests.rs` to
   `write_audience_tests`/`tests/write_audience_tests.rs`, keeping
   `required-features = ["server"]` unchanged) — cargo resolves that path explicitly, so renaming
   the file alone breaks the build.
5. **Update `firehose_tests.rs` / `firehose_cloudwatch_logs_tests.rs`**: `stamping_off()` helpers and
   their call sites go away along with the `stamping` parameter on `firehose_router`. The on-branch
   403 test (`firehose_tests.rs:199-243`) loses its reason for existing as a 403 test, but it is the
   only test in either file that exercises a bound-audience provider through `firehose_auth_middleware`
   — the regression guard for #1373's context-propagation fix ("`firehose_auth_middleware` no longer
   discards the validated context"). Don't delete that coverage outright — but this harness's pool
   (`postgres://localhost/unused`, lazily connected) is never actually touched, and per the file's own
   header comment "every case must stop before touching it," so asserting a stamped `micromegas.audience`
   on a resulting process needs real DB writes that this harness cannot do. `firehose_auth_middleware`
   is also `pub(crate)` (`firehose_common.rs:71`), so a test under `tests/` can't call it directly
   either. Resolve this with a small `pub` seam instead of a DB write: change
   `firehose_auth_middleware` from `pub(crate)` to `pub`. This is the only viable option of the
   three considered — a `#[cfg(test)]` hook is invisible to integration tests under
   `rust/public/tests/`, which link the lib compiled without `cfg(test)`, and a thin `pub fn`
   wrapper the test calls directly can't work either, since `firehose_auth_middleware` takes an
   `axum::middleware::Next`, and in axum 0.8 `Next` has a single private field and no public
   constructor, so no external crate can build one to pass in. With `firehose_auth_middleware`
   made `pub`, layer it via `middleware::from_fn` over a test-local probe route that reads the
   `AuthContext` extension the middleware inserts, so the DB-less harness can assert the propagated
   `bound_audience` reaches that extension without going through ingestion at all. Reshape the test
   to build that router, send the bound-audience request, and assert on the probe route's extension
   instead of on a 403.
6. **Fix now-stale doc comments referencing the removed knob** (not user docs, code comments):
   `rust/analytics/src/lakehouse/ownership_rewrite.rs:85` and `rust/auth/src/env.rs:4,15` (drop
   `REQUIRE_WRITE_AUDIENCE` from the list of example prefixed vars — check
   `read_scope.rs`'s parallel `resolved_var` copy for the same stale mention), and
   `rust/auth/tests/policy_tests.rs:713`. Also `rust/ingestion/src/write_audience.rs`'s
   `WriteAudience::new` doc comment (`:20-28`) — the same comment the Design section above quotes —
   which currently frames the malformed-label case as being "rejected" as "defence in depth ...
   rather than stamping it": narrow it to say the ingestion edge now warns and degrades to
   `WriteAudience::none()` instead of rejecting, since that defence-in-depth no longer exists for
   the HTTP-edge caller the comment names. Also `rust/otel-ingestion/src/error.rs`: the module doc
   (`:7-9`), the `Denied` variant doc (`:56-58`), the `public_message` field doc (`:62-67`), and the
   comment at `:126` all describe `Denied` as having two causes — "gate rejection vs. audience
   conflict" — but once `otlp.rs`'s gate-rejection producer (`:152-156`) is gone, `AudienceConflict`
   (`:150`) is the only remaining producer, and `error.rs:146-158`'s `AudienceConflict` arm is then
   `Denied`'s only construction site, always passing the same hard-coded string. `webhook.rs:130-135`
   never builds `Denied` (it renders its own `text/plain` 403), and
   `rust/otel-ingestion/tests/error_tests.rs:20` only pattern-matches the variant, so nothing depends
   on `public_message` varying. Remove the `public_message` field from `Denied` outright: change
   `public_message()` to return the single constant string directly instead of reading the field,
   drop the now-single-copy `public_message` from the `AudienceConflict` arm's constructor and from
   `with_context`'s match arms (`:179-187`), and narrow all four doc comments (module doc, `Denied`
   variant doc, and the `:126` comment; the field doc disappears with the field) to the conflict-only
   cause.
7. **File a tracking issue for the now-unmitigated residual gap**, before writing the doc text
   below: open a GitHub issue on this gap (unstamped pre-registration squatting, live today for any
   deployment mixing DB-backed keys with an env-keyring/OIDC credential per the Design section
   above) and note its number for use in the doc edits. If no issue is filed, the doc edits below
   must say plainly that no mitigation exists and nothing tracks it yet — never write "tracked as a
   follow-up" without a real issue number to cite.
8. **Documentation** (see below) — update in the same change, not a follow-up, since a stale doc
   describing a removed enforcement knob as available is actively misleading.
9. **Changelog**: no new entry — `StampingConfig`/`WriteAudienceError`/`resolve_write_audience`/
   `serve_ingestion`/`firehose_router` were all introduced under AbAC Stage 5 (#1373) in the
   still-open `## Unreleased` section, so nothing about them has ever shipped and there is no public
   API break to record. Edit the existing Stage 5 entry in `CHANGELOG.md` in place instead (see
   Documentation below), the same way the `OwnershipRewriteConfig` → `IsolationConfig` rename earlier
   in that same `## Unreleased` section is folded into its own still-unreleased entry rather than
   given a separate breaking-change clause.

## Files to Modify

- `rust/public/src/servers/write_audience.rs` — delete `StampingConfig`/`WriteAudienceError`, simplify `resolve_write_audience`
- `rust/public/src/servers/ingestion.rs` — drop param from handlers + `serve_ingestion`, narrow `Forbidden` doc
- `rust/public/src/servers/otlp.rs` — drop param from handlers, delete wrapper
- `rust/public/src/servers/webhook.rs` — drop param from handler
- `rust/public/src/servers/firehose.rs` — drop param from handler + `firehose_router`
- `rust/public/src/servers/firehose_cloudwatch_logs.rs` — drop param from handler + `firehose_router`
- `rust/monolith/src/main.rs` — delete `ingestion_stamping` construction
- `rust/telemetry-ingestion-srv/src/main.rs` — delete `stamping` construction
- `rust/public/tests/ingestion_stamping_tests.rs` — rename to `write_audience_tests.rs`, delete gate tests, keep/trim stamping-pass-through tests
- `rust/public/Cargo.toml` — rename the `[[test]]` block's `name`/`path` from `ingestion_stamping_tests` to `write_audience_tests` to match the renamed file, keeping `required-features = ["server"]`
- `rust/public/tests/firehose_tests.rs` — reshape 403 test into an audience-propagation assertion, drop `stamping` param plumbing
- `rust/public/tests/firehose_cloudwatch_logs_tests.rs` — drop `stamping` param plumbing
- `rust/analytics/src/lakehouse/ownership_rewrite.rs` — fix stale doc comment
- `rust/auth/src/env.rs` — drop `REQUIRE_WRITE_AUDIENCE` from example list
- `rust/analytics/src/lakehouse/read_scope.rs` — check/fix its parallel `resolved_var` doc copy
- `rust/auth/tests/policy_tests.rs` — fix stale comment (line 713)
- `rust/ingestion/src/write_audience.rs` — narrow `WriteAudience::new`'s doc comment to describe warn-and-degrade instead of rejection
- `rust/otel-ingestion/src/error.rs` — remove the `public_message` field from `Denied`, narrow the four `Denied`-has-two-causes doc comments to conflict-only
- `mkdocs/docs/admin/authentication.md` — rewrite §"Write-Side Stamping" knob mention + residual-gap admonition
- `mkdocs/docs/admin/ingestion.md` — remove knob row + "What gets stamped" mention
- `mkdocs/docs/admin/monolith.md` — remove knob row, rewrite the prefix-asymmetry admonition
- `mkdocs/docs/otlp/index.md` — narrow the two 403-cause descriptions to conflict-only
- `CHANGELOG.md` — in-place edits to the still-unreleased Stage 5 (#1373) entry (lines 37, 38, 40, 42)
- `tasks/data_isolation/audience_based_access_control_plan.md` — in-place status note that the
  `REQUIRE_WRITE_AUDIENCE` knob described as Stage 5's deliverable (line 1324) was removed before
  release

## Trade-offs

- **Delete outright vs. keep as a no-op knob.** Considered leaving `StampingConfig` in place but
  making `require_write_audience` always inert (accept and ignore the env var), to avoid breaking
  the Rust API. Rejected: per `rust/CLAUDE.md`, Rust API churn is fine and preferred over a
  parameter that looks load-bearing but silently does nothing — a future reader would reasonably
  set `REQUIRE_WRITE_AUDIENCE=true` and assume it works.
- **Residual-gap doc: rewrite vs. delete the admonition.** Deleting it would make the doc simpler,
  but the underlying gap (unstamped pre-registration squatting) is already real for any deployment
  mixing DB-backed and env-keyring/OIDC credentials — only its *closure story* goes away. Rewriting
  to say "open, no current mitigation" is more honest than silence.
- **Scope**: this plan only removes the enforcement gate. It does not add a default `"public"`
  audience stamp for audience-less credentials (a separate idea raised in conversation) — that
  would be a materially different design (every row always stamped, `WriteAudience` no longer
  `Option`-shaped) and is left for a future plan if wanted.

## Documentation

- `mkdocs/docs/admin/authentication.md`:
  - Lines 206-215: drop the `{prefix}_REQUIRE_WRITE_AUDIENCE` sentence from the "Write-Side
    Stamping" intro paragraph.
  - Lines 236-286 (the "Residual gap" admonition): remove every `REQUIRE_WRITE_AUDIENCE`-as-fix
    reference (lines 267, 286) and replace with an explicit "no current mitigation" statement for
    the "unstamped pre-registration" scenario specifically, citing the tracking issue filed in
    Implementation Step 7 by number (e.g. "tracked in #NNNN") — never the bare phrase "tracked as a
    follow-up" with no issue to point to. If Step 7 was skipped, state plainly that no mitigation
    exists and nothing tracks it yet. The first scenario (stamped squatter → 403 + manual `DELETE`)
    is unaffected and stays as-is.
- `mkdocs/docs/admin/ingestion.md`: delete the `MICROMEGAS_REQUIRE_WRITE_AUDIENCE` env var row
  (line 32) and, under "What gets stamped," the whole `MICROMEGAS_REQUIRE_WRITE_AUDIENCE` paragraph
  (lines 93-97) — it's one five-line paragraph, not a single sentence, and deleting only line 93
  would strand "with it set, a credential carrying no write audience gets a 403 (native/OTLP/webhook)
  or a Firehose retry-then-spill... Leave it off for a mixed fleet still migrating onto DB-backed
  keys; set it once every producer authenticates with an audience-bound credential" with no
  antecedent.
- `mkdocs/docs/admin/monolith.md`: delete the `MICROMEGAS_INGESTION_REQUIRE_WRITE_AUDIENCE` row
  (line 50), then rewrite the whole `!!! note "One prefix asymmetry, pre-existing"` admonition
  (lines 61-73) in place rather than deleting only lines 62-71 — lines 69-73 also depend on the
  removed knob as their contrast case ("follow `MICROMEGAS_DEFAULT_KEY_AUDIENCE`'s convention, not
  `MICROMEGAS_INGESTION_REQUIRE_WRITE_AUDIENCE`'s") and would otherwise strand a still-true
  statement with no antecedent. Re-anchor the prefix-asymmetry contrast on
  `MICROMEGAS_INGESTION_API_KEYS`, as documented in the "Authentication" section below (line 101)
  — not a table row itself, since after this deletion the env-var table has no
  `MICROMEGAS_INGESTION_*` row left.
- `mkdocs/docs/otlp/index.md`: narrow both 403 descriptions (lines 235, 720) to just the
  audience-conflict cause; drop the `{prefix}_REQUIRE_WRITE_AUDIENCE` clause from each.
- `CHANGELOG.md`: no new entry — edit the still-`## Unreleased` Stage 5 (#1373) entry in place,
  the same way the `OwnershipRewriteConfig` → `IsolationConfig` rename earlier in that section is
  folded into its own entry rather than given a separate clause:
  - Line 37: delete the sentence introducing the `{prefix}_REQUIRE_WRITE_AUDIENCE` knob
    ("A new `{prefix}_REQUIRE_WRITE_AUDIENCE` knob ... instead of a silent unstamped write — see
    `mkdocs/docs/admin/ingestion.md`.").
  - Line 38 (the "Known gap, documentation-only for now" paragraph): drop the closing sentence
    naming `{prefix}_REQUIRE_WRITE_AUDIENCE=true` as the fix, and replace it with a plain "no
    current mitigation" statement citing the same tracking issue number as the `authentication.md`
    edit above (or, if Step 7 was skipped, the same plain "nothing tracks it yet" wording) — this
    gap stays open either way.
  - Line 40 (the Firehose/CloudWatch Logs sentence): drop the trailing "and gated" so it reads
    "Firehose and CloudWatch Logs deliveries are stamped the same as every other entry point" —
    the write-audience gate being deleted is exactly what made them "gated" too.
  - Line 42 (the breaking-change clause): delete the `serve_ingestion`/`StampingConfig` and
    `firehose_router`/`Arc<StampingConfig>` clauses outright (they never shipped). Rewrite the
    `resolve_write_audience` clause to describe its *final* shape instead of the intermediate one:
    it now takes `Option<&Extension<AuthContext>>` and returns `WriteAudience` directly
    (infallible, no `Result`) — and explicitly note, per the Design section above, that a malformed
    `bound_audience` now resolves to `WriteAudience::none()` (fail-open) instead of a 403
    (fail-closed), a deliberate narrowing of the defence-in-depth check versus what shipped in this
    same entry, not a behavior-preserving refactor. Leave the `WebIngestionService`/`handler`
    `&WriteAudience`-parameter clauses as-is.

## Testing Strategy

- `cargo test -p micromegas --features server` (or workspace-wide `cargo test` /
  `python3 ../build/rust_ci.py`) after trimming and renaming `ingestion_stamping_tests.rs` to
  `write_audience_tests.rs` — confirm the surviving stamping-pass-through cases (bound audience →
  stamped; audience-less → unstamped) still pass with the new infallible signature. `--features
  server` is required: every test target in `rust/public/Cargo.toml` is
  `required-features = ["server"]` and the crate's `default = []`, so without it (or without
  running the workspace-wide command, where another member pulls the feature in) the tests are
  silently skipped rather than run.
- `cargo test -p micromegas --features server --test firehose_tests --test
  firehose_cloudwatch_logs_tests` after removing `stamping_off()` and reshaping the former 403 test
  into an assertion on the propagated `AuthContext` extension via the new `firehose_common.rs` test
  seam (§5) — confirm it and the remaining ingest-and-stamp assertions pass.
- `cargo clippy --workspace -- -D warnings` and `cargo fmt` per `rust/CLAUDE.md`.
- Manual smoke: start services (`local_test_env/ai_scripts/start_services.py`), post a native
  `insert_process` with no `Authorization` header (dev-mode / `--disable-auth`) and confirm it still
  succeeds (no 403 possible anymore, since there's no gate left to trip).
- `mkdocs`: `python build-docs.py` (from `mkdocs/`) to catch any broken internal anchor links left
  by the doc edits (e.g. `#what-gets-stamped` references from other pages).
