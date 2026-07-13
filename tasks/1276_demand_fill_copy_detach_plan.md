# object-cache Demand-Fill Copy + RAM-Tier Observability Plan

## Overview
On the demand read path, each cached block is stored in the foyer RAM tier as a
`Bytes::slice` **view** into its coalesced origin-GET buffer (up to
`max_coalesced_get_bytes`). foyer's byte weigher counts only the slice length
(`block_size`), but a live slice keeps the **entire parent GET allocation**
alive. When sibling blocks from one coalesced run evict at different rates,
"lone-survivor" slices each pin a full parent buffer, so the RAM tier's real
resident bytes can grow to `max_coalesced_get_bytes / block_size`× its accounted
size while foyer believes it is under budget and never evicts hard enough — a
monotonic RSS creep toward OOM. This plan detaches each demand-admitted block
from its parent buffer with a single copy (making the weigher's accounting
truthful) and exports the accounted RAM-tier usage as a saturation gauge so the
"real footprint > accounted" divergence is observable in telemetry.

## Current State

### The leak
`rust/object-cache/src/range_cache/fetch.rs:403-412` — the success tail of a
coalesced run splits the single origin GET buffer into per-block slices and
puts each into the backend:
```rust
for i in 0..run_len {
    let offset = i as u64 * block_size;
    let local_start = offset as usize;
    let local_end = (offset + block_size).min(data.len() as u64) as usize;
    let chunk = data.slice(local_start..local_end);   // view into the ≤ max_coalesced_get_bytes run buffer
    self.backend
        .put(run.keys[i].clone(), chunk.clone(), hint)
        .await;
    run.entries[i].fulfill(Ok(chunk));
}
```

`rust/object-cache/src/foyer_backend.rs:140-142` — the demand branch stores the
slice verbatim (zero-copy):
```rust
FillHint::Demand => {
    self.cache.insert(key, value);
}
```
The byte weigher at `foyer_backend.rs:71` charges `value.len()` (one
`block_size` block), but the slice holds a strong reference to the whole
coalesced parent buffer, so real RSS can be several× the accounted RAM-tier
bytes. Every demand-admitted block from a multi-block run over-retains until the
*last* surviving sibling of that run is evicted.

The **prefetch** path is already immune: `foyer_backend.rs:128-139` admits
SSD-only via `storage_writer(key).force().insert(value)` and drops the RAM
record immediately, and `join_prefetch` (`fetch.rs:452-461`) drops each entry as
it completes to bound the prefetch peak. Only the demand path retains slices in
the RAM eviction structure.

The zero-copy insert was an intentional optimization; the DEFAULT_MAX_COALESCED
default and `block_size` set the amplification factor. With `block_size = 1 MiB`
and the default `max_coalesced_get_bytes = 8 MiB`, a lone survivor pins up to
8× its accounted weight.

`rust/object-cache/src/bounded_memory_backend.rs:47-52` — `BoundedMemoryBackend`
(the in-process L1 backend used by `l1_store.rs`) has the identical bug: `put`
stores the slice verbatim, and its weigher (`value.len()`, line 29) charges
only the slice length while the LFU eviction structure holds a strong
reference to the same coalesced-GET parent buffer. L1 is enabled by default
(200 MB) and wired into production read paths
(`analytics/src/lakehouse/lakehouse_context.rs:76,94`,
`static_tables_configurator.rs:79`), so this backend needs the same fix.

### The observability gap
`FoyerBackend::ram_usage()` (`foyer_backend.rs:98-100`) returns
`self.cache.memory().usage()` — the accounted RAM-tier bytes — but it is only
consumed by an integration test; it is never sampled into the saturation
monitor. `sample_once` (`object-cache-srv/src/saturation_monitor.rs:40-146`)
emits fetch-budget occupancy, in-flight entries, the **request-window permit
budget** (`object_cache_mem_budget_*`, unrelated to the RAM tier and normally
near-idle), prefetch queue depth, NIC throughput, and foyer disk write-path
rates — but nothing for RAM-tier residency. So a divergence between accounted
RAM-tier usage and actual process memory is currently undiagnosable from the
cache's own telemetry.

Host/process memory *is* already covered: `telemetry-sink`'s
`send_system_metrics_forever` emits `total_memory` / `used_memory` /
`free_memory`, and `object-cache-srv` installs `tikv_jemallocator` as its global
allocator (`object_cache_srv.rs:2-3`). What is missing is the *cache-specific*
counterpart — the accounted RAM-tier gauge — that turns "used_memory is
climbing" into "used_memory is climbing **while** the RAM tier reports it is at
budget," which is the signature of this exact bug.

## Design

### 1. Fix: copy on demand admission
Detach the cached block from its parent buffer in the demand branch of
`FoyerBackend::put`, so a `block_size` block no longer pins a
`max_coalesced_get_bytes` allocation:

```rust
FillHint::Demand => {
    // Copy so the cached block does not retain its whole coalesced-GET
    // parent buffer; otherwise RAM-tier RSS runs up to
    // (max_coalesced_get_bytes / block_size)x its accounted weight while the
    // weigher (value.len()) believes the tier is under budget. One memcpy per
    // admitted block is negligible against the origin GET.
    let owned = Bytes::copy_from_slice(&value);
    self.cache.insert(key, owned);
}
```

