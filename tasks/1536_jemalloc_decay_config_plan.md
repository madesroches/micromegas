# jemalloc Decay / Background-Thread Configuration Plan

Issue: https://github.com/madesroches/micromegas/issues/1536

## Overview

Every jemalloc binary in the workspace runs with jemalloc's stock runtime configuration:
`background_thread` off, `dirty_decay_ms:10000`. With `background_thread` off, decay only
advances opportunistically when an allocation event happens to touch the owning arena, so a
process that has just finished a burst of churn leaves dirty pages mapped indefinitely. A
production `object-cache-srv` (8 GiB, 2 vCPU) measured a ~950 MB gap between
`jemalloc_allocated_bytes` (~6.2 GB) and `jemalloc_resident_bytes` (~7.15 GB) after 5.2M RAM-tier
evictions in 45 minutes — on that box the gap is the difference between a stable plateau and an
OOM kill.

This plan exports a `malloc_conf` symbol enabling background purging and a tightened dirty decay,
declared through one shared macro invoked in each of the eight jemalloc binaries.

## Current State

Eight binaries declare jemalloc as the global allocator, each with the same three lines at the top
of its entry point (from `tasks/completed/1129_global_allocator_plan.md`):

```rust
#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
```

| Crate | Entry point |
|---|---|
| `object-cache-srv` | `rust/object-cache-srv/src/object_cache_srv.rs:1` |
| `flight-sql-srv` | `rust/flight-sql-srv/src/flight_sql_srv.rs:1` |
| `http-gateway` | `rust/http-gateway/src/http_gateway_srv.rs:1` |
| `analytics-web-srv` | `rust/analytics-web-srv/src/main.rs:1` |
| `monolith` | `rust/monolith/src/main.rs:17` |
| `telemetry-maintenance-srv` | `rust/telemetry-maintenance-srv/src/main.rs:3` |
| `telemetry-ingestion-srv` | `rust/telemetry-ingestion-srv/src/main.rs:18` |
| `redis-exporter` | `rust/redis-exporter/src/main.rs:19` |

`rust/telemetry-sink/tests/jemalloc_stats_tests.rs:14` carries a ninth declaration so the
`stats.*` gauges it exercises reflect real activity.

No `malloc_conf` / `MALLOC_CONF` configuration exists anywhere in the workspace, and
`rust/.cargo/` holds only `audit.toml` — no `[env]` table setting
`JEMALLOC_SYS_WITH_MALLOC_CONF` at build time either.

The gap this creates is already observable: `emit_jemalloc_stats`
(`rust/telemetry-sink/src/system_monitor.rs:48`) emits `jemalloc_allocated_bytes`,
`jemalloc_resident_bytes`, `jemalloc_mapped_bytes` and `jemalloc_retained_bytes` every 5s from all
eight binaries.

### How jemalloc resolves its configuration

`conf.c`'s `obtain_malloc_conf` reads five sources in ascending precedence — **later sources win**:

| # | Source | Reachable here? |
|---|---|---|
| 0 | `--with-malloc-conf` compile-time string | via the `JEMALLOC_SYS_WITH_MALLOC_CONF` build env var |
| 1 | the weak `je_malloc_conf` symbol | **this plan** |
| 2 | `/etc/_rjem_malloc.conf` symlink | yes, unused |
| 3 | `_RJEM_MALLOC_CONF` env var | yes — operator override |
| 4 | `je_malloc_conf_2_conf_harder` | deliberately undocumented upstream |

`tikv-jemalloc-sys` builds with `--with-jemalloc-prefix=_rjem_`, so the symbol is
`_rjem_malloc_conf` and the env var is `_RJEM_MALLOC_CONF`. The issue states that the environment
variable "will not work here"; that is true only of the unprefixed spelling — the prefixed
`_RJEM_MALLOC_CONF` does work, and outranks the symbol this plan exports. It stays available as
the per-deployment escape hatch, which is why this plan adds no code knob of its own.

## Design

### Configuration string

```
background_thread:true,dirty_decay_ms:5000,muzzy_decay_ms:0
```

- **`background_thread:true`** — the actual fix. jemalloc spawns purge threads at initialization
  and advances decay on a timer instead of only when an allocation happens to land in the arena.
- **`dirty_decay_ms:5000`** — halves the stock 10s idle window before dirty pages are handed back.
- **`muzzy_decay_ms:0`** — already jemalloc 5.3's default (`arena_types.h`:
  `MUZZY_DECAY_MS_DEFAULT (0)`), pinned explicitly for two reasons. First, the default has moved
  across jemalloc releases. Second, the issue's suggested `muzzy_decay_ms:5000` would move it the
  wrong way: `0` means muzzy pages are `MADV_DONTNEED`'d immediately, so `5000` would *add* five
  seconds of retention rather than removing any.

