# Process RSS and jemalloc Stats Telemetry Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1319

## Overview

During a production incident, `object-cache-srv` was hard-killed twice by the OOM killer
in one day; host memory climbed for over an hour before each kill while every existing
metric (RAM-tier usage, budget occupancy, permit occupancy, request rates, NIC/disk
throughput) stayed flat. The gap: all existing memory signals are either host-wide
(`used_memory`/`free_memory` from `sysinfo::System`) or cache-accounted (foyer's own
weigher total) — nothing measures *this process's* actual memory footprint, and nothing
looks inside jemalloc (the configured global allocator on every production service) to
tell a logical leak apart from allocator retention/fragmentation.

This plan adds six gauges — process RSS/virtual size (via `sysinfo`, allocator-agnostic)
and jemalloc's `stats.allocated`/`stats.resident`/`stats.mapped`/`stats.retained` (via
`tikv-jemalloc-ctl`) — to the **shared** `telemetry-sink::system_monitor` sampler that
every service already runs, behind a new Cargo feature so only the 7 binaries that
actually declare jemalloc as their global allocator turn it on. One feature per binary,
no new crate. Together with the existing host-level `used_memory` gauge, this forms the
decision tree the issue describes: host climbing but RSS flat → not this process; RSS
climbing but jemalloc `allocated` flat → non-heap growth; `allocated` climbing → logical
leak; `resident` climbing while `allocated` is flat → allocator fragmentation/retention.

## Current State

- `rust/telemetry-sink/src/system_monitor.rs` — `send_system_metrics_forever` is a
  loop, woken every `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL` (~200ms on Linux — the floor
  needed for a valid CPU-usage delta), emitting unprefixed `used_memory`, `free_memory`,
  `cpu_usage`, and once at startup `total_memory`. `spawn_system_monitor` starts this on
  a background thread.
- `rust/telemetry-sink/src/lib.rs:33` gates the whole module
  `#[cfg(not(target_arch = "wasm32"))]`. `lib.rs:540` calls `spawn_system_monitor()`
  automatically whenever `system_metrics_enabled` is true, which is the default
  (`system_metrics_enabled: true`, `lib.rs:125`) for every binary using
  `#[micromegas_main]` — that's all 7 production services, but also `write-perfetto`,
  `telemetry-generator`, the `public` crate's dev examples, and `micromegas-capi`
  (embedded in the Unreal Engine client via `TelemetryGuardBuilder::default()`, which
  must never link jemalloc — see Design).
- `rust/object-cache-srv/src/object_cache_srv.rs:1-3` and six other production
  binaries (`telemetry-ingestion-srv`, `flight-sql-srv`, `analytics-web-srv`,
  `http-gateway`, `telemetry-maintenance-srv`, `monolith` — this set of 7 was confirmed
  by direct inspection of each crate's source, not solely from
  `tasks/completed/1129_global_allocator_plan.md`, whose own Implementation Steps cover
  only 5 of these and also lists `telemetry-admin-cli`, which no longer declares
  `#[global_allocator]`) each declare
  `#[global_allocator] static ALLOC: tikv_jemallocator::Jemalloc`, gated
  `#[cfg(not(target_os = "windows"))]`, and each has an identical
  `[target.'cfg(not(target_os = "windows"))'.dependencies] tikv-jemallocator.workspace = true`
  block in their own `Cargo.toml`. All 7 depend on the facade crate via
  `micromegas = { workspace = true, features = ["server"] }` — none depend on
  `micromegas-telemetry-sink` directly.
- `write-perfetto` (`rust/examples/write-perfetto/Cargo.toml`) also enables
  `features = ["server"]` on `micromegas`, but is **not** one of the 7 — it declares no
  `#[global_allocator]`. `server` alone can't be the gate for jemalloc stats (see Design).
- `rust/Cargo.toml:90` pins `tikv-jemallocator = "0.7"`, which resolves (per
  `rust/Cargo.lock`) to `tikv-jemalloc-sys 0.7.1`. `tikv-jemalloc-ctl` must resolve to
  the *same* `tikv-jemalloc-sys` major version or Cargo links two independent jemalloc
  builds and `stats::*` silently reads the wrong (inert) one. `tikv-jemalloc-ctl 0.7.0`
  depends on `tikv-jemalloc-sys ^0.7`, so pinning `tikv-jemalloc-ctl = "0.7"` matches.