**Why the backend, not `fetch.rs`:** the weigher that this copy makes truthful
lives in `foyer_backend.rs`, and the prefetch path's own detachment
(SSD-only + immediate RAM-record drop) is already expressed here — so the
"RAM residency should be truthful" invariant belongs in one place, the backend.
Keeping `fetch.rs` producing zero-copy slices also preserves the cheap transient
`fulfill(Ok(chunk))` hand-off (that value is short-lived and released as soon as
the read completes, so it is not part of the leak). The copy runs once per
freshly-fetched demand block (only from `fulfill_run_success`); backend *hits*
never re-`put`, so there is no per-read copy on the hot cached path.

The copy is unconditional — even a single-block run (where `block_size` == run
size and there is no over-retention) is copied, because `Bytes` exposes no cheap
"do I own my whole allocation?" check and the memcpy is negligible.

The identical copy is needed in `BoundedMemoryBackend::put`
(`rust/object-cache/src/bounded_memory_backend.rs:47-52`), which has no
demand/prefetch distinction (see its comment: L1 has no disk tier to route a
prefetch-only admission through, so both hints take the same path) — every
`put` there is the demand case, so the copy applies unconditionally to the
whole method:
```rust
async fn put(&self, key: String, value: Bytes, _hint: FillHint) {
    let owned = Bytes::copy_from_slice(&value);
    self.cache.insert(key, owned);
}
```
This keeps the "RAM residency should be truthful" invariant in both backends
that carry a byte weigher, matching the rationale above.

### 2. Observability: export accounted RAM-tier usage
Add a RAM-tier usage accessor to the backend abstraction, mirroring the existing
`disk_stats()` shape (defaulted so non-foyer backends need no override), and emit
it as a saturation gauge.

`rust/object-cache/src/backend.rs` — add to the `RangeCacheBackend` trait:
```rust
/// Accounted RAM-tier byte usage, for the saturation monitor's residency
/// gauge. `None` for backends with no RAM tier accounting (e.g.
/// `MemoryBackend`). Divergence between this and process RSS is the
/// signature of a cached value pinning more than its accounted weight.
fn ram_usage_bytes(&self) -> Option<usize> {
    None
}
```

`rust/object-cache/src/foyer_backend.rs` — implement it by reusing the existing
inherent `ram_usage()` (keeps the two DRY and leaves the test-facing inherent
method untouched):
```rust
fn ram_usage_bytes(&self) -> Option<usize> {
    Some(self.ram_usage())
}
```

`rust/object-cache/src/range_cache/mod.rs` — expose it alongside
`backend_disk_stats`:
```rust
/// Accounted RAM-tier usage (`None` for a backend with no RAM tier), for the
/// saturation sampler's residency gauge.
pub fn backend_ram_usage(&self) -> Option<usize> {
    self.backend.ram_usage_bytes()
}
```

`rust/object-cache-srv/src/saturation_monitor.rs` — emit in `sample_once`
(no signature change; `cache` is already a parameter):
```rust
if let Some(ram_bytes) = cache.backend_ram_usage() {
    imetric!("object_cache_ram_tier_usage_bytes", "bytes", ram_bytes as u64);
}
```
Compared against the existing `used_memory` system metric, this gauge staying
at/below the configured `--ram-mb` tier size *while* `used_memory` climbs
confirms the slice-retention class of divergence.

No separate process-RSS gauge is added: host/process memory is already covered
by `send_system_metrics_forever`, and the missing signal was specifically the
accounted RAM-tier side of the comparison.

The gauge rides the existing 5s `SAMPLE_INTERVAL` (`saturation_monitor.rs:27-35`)
rather than a dedicated cadence: that interval is a telemetry-volume tradeoff
shared by every sibling saturation gauge, and the failure mode this gauge
targets is a monotonic creep over minutes-to-hours, which 5s resolves with
large margin. No dedicated alert is added either: a repo-wide search found no
Prometheus/Grafana alerting infrastructure in-repo, so a `used_memory` vs.
`object_cache_ram_tier_usage_bytes` alert has nowhere to live here and stays
out of scope — this plan only exports the signal.

## Implementation Steps
1. **Fix the leak** — `rust/object-cache/src/foyer_backend.rs`: copy the value
   in the `FillHint::Demand` branch of `put` (`Bytes::copy_from_slice(&value)`),
   with the explanatory comment above.
2. **Fix the same leak in L1** — `rust/object-cache/src/bounded_memory_backend.rs`:
   copy the value in `put` before `self.cache.insert`.
3. **Add trait accessor** — `rust/object-cache/src/backend.rs`: add
   `ram_usage_bytes(&self) -> Option<usize>` (default `None`) to
   `RangeCacheBackend`.
4. **Implement for foyer** — `rust/object-cache/src/foyer_backend.rs`: implement
   the trait method returning `Some(self.ram_usage())`.
