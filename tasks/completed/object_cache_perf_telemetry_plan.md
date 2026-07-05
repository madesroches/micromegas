# Object Cache Performance Telemetry Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1206

## Overview

The object cache (#1188 range-aware read cache, #1203 fetch rework, #1197/#1198 prefetch,
#1218/#1222 streaming) has a solid *counter* layer but no way to locate a bottleneck or tune it:
latency is measured only at the three outer methods, hit-rate is only globally derivable, there are
no saturation signals, and one request counter is undercounted. This plan fills those gaps —
per-stage latency spans, prefix/tier/class dimensions on the hit-rate metrics, saturation gauges,
server/client latency — and fixes two counter-correctness bugs (success-only request counting and a
double-counted size hit) called out in the issue and its audit comment.

The work is dogfooded: every signal is emitted through the standard micromegas tracing sink and is
queryable like any other process telemetry.

## Current State

### Instrumentation that exists today

- **Core counters** (`rust/object-cache/src/range_cache.rs`): `range_cache_block_request`
  (per block probe, `:602`), `range_cache_block_backend_hit` (`:629`),
  `range_cache_origin_block_fetch` / `range_cache_origin_block_bytes` (`:748`–`749`),
  `range_cache_size_backend_hit` (`:515`), `range_cache_origin_head` (`:535`),
  `range_cache_get_range_error` / `range_cache_get_ranges_error` (`:42`–`45`),
  `range_cache_block_len_mismatch` (`:620`), `range_cache_origin_run_len_mismatch` (`:733`).
- **Backend counter** (`foyer_backend.rs:65`): `range_cache_backend_error`.
- **Server counters** (`object-cache-srv/src/handlers.rs`): `object_cache_get_requests` (`:287`),
  `object_cache_get_bytes_served` (`:295`), `object_cache_ranges_requests` / `_ranges_count` /
  `_ranges_bytes_served` (`:450`–`458`), plus prefetch counters (`:532`, `:589`–`594`).
- **Client counters** (`client.rs`): `range_cache_client_fallback` (7 sites),
  `range_cache_client_prefetch_error` (`:217`).
- **Spans**: `#[span_fn]` on `RangeCache::size` / `stream_ranges` / `get_range` / `get_ranges`
  (`range_cache.rs:505`, `:844`, `:934`, `:947`) — top-level latency only.

### Facilities to build on

- **Tagged metrics** already exist: `imetric!(name, unit, properties, value)` /
  `fmetric!(...)` (`tracing/src/macros.rs:176`, `:217`) route to
  `dispatch::tagged_integer_metric` / `tagged_float_metric`, taking a
  `&'static PropertySet` (`tracing/src/property_set.rs`). `PropertySet::find_or_create(vec![
  Property::new(name, value)])` interns and returns `&'static Self`. **Constraint**: property
  names *and values* must be `&'static str`, and "the user is expected to manage the cardinality"
  (`property_set.rs:2`). A 4-arg tagged metric is already used today —
  `web_ingestion_service.rs:204` emits `imetric!("payload_size_inserted", "bytes",
  PropertySet::find_or_create(vec![Property::new("target", "micromegas::ingestion")]), …)` — so
  this plan follows that precedent for inline `find_or_create` tagging rather than introducing the
  pattern.
- **Async span macros**: `instrument_named!(fut, "name")` / `span_async_named!("name", async {…})`
  (`macros.rs:142`, `:119`) instrument a future — the correct tool inside the detached
  `tokio::spawn` fetch tasks, where thread-local `span_scope!` guards do not apply. `#[span_fn]`
  works on async fns (already applied to the async methods above).
- **Periodic gauge pattern**: `telemetry-sink/src/system_monitor.rs` runs a background thread that
  `refresh`es `sysinfo` and emits `imetric!`/`fmetric!` on an interval — the template for the
  saturation sampler. `sysinfo` also exposes `Networks` (NIC bytes) and disk IO.
- **Semaphore introspection**: `tokio::sync::Semaphore::available_permits()`; `mpsc::Sender`
  exposes `capacity()` / `max_capacity()` (queue depth = `max_capacity - capacity`).
- **Test capture**: `tracing::test_utils::init_in_memory_tracing()` returns a guard whose
  `sink.state.lock().unwrap().metrics_blocks` / `sink.total_metrics_events()` let a `#[serial]` unit
  test assert exactly which metrics fired (`tracing/src/event/in_memory_sink.rs`).

### The two correctness bugs

1. **Success-only request counting.** `object_cache_get_requests` is emitted at `handlers.rs:287`,
   only after the commit-before-stream first chunk succeeds — so every `400`/`404`/`416`/`500`
   GET (`get_range_handler` has no request-body extractor, so it can never produce `413`) is
   uncounted, undercounting both request rate and (by omission) error rate. `/ranges` emits
   `object_cache_ranges_requests` at `:450`, likewise only on the success path.
2. **Double `size()` resolution double-counts the size hit-rate.** `get_range_handler` resolves size
   for range validation (`handlers.rs:182`), then calls `stream_ranges` (`:256`), which resolves
   size *again* via `self.size()` (`range_cache.rs:851`). On a cache hit the second resolution is a
   backend hit that fires `range_cache_size_backend_hit` a **second** time — so the very size
   hit-rate this issue wants to break down by tier is double-counted on every ranged GET.
   `post_ranges_handler` does not pre-resolve size, but `get_ranges`/`get_range`/`prefetch_ranges`
   share the same internal `self.size()` call.

## Design

### Dimension strategy (bounded cardinality, `'static`-safe)

All new dimensions use small, closed label sets so cardinality stays bounded and every value is a
compile-time `&'static str`:

- **`class`** — `"demand"` | `"prefetch"`. Derived from the existing `Priority` enum, already
  threaded through `fetch_blocks` (`range_cache.rs:576`).
- **`tier`** — `"backend"` | `"origin"`. Locally known at each emission site inside `range_cache`
  (a foyer hit vs an origin GET). The RAM-vs-SSD split *within* foyer is **out of scope** (see
  Trade-offs) — foyer 0.14's `obtain` does not report which tier served a hit, and the clean L1
  split is #1205's job.
- **`status`** — the HTTP status as a static literal (`"200"`, `"206"`, `"400"`, `"404"`,
  `"416"`, `"500"`, `"503"`), via a `status_label(StatusCode) -> &'static str` match with an
  `"other"` fallback. `"413"` is intentionally excluded: axum 0.8's `DefaultBodyLimit` rejects
  oversized `/ranges` bodies with `413` at extraction time, before the handler function body (and
  therefore the in-handler wrapper) ever runs, so that outcome is not observable by this mechanism —
  counting it would require a tower response-observing layer instead, which is out of scope here.
- **`prefix`** — an object-category label (`"blobs"`, `"views"`, … or `"other"`), resolved by
  matching a request key against the server's configured allowed prefixes.

**Prefix classification without hot-path plumbing.** `RangeCache` is a generic component and must
not hard-code the micromegas key taxonomy, and threading a label through every read method would
churn the just-reworked hot path. Instead, inject the classifier at construction:

- Add a `prefix_labels: Arc<[&'static str]>` field to `RangeCache` (built from the server's
  configured `allowed_prefixes`, leaked to `'static` once at startup — bounded, low-cardinality,
  set once). Add a `RangeCache::classify(key) -> &'static str` that longest-prefix-matches the key
  against those labels and returns `"other"` on no match.
- **Precompute the interned `&'static PropertySet` per label at construction**, so the hot per-block
  emission does an array lookup rather than allocating a `Vec` and taking the intern lock on every
  call. Store, alongside the labels, the per-`prefix` set and the per-`(prefix, tier)` /
  `(prefix, class)` sets the emission sites need. A tiny `metric_tags` module in `object-cache`
  owns the `Property`/`PropertySet` construction so the taxonomy lives in one place (DRY).

**Non-breaking injection.** `RangeCache::new` (`range_cache.rs:473`) already takes 8 positional
args and has ~20 existing call sites (16 in `object-cache/tests/range_cache_tests.rs`, 3 in
`object-cache-srv/tests/prefetch_tests.rs`, 1 in `object-cache-srv/tests/memory_budget_tests.rs`).
Rather than adding a 9th constructor parameter and updating all of them, add a builder-style
`RangeCache::with_prefix_labels(self, labels: Arc<[&'static str]>) -> Self` that defaults to empty
`prefix_labels` when unused (every key classifies as `"other"`). `RangeCache::new` itself is
unchanged, so the ~20 existing callers/tests compile without modification; only
`object_cache_srv.rs` opts in. The server constructs the cache at `object_cache_srv.rs:144` with
`RangeCache::new(...)` unchanged, then — *after* the `allowed_prefixes` resolution/leak at
`:159`–`179` — rebinds it with a separate statement (`let cache = cache.with_prefix_labels(leaked_labels);`)
before the cache is moved into `AppState` (`:187`). Applying the setter as a separate statement
placed after the existing resolution is what keeps this reordering-free: the leaked list need not
exist before `:144`, so neither `new` nor the resolution block moves. When `--allow-all-prefixes`
is set, the server skips the rebind (or passes an empty list) and every key classifies as `"other"`.

**Classify once per `fetch_blocks` call, not per block probe.** `classify(key)` does a
longest-prefix string match; `fetch_blocks` probes multiple blocks per call (`range_cache.rs:602`).
Resolve `&'static PropertySet` via `classify(key)` once at `fetch_blocks` entry and reuse that
reference across all of that call's block probes — recomputing it per probe would reintroduce the
per-block cost the precomputed-`PropertySet` design is meant to avoid.

### 1. Correctness fixes (`handlers.rs`, `range_cache.rs`)

**Count all outcomes with a `status` dimension.** Wrap each data handler body so the request counter
is emitted exactly once, on every return path the handler body can take, tagged with the final
status — DRY and impossible to miss an arm. This covers every status the handler body itself can
produce (`get_range_handler`: `200`/`206`/`400`/`404`/`416`/`500`; `post_ranges_handler`:
`200`/`400`/`404`/`416`/`500`); it does not cover extractor-level rejections that short-circuit
before the handler body runs (e.g. axum's `DefaultBodyLimit` `413` on `/ranges`), since the wrapper
only runs after all extractors succeed — see the `status` dimension note above. Concretely, split
each handler into an inner
`*_inner(...) -> Result<Response, StatusCode>` and a thin public wrapper that runs the inner, derives
the status (`Ok(resp) => resp.status()`, `Err(code) => code`), emits
`imetric!("object_cache_get_requests", "count", tags(status), 1)` (resp.
`object_cache_ranges_requests`), and returns. `object_cache_ranges_count` /
`_bytes_served` stay where they are (they are meaningful only on the success path). This removes the
success-only bias and adds the status breakdown in one place. **Phased rollout**: the wrapper ships in
Phase 1 tagged with `status` only, so it is independently shippable — a `prefix` tag needs
`&'static str` labels, and the only source of those (`metric_tags.rs`, `RangeCache::classify`,
`prefix_labels`) is Phase 2. Phase 2 adds the `prefix` tag to these same two counters once the
classifier lands.

`post_ranges_handler`'s empty-ranges short-circuit (`handlers.rs:379`–`389`) already emits
`object_cache_ranges_requests` itself before returning, in addition to `_count` / `_bytes_served`
(`:380`–`382`). Once the wrapper becomes the sole emitter of `object_cache_ranges_requests`, that
inner emission at `:380` must be dropped — leaving `_ranges_count` / `_ranges_bytes_served` in place
at `:381`–`382` — so a `{"ranges":[]}` request isn't double-counted.

**Remove the double size resolution.** Add size-carrying variants so the handler's already-resolved
size is reused instead of re-resolved:

```
// range_cache.rs
pub async fn stream_ranges_with_size(&self, key, ranges, file_size, caller) -> Result<impl Stream…>
pub async fn get_range_with_size(&self, key, file_size, range) -> Result<Bytes>
pub async fn get_ranges_with_size(&self, key, file_size, ranges) -> Result<Vec<Bytes>>
```

Refactor the current `stream_ranges` so the public no-size entry point resolves size via
`self.size()` and delegates to a private `stream_ranges_inner(key, ranges, file_size, caller)` that
takes size as a parameter and does the out-of-bounds validation + streaming. The `_with_size`
variants call the inner directly, skipping the second `self.size()`. `get_range_handler` calls
`get_range_with_size` / `stream_ranges_with_size` with the size it already resolved at `:182`, so
`range_cache_size_backend_hit` fires exactly once per ranged GET. `prefetch_ranges` (`:1004`, which
resolves size once directly and does not go through `stream_ranges`) and `post_ranges_handler`
(no pre-resolution to reuse) are left unchanged. Existing `get_range`/`get_ranges` keep their
current signatures (they still resolve size once) for callers/tests that don't have a size in hand.

### 2. Per-stage latency (`range_cache.rs`)

Wrap the individual awaits in the detached fetch tasks with `instrument_named!`, and additionally
emit explicit duration `fmetric!`s (ms) for the headline scalars the issue calls out so they are
trivially aggregatable and dimensionable by `class`:

| Stage | Where | Instrument |
|---|---|---|
| Origin block GET | run task, `origin.get_range(...)` `:714` | span `range_cache_origin_get` + `fmetric!("range_cache_origin_get_ms","ms",tags(class),elapsed)` |
| Origin head | `origin.head(...)` `:536` | span `range_cache_origin_head_latency` |
| Backend probe read | probe fut `:603`, `backend.get(...)` | span `range_cache_backend_read` (bounded by `BACKEND_PROBE_CONCURRENCY`, so keep it a span only — no per-probe fmetric, that path is the hottest) |
| **Fetch permit/queue wait** | `acquire_run_permit(...)` `:710` | time the await; `fmetric!("range_cache_fetch_permit_wait_ms","ms",tags(class),elapsed)` + span `range_cache_fetch_permit_wait`. Highest value for the #1203 scheduler. |

`elapsed` uses `std::time::Instant` (runtime `Instant` is fine — only workflow scripts forbid the
clock). `class` is the run's effective priority (`Demand` if any entry is demand, else `Prefetch`),
computed the same way `acquire_run_permit` does.

### 3. Tiered / prefix hit-rate (`range_cache.rs`)

Add `prefix` (and where locally known, `tier` / `class`) dimensions to the hit-rate metrics using
the precomputed `&'static PropertySet`s:

- `range_cache_block_request` → `+ prefix`.
- `range_cache_block_backend_hit` → `+ prefix` (this is `tier="backend"`).
- `range_cache_origin_block_fetch` / `_bytes` → `+ prefix + class` (this is `tier="origin"`).
- `range_cache_size_backend_hit` → `+ prefix`; `range_cache_origin_head` → `+ prefix`.

The existing metric **names are preserved** so the documented hit-rate formula
(`1 - origin_block_fetch / block_request`) keeps working; the dimensions just let it be sliced by
prefix and (for the miss side) demand/prefetch. Hit rate can then be computed per prefix, and the
tier breakdown is `backend_hit` vs `origin_block_fetch` per prefix.

### 4. Demand vs prefetch split (`range_cache.rs`)

The `class` dimension added in §2/§3 to `range_cache_origin_block_fetch` / `_bytes`,
`range_cache_origin_get_ms`, and `range_cache_fetch_permit_wait_ms` *is* the demand/prefetch split —
`Priority` is already available at every one of those sites. This makes "demand was not starved by
prefetch" provable: compare demand-class permit-wait against prefetch-class.

### 5. Saturation gauges (`object-cache-srv`)

Add a periodic sampler modeled on `system_monitor.rs`: a background task
(`object-cache-srv/src/saturation_monitor.rs`) that wakes every `SAMPLE_INTERVAL` (e.g. 5s) and
emits gauges. Give it read-only access to the state it samples via new accessors (open/closed —
add getters, don't expose internals):

- **Fetch budget** — `tokio::sync::Semaphore` has no capacity accessor, so `FetchScheduler`
  (`range_cache.rs:185`–`194`), which today stores only the two `Semaphore`s, must also keep the
  configured `shared_total` / `prefetch_total` as fields set at construction. Add
  `RangeCache::fetch_budget_stats()` delegating to `FetchScheduler`: returns
  `(shared_available, shared_total, prefetch_available, prefetch_total)` from
  `Semaphore::available_permits()` plus those stored totals. Emit
  `object_cache_fetch_shared_occupancy` / `object_cache_fetch_prefetch_occupancy` (occupied =
  total − available) and their available counterparts.
- **In-flight entries** — `FetchScheduler::inflight_len()` (map size under its lock) →
  `object_cache_inflight_entries`. Key signal for #1203.
- **Mem budget** — `AppState.mem_permits.available_permits()` and `memory_budget_mb` →
  `object_cache_mem_budget_occupancy_mb` / `_available_mb`.
- **Prefetch queue depth** — `prefetch_tx.max_capacity() - prefetch_tx.capacity()` →
  `object_cache_prefetch_queue_depth`.
- **NIC** — `sysinfo::Networks`, delta bytes since last sample / interval →
  `object_cache_nic_rx_bytes_per_sec` / `_tx_bytes_per_sec`. The NIC is the expected ceiling on the
  target im4gn.large (#1197) and is currently unmeasured.
- **SSD bandwidth** — `sysinfo = "0.37"` (manifest, `rust/Cargo.toml:84`), resolved to `0.37.2` in
  `Cargo.lock`, with the `disk` feature on by default, which exposes per-device IO via `Disk::usage()` →
  `DiskUsage { read_bytes, written_bytes, … }` (reads `/proc/diskstats` on Linux, per-device, delta
  since last refresh). Use `Disk::usage()` with io-usage refresh enabled (it is disabled under
  `DiskRefreshKind::nothing()`, so the sampler must request it explicitly) to emit
  `object_cache_ssd_read_bytes_per_sec` / `_write_bytes_per_sec`.

The sampler is spawned from `main` (`object_cache_srv.rs`) after `AppState` is built, holding a
clone of the `RangeCache` and the `mem_permits` / `prefetch_tx` handles.

**Mem-permit wait** (issue-comment #3) — time the `acquire_many_owned` await in both handlers
(`handlers.rs:249`, `:405`) and emit `fmetric!("object_cache_mem_permit_wait_ms","ms",elapsed)` so
head-of-line blocking on the memory budget is visible, complementing the occupancy gauge above.

### 6. Server-side spans + TTFB (`handlers.rs`)

- Add `#[span_fn]` to `head_handler`, `get_range_handler`, `post_ranges_handler`,
  `prefetch_handler` (or the inner fns from §1) for request-handling latency.
- **TTFB**: the handlers already use commit-before-stream (await the first chunk before building the
  response, `:278` / `:441`). Record `Instant` at handler entry and emit
  `fmetric!("object_cache_ttfb_ms","ms",tags(prefix),elapsed)` at that first-chunk point — the time
  to first byte now that streaming (#1189/#1222) has landed.

### 7. Client cache-vs-direct latency (`client.rs`)

The client only counts fallbacks today. Add latency so "is the cache actually winning end-to-end"
is observable:

- Time the cache round-trip in `get_range_stream` / `get_full_stream` / `get_ranges` up to the first
  usable byte and emit `fmetric!("range_cache_client_roundtrip_ms","ms",elapsed)`.
- Time the direct-store path taken on fallback (`full_stream_with_fallback` `:286`, the `get_ranges`
  fallback arms, and the `get_opts` fallback `:509`) and emit
  `fmetric!("range_cache_client_direct_ms","ms",elapsed)`.

Comparing the two distributions answers whether the cache path beats going straight to the store.

## Implementation Steps

Phased, each independently shippable; ordered by value/risk.

**Phase 1 — correctness fixes (highest value, low risk).**
1. `handlers.rs`: add `status_label` at the handler boundary; split
   `get_range_handler` / `post_ranges_handler` into inner + counter-emitting wrapper; count all
   outcomes with a `status` tag only (no `prefix` yet — the classifier is delivered in Phase 2, so
   this step is independently shippable). Drop the inner's now-redundant
   `object_cache_ranges_requests` emit in the empty-ranges short-circuit (`:380`), keeping its
   `_ranges_count` / `_ranges_bytes_served` emits, so the wrapper is the sole emitter.
2. `range_cache.rs`: refactor `stream_ranges` into a size-taking inner; add `stream_ranges_with_size`
   / `get_range_with_size` / `get_ranges_with_size`; route `get_range_handler` through the
   size-carrying variant so `range_cache_size_backend_hit` fires once.

**Phase 2 — dimension plumbing.**
3. New `object-cache/src/metric_tags.rs`: `Property`/`PropertySet` builders + `class_label`,
   `tier` constants; the leaked-prefix classifier helper.
4. `RangeCache`: add `prefix_labels` + precomputed `PropertySet`s + `classify()`, plus the
   non-breaking `with_prefix_labels` builder setter (`RangeCache::new` is unchanged); in
   `object_cache_srv.rs`, after the existing `allowed_prefixes` resolution/leak (`:159`–`179`),
   rebind the cache built at `:144` with a separate `let cache = cache.with_prefix_labels(...)`
   statement (before it is moved into `AppState` at `:187`) — no reordering of that resolution is
   needed. Add the `prefix` tag to the Phase 1 wrapper's
   `object_cache_get_requests` / `object_cache_ranges_requests` emission now that `classify()` is
   available.

**Phase 3 — hit-rate dimensions + per-stage latency + class split (§2/§3/§4).**
5. Apply `prefix`/`tier`/`class` tags to the hit-rate counters.
6. Add the origin-GET / head / backend-read / permit-wait spans and the
   `*_ms` duration metrics.

**Phase 4 — saturation (§5).**
7. Add `FetchScheduler::inflight_len` + `RangeCache::fetch_budget_stats`.
8. Add `object-cache-srv/src/saturation_monitor.rs` + spawn from `main`; add mem-permit-wait timing.

**Phase 5 — server + client latency (§6/§7).**
9. `#[span_fn]` on handlers + TTFB metric.
10. Client round-trip vs direct-store duration metrics.

**Phase 6 — docs + tests.**
11. Update `mkdocs/docs/admin/object-cache.md` Monitoring table.
12. Tests (see Testing Strategy).

## Files to Modify

- `rust/object-cache/src/range_cache.rs` — inner refactor, `_with_size` variants, dimensions, spans,
  duration metrics, `classify`/`prefix_labels`/`with_prefix_labels`, `fetch_budget_stats`,
  `inflight_len`.
- `rust/object-cache/src/metric_tags.rs` — **new**: tag/PropertySet builders + classifier.
- `rust/object-cache/src/lib.rs` — `pub mod metric_tags;`.
- `rust/object-cache/src/foyer_backend.rs` — (optional) `tier` context if backend errors get a tag.
- `rust/object-cache-srv/src/handlers.rs` — inner/wrapper split, all-outcome counting, status/prefix
  tags, `#[span_fn]`, TTFB, mem-permit-wait timing.
- `rust/object-cache-srv/Cargo.toml` — add `sysinfo.workspace = true` (alphabetically ordered),
  needed by `saturation_monitor.rs`'s `Networks`/`Disks` use.
- `rust/object-cache-srv/src/saturation_monitor.rs` — **new**: periodic gauge sampler.
- `rust/object-cache-srv/src/lib.rs` — `pub mod saturation_monitor;`.
- `rust/object-cache-srv/src/object_cache_srv.rs` — move the `allowed_prefixes` resolution/leak
  above the `RangeCache::new` call and pass the leaked prefixes into it; spawn the saturation
  monitor.
- `rust/object-cache-srv/src/app_state.rs` — expose handles the monitor needs (already holds
  `mem_permits`, `prefetch_tx`, `cache`).
- `mkdocs/docs/admin/object-cache.md` — Monitoring table.

## Trade-offs

- **RAM-vs-SSD tier split deferred.** foyer 0.14's `HybridCache::obtain` returns a value without
  saying which tier served it, so a true RAM/SSD hit-rate split would need foyer's cumulative
  statistics (diffed per interval) or the dedicated in-process L1 tier of #1205. This plan ships the
  `backend` vs `origin` split now and leaves RAM/SSD to #1205, rather than bolting a fragile
  stats-diff onto foyer.
- **Precomputed PropertySets vs per-call interning.** Tagged metrics intern a `PropertySet` per
  call (a `Vec` alloc + intern lock). On the hottest counter (`range_cache_block_request`, one per
  block probe) that would add real overhead, so the plan precomputes the `&'static PropertySet` per
  label at construction and does an array lookup on the hot path. Cost: a little construction-time
  setup and a fixed per-`RangeCache` table; benefit: hot-path emission stays close to the
  undimensioned dispatch.
- **Spans *and* duration metrics for the headline scalars.** Permit-wait, TTFB, and client
  round-trip get both a named span (queryable in the spans table, correlatable with a trace) and an
  `fmetric!` (trivially aggregatable/alertable, dimensionable by class). The small duplication is
  deliberate for the few highest-value signals; the bulk of stages get a span only.
- **Injected classifier vs threaded label.** Injecting the prefix classifier at `RangeCache`
  construction avoids churning every read-path signature and keeps the micromegas key taxonomy out
  of the generic cache core, at the cost of the cache carrying a small config-derived table.

## Documentation

Update `mkdocs/docs/admin/object-cache.md` **Monitoring** table (`:202`):
- Note the new `status` / `prefix` / `class` / `tier` dimensions on the request and hit-rate
  metrics, and that hit rate can now be sliced by prefix.
- Add rows for the saturation gauges (`object_cache_fetch_*_occupancy`,
  `object_cache_inflight_entries`, `object_cache_mem_budget_*`,
  `object_cache_prefetch_queue_depth`, `object_cache_nic_*`, `object_cache_ssd_*`) with what each
  signals (esp. NIC as the im4gn.large ceiling and fetch/queue depth as the #1203 scheduler signal).
- Add the latency signals (`range_cache_origin_get_ms`, `range_cache_fetch_permit_wait_ms`,
  `object_cache_mem_permit_wait_ms`, `object_cache_ttfb_ms`, `range_cache_client_roundtrip_ms` vs
  `range_cache_client_direct_ms`) and the per-stage spans.
- Add rows for the request counters (`object_cache_get_requests`, `object_cache_ranges_requests`,
  `object_cache_ranges_count`), now `status`/`prefix`-tagged and covering all outcomes, which are
  currently absent from the table entirely.

## Testing Strategy

Unit tests use `micromegas_tracing::test_utils::init_in_memory_tracing()` (mark `#[serial]`) and
assert against `guard.sink` metric blocks.

- **All-outcome counting**: drive `get_range_handler` / `post_ranges_handler` to `404` (missing
  key), `400` (bad range / inverted range), `416` (out of bounds), and `200`/`206` (success); assert
  `object_cache_get_requests` / `object_cache_ranges_requests` fires once per call with the expected
  `status` tag. Regression guard for the success-only bug.
- **No double size-hit**: with a warm size cache, issue one ranged GET through `get_range_handler`
  and assert `range_cache_size_backend_hit` fires exactly once (fails on today's code). Add to
  `object-cache/tests/range_cache_tests.rs` or a new `telemetry_tests.rs`.
- **Prefix classifier**: unit-test `classify` (longest-match, `"other"` fallback, empty/allow-all
  list) in `metric_tags` tests.
- **`_with_size` equivalence**: property-style test that `get_range_with_size(k, size, r)` and
  `get_range(k, r)` return identical bytes for warm and cold keys.
- **Fetch-budget stats**: unit-test `fetch_budget_stats` / `inflight_len` reflect
  acquired/held permits and in-flight entries under a controlled fetch.
- **Manual/integration smoke** (`verify` skill / local monolith): start the services
  (`local_test_env/ai_scripts/start_services.py`), issue GETs/`/ranges`/`/prefetch`, and query the
  new metrics via `micromegas-query` to confirm they land with the expected dimensions and that the
  saturation sampler emits on its interval.
- Full `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and the `object-cache` /
  `object-cache-srv` suites (`python3 build/rust_ci.py`).

## Open Questions

- **Sampler interval.** The existing `sysinfo` sampler (`system_monitor.rs:16`) sleeps for
  `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL` (~200ms on Linux, the floor needed for a valid CPU-usage
  delta) — there's no existing 5s cadence to match. 5s is proposed here on telemetry-volume
  grounds instead; a shorter interval gives finer saturation resolution at more telemetry volume.
  Default 5s unless there's a reason to go finer.