- `rust/public/Cargo.toml:13-44` already has precedent for a feature enabling another
  crate's feature on an *unconditionally-present* dependency (not just optional ones):
  `server = [..., "micromegas-telemetry/server", ...]` even though
  `micromegas-telemetry.workspace = true` (`:50`) carries no `optional = true`. The same
  mechanism lets `micromegas`'s features toggle a feature on `micromegas-telemetry-sink`
  (`:51`, also unconditional) without making it optional.

## Design

### One feature, one place, one line per binary

Add a `jemalloc` Cargo feature to `micromegas-telemetry-sink` (an *existing* crate — no
new crate). It gates an optional `tikv-jemalloc-ctl` dependency and the code that reads
it. Wire it through the facade:

```toml
# rust/public/Cargo.toml [features]
jemalloc-metrics = ["micromegas-telemetry-sink/jemalloc"]
```

kept separate from `server` (not folded into it), because `server` is also enabled by
`write-perfetto`, which isn't jemalloc-backed. Each of the 7 jemalloc-declaring services
then adds one word to a line they already have:

```toml
micromegas = { workspace = true, features = ["server", "jemalloc-metrics"] }
```

right next to their existing `tikv-jemallocator.workspace = true` line — the same place
#1129 already touches when a binary opts into jemalloc as its allocator. No other file
in any of those 7 crates changes. `write-perfetto`, `telemetry-generator`, the `public`
examples, and `micromegas-capi` are untouched and never link `tikv-jemalloc-ctl`.

### Where the code lives

Both gauges are added to the existing `send_system_metrics_forever` loop in
`system_monitor.rs` — the one sampler every one of these binaries already runs, so
nothing new needs spawning. Two small functions, called from the loop:

**Process memory** (cross-platform, unconditional — `sysinfo` is already a dependency of
this module, no feature/cfg needed beyond the module's existing wasm32 exclusion):

```rust
pub fn emit_process_memory_stats() {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let Ok(pid) = sysinfo::get_current_pid() else {
        return;
    };
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        false,
        ProcessRefreshKind::nothing().with_memory(),
    );
    if let Some(process) = system.process(pid) {
        imetric!("process_resident_bytes", "bytes", process.memory());
        imetric!("process_virtual_bytes", "bytes", process.virtual_memory());
    }
}
```

A fresh `System` per call (rather than a threaded/reused instance) is deliberate: unlike
the CPU-usage delta this loop already computes, a point-in-time RSS/virtual read needs
no prior sample, so there's no state worth carrying and no correctness reason to hold a
long-lived `System`.

This function runs unconditionally for **every** consumer of `spawn_system_monitor`,
including non-jemalloc ones (`write-perfetto`, `capi`, generators) — it's allocator
agnostic and as cheap as the `used_memory`/`free_memory` reads already sitting next to
it, so there is no reason to gate it. Only the jemalloc half needs gating, because only
7 binaries actually use jemalloc as their global allocator, and reading `mallctl` stats
from a non-global jemalloc instance would be either meaningless (the numbers wouldn't
reflect the process's real allocation activity) or, for `micromegas-capi` specifically,
an unwanted forced link of jemalloc into a game engine process.

**jemalloc stats** (feature + platform gated — jemalloc doesn't support Windows, and the
optional dependency itself is target-gated to match, mirroring how `tikv-jemallocator`
is already gated in each service's own `Cargo.toml`):

```rust
#[cfg(all(feature = "jemalloc", not(target_os = "windows")))]
pub fn emit_jemalloc_stats() {
    use tikv_jemalloc_ctl::{epoch, stats};
    // jemalloc caches these counters; advance the epoch to refresh them before
    // reading, per tikv-jemalloc-ctl's documented usage.
    if epoch::advance().is_err() {
        return;
    }
    if let Ok(v) = stats::allocated::read() {
        imetric!("jemalloc_allocated_bytes", "bytes", v as u64);
    }
    if let Ok(v) = stats::resident::read() {
        imetric!("jemalloc_resident_bytes", "bytes", v as u64);
    }
    if let Ok(v) = stats::mapped::read() {
        imetric!("jemalloc_mapped_bytes", "bytes", v as u64);
    }
    if let Ok(v) = stats::retained::read() {
        imetric!("jemalloc_retained_bytes", "bytes", v as u64);
    }
}

#[cfg(not(all(feature = "jemalloc", not(target_os = "windows"))))]
pub fn emit_jemalloc_stats() {}
```

The Windows/feature-off stub keeps the call site itself free of `cfg` — the loop just
calls both functions plainly, same as any other gauge in this file.

The direct `read()` form is used rather than the `mib()`-caching form the crate also
offers: `mib()` amortizes the mallctl string lookup for hot, high-frequency call sites;
at this sampler's cadence (see below) the simpler form is the right level of effort and
avoids threading new cached-handle state through the loop.

### Cadence: don't run these at the 200ms CPU-update floor

`send_system_metrics_forever`'s sleep interval is `MINIMUM_CPU_UPDATE_INTERVAL` (~200ms)
purely because that's the floor `sysinfo` needs for a valid CPU-usage delta —
`used_memory`/`free_memory` just piggyback on it today because a plain memory read is
cheap at any cadence. Adding 6 more gauges *per binary* at 200ms (vs. today's 3) is a
3x volume increase across all 7 services for signals that, per the issue, are about a
climb over an hour-plus timescale — no value is lost sampling them much less often.
`object-cache-srv`'s own `saturation_monitor.rs` already made this same call explicitly
(its doc comment: "5s is a telemetry-volume tradeoff instead" of the CPU-update floor).
Reuse that reasoning here: gate the two new emit calls behind a tick counter so they
fire roughly every 5s instead of every ~200ms, without adding a second thread/timer:

```rust
/// ~5s at the sysinfo CPU-update floor (~200ms) -- matches the cadence
/// object-cache-srv's saturation_monitor.rs already chose for the same
/// telemetry-volume reason; process/jemalloc memory gauges don't need
/// 200ms resolution to catch an hour-plus OOM climb.
const SLOW_SAMPLE_TICKS: u32 = 25;

/// True on every `SLOW_SAMPLE_TICKS`-th tick. Extracted as a pure function (mirroring
/// `saturation_monitor.rs`'s `sample_once` extraction) so the gating decision itself is
/// directly callable from a test, rather than only reachable from inside the infinite
/// loop below.
pub fn should_sample_slow(tick: u32) -> bool {
    tick % SLOW_SAMPLE_TICKS == 0
}

pub fn send_system_metrics_forever() {
    let what_to_refresh = RefreshKind::nothing()
        .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
        .with_memory(MemoryRefreshKind::nothing().with_ram());
    let mut system = System::new_with_specifics(what_to_refresh);
    imetric!("total_memory", "bytes", system.total_memory());
    let mut tick: u32 = 0;
    loop {
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        system.refresh_specifics(what_to_refresh);
        imetric!("used_memory", "bytes", system.used_memory());
        imetric!("free_memory", "bytes", system.free_memory());
        fmetric!("cpu_usage", "percent", system.global_cpu_usage() as f64);

        tick = tick.wrapping_add(1);
        if should_sample_slow(tick) {
            emit_process_memory_stats();
            emit_jemalloc_stats();
        }
    }
}
```

`used_memory`/`free_memory`/`cpu_usage` keep their existing 200ms cadence unchanged —
zero behavior change for anything that exists today.

## Implementation Steps

1. **`rust/Cargo.toml`** — add `tikv-jemalloc-ctl = "0.7"` to `[workspace.dependencies]`,
   alphabetically immediately before the existing `tikv-jemallocator = "0.7"` line.
2. **`rust/telemetry-sink/Cargo.toml`**:
   - Add `[features]` section: `default = []`, `jemalloc = ["dep:tikv-jemalloc-ctl"]`.
   - Add a new target table (native, non-Windows only — the existing
     `cfg(not(target_arch = "wasm32"))` block already covers wasm exclusion for the
     module, but the dependency itself should also be excluded on Windows to mirror
     `tikv-jemallocator`'s own gating):
     ```toml
     [target.'cfg(all(not(target_arch = "wasm32"), not(target_os = "windows")))'.dependencies]
     tikv-jemalloc-ctl = { workspace = true, optional = true, features = ["stats"] }
     ```
     (`stats` gates `tikv-jemalloc-ctl`'s `stats` module — required for the
     `stats::allocated`/`resident`/`mapped`/`retained` reads below; it isn't a default
     feature of the crate.)
   - Add a `[target.'cfg(all(not(target_arch = "wasm32"), not(target_os =
     "windows")))'.dev-dependencies]` table (same cfg as above) with
     `tikv-jemallocator.workspace = true` — needed by `jemalloc_stats_tests.rs` added in
     step 6, which declares `#[global_allocator] static ALLOC: tikv_jemallocator::Jemalloc`
     to make jemalloc the active allocator for that test binary.
   - Add `serial_test = "3.2"` to a plain (non-target-gated) `[dev-dependencies]` table,
     matching the existing pin in `rust/object-cache-srv/Cargo.toml:45` — needed because
     the new tests in step 6 use the global `InMemorySink`/`flush_metrics_buffer()`
     dispatch, which is process-wide state and would race under cargo's default
     parallel-test execution without `#[serial]`.
3. **`rust/telemetry-sink/src/system_monitor.rs`** — add `emit_process_memory_stats`,
   `emit_jemalloc_stats` (+ stub), the `SLOW_SAMPLE_TICKS` tick-gating, and the two new
   call sites in `send_system_metrics_forever`, as shown above.
4. **`rust/public/Cargo.toml`** — add `jemalloc-metrics = ["micromegas-telemetry-sink/jemalloc"]`
   to `[features]`, alongside (not inside) `server`.
5. **Seven service `Cargo.toml`s** — change
   `micromegas = { workspace = true, features = ["server"] }` to
   `features = ["server", "jemalloc-metrics"]` in: `object-cache-srv`, `monolith`,
   `flight-sql-srv`, `telemetry-ingestion-srv`, `telemetry-maintenance-srv`,
   `analytics-web-srv`, `http-gateway`.
6. **`rust/telemetry-sink/tests/`** — add two new test files per Testing Strategy below,
   split by feature gate so `required-features`/file-level `#![cfg(...)]` (which apply to
   an entire `[[test]]` target, not individual `#[test]` functions) never silently gate
   tests the plan says need no feature:
   - `system_monitor_tests.rs` — process-memory and tick-gating tests. No feature
     requirement and no platform gate, so no `[[test]]` entry is needed; Cargo
     auto-discovers it as a test target.
   - `jemalloc_stats_tests.rs` — jemalloc-stats tests only. Add a `[[test]]` entry in
     `telemetry-sink/Cargo.toml` with `required-features = ["jemalloc"]`, matching the
     `required-features = ["server"]` pattern already used by `public/Cargo.toml`'s
     `[[test]]` entries (each single-purpose, never mixing gated and ungated tests — the
     same precedent this split preserves). Since all 7 services request
     `jemalloc-metrics` unconditionally, workspace feature unification enables
     telemetry-sink's `jemalloc` feature — and thus this file's `required-features`
     gate — even on Windows, where `tikv-jemallocator` is absent from the dependency
     graph; start the file with `#![cfg(not(target_os = "windows"))]` so it compiles to
     an empty harness there instead of failing to find the crate. This file's
     `#[global_allocator]` declaration is what the `tikv-jemallocator` dev-dependency
     added in step 2 is for.

   Mark every test in either file that uses `InMemorySink`/`flush_metrics_buffer()` with
   `#[serial_test::serial]`, matching `object-cache-srv/tests/saturation_tests.rs`'s use
   of the `serial_test` dev-dependency added in step 2 — the global tracing dispatch
   these tests observe is process-wide state and would otherwise race under cargo's
   default parallel execution.
7. **`mkdocs/docs/admin/object-cache.md`** — add the six new gauges to the Saturation
   table (see Documentation).
8. **`CHANGELOG.md`** — add an entry under **Observability:** (or **Caching:**, matching
   whichever section fits by the time this lands) in the Unreleased section.
9. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and `cargo test`
   (workspace-wide). Because 7 workspace-member crates request `jemalloc-metrics`
   unconditionally in their own `[dependencies]` (not behind one of their own optional
   features), Cargo's normal feature unification already turns on
   `micromegas-telemetry-sink/jemalloc` for a whole-workspace `cargo test` or
   `cargo clippy --workspace` — no separate `--features jemalloc` invocation is needed
   to get the jemalloc path exercised (a single-package `cargo test -p
   micromegas-telemetry-sink` in isolation would *not* enable it, since that only pulls
   in that one crate's own default features; use the workspace-wide command for real
   coverage). Also run `cargo build -p micromegas-object-cache-srv` (or any one of the
   7) once to confirm the feature threads through the facade end to end.

## Files to Modify

- `rust/Cargo.toml` — new workspace dependency.
- `rust/telemetry-sink/Cargo.toml` — new feature + target-gated optional dependency +
  target-gated `tikv-jemallocator` dev-dependency + new `[[test]]` entry.
- `rust/telemetry-sink/src/system_monitor.rs` — two new emitter functions, tick-gating,
  two new call sites.
- `rust/telemetry-sink/tests/` — two new test files (`system_monitor_tests.rs`,
  `jemalloc_stats_tests.rs`).
- `rust/public/Cargo.toml` — new `jemalloc-metrics` feature.
- `rust/object-cache-srv/Cargo.toml`, `rust/monolith/Cargo.toml`,
  `rust/flight-sql-srv/Cargo.toml`, `rust/telemetry-ingestion-srv/Cargo.toml`,
  `rust/telemetry-maintenance-srv/Cargo.toml`, `rust/analytics-web-srv/Cargo.toml`,
  `rust/http-gateway/Cargo.toml` — one-word feature-list edit each.
- `mkdocs/docs/admin/object-cache.md` — Saturation table rows.
- `CHANGELOG.md` — changelog entry.

## Trade-offs

- **Shared crate + feature vs. per-service duplication (the earlier draft of this
  plan).** An earlier version of this plan scoped everything to
  `object-cache-srv/src/saturation_monitor.rs` alone, to sidestep exactly this
  feature-wiring. Superseded: the whole point is that all 7 jemalloc-declaring
  services should get this for free, and duplicating ~30 lines into 7 separate
  `saturation_monitor.rs`-style files would violate DRY for no benefit once the feature
  mechanism is in place. The one-line-per-binary cost is small and precisely mirrors how
  `tikv-jemallocator` itself is already opted into per-binary.
- **New Cargo feature vs. "just always compile it in."** Every binary in the workspace
  that transitively depends on `micromegas-telemetry-sink` would otherwise link
  `tikv-jemalloc-ctl` unconditionally — including `micromegas-capi`, embedded in Unreal
  Engine game processes that use their own allocator and must not pull in jemalloc.
  A Cargo feature (an optional dependency, the standard tool for this) is the correct
  fit here, not the kind of business-logic feature flag project conventions otherwise
  discourage.
- **`jemalloc-metrics` as its own feature, not folded into `server`.** `server` is also
  enabled by `write-perfetto`, which does not declare `#[global_allocator]`. Folding
  jemalloc stats into `server` would silently turn on meaningless/inert jemalloc gauges
  there. A separate feature keeps the gate exact.
- **Tick-gating the two new emitters to ~5s instead of the loop's native ~200ms.**
  Chosen to avoid a 3x per-tick metric volume increase for a signal whose whole purpose
  is catching an hour-plus climb; mirrors the identical trade-off `saturation_monitor.rs`
  already documents for its own 5s cadence. Rejected: a second background thread/timer
  just for these two gauges — unnecessary complexity when a tick-modulo check inline in
  the existing loop does the same job.
- **Fresh `sysinfo::System` per call vs. a threaded/reused instance.** No delta is
  needed between process-memory samples (unlike the CPU-usage computation this same
  loop already does), so there's nothing to carry across calls and no correctness
  reason to add state.
- **`read()` vs. `mib()` for jemalloc stats.** `mib()` exists to amortize the mallctl
  string lookup for hot paths; at a ~5s effective cadence here, the simpler `read()`
  form is the right level of effort and avoids new cached-handle state.

## Documentation

Add to the Saturation table in `mkdocs/docs/admin/object-cache.md` (near
`object_cache_ram_tier_usage_bytes`, `:256`). This isn't the only metrics catalogue in
the repo — the same file's Monitoring table (~28 rows) and
`mkdocs/docs/admin/maintenance.md`'s pg_stat table are both bigger — but the Saturation
table is the right fit by *kind*: it's specifically the gauges a background sampler
emits on a fixed interval, independent of request volume, and that's exactly what these
six process/jemalloc gauges are (see `system_monitor.rs`), unlike the request-driven
counters in the Monitoring table above it.

The Saturation table is 2-column (`| Metric | Meaning |`) and its existing rows are
one-liners, so each new `Meaning` cell must stay a concise single line — the entries
below are the row text; the multi-signal diagnostic guidance goes in prose *after* the
table, not inside a cell. State in each cell that these gauges are process-wide, not
object-cache-specific:

- `process_resident_bytes` / `process_virtual_bytes` — this process's own RSS/virtual
  size (`sysinfo`, allocator-agnostic; emitted by any service built with the
  `jemalloc-metrics` feature).
- `jemalloc_allocated_bytes` / `jemalloc_resident_bytes` / `jemalloc_mapped_bytes` /
  `jemalloc_retained_bytes` — jemalloc's own runtime accounting (`tikv-jemalloc-ctl`
  `stats.allocated`/`resident`/`mapped`/`retained`; only binaries built with the
  `jemalloc-metrics` feature, all 7 production services).

Then, in prose below the table, add the diagnostic decision tree that doesn't fit a
one-line cell:

- Compare `process_resident_bytes` against the host-level `used_memory` system metric:
  RSS climbing while the delta between `used_memory` and this process's RSS stays flat
  means the growth *is* this process; RSS flat while `used_memory` climbs means it's
  some other process on the host.
- `jemalloc_allocated_bytes` climbing points to a logical leak inside the process (reach
  for a jemalloc heap profile next); `resident` climbing while `allocated` is flat points
  to allocator fragmentation/retention (tune `MALLOC_CONF`); process RSS climbing while
  both jemalloc gauges are flat points to non-heap growth (mmap, thread stacks, kernel).

## Testing Strategy

Add two new files under `rust/telemetry-sink/tests/`, following the
`InMemorySink`-observing pattern already used in `object-cache-srv/tests/saturation_tests.rs`:
`system_monitor_tests.rs` (process-memory + tick-gating, no feature requirement, no
platform gate) and `jemalloc_stats_tests.rs` (jemalloc stats,
`required-features = ["jemalloc"]` plus the `#![cfg(not(target_os = "windows"))]` stub
and `#[global_allocator]` declaration). They're kept separate because Cargo's
`required-features` and a file-level `#![cfg(...)]` apply to the whole `[[test]]`
target, not to individual `#[test]` functions — bundling the ungated tests into the
gated file would silently make them require the `jemalloc` feature too and skip them on
Windows. This also matches `public/Cargo.toml`'s six `[[test]]` entries, each a
single-purpose file that never mixes gated and ungated tests.
Both `emit_process_memory_stats` and `emit_jemalloc_stats` are plain, directly-callable
functions (no loop/thread/sleep needed to exercise them — consistent with this
project's preference for deterministic test synchronization over timing-based waits).
As in `saturation_tests.rs`, every test below that uses the global `InMemorySink`
dispatch is marked `#[serial_test::serial]` (via the `serial_test` dev-dependency added
in step 2), since that dispatch is process-wide state that would otherwise race under
cargo's default parallel test execution:

- **Process memory** (in `system_monitor_tests.rs`, no feature requirement):
  `#[serial_test::serial]` test that calls
  `emit_process_memory_stats()`, `flush_metrics_buffer()`, and asserts
  `process_resident_bytes` and `process_virtual_bytes` each fire exactly once with a
  value `> 0` — any running process has nonzero RSS/virtual size, so this is a real
  assertion, and it needs no allocator-specific setup since `sysinfo` reads it from the
  OS regardless of which allocator is active.
- **jemalloc stats** (in `jemalloc_stats_tests.rs`, `required-features = ["jemalloc"]`
  in a `[[test]]` entry in `telemetry-sink/Cargo.toml`, matching `public/Cargo.toml`'s
  existing `required-features = ["server"]` pattern): the test *binary* does not carry any
  service's `#[global_allocator]` declaration (that lives in each service's own bin
  entry point, not in this library), so by default jemalloc would not be this test
  binary's active allocator and `stats.allocated` would reflect only jemalloc's own
  incidental bookkeeping. Start the file with `#![cfg(not(target_os = "windows"))]` —
  `tikv-jemallocator` is only a dev-dependency on non-Windows targets (see step 2), so
  the file must compile to an empty test harness on Windows rather than fail to find the
  crate; this mirrors the `not(target_os = "windows")` gating used everywhere else in
  this feature. Then declare
  `#[global_allocator] static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;`
  at the top of this test file (requires adding `tikv-jemallocator` as a
  `dev-dependencies` entry in `telemetry-sink/Cargo.toml`, target-gated the same way),
  making jemalloc genuinely active for *this* test binary. Then, in a
  `#[serial_test::serial]` test: call `emit_jemalloc_stats()` once and flush, record the
  first `jemalloc_allocated_bytes` reading; allocate and `std::hint::black_box` (or
  otherwise keep live) a large `Vec<u8>` (e.g. 8 MiB); call `emit_jemalloc_stats()` again
  and flush; assert the second reading is larger than the first by a plausible margin.
  Also assert all four jemalloc metrics fire exactly once per call as a basic regression
  guard.
- **Tick-gating**: a focused test in `system_monitor_tests.rs` (matching this
  project's convention of keeping unit tests under `tests/`, not inline with the lib
  implementation) that calls the production `should_sample_slow(tick: u32) -> bool`
  function directly and asserts it's `true` on tick 25, 50, ... and `false` on the
  ticks in between. Since `send_system_metrics_forever` itself is an infinite loop,
  don't test it end to end — test `should_sample_slow` and the two emit functions
  independently, the same way `saturation_monitor.rs` tests `sample_once` directly
  rather than `spawn_saturation_monitor`'s loop.
- Existing `used_memory`/`free_memory`/`cpu_usage`/`total_memory` behavior is unchanged
  — no existing test coverage to update (there is none today for this file), but a
  full `cargo build`/`cargo run` smoke check of one service (e.g.
  `local_test_env/ai_scripts/start_services.py`, then `micromegas-query` for the new
  metric names) is worth doing once before considering this done, since this is the
  first time `system_monitor.rs` gets any direct test coverage at all.
- Run `cargo test -p micromegas-telemetry-sink --features jemalloc` directly during
  development for a fast, isolated check of the new test files. For the real coverage
  that matters for CI, a workspace-wide `cargo test` / `cargo clippy --workspace -- -D
  warnings` (what `python3 ../build/rust_ci.py` already runs, per `build/rust_ci.py:13,27`)
  is sufficient and requires no changes: it already unifies features across all
  workspace members, and 7 of them request `jemalloc-metrics` unconditionally, so the
  `#[cfg(all(feature = "jemalloc", not(target_os = "windows")))]` path is compiled and
  tested by the existing CI invocation with no new CI leg needed.

## Open Questions

None — the CI-coverage question was resolved by inspecting `build/rust_ci.py`: it runs
plain workspace-wide `cargo test`/`cargo clippy --workspace`, which already unifies in
the `jemalloc` feature via the 7 services that request it unconditionally.