5. **Expose on RangeCache** — `rust/object-cache/src/range_cache/mod.rs`: add
   `backend_ram_usage() -> Option<usize>` delegating to `self.backend`.
6. **Emit the gauge** — `rust/object-cache-srv/src/saturation_monitor.rs`: emit
   `object_cache_ram_tier_usage_bytes` in `sample_once`; extend the module
   doc-comment's gauge list.
7. **Tests** — add the foyer detachment regression test, the L1
   (`BoundedMemoryBackend`) detachment regression test, and the gauge test
   (below).
8. **Docs** — update `mkdocs/docs/admin/object-cache.md` saturation table.
9. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and the
   object-cache test suites.

## Files to Modify
- `rust/object-cache/src/foyer_backend.rs` — copy on demand admission; trait impl.
- `rust/object-cache/src/bounded_memory_backend.rs` — copy on admission in `put`.
- `rust/object-cache/src/backend.rs` — new trait method.
- `rust/object-cache/src/range_cache/mod.rs` — `backend_ram_usage()` accessor.
- `rust/object-cache-srv/src/saturation_monitor.rs` — new gauge + doc comment.
- `rust/object-cache/tests/foyer_backend_tests.rs` — detachment regression test.
- `rust/object-cache/tests/l1_store_tests.rs` — L1 (`BoundedMemoryBackend`)
  detachment regression test.
- `rust/object-cache-srv/tests/saturation_tests.rs` — RAM-tier gauge test.
- `mkdocs/docs/admin/object-cache.md` — saturation metrics table.

## Trade-offs
- **Copy per demand block vs. zero-copy slice.** The copy costs one memcpy per
  freshly-fetched `block_size` block, negligible against the origin GET that
  produced it, and it makes the weigher's accounting truthful (RAM-tier RSS then
  caps near the configured tier size). Alternative rejected: teaching the weigher
  to charge the parent-buffer size — that would over-charge shared siblings
  (double-counting the same parent across N blocks) and still leave real RSS
  unbounded relative to the tier budget.
- **Copy in backend vs. in `fetch.rs`.** Placing it in the backend keeps the
  "RAM residency truthful" invariant co-located with the weigher and the
  prefetch path's detachment, and leaves the transient fulfill hand-off
  zero-copy. Copying at the slice site in `fetch.rs` is equivalent for the leak
  but spreads the invariant across two crates.
- **RAM-tier gauge only, no new RSS gauge.** Process/host memory is already
  reported; adding the accounted-usage side is the minimum needed to make the
  divergence observable without duplicating an existing signal.

## Documentation
- `mkdocs/docs/admin/object-cache.md` — add `object_cache_ram_tier_usage_bytes`
  to the **Saturation** gauge table (§Monitoring), noting the intended
  comparison against `used_memory` for detecting RAM-tier over-retention.
- Update the `saturation_monitor.rs` module doc comment's enumerated gauge list.

## Testing Strategy
- **Detachment regression** (`foyer_backend_tests.rs`, deterministic, no RSS
  measurement): slice a block out of a larger parent buffer, demand-`put` it,
  `get` it back, and assert the returned bytes no longer share the parent's
  allocation:
  ```rust
  let parent = Bytes::from(vec![7u8; 8192]);
  let block = parent.slice(0..4096);
  let block_ptr = block.as_ptr();          // points into `parent`'s allocation
  backend.put("k".into(), block, FillHint::Demand).await;
  let got = backend.get("k").await.expect("hit");
  assert_eq!(got, vec![7u8; 4096]);
  assert_ne!(got.as_ptr(), block_ptr,
      "demand admission must copy, detaching the cached block from its parent GET buffer");
  ```
  (`Bytes::as_ptr` is public; a clone from the RAM tier preserves the stored
  buffer's base pointer, so this fails before the fix and passes after.)
- **L1 detachment regression** (`l1_store_tests.rs`, same technique against
  `BoundedMemoryBackend` directly): slice a block out of a larger parent
  buffer, `put` it (any `FillHint`, since L1 treats them identically), `get`
  it back, and assert the returned bytes' base pointer differs from the
  slice's — covering the identical bug in the in-process L1 backend.
- **Gauge emission** (`saturation_tests.rs`): drive `sample_once` against a
  `FoyerBackend`-backed `RangeCache`, and assert `object_cache_ram_tier_usage_bytes`
  fires; after a demand `put` of one block, assert the sampled value is ≥ that
  block's size (residency is now visible). `saturation_tests.rs` currently only
  has a float-metric extraction helper (`float_metric_values`); add an
  integer-metric extraction helper following the pattern in
  `telemetry_tests.rs` (`rust/object-cache/tests/telemetry_tests.rs:42-60`,
  `rust/object-cache-srv/tests/telemetry_tests.rs:62`).
- **Existing suites** — the round-trip and prefetch tests in
  `foyer_backend_tests.rs` are unaffected (prefetch path unchanged; demand
  round-trip still returns identical bytes).
- Full gate: `cargo fmt`, `cargo clippy --workspace -- -D warnings`,
  `cargo test -p micromegas-object-cache -p micromegas-object-cache-srv`.
