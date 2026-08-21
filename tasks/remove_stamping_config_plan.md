# Remove `StampingConfig` Plan

## Overview

Delete the `REQUIRE_WRITE_AUDIENCE` enforcement gate (`StampingConfig`, `WriteAudienceError`, the
403-on-missing-audience branch of `resolve_write_audience`) added by AbAC Stage 5 (#1373). The gate
was designed for a future where some ingestion credentials are bound to an audience and others
aren't, so an operator could eventually force every writer onto a bound credential. In practice no
deployment binds an audience to an ingestion key yet — every credential in use today is
audience-less and therefore writes unstamped (== implicitly public). A gate that would reject 100%
of current traffic the moment it's flipped on is not a real migration tool, just complexity held in
reserve. Remove it; `resolve_write_audience` becomes infallible, and every write without a bound
audience simply stays unstamped, exactly like the `require_write_audience: false` behavior today.

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
case is defence-in-depth that was already unreachable in practice; keep it as a warn-and-degrade
rather than a panic, matching the "never a silent hard failure on a bad value" spirit of the crate,
but there's no longer an error type to propagate. `WriteAudienceError` and `StampingConfig` are
deleted outright — no deprecation shim, per this repo's Rust-API-churn stance
(`rust/CLAUDE.md` § Interface stability).

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
gap has no mitigation at all — the doc must say so plainly (open, unmitigated, tracked as a
follow-up) rather than quietly drop the sentence and leave a stale promise of protection. Given
every current ingestion key is audience-less already, this gap is not live in practice yet, but the
doc has to remain accurate for the day a deployment starts binding audiences to keys, so don't
soften it to the point of implying it's fine forever.

## Implementation Steps

1. **Delete the gate in `rust/public/src/servers/write_audience.rs`**: remove `StampingConfig` and
   `WriteAudienceError`; rewrite `resolve_write_audience` to take only `ctx` and return
   `WriteAudience` (infallible), per Design above. Trim the module doc comment's description of the
   config/knob.
2. **Update the five write-path modules** (`ingestion.rs`, `otlp.rs`, `webhook.rs`, `firehose.rs`,
   `firehose_cloudwatch_logs.rs`):
   - Drop the `Extension<Arc<StampingConfig>>` param from every handler.
   - Drop `resolve_native_write_audience`/`resolve_otlp_write_audience` wrappers; call
     `resolve_write_audience(ctx.as_ref())` directly and drop the `?`/`match Err` branch at each
     call site (`insert_process_request`, `insert_stream_request`, `insert_block_request`,
     `logs_handler`, `metrics_handler`, `traces_handler`, `webhook_handler`, `firehose_handler`,
     `cloudwatch_logs_firehose_handler`).
   - `firehose_router()` in both firehose modules: drop the `stamping: Arc<StampingConfig>`
     parameter and the `.layer(Extension(stamping))` call; update the doc comments that currently
     justify `stamping` as an explicit (non-ambient) parameter.
   - `ingestion.rs::serve_ingestion`: drop the `stamping: StampingConfig` parameter, the
     `Arc::new(stamping)` binding, the `.layer(Extension(stamping.clone()))` on `protected_app`, and
     the two `stamping.clone()` args passed into the firehose router constructors.
   - Narrow `IngestionError::Forbidden`'s doc comment to the `AudienceConflict`-only case.
3. **Update the two service entry points**:
   - `rust/monolith/src/main.rs`: delete the `ingestion_stamping` binding (`main.rs:226-230`) and its
     doc comment, and the argument passed to `serve_ingestion`.
   - `rust/telemetry-ingestion-srv/src/main.rs`: delete the `stamping` binding (`main.rs:75`) and the
     argument passed to `serve_ingestion`.
4. **Rewrite `rust/public/tests/ingestion_stamping_tests.rs`**: delete every test about the
   `require_write_audience` truth table and `from_env` parsing (the whole point of the file). What
   survives, trimmed to the new infallible signature: the "bound audience always stamps" cases and
   the "no bound audience → unstamped" case, since those still exercise real behavior
   (`resolve_write_audience`'s only remaining branch, plus the HTTP-level pass-through tests for
   OTLP/native/webhook with a bound audience). Consider folding what's left into
   `write_audience.rs`'s own module or renaming the file, since "stamping" (does a bound credential
   get stamped) is now the entire scope — "gate" tests no longer exist to justify the separate file.