`max_background_threads` is left at its default. That default is a fixed `DEFAULT_NUM_BACKGROUND_THREAD`
(4), not one per CPU — `background_thread_boot1` resets any unset/over-limit value to 4 regardless
of `ncpus` — so on the measured 2-vCPU box up to 4 purge threads can be created.

`narenas` is deliberately not set — see Trade-offs.

### Where the symbol has to live

The exported static must be compiled into each **binary** crate, not into a shared library crate.
jemalloc's C code defines `_rjem_malloc_conf` as a *weak definition* valued `NULL`, and our strong
Rust definition overrides it at link time. A weak definition creates no undefined symbol, so
nothing would ever force the linker to extract an rlib member that carried only our override —
the static would silently vanish from some builds. A binary crate's own object files are always
linked in full, so the definition survives there.

`#[used]` is applied on top as insurance against LLVM internalizing it under LTO.

### The macro

New module `rust/telemetry-sink/src/jemalloc_conf.rs`, hosting the conf string and a
`#[macro_export] macro_rules! declare_jemalloc_conf`. `micromegas-telemetry-sink` is the right home:
it already owns the jemalloc surface (`system_monitor`'s gauges, the `jemalloc` feature), and every
one of the eight binaries already depends on `micromegas`, which re-exports it. The macro itself
pulls in no dependency — it expands to a byte string and two attributes — so it can be declared
unconditionally, outside the `jemalloc` feature and outside the `wasm32` gating that wraps the
other native modules.

`jemalloc_conf.rs` defines the conf string once, module-level:

```rust
pub const CONF: &[u8] = b"background_thread:true,dirty_decay_ms:5000,muzzy_decay_ms:0\0";
```

Expansion shape (`c_char` is `i8` on x86-64 and `u8` on aarch64, so the static is typed as a
pointer-sized `Option<&'static u8>` — layout-identical to the `const char *` jemalloc reads, and
the same trick `tikv-jemalloc-sys`' own `tests/malloc_conf_set.rs` uses via a union):

```rust
#[cfg(not(target_os = "windows"))]
#[used]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static MICROMEGAS_MALLOC_CONF: Option<&'static u8> = Some(&$crate::jemalloc_conf::CONF[0]);
```

`CONF` is NUL-terminated: jemalloc reads source 1 as a `const char *` (`jemalloc/src/conf.c`,
`jemalloc/src/jemalloc.c`), and an unterminated byte string is exactly the silent-garbage failure
the assertion test below exists to catch. The expansion references `$crate::jemalloc_conf::CONF`
rather than a bare `CONF`, since the macro expands inside each binary crate, not inside
`telemetry-sink` — `$crate` resolves through the re-export chain even though the binaries depend on
`micromegas`, not `micromegas-telemetry-sink`, directly.

`#[unsafe(export_name = ...)]` is the edition-2024 spelling (the workspace is on edition 2024,
Rust 1.97.1). The `not(target_os = "windows")` gate lives inside the macro so each call site is one
line, mirroring the gate already on the `#[global_allocator]` static above it.

Call sites become:

```rust
#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
micromegas::declare_jemalloc_conf!();
```

### Drift guard

New `build/check_jemalloc_conf.py`, wired into `build/rust_ci.py`'s native step list next to the
existing `check_wasm_deps.py` precedent: every `rust/**/src/*.rs` file containing
`tikv_jemallocator::Jemalloc` under a `#[global_allocator]` must also contain
`declare_jemalloc_conf!`. This is what keeps a ninth binary added later from silently shipping
unconfigured — the concern the issue raises directly.

## Implementation Steps

1. **`rust/telemetry-sink/src/jemalloc_conf.rs`** — new module with
   `pub const CONF: &[u8] = b"background_thread:true,dirty_decay_ms:5000,muzzy_decay_ms:0\0";`
   (NUL-terminated, since jemalloc reads it as a `const char *`) and
   `#[macro_export] macro_rules! declare_jemalloc_conf`, whose expansion references
   `$crate::jemalloc_conf::CONF`. Register `pub mod jemalloc_conf;` in
   `rust/telemetry-sink/src/lib.rs`, ungated (the surrounding native modules are
   `cfg(not(target_arch = "wasm32"))`; this one needs no gate because it expands to nothing on
   Windows and is inert unless invoked).
2. **`rust/public/src/lib.rs`** — `pub use micromegas_telemetry_sink::declare_jemalloc_conf;` at
   crate root, alongside the existing `pub use micromegas_proc_macros::*;`, so binaries spell it
   `micromegas::declare_jemalloc_conf!()`.
3. **Invoke it in all eight binaries**, immediately below the existing `#[global_allocator]`
   static in each entry point listed under Current State.
4. **`rust/telemetry-sink/tests/jemalloc_conf_tests.rs`** — new test binary (see Testing Strategy);
   add its `[[test]]` entry with `required-features = ["jemalloc"]` to
   `rust/telemetry-sink/Cargo.toml`, matching the `jemalloc_stats_tests` entry.
5. **`build/check_jemalloc_conf.py`** + a step in `build/rust_ci.py`'s `run_native()` list.
6. **Docs** — new `mkdocs/docs/admin/memory-allocator.md`, nav entry, and the `admin/object-cache.md`
   pointer (see Documentation).
7. **`CHANGELOG.md`** — entry under `## Unreleased`.
8. `cargo fmt` + `cargo clippy --workspace -- -D warnings` from `rust/`.

## Files to Modify

| File | Change |
|---|---|
| `rust/telemetry-sink/src/jemalloc_conf.rs` | **New** — conf string + `declare_jemalloc_conf!` |
| `rust/telemetry-sink/src/lib.rs` | Register the module |
| `rust/telemetry-sink/Cargo.toml` | `[[test]]` entry for the new test binary |
| `rust/telemetry-sink/tests/jemalloc_conf_tests.rs` | **New** — asserts the conf took effect |
| `rust/public/src/lib.rs` | Re-export the macro at crate root |
| `rust/object-cache-srv/src/object_cache_srv.rs` | Invoke the macro |
| `rust/flight-sql-srv/src/flight_sql_srv.rs` | Invoke the macro |
| `rust/http-gateway/src/http_gateway_srv.rs` | Invoke the macro |
| `rust/analytics-web-srv/src/main.rs` | Invoke the macro |
| `rust/monolith/src/main.rs` | Invoke the macro |
| `rust/telemetry-maintenance-srv/src/main.rs` | Invoke the macro |
| `rust/telemetry-ingestion-srv/src/main.rs` | Invoke the macro |
| `rust/redis-exporter/src/main.rs` | Invoke the macro |
| `build/check_jemalloc_conf.py` | **New** — drift guard |
| `build/rust_ci.py` | Add the guard to `run_native()` |
| `mkdocs/docs/admin/memory-allocator.md` | **New** |
| `mkdocs/mkdocs.yml` | Nav entry |
| `mkdocs/docs/admin/object-cache.md` | Replace the bare "tune `MALLOC_CONF`" advice |
| `CHANGELOG.md` | Unreleased entry |

## Trade-offs

**Exported symbol vs. `JEMALLOC_SYS_WITH_MALLOC_CONF` build env var.** A `[env]` table in
`rust/.cargo/config.toml` would configure every binary with one line and no Rust code. Rejected:
`.cargo/config.toml` only applies to builds run from inside the workspace, so it would silently
not apply to `cargo install` or to any downstream consumer, and it forces a full jemalloc C rebuild
on change. It also sits at precedence 0, the weakest source. The exported symbol is committed code
that travels with the binary.

**Exported symbol vs. `tikv-jemallocator`'s `background_threads` cargo feature.** This feature
turns into `--with-malloc-conf=background_thread:true` at the `tikv-jemalloc-sys` build-script
level, needing no Rust code. Rejected: it only sets `background_thread`, leaving
`dirty_decay_ms`/`muzzy_decay_ms` unconfigured, so the exported symbol would still be needed for
decay — and it forces a jemalloc C rebuild on change, same as the env-var option above.

**Exported symbol vs. runtime `mallctl`.** `background_thread` alone is writable at runtime
(`tikv_jemalloc_ctl::background_thread::write(true)`), but decay is not: `arenas.dirty_decay_ms`
only affects arenas created *after* the write, so already-live arenas would need a per-arena walk
or the undocumented `MALLCTL_ARENAS_ALL` index. Configuring at initialization covers every arena
with no special cases.

**One shared macro vs. one macro that also declares the allocator.** Folding the
`#[global_allocator]` static into the same macro would collapse four lines per binary to one, but
`tikv-jemallocator` is only a *dev*-dependency of `micromegas-telemetry-sink`; the macro would
either force it on every telemetry-sink consumer or expand to an unhygienic reference to the
caller's `tikv_jemallocator` crate name. Keeping the allocator declaration explicit and adding one
line beneath it avoids both.

**Per-binary conf values.** `object-cache-srv` is the binary with measured churn; `flight-sql-srv`
has a very different allocation profile. One shared string is used anyway — a macro argument for a
per-binary override would be a code knob duplicating `_RJEM_MALLOC_CONF`, which already lets an
operator override any single deployment without a rebuild.

**`narenas`.** Default is `4 × ncpus`, and each arena retains dirty pages independently, so
`narenas:2` on the 2-vCPU box would cut the retained footprint further. Not included: it trades
directly against allocator lock contention on a service whose hot path is small-block churn across
many threads, and it should be measured on its own rather than bundled with a change whose effect
is otherwise strictly a reduction in idle memory.

## Decisions

- `muzzy_decay_ms` is pinned to `0`, not the `5000` the issue suggests; `5000` would increase
  retention relative to jemalloc 5.3's default.
- No new configuration knob is added. `_RJEM_MALLOC_CONF` is the documented per-deployment
  override.

## Performance

`background_thread:true` costs up to `max_background_threads` purge threads, waking on the decay
timer. That default is a fixed 4 (`DEFAULT_NUM_BACKGROUND_THREAD`), not one per CPU, so on the
measured 2-vCPU box that is up to four mostly-idle threads against a ~950 MB reduction in idle
resident memory. The purge work itself is not new — it is the same `madvise` traffic the allocating thread
would otherwise do inline, moved off the hot path.

`dirty_decay_ms:5000` returns pages sooner, which means re-faulting them if the workload's churn
resumes within the window. On a memory-constrained box that is the intended trade; the measured
average RAM-tier entry lifetime (66s) is an order of magnitude above the decay window, so the
steady-state working set is not what is being purged.

## Platform notes

`background_thread` is unsupported on musl (`tikv-jemalloc-sys`' `NO_BG_THREAD_TARGETS`), where
the sys crate compiles in `background_thread:false` at precedence 0. All release images build
`*-unknown-linux-gnu` (`docker/*.Dockerfile`), so this does not apply today; if a musl target is
ever added, jemalloc would reject the option on stderr rather than fail — `opt.abort_conf` defaults
to false, so an invalid or misspelled conf pair is a startup warning, not a crash. That silence is
precisely why the assertion test below matters.

## Testing Strategy

**`rust/telemetry-sink/tests/jemalloc_conf_tests.rs`** — a dedicated test binary declaring both the
jemalloc global allocator and `declare_jemalloc_conf!()`, then asserting through `mallctl` that the
configuration actually reached jemalloc:

- `tikv_jemalloc_ctl::opt::background_thread::read()? == true`
- `raw::read::<isize>(b"opt.dirty_decay_ms\0")? == 5000`
- `raw::read::<isize>(b"opt.muzzy_decay_ms\0")? == 0`

This is the guard against every silent-failure mode in this change: a wrong symbol prefix, a
dropped static, a typo in the conf string, or an upstream default shift. `#![cfg(not(target_os =
"windows"))]` and `required-features = ["jemalloc"]` gate it the same way
`jemalloc_stats_tests.rs` is gated. The test must skip (not fail) when `_RJEM_MALLOC_CONF` is set
in the environment, since that source legitimately outranks the symbol under test.

**`build/check_jemalloc_conf.py`** — asserts the macro is invoked in every binary that declares
jemalloc as its global allocator. Run in CI via `python3 build/rust_ci.py native`.

**Manual verification** — start `object-cache-srv` locally, drive eviction churn, and compare
`jemalloc_resident_bytes - jemalloc_allocated_bytes` against a build without the change:

```sql
SELECT time, name, value FROM measures
WHERE name IN ('jemalloc_allocated_bytes', 'jemalloc_resident_bytes')
```

The gap should fall and, more importantly, should recover after churn stops rather than staying at
its peak.

## Documentation

- **New `mkdocs/docs/admin/memory-allocator.md`** (nav: Operations → Administration, after
  "Telemetry Sink Configuration"): the conf string every service is built with, what each option
  does, the `_RJEM_MALLOC_CONF` override and its precedence over the built-in value, and the
  `jemalloc_*` gauges to watch. Short — one screen.
- **`mkdocs/docs/admin/object-cache.md:317`** currently ends its memory-diagnosis paragraph with
  "tune `MALLOC_CONF`", which names a variable that does nothing on these binaries. Replace with a
  link to the new page.
