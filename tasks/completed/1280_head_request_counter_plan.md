# HEAD Request Counter for object-cache-srv Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1280

## Overview

Add an `object_cache_head_requests` counter to the `HEAD /obj/{key}` path in
`object-cache-srv`, tagged with `status` and `prefix`, emitted from a thin
wrapper around the HEAD handler — mirroring exactly how `get_range_handler`
and `post_ranges_handler` already wrap their inner handlers to count every
outcome. Today `head_handler` emits no request counter of its own, so HEAD
traffic volume and its status-code distribution are invisible except as an
inferred residual of two other call sites' size/HEAD-tier metrics. This closes
that observability gap with a direct counter.

## Current State

`head_handler` lives in `rust/object-cache-srv/src/handlers.rs:162-189`. It is a
single `#[span_fn]` function that validates the key, calls
`state.cache.size(&key)`, and returns `200` (with `Content-Length`), `400`
(invalid key), `404` (not found), or `500` (other size error). It emits **no**
request counter.

By contrast, the two sibling read handlers each use a two-function
wrapper/inner split so they can count every outcome — success and failure
alike — exactly once:

- `get_range_handler` (`handlers.rs:198-219`) — the wrapper: classifies the
  prefix, awaits `get_range_handler_inner`, derives the final `status` from the
  `Ok(resp)`/`Err(code)` result, and emits `object_cache_get_requests` tagged
  `status`/`prefix`. The inner (`get_range_handler_inner`,
  `handlers.rs:221-391`) carries the `#[span_fn]` and does the real work.
- `post_ranges_handler` (`handlers.rs:406-427`) — the same pattern for
  `object_cache_ranges_requests`, with `post_ranges_handler_inner` at
  `handlers.rs:429-587`.

Supporting pieces already in place and reusable as-is:

- `status_label(StatusCode) -> &'static str` (`handlers.rs:149-160`) maps status
  codes to the bounded `status` tag values. It already covers every status the
  HEAD path produces: `200`, `400`, `404`, `500`.
- `state.cache.classify(&key)` produces the `prefix` tag (called exactly as in
  the GET/ranges wrappers).
- The `imetric!` + `PropertySet::find_or_create(vec![Property::new("status", …),
  Property::new("prefix", …)])` emission shape.

Router wiring in `rust/object-cache-srv/src/object_cache_srv.rs:174` maps
`.head(head_handler)`; `head_handler` is imported at line 24. The wrapper keeps
the public name `head_handler`, so **no router or import change is needed**.

Docs: `mkdocs/docs/admin/object-cache.md:208-209` document
`object_cache_get_requests` and `object_cache_ranges_requests` as table rows.
Tests: `rust/object-cache-srv/tests/telemetry_tests.rs` has one
"counts every outcome" regression test per existing wrapper, plus a shared
`tagged_status_prefix_pairs` helper.

## Design

Split `head_handler` into a thin counting wrapper (public, keeps the name
`head_handler`) and a `head_handler_inner` that carries the existing logic —
byte-for-byte the structure of `get_range_handler` / `get_range_handler_inner`.

```rust
/// Thin wrapper around `head_handler_inner`, mirroring
/// `get_range_handler`/`post_ranges_handler`: counts every outcome (not just
/// the success path) with a `status`/`prefix`-tagged `object_cache_head_requests`.
/// Before this, HEAD traffic had no direct counter and could only be inferred
/// as a residual of the size/HEAD-tier metrics (#1280).
pub async fn head_handler(
    Path(key): Path<String>,
    State(state): State<AppState>,
) -> Result<Response, StatusCode> {
    let prefix = state.cache.classify(&key);
    let result = head_handler_inner(key, state).await;
    let status = match &result {
        Ok(resp) => resp.status(),
        Err(code) => *code,
    };
    imetric!(
        "object_cache_head_requests",
        "count",
        PropertySet::find_or_create(vec![
            Property::new("status", status_label(status)),
            Property::new("prefix", prefix),
        ]),
        1_u64
    );
    result
}

#[span_fn]
async fn head_handler_inner(
    key: String,
    state: AppState,
) -> Result<Response, StatusCode> {
    // ...body identical to today's head_handler (validate_key + cache.size + build response)...
}
```

Notes matching the existing pattern precisely:

- The `#[span_fn]` moves from the old `head_handler` onto `head_handler_inner`.
  The wrapper is **not** `#[span_fn]` (neither is `get_range_handler` nor
  `post_ranges_handler`), so the span still wraps the real work only.
- `classify(&key)` is computed in the wrapper **before** `state` is moved into
  the inner, exactly as `get_range_handler:203-204` does.
