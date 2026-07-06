# Foyer 0.22 Upgrade + Disk-Cache Write Knobs + Write-Path Telemetry Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1228

## Overview

The object-cache service builds its foyer `HybridCache` with all disk-engine defaults, so prefetch
bursts overflow foyer's submit queue and drop entries with the recurring
`submit queue overflow, new entry ignored` WARN (~11/s sustained in production). This plan:

1. **Upgrades foyer `0.14` → `0.22.3`** (a prerequisite the user requested). 0.22 is a major API
   overhaul that rewrites the disk-cache construction path and, importantly, (a) *fixes* the
   `with_submit_queue_size_threshold` setter that was buggy in 0.14, and (b) makes the disk
   `Statistics` publicly accessible via `HybridCache::statistics()` — which is exactly the write-path
   telemetry the issue asks for.
2. **Exposes the write knobs** — `flushers` and the write-buffer pool size — as env vars / CLI args
   on the object-cache service, threaded into `FoyerBackend` and applied via `BlockEngineConfig`,
   with deployment-tuned defaults (`flushers=2`, `write buffer=128 MiB`, submit-queue threshold set
   explicitly to `2×` the buffer ≈ 256 MiB).
3. **Fixes the two telemetry gaps** the issue calls out. `object_cache_ssd_write_bytes_per_sec` reads
   0 (sysinfo can't see the cache device inside the deployed container), and foyer's own write-path
   stats aren't exported. Both are resolved by reading foyer 0.22's public `Statistics`
   (`disk_write_bytes` / `disk_read_bytes` / `disk_write_ios` / `disk_read_ios`, cumulative,
   measured by the cache engine itself) and emitting per-second gauges — retiring the broken sysinfo
   disk gauges.

foyer is used in exactly one file in the repo (`rust/object-cache/src/foyer_backend.rs`), so the
upgrade's blast radius is contained. The work is dogfooded through the standard micromegas tracing
sink and follows the saturation-gauge pattern from the completed `object_cache_perf_telemetry_plan.md`
(#1206).

## Current State

### foyer version & usage

`rust/Cargo.toml:52` pins `foyer = "0.14"`. The only Rust file that imports foyer is
`rust/object-cache/src/foyer_backend.rs`:
```rust
use foyer::{CacheHint, DirectFsDeviceOptions, Engine, HybridCache, HybridCacheBuilder, LruConfig};
```
`object-cache/Cargo.toml` gates it behind a `foyer` feature; `object-cache-srv` enables that feature.
No other crate references foyer types.

### Foyer backend construction — all disk-engine defaults (0.14)

`foyer_backend.rs:14-40` builds the cache with no submit-queue/flusher tuning:
```rust
HybridCacheBuilder::new()
    .memory(ram_bytes)
    .with_weighter(|_key, value: &Bytes| value.len())
    .with_shards(shards)
    .with_eviction_config(LruConfig::default())
    .storage(Engine::Large)
    .with_device_options(DirectFsDeviceOptions::new(dir).with_capacity(disk_bytes))
    .build().await?
```
So foyer's defaults apply: `flushers=1`, `buffer_pool_size=16 MiB`, submit-queue threshold
`= buffer_pool_size * 2 = 32 MiB`. Once >32 MiB of writes are in flight, foyer drops the new entry —
no crash or data loss, but hit rate falls and origin traffic rises. The overflow driver is the
**prefetch** path (`foyer_backend.rs:80`), which force-inserts every block via
`storage_writer(key).force().insert(value)`; `.force()` bypasses the admission picker, so the WARN's
first hint ("set a rate limiter as the admission picker") does not apply. The levers are scaling out
flushers and enlarging the buffer pool / submit-queue threshold.

Two constructors: `FoyerBackend::new(dir, ram_bytes, disk_bytes)` (`:15`, delegates with `shards=8`)
and `new_with_shards(dir, ram_bytes, disk_bytes, shards)` (`:19`). The server calls `new` at
`object_cache_srv.rs:144`; tests call both (`object-cache/tests/foyer_backend_tests.rs`).

The demand put path uses `insert_with_hint(key, value, CacheHint::Normal)` (`:96`); the prefetch put
uses `storage_writer(key).force().insert(value)` (`:80`). `ram_usage()` reads
`self.cache.memory().usage()` (`:52`); `close()` calls `self.cache.close()` (`:43`).

### CLI / env-var surface

`rust/object-cache-srv/src/cli.rs` defines all `MICROMEGAS_OBJECT_CACHE_*` knobs as clap fields with
`env`/defaults; `object_cache_srv.rs:38-102` validates each numeric knob at startup with a fatal
`anyhow!` on zero/invalid (e.g. `prefetch_worker_concurrency == 0`). No flusher/buffer knob exists.

### Saturation monitor — the broken SSD gauge

`rust/object-cache-srv/src/saturation_monitor.rs` runs a 5s sampler (`spawn_saturation_monitor`,
wired at `object_cache_srv.rs:186`) emitting fetch-budget, in-flight, memory-budget, prefetch-queue,
NIC, and SSD gauges. The SSD gauges (`object_cache_ssd_read_bytes_per_sec` / `_write_bytes_per_sec`,
`:112-130`) come from `sysinfo::Disks::...usage()` and read **0** on every sample in the deployed
container (the device/mount isn't enumerated there), so they are useless in production. NIC gauges
work and are kept. `sample_once` currently takes `&mut Disks`; `spawn_saturation_monitor` owns the
`Disks` handle across iterations.

## foyer 0.14 → 0.22.3 API migration (verified against the 0.22.3 sources)

Verified against the vendored `foyer-0.22.3` / `foyer-storage-0.22.3` / `foyer-memory-0.22.3` crates.

### The disk engine model changed (`Engine::Large` → Block engine + explicit `Device`)

- `.storage(Engine::Large).with_device_options(DirectFsDeviceOptions::new(dir).with_capacity(cap))`
  is replaced by building a device, then `.storage().with_engine_config(BlockEngineConfig::new(device)…)`.
- Device: `FsDeviceBuilder::new(dir).with_capacity(cap).build() -> Result<Arc<dyn Device>>`
  (`foyer-storage-0.22.3/src/io/device/fs.rs`). Direct I/O (the old `DirectFs` behavior) is now
  `FsDeviceBuilder::…with_direct(true)` (Linux-only flag). The device's `Statistics` object is
  created inside `build()` and threaded up to the cache.
- `BlockEngineConfig::new(device)` (`engine/block/engine.rs:140`) exposes
  `.with_flushers(usize)` (default `1`), `.with_buffer_pool_size(bytes)` (default `16 MiB`), and
  `.with_submit_queue_size_threshold(bytes)`.
- **The 0.14 `with_submit_queue_size_threshold` bug is fixed**: in 0.22.3 the setter assigns
  `self.submit_queue_size_threshold` correctly (`engine.rs:256-257`), so the threshold is directly
  configurable.
- **Watch the 0.22 default threshold**: `BlockEngineConfig::new` sets `submit_queue_size_threshold =
  16 MiB` (`engine.rs:151`) — equal to `buffer_pool_size`, *not* `2×` (the `*2` only appears in the
  doc comment and internal test fixtures). So relying on the default gives an even tighter ceiling
  than 0.14. This plan therefore always sets the threshold explicitly.

### Put / insert path changed

- `insert_with_hint(key, value, CacheHint::Normal)` is **removed**. The `Normal` case is just
  `insert(key, value)` (`hybrid/cache.rs:519`); for a non-default hint use
  `insert_with_properties(key, value, HybridCacheProperties::new().with_hint(Hint::…))`. `CacheHint`
  no longer exists; the hint enum is `foyer::Hint` and cache-level props are `HybridCacheProperties`.
- The prefetch path `storage_writer(key).force().insert(value)` **survives unchanged**:
  `HybridCacheStorageWriter::force()` (`hybrid/writer.rs:126`) and `.insert() -> Option<…>` (`:155`)
  still exist with the same semantics.

### Stats / accessors changed (this is the telemetry win)

- `HybridCache::stats() -> Arc<DeviceStats>` is replaced by
  `HybridCache::statistics() -> &Arc<Statistics>` (`hybrid/cache.rs:629`).
- `Statistics` (`io/device/statistics.rs`) is **public** and exposes cumulative counters:
  `disk_write_bytes()`, `disk_read_bytes()`, `disk_write_ios()`, `disk_read_ios()` (plus throttle
  state: `is_write_throttled()` / `write_throttle()` etc.). Note there is **no** `flush_ios` in 0.22
  (that field was on 0.14's `DeviceStats`).
- Submit-queue occupancy (`engine/block/flusher.rs:128`, `submit_queue_size: Arc<AtomicUsize>`)
  remains an **internal** field with no public accessor — still not exportable as a gauge.
- Unchanged: `memory().usage()` (`foyer-memory-0.22.3/src/cache.rs:832`), `close()`, `obtain`/`get`,
  `LruConfig`, `with_weighter`, `with_shards`, `with_eviction_config`.

### New import set for `foyer_backend.rs`
```rust
use foyer::{
    BlockEngineConfig, DeviceBuilder, FsDeviceBuilder, HybridCache, HybridCacheBuilder, LruConfig,
};
```
(`HybridCacheProperties` / `Hint` only if a non-default hint is needed — the demand path uses plain
`insert`, so likely neither is required.)

### Open verification points for the implementer (compiler-checkable)

- Whether `.storage().with_engine_config(...)` builds without an explicit `.with_io_engine_config(...)`.
  0.22 introduces an IO-engine layer (`PsyncIoEngineConfig` / `UringIoEngineConfig`); confirm a
  default is applied, else add the platform-appropriate config (psync is the portable default).
- Whether preserving direct I/O (`with_direct(true)` on Linux) is desired vs. foyer's buffered
  default. Recommend preserving direct I/O to match today's behavior; verify alignment constraints
  don't reject the configured `disk_bytes`/block size.
- foyer 0.22's MSRV / edition vs. the workspace toolchain — confirm `cargo build` succeeds after the
  bump and `Cargo.lock` updates cleanly.

## Design

### Part 0 — Upgrade foyer to 0.22.3

- `rust/Cargo.toml`: `foyer = "0.22"` (keep alphabetical order). Run `cargo update -p foyer` (and let
  the transitive `foyer-*` subcrates resolve to `0.22.3`); commit the `Cargo.lock` change.
- Rewrite `foyer_backend.rs` construction and put/stats calls per the migration above. This is a
  behavior-preserving port *plus* the new tuning hooks from Part 1 and the stats accessor from Part 2.

### Part 1 — Configurable write knobs

**New CLI/env knobs** (`cli.rs`), following the `MICROMEGAS_OBJECT_CACHE_*` convention and the
issue's suggested defaults:

| Field | Env var | Default | Meaning |
|---|---|---|---|
| `flushers: usize` | `MICROMEGAS_OBJECT_CACHE_FLUSHERS` | `2` | foyer flusher count (≈1 per vCPU on the 2-vCPU target box) |
| `write_buffer_mb: usize` | `MICROMEGAS_OBJECT_CACHE_WRITE_BUFFER_MB` | `128` | foyer `buffer_pool_size` in MiB; the submit-queue threshold is set to `2×` this (≈256 MiB) |

Rationale (kept as a code/doc comment): foyer splits the pool as `buffer_pool_size / flushers`, and
the device block size is 16 MiB — 128 MiB / 2 flushers gives each flusher an 8-block buffer. ~128 MiB
of extra write-buffer RAM is comfortable next to the multi-GB RAM tier. Safe for smaller deployments;
operators override via the env vars.

The submit-queue threshold is derived in code as `2 × buffer_pool_bytes` and set **explicitly** via
`with_submit_queue_size_threshold` (necessary now that the setter works and the 0.22 default is only
`1×`). Exposing a separate `MICROMEGAS_OBJECT_CACHE_SUBMIT_QUEUE_MB` override is deferred (see
Open Questions) to keep the operator surface to the two knobs the issue asked for.

**Startup validation** (`object_cache_srv.rs`, alongside the existing guards): both `> 0`, each with a
fatal `anyhow!` naming the env var — mirroring `prefetch_worker_concurrency == 0` etc.

**Threading into `FoyerBackend`.** To avoid growing the constructor's positional list (and churning
test call sites with easily-transposed `usize`s), introduce a small tuning struct in
`foyer_backend.rs`:
```rust
/// foyer disk-engine write-path tuning. Defaults reproduce foyer's
/// behavior (with the submit-queue threshold pinned to 2× the buffer pool,
/// which foyer documents as its intended default but no longer applies
/// automatically) so existing callers/tests are unaffected unless they opt in.
#[derive(Clone, Copy, Debug)]
pub struct WriteTuning {
    /// `BlockEngineConfig::with_flushers`.
    pub flushers: usize,
    /// `BlockEngineConfig::with_buffer_pool_size`, in bytes.
    pub buffer_pool_bytes: usize,
    /// `BlockEngineConfig::with_submit_queue_size_threshold`, in bytes.
    pub submit_queue_threshold_bytes: usize,
}

impl Default for WriteTuning {
    fn default() -> Self {
        let buffer = 16 * 1024 * 1024;
        Self { flushers: 1, buffer_pool_bytes: buffer, submit_queue_threshold_bytes: buffer * 2 }
    }
}
```
- `new_with_shards` gains a `tuning: WriteTuning` parameter and applies it via `BlockEngineConfig`.
- `new(dir, ram, disk)` keeps its signature and passes `WriteTuning::default()`. Existing
  `new_with_shards` test call sites pass `WriteTuning::default()`.
- The server builds `WriteTuning { flushers: args.flushers, buffer_pool_bytes: args.write_buffer_mb *
  1024 * 1024, submit_queue_threshold_bytes: args.write_buffer_mb * 1024 * 1024 * 2 }` and calls
  `FoyerBackend::new_with_shards(&args.disk_path, ram, disk, 8, tuning)` (explicit `shards=8`, so no
  third near-duplicate constructor — keeps the surface minimal per DRY).

### Part 2 — Write-path telemetry from foyer `Statistics`

**Expose backend disk stats without leaking foyer types into the trait.** `RangeCacheBackend`
(`object-cache/src/backend.rs`) is compiled without the `foyer` feature (e.g. for `MemoryBackend`),
so it must not reference foyer's `Statistics`. Add a plain, foyer-independent snapshot + a defaulted
trait method:
```rust
// backend.rs
/// Point-in-time disk write-path counters (cumulative since process start).
/// foyer-independent so the trait stays buildable without the `foyer`
/// feature; `None` for backends with no disk tier (e.g. in-memory).
#[derive(Clone, Copy, Debug, Default)]
pub struct BackendDiskStats {
    pub write_bytes: u64,
    pub read_bytes: u64,
    pub write_ios: u64,
    pub read_ios: u64,
}

// in trait RangeCacheBackend:
fn disk_stats(&self) -> Option<BackendDiskStats> { None }
```
- `FoyerBackend` overrides it (only compiled with the `foyer` feature) from `self.cache.statistics()`:
  `disk_write_bytes()` / `disk_read_bytes()` / `disk_write_ios()` / `disk_read_ios()` → `u64`.
- `MemoryBackend` uses the default (`None`).
- `RangeCache` gains `pub fn backend_disk_stats(&self) -> Option<BackendDiskStats> {
  self.backend.disk_stats() }`, mirroring the existing `fetch_budget_stats` / `inflight_len`
  accessors (`range_cache.rs:600-612`).

**Emit per-second gauges in the saturation monitor.** The counters are cumulative, so the sampler
deltas against the previous sample. Replace the sysinfo `Disks` handling:
- `spawn_saturation_monitor` drops the `Disks`/`DiskRefreshKind` usage and keeps a
  `prev: Option<BackendDiskStats>` across iterations (seeded `None`; a rate needs two samples).
- `sample_once` takes `&mut Option<BackendDiskStats>` (prev) instead of `&mut Disks`. Each tick it
  calls `cache.backend_disk_stats()`; if both prev and current are present, it emits the deltas /
  `interval_secs`, then stores current as prev:

  | Metric | Unit | Source |
  |---|---|---|
  | `object_cache_foyer_disk_write_bytes_per_sec` | `bytes_per_sec` | Δ`write_bytes` — disk drain throughput (the signal the SSD gauge missed) |
  | `object_cache_foyer_disk_read_bytes_per_sec` | `bytes_per_sec` | Δ`read_bytes` |
  | `object_cache_foyer_disk_write_ios_per_sec` | `bytes_per_sec` (rate via `fmetric!`) | Δ`write_ios` |
  | `object_cache_foyer_disk_read_ios_per_sec` | `bytes_per_sec` | Δ`read_ios` |

  If `backend_disk_stats()` returns `None` (non-foyer backend), emit nothing and leave prev `None`.

**Retire the broken sysinfo SSD gauges.** Remove `object_cache_ssd_read_bytes_per_sec` /
`_write_bytes_per_sec` and the `sysinfo::Disks` / `DiskRefreshKind` usage — they read 0 in production
and are strictly superseded by the foyer-sourced gauges (which measure the cache's own device I/O
accurately). Keep the NIC gauges and the `sysinfo::Networks` handle.

> Scope note (submit-queue depth): foyer 0.22 still does not expose submit-queue occupancy publicly,
> so "how full the submit queue gets" cannot be a gauge without an upstream change. The overflow WARN
> log remains the drop signal, and `write_bytes` + `write_ios` rates indicate whether the flushers
> keep up with write-in pressure. foyer 0.22's throttle state (`Statistics::is_write_throttled`) is a
> possible future signal but is inactive unless a device `Throttle` is configured (we don't), so it's
> out of scope here.

### Wiring diagram
```
cli.rs (flushers, write_buffer_mb)
  -> object_cache_srv.rs (validate >0; WriteTuning{flushers, buffer, threshold=2×buffer})
       -> FoyerBackend::new_with_shards(dir, ram, disk, 8, tuning)
            -> FsDeviceBuilder::new(dir).with_capacity(disk).build()   (Arc<dyn Device>)
            -> HybridCacheBuilder ... .storage()
                 .with_engine_config(BlockEngineConfig::new(device)
                     .with_flushers().with_buffer_pool_size().with_submit_queue_size_threshold())

FoyerBackend::disk_stats() -> HybridCache::statistics() (Statistics) -> BackendDiskStats
  -> RangeCache::backend_disk_stats()
       -> saturation_monitor::sample_once (delta vs prev / interval) -> fmetric! gauges
```

## Implementation Steps

1. **`rust/Cargo.toml`** — bump `foyer = "0.22"`; `cargo update -p foyer`; commit `Cargo.lock`.
2. **`object-cache/src/foyer_backend.rs`** — port construction to the 0.22 Block-engine + device
   builder; port the demand put to `insert` and keep the prefetch `storage_writer().force().insert()`;
   update imports; add `WriteTuning` (+`Default`), add `tuning` param to `new_with_shards`, `new`
   passes `WriteTuning::default()`; set flushers/buffer/threshold via `BlockEngineConfig`; implement
   `disk_stats()` from `self.cache.statistics()`.
3. **`object-cache/src/backend.rs`** — add `BackendDiskStats` + defaulted `disk_stats()` trait method.
4. **`object-cache/src/memory_backend.rs`** — inherits `None` default; confirm it compiles.
5. **`object-cache/src/range_cache.rs`** — add `backend_disk_stats()` accessor.
6. **`object-cache-srv/src/cli.rs`** — add `flushers` (default `2`) and `write_buffer_mb` (default
   `128`) fields with env vars.
7. **`object-cache-srv/src/object_cache_srv.rs`** — validate both `> 0`; build `WriteTuning`; call
   `FoyerBackend::new_with_shards(&args.disk_path, ram, disk, 8, tuning)`.
8. **`object-cache-srv/src/saturation_monitor.rs`** — drop `sysinfo::Disks`/`DiskRefreshKind`; thread
   `prev: Option<BackendDiskStats>`; emit the four `object_cache_foyer_disk_*` gauges from deltas;
   remove the two `object_cache_ssd_*` emissions.
9. **Tests** (see Testing Strategy).
10. **Docs** — update `mkdocs/docs/admin/object-cache.md` (env-var table, CLI flags, Saturation
    metrics table).

## Files to Modify

- `rust/Cargo.toml` (+ `rust/Cargo.lock`)
- `rust/object-cache/src/foyer_backend.rs`
- `rust/object-cache/src/backend.rs`
- `rust/object-cache/src/range_cache.rs`
- `rust/object-cache-srv/src/cli.rs`
- `rust/object-cache-srv/src/object_cache_srv.rs`
- `rust/object-cache-srv/src/saturation_monitor.rs`
- `rust/object-cache/tests/foyer_backend_tests.rs` (extend)
- `rust/object-cache-srv/tests/telemetry_tests.rs` (extend, or a new saturation test)
- `mkdocs/docs/admin/object-cache.md`

## Trade-offs

- **Upgrade to 0.22 now vs. patch on 0.14.** The user asked to upgrade first; independently, 0.22 is
  the right call: it fixes the `with_submit_queue_size_threshold` bug (so the knob works) and makes
  disk `Statistics` public (so the telemetry gap closes cleanly, no `DeviceStats`/internal-field
  plumbing). foyer touches one file, so the risk is contained.
- **Options struct (`WriteTuning`) vs. more positional params.** A `Default`-carrying struct keeps the
  constructor surface small, makes call sites self-documenting, and absorbs future foyer knobs without
  another signature break. Positional params would be terser but transposition-prone and churn tests.
- **Explicitly setting the submit-queue threshold vs. relying on the default.** 0.22's default is `1×`
  the buffer (16 MiB), not the documented `2×`; setting it explicitly makes the ceiling predictable
  and independent of foyer's default drift.
- **foyer `Statistics` vs. fixing sysinfo enumeration.** Fixing sysinfo (enumerate the mount's device,
  or parse `/proc/diskstats`/cgroup io) is fragile, environment-specific, and measures whole-device
  I/O (OS + neighbors). `Statistics` is exact, app-level, and needs no host assumptions — strictly
  better for sizing the write knobs.
- **New metric names (`object_cache_foyer_disk_*`) vs. reusing `object_cache_ssd_*`.** The old names
  imply a host-SSD reading; the source is now the foyer engine. The old gauges emitted 0 in
  production, so nothing useful is lost by renaming to accurate names.
- **Preserve direct I/O.** Keeping `with_direct(true)` on Linux matches today's `DirectFs` behavior;
  buffered I/O would change durability/latency characteristics silently.

## Documentation

`mkdocs/docs/admin/object-cache.md`:
- **Environment variables table** — add `MICROMEGAS_OBJECT_CACHE_FLUSHERS` (default `2`) and
  `MICROMEGAS_OBJECT_CACHE_WRITE_BUFFER_MB` (default `128`; note the submit-queue threshold is `2×`).
- **CLI flags** — mirror the two new flags if that section enumerates them.
- **Saturation metrics table** — replace the `object_cache_ssd_*` row with the four
  `object_cache_foyer_disk_*` gauges, described as the foyer engine's own disk write-path counters
  (drain throughput / IO rate) that supersede the removed host-SSD gauges. Optionally add a short
  "Tuning the write path" note tying the overflow WARN + these gauges to the two new knobs.

## Testing Strategy

- **foyer_backend_tests.rs**: keep the existing disk round-trip green through the 0.22 port; add a
  variant constructing with a non-default `WriteTuning` (e.g. `flushers: 2, buffer: 32 MiB, threshold:
  64 MiB`) and assert the round-trip still works. After `put` + `close`, assert `disk_stats()` is
  `Some` with `write_bytes > 0`.
- **memory_backend**: assert `MemoryBackend::disk_stats()` is `None` (covers the monitor's non-foyer
  branch).
- **saturation monitor**: a focused `#[serial]` test (in `telemetry_tests.rs` or a new
  `saturation_tests.rs`) that drives `sample_once` twice against a `FoyerBackend`-backed `RangeCache`
  with `prev` threaded through, asserting (a) the first tick emits **no** `object_cache_foyer_disk_*`
  metric, and (b) after writes + a second tick, `write_bytes_per_sec` is emitted. Use the in-memory
  sink capture (`init_in_memory_tracing`, `metrics_blocks`) already used in `telemetry_tests.rs`.
  Avoid fixed `sleep`s — use `FoyerBackend::close`/deterministic waits per the note atop
  `foyer_backend_tests.rs`.
- **CLI defaults**: assert `Cli::parse_from([...])` yields `flushers == 2` / `write_buffer_mb == 128`
  when unset, and that startup validation rejects `0` for each.
- **Full gate**: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test` from `rust/`
  (foyer feature enabled for the srv + foyer tests), then `python3 build/rust_ci.py`.

## Open Questions

- **IO-engine config**: does `.storage().with_engine_config(...)` build with foyer's default IO
  engine, or must we pass `.with_io_engine_config(PsyncIoEngineConfig::new())` (portable) / uring on
  Linux? Resolve against the compiler during Part 0; default to psync if an explicit choice is needed.
- **Metric rename vs. compatibility**: this renames the disk gauges to `object_cache_foyer_disk_*` and
  drops `object_cache_ssd_*`. Since the old gauges read 0 in production there is assumed to be no
  dashboard depending on their values. If literal name stability is required, keep the `_ssd_` names
  and merely re-source them from foyer.
- **Separate submit-queue-threshold knob**: the plan derives the threshold as `2× buffer` in code. If
  operators need to decouple it, add a third `MICROMEGAS_OBJECT_CACHE_SUBMIT_QUEUE_MB` (default:
  `2× write_buffer_mb`). Left out for now to match the issue's two-knob request.