5. **Update `firehose_tests.rs` / `firehose_cloudwatch_logs_tests.rs`**: delete the on-branch 403
   test (`firehose_tests.rs:199-243`); `stamping_off()` helpers and their call sites go away along
   with the `stamping` parameter on `firehose_router`.
6. **Fix now-stale doc comments referencing the removed knob** (not user docs, code comments):
   `rust/analytics/src/lakehouse/ownership_rewrite.rs:85` and `rust/auth/src/env.rs:4,15` (drop
   `REQUIRE_WRITE_AUDIENCE` from the list of example prefixed vars — check
   `read_scope.rs`'s parallel `resolved_var` copy for the same stale mention), and
   `rust/auth/tests/policy_tests.rs:713`.
7. **Documentation** (see below) — update in the same change, not a follow-up, since a stale doc
   describing a removed enforcement knob as available is actively misleading.
8. **Changelog**: add a `Removed`/breaking-change entry (see Documentation below) — this is a public
   Rust API removal (`StampingConfig`, `WriteAudienceError`, `resolve_write_audience`'s signature,
   `serve_ingestion`'s and both `firehose_router`s' parameter lists).

## Files to Modify

- `rust/public/src/servers/write_audience.rs` — delete `StampingConfig`/`WriteAudienceError`, simplify `resolve_write_audience`
- `rust/public/src/servers/ingestion.rs` — drop param from handlers + `serve_ingestion`, narrow `Forbidden` doc
- `rust/public/src/servers/otlp.rs` — drop param from handlers, delete wrapper
- `rust/public/src/servers/webhook.rs` — drop param from handler
- `rust/public/src/servers/firehose.rs` — drop param from handler + `firehose_router`
- `rust/public/src/servers/firehose_cloudwatch_logs.rs` — drop param from handler + `firehose_router`
- `rust/monolith/src/main.rs` — delete `ingestion_stamping` construction
- `rust/telemetry-ingestion-srv/src/main.rs` — delete `stamping` construction
- `rust/public/tests/ingestion_stamping_tests.rs` — delete gate tests, keep/trim stamping-pass-through tests
- `rust/public/tests/firehose_tests.rs` — delete 403 test, drop `stamping` param plumbing
- `rust/public/tests/firehose_cloudwatch_logs_tests.rs` — drop `stamping` param plumbing
- `rust/analytics/src/lakehouse/ownership_rewrite.rs` — fix stale doc comment
- `rust/auth/src/env.rs` — drop `REQUIRE_WRITE_AUDIENCE` from example list
- `rust/analytics/src/lakehouse/read_scope.rs` — check/fix its parallel `resolved_var` doc copy
- `rust/auth/tests/policy_tests.rs` — fix stale comment (line 713)
- `mkdocs/docs/admin/authentication.md` — rewrite §"Write-Side Stamping" knob mention + residual-gap admonition
- `mkdocs/docs/admin/ingestion.md` — remove knob row + "What gets stamped" mention
- `mkdocs/docs/admin/monolith.md` — remove knob row + surrounding prose
- `mkdocs/docs/otlp/index.md` — narrow the two 403-cause descriptions to conflict-only
- `CHANGELOG.md` — new entry

## Trade-offs

- **Delete outright vs. keep as a no-op knob.** Considered leaving `StampingConfig` in place but
  making `require_write_audience` always inert (accept and ignore the env var), to avoid breaking
  the Rust API. Rejected: per `rust/CLAUDE.md`, Rust API churn is fine and preferred over a
  parameter that looks load-bearing but silently does nothing — a future reader would reasonably
  set `REQUIRE_WRITE_AUDIENCE=true` and assume it works.
- **Residual-gap doc: rewrite vs. delete the admonition.** Deleting it would make the doc simpler,
  but the underlying gap (unstamped pre-registration squatting) is still real once audiences are
  actually bound to keys — only its *closure story* goes away. Rewriting to say "open, no current
  mitigation" is more honest than silence.
- **Scope**: this plan only removes the enforcement gate. It does not add a default `"public"`
  audience stamp for audience-less credentials (a separate idea raised in conversation) — that
  would be a materially different design (every row always stamped, `WriteAudience` no longer
  `Option`-shaped) and is left for a future plan if wanted.

## Documentation

- `mkdocs/docs/admin/authentication.md`:
  - Lines 206-215: drop the `{prefix}_REQUIRE_WRITE_AUDIENCE` sentence from the "Write-Side
    Stamping" intro paragraph.
  - Lines 236-286 (the "Residual gap" admonition): remove every `REQUIRE_WRITE_AUDIENCE`-as-fix
    reference (lines 267, 286) and replace with an explicit "no current mitigation; tracked as a
    follow-up" statement for the "unstamped pre-registration" scenario specifically. The first
    scenario (stamped squatter → 403 + manual `DELETE`) is unaffected and stays as-is.
- `mkdocs/docs/admin/ingestion.md`: delete the `MICROMEGAS_REQUIRE_WRITE_AUDIENCE` env var row
  (line 32) and the sentence at line 93 that introduces it under "What gets stamped."
- `mkdocs/docs/admin/monolith.md`: delete the `MICROMEGAS_INGESTION_REQUIRE_WRITE_AUDIENCE` row
  (line 50) and the surrounding explanatory paragraph (lines 62-71).
- `mkdocs/docs/otlp/index.md`: narrow both 403 descriptions (lines 235, 720) to just the
  audience-conflict cause; drop the `{prefix}_REQUIRE_WRITE_AUDIENCE` clause from each.
- `CHANGELOG.md`: add an entry under the next unreleased heading, minor-breaking-change clause per
  `rust/CLAUDE.md`'s Interface Stability section, e.g.: `StampingConfig` and `WriteAudienceError`
  (published, `micromegas::servers::write_audience`) are removed; `resolve_write_audience` now
  takes only `Option<&Extension<AuthContext>>` and returns `WriteAudience` directly (infallible,
  no `Result`); `serve_ingestion` and both `firehose_router`s drop their `StampingConfig`/
  `Arc<StampingConfig>` parameter. No behavior change for any currently-issued credential — every
  ingestion key today is audience-less and continues to write unstamped, exactly as under
  `require_write_audience: false` (the only value ever exercised in production).

## Testing Strategy

- `cargo test -p micromegas` (or workspace-wide) after trimming `ingestion_stamping_tests.rs` —
  confirm the surviving stamping-pass-through cases (bound audience → stamped; audience-less →
  unstamped) still pass with the new infallible signature.
- `cargo test -p micromegas --test firehose_tests --test firehose_cloudwatch_logs_tests` after
  removing `stamping_off()`/the 403 test — confirm the remaining ingest-and-stamp assertions pass.
- `cargo clippy --workspace -- -D warnings` and `cargo fmt` per `rust/CLAUDE.md`.
- Manual smoke: start services (`local_test_env/ai_scripts/start_services.py`), post a native
  `insert_process` with no `Authorization` header (dev-mode / `--disable-auth`) and confirm it still
  succeeds (no 403 possible anymore, since there's no gate left to trip).
- `mkdocs`: `python build-docs.py` (from `mkdocs/`) to catch any broken internal anchor links left
  by the doc edits (e.g. `#what-gets-stamped` references from other pages).

## Open Questions

- Should the residual-gap admonition in `authentication.md` be softened further given the user's
  point that no deployment binds audiences to keys yet (i.e. mark it explicitly "not exploitable
  today, only once audience-bound keys exist")? Current plan keeps the warning as-is in spirit but
  drops the false closure claim — open to making it even more low-key if the gap is considered
  entirely theoretical right now.
- Rename `ingestion_stamping_tests.rs` (e.g. to `write_audience_tests.rs`) now that "gate" tests are
  gone, or keep the filename for a smaller diff? Leaning toward rename since the name is actively
  misleading post-removal, but flagging it as a judgment call.