- The inner's signature becomes `(key: String, state: AppState)` — the
  `Path`/`State` extractors stay on the wrapper (the axum entry point), matching
  the GET/ranges inners which take plain `String`/`AppState`.
- `status_label` needs no change: the HEAD path's `200`/`400`/`404`/`500` are all
  already mapped.
- No new imports: `imetric!`, `Property`, `PropertySet`, `status_label` are all
  already in scope in this file.

There is no double-counting concern (unlike the ranges empty-short-circuit case):
the inner never emitted a request counter, so the wrapper becomes its sole and
only emitter.

## Implementation Steps

1. **`rust/object-cache-srv/src/handlers.rs`** — refactor `head_handler`
   (lines 162-189):
   - Rename the current `#[span_fn] pub async fn head_handler(...)` to
     `#[span_fn] async fn head_handler_inner(key: String, state: AppState) -> Result<Response, StatusCode>`,
     replacing the `Path(key): Path<String>, State(state): State<AppState>`
     extractor params with plain `key: String, state: AppState`. The body is
     unchanged.
   - Add a new `pub async fn head_handler(Path(key): Path<String>, State(state):
     State<AppState>) -> Result<Response, StatusCode>` wrapper above it that
     classifies the prefix, awaits the inner, derives `status`, and emits
     `object_cache_head_requests` (see Design). Place it in the same
     wrapper-above-inner order used for the GET/ranges pair, with an analogous
     doc comment.

2. **`rust/object-cache-srv/tests/telemetry_tests.rs`** — add a
   `head_handler_counts_every_outcome` regression test mirroring
   `get_range_handler_counts_every_outcome` (see Testing Strategy). Import
   `head_handler` alongside the existing handler imports at line 12.

3. **`mkdocs/docs/admin/object-cache.md`** — add a metric table row for
   `object_cache_head_requests` next to the GET/ranges request-counter rows
   (after line 209).

## Files to Modify

- `rust/object-cache-srv/src/handlers.rs` — wrapper/inner split + new counter.
- `rust/object-cache-srv/tests/telemetry_tests.rs` — new regression test + import.
- `mkdocs/docs/admin/object-cache.md` — new metric doc row.

## Trade-offs

- **Wrapper/inner split vs. inlining the `imetric!` into the existing single
  function.** The wrapper approach is chosen to match the two sibling handlers
  exactly (open/closed: the counting concern wraps the handler rather than
  threading a status variable through every early `return`), and — like the GET
  fix — it guarantees *every* outcome including the error `return`s is counted
  once, which an inlined emit before each `return` is easy to get wrong. The DRY
  cost (a second wrapper) is already the established idiom here.
- **Reusing `status_label` unchanged.** It already covers the HEAD status set,
  and its deliberately-bounded closed set keeps the `status` tag cardinality
  within the tagged-metric contract. No reason to touch it.

## Documentation

- `mkdocs/docs/admin/object-cache.md`: add an `object_cache_head_requests`
  (`status, prefix`) row to the metrics table, worded to match the existing
  GET/ranges rows — e.g. "Every `HEAD /obj/{key}` outcome — success and failure
  alike. Slice by `status` for the error-rate breakdown." This is the only
  user-facing doc that enumerates these counters.
- `CHANGELOG.md`: the `pr` skill adds the changelog entry at finalization; no
  manual edit needed here.

## Testing Strategy

Add `head_handler_counts_every_outcome` to `telemetry_tests.rs`, mirroring the
existing `get_range_handler_counts_every_outcome`, reusing the shared
`tagged_status_prefix_pairs(&guard.sink, "object_cache_head_requests")` helper.
Drive each reachable HEAD outcome and assert one tagged fire each:

- **200**: `HEAD obj/a` on an existing key → `Ok`, status `200`.
- **400**: an invalid key that fails `validate_key` (a key outside the allowed
  `obj` prefix, e.g. `bad/x`) → `Err(BAD_REQUEST)`.
- **404**: `HEAD obj/missing` on an absent key → `Err(NOT_FOUND)`.

Assert exactly three fires with statuses `{200, 400, 404}` and, as in the GET
test, that every `prefix` tag is `"other"` (this test's cache applies no
prefix labels). `500` is an origin/IO fault not reproducible against the
in-memory store, so — like the GET/ranges tests, which also omit `500` — it is
not exercised; `status_label` already covers it.

Run:

- `cargo test -p micromegas-object-cache-srv -- --nocapture` (the new test plus
  the existing telemetry suite).
- `cargo fmt` and `cargo clippy --workspace -- -D warnings`.

## Open Questions

None.
