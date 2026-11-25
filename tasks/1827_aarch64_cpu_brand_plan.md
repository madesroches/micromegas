# Record the Actual CPU Model in cpu_brand on aarch64 Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1827

## Overview
On aarch64 the `cpu_brand` recorded for a process is the literal string `"aarch64"`, so
processes running on different ARM CPU generations (e.g. cloud instances silently placed on a
newer or older Graviton host) can't be told apart in the `processes` table. This plan adds one
shared helper in `micromegas-tracing` that reports the real CPU model — decoded from the MIDR on
Linux/Android and from `machdep.cpu.brand_string` on macOS — and uses it from both places that
fill `cpu_brand` today.

## Current State
Two independent code paths compute `cpu_brand`:

- `rust/tracing/src/dispatch.rs:1365` — `get_cpu_brand()`, used by `make_process_info()` to fill
  `ProcessInfo::cpu_brand` (the `processes.cpu_brand` column). x86/x86_64 uses `raw_cpuid`'s
  brand string (`"unknown"` if missing); aarch64 returns `"aarch64"`. Any other native
  architecture has no `return` and would not compile.
- `rust/telemetry-sink/src/lib.rs:514` — the `cpu_brand` default process property in
  `populate_default_system_properties()`. x86_64 uses `raw_cpuid`; everything else uses
  `std::env::consts::ARCH`.

Dependencies: `raw-cpuid` is a native-only (non-wasm) dependency of `micromegas-tracing`
(`rust/tracing/Cargo.toml:33`) and an x86_64-only dependency of `micromegas-telemetry-sink`
(`rust/telemetry-sink/Cargo.toml:45`). `telemetry-sink` already depends on `micromegas-tracing`.

`dispatch.rs` is native-only (`rust/tracing/src/lib.rs:195`); wasm uses `dispatch_wasm.rs`.

## Design

### New module `rust/tracing/src/cpu_brand.rs` (native-only)
Declared in `rust/tracing/src/lib.rs` as `#[cfg(not(target_arch = "wasm32"))] pub mod cpu_brand;`.

Public API:

```rust
/// Human-readable identification of the host CPU model.
pub fn get_cpu_brand() -> String;
```

Platform dispatch inside `get_cpu_brand()`:

| target | source | fallback |
|---|---|---|
| `x86` / `x86_64` | `raw_cpuid` brand string (unchanged) | `"unknown"` (unchanged) |
| `aarch64` + `linux`/`android` | MIDR (see below) | `std::env::consts::ARCH` |
| `aarch64` + `macos` | `sysctlbyname("machdep.cpu.brand_string")` | `std::env::consts::ARCH` |
| anything else | — | `std::env::consts::ARCH` |

Each branch is `#[cfg]`-gated so non-matching code is not compiled. The final fallback branch
also fixes the latent compile failure on non-x86/non-aarch64 native targets in `dispatch.rs`.

### MIDR decoding (aarch64 Linux/Android)
1. Read `/sys/devices/system/cpu/cpu0/regs/identification/midr_el1` (content like
   `0x00000000413fd0c1\n`). Parse as hex (optional `0x` prefix, trimmed). Implementer =
   bits [31:24], part number = bits [15:4].
2. If that fails (file missing, unreadable, unparsable), read `/proc/cpuinfo` and take the first
   `CPU implementer` and `CPU part` lines (values like `0x41`, `0xd0c`; key and value separated
   by `:` with arbitrary whitespace).
3. If both fail, fall back to `"aarch64"`.

Pure, unit-testable functions (private, `pub(crate)` or `#[cfg(test)]`-visible as needed),
compiled on all native targets so tests run on the x86_64 CI host:

```rust
fn parse_midr(text: &str) -> Option<(u32, u32)>;          // (implementer, part)
fn parse_cpuinfo_ids(text: &str) -> Option<(u32, u32)>;   // first CPU implementer / CPU part
fn format_arm_brand(implementer: u32, part: u32) -> String;
fn arm_part_name(implementer: u32, part: u32) -> Option<&'static str>;
```

`format_arm_brand` returns `"<name> (0x<impl:02x>:0x<part:03x>)"` when the part is known, e.g.
`"Neoverse-N1 (0x41:0xd0c)"`, and `"0x<impl:02x>:0x<part:03x>"` when it is not, so unknown chips
remain distinguishable. Only the I/O wrapper (`read_arm_brand()`) is cfg-gated to aarch64
Linux/Android; to avoid dead-code warnings on other targets, the pure helpers are
`#[cfg_attr(not(all(target_arch = "aarch64", any(target_os = "linux", target_os = "android"))), allow(dead_code))]`
(or equivalent).

`arm_part_name` is a small static `match` covering the parts that matter for server/cloud and
common dev hardware — not an exhaustive database:

- Arm Ltd (`0x41`): Cortex-A53 `0xd03`, Cortex-A55 `0xd05`, Cortex-A57 `0xd07`, Cortex-A72
  `0xd08`, Cortex-A73 `0xd09`, Cortex-A75 `0xd0a`, Cortex-A76 `0xd0b`, Neoverse-N1 `0xd0c`,
  Cortex-A77 `0xd0d`, Neoverse-V1 `0xd40`, Cortex-A78 `0xd41`, Cortex-X1 `0xd44`, Cortex-A510
  `0xd46`, Cortex-A710 `0xd47`, Cortex-X2 `0xd48`, Neoverse-N2 `0xd49`, Neoverse-V2 `0xd4f`,
  Neoverse-V3 `0xd84`, Neoverse-N3 `0xd8e`.
- Ampere (`0xc0`): Ampere-1 `0xac3`, Ampere-1a `0xac4`.
- Apple (`0x61`) is not listed: macOS gets its name from sysctl, and Asahi Linux users get the
  raw ids.

Names intentionally do not include cloud product names (Graviton) — the mapping from core to
product is the cloud vendor's, not the CPU's identity, and the raw ids disambiguate anyway.

### macOS (aarch64)
Call `sysctlbyname` through a minimal `extern "C"` declaration (libSystem is always linked on
macOS, so no new crate dependency): query the size with a null buffer, allocate, query again,
trim the trailing NUL, and convert lossily to UTF-8. Any error or empty result → fallback.

### Callers
- `rust/tracing/src/dispatch.rs`: delete the local `get_cpu_brand()`; `make_process_info()` calls
  `crate::cpu_brand::get_cpu_brand()`.
- `rust/telemetry-sink/src/lib.rs`: replace the inline closure with
  `Box::new(micromegas_tracing::cpu_brand::get_cpu_brand)`.

### Dependencies
- `rust/tracing/Cargo.toml`: move `raw-cpuid` from the native-only section into a
  `[target.'cfg(any(target_arch = "x86", target_arch = "x86_64"))'.dependencies]` section.
- `rust/telemetry-sink/Cargo.toml`: remove the `raw-cpuid` x86_64 dependency (no longer used).

No SQL surface change: `cpu_brand` keeps its name, type and position; only the value on aarch64
changes.

## Implementation Steps
1. Create `rust/tracing/src/cpu_brand.rs` with `get_cpu_brand()`, the platform branches, the pure
   parsing/formatting helpers, and their unit tests (`#[cfg(test)] mod tests`).
2. Register the module in `rust/tracing/src/lib.rs` (native-only).
3. Switch `make_process_info()` in `rust/tracing/src/dispatch.rs` to the shared helper and delete
   the old function.
4. Switch the `cpu_brand` default property in `rust/telemetry-sink/src/lib.rs` to the shared
   helper.
5. Adjust `raw-cpuid` dependencies in `rust/tracing/Cargo.toml` and
   `rust/telemetry-sink/Cargo.toml`.
6. Update docs (see Documentation).
7. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test -p micromegas-tracing`,
   and `cargo check -p micromegas-tracing -p micromegas-telemetry-sink --target aarch64-linux-android`
   (that target is installed locally and exercises the aarch64 Linux-family cfg branch).

## Files to Modify
- `rust/tracing/src/cpu_brand.rs` (new)
- `rust/tracing/src/lib.rs`
- `rust/tracing/src/dispatch.rs`
- `rust/telemetry-sink/src/lib.rs`
- `rust/tracing/Cargo.toml`
- `rust/telemetry-sink/Cargo.toml`
- `mkdocs/docs/query-guide/schema-reference.md`
- `CHANGELOG.md` (via the `pr` step)

## Trade-offs
- **Own MIDR decoder vs. `sysinfo`.** `sysinfo` already decodes ARM implementer/part and reads
  the macOS brand string, but it is a heavy dependency for the instrumentation library
  (`micromegas-tracing` is meant to stay lean and is embedded in game engines), its brand string
  drops the raw part number the issue wants for unknown chips, and it reads only `/proc/cpuinfo`.
  A ~100-line decoder with a small table is cheaper and gives the exact format requested.
- **Helper location: `micromegas-tracing`.** Both callers already depend on it, and it is the
  crate that owns `ProcessInfo`. A separate crate would add packaging for one function.
- **`cpu0` only.** Heterogeneous (big.LITTLE) SoCs report the first core's part. Server parts are
  homogeneous, and the goal is distinguishing hosts, not describing every core.
- **No caching.** `get_cpu_brand()` runs once per call site per process start; a `OnceLock` would
  add state for no measurable gain.

## Decisions

## Documentation
- `mkdocs/docs/query-guide/schema-reference.md` (lines 47 and 144): describe `cpu_brand` as the CPU
  model, noting that on aarch64 Linux it is `"<core name> (0x<implementer>:0x<part>)"` (or the raw
  ids for unrecognized parts) and falls back to the architecture name when unavailable.
- `CHANGELOG.md`: entry under Unreleased (added by `pr`).

## Testing Strategy
Unit tests in `rust/tracing/src/cpu_brand.rs`, compiled and run on the x86_64 CI host:
- `parse_midr`: `0x00000000413fd0c1` → `(0x41, 0xd0c)`; with trailing newline; without `0x`
  prefix; empty / garbage → `None`.
- `parse_cpuinfo_ids`: a realistic multi-core aarch64 `/proc/cpuinfo` excerpt → first core's ids;
  missing `CPU part` → `None`; x86-style cpuinfo → `None`.
- `format_arm_brand`: known parts (`0x41:0xd0c` → `Neoverse-N1 (0x41:0xd0c)`, `0xd40`, `0xd4f`);
  unknown part → `0x41:0xfff`; unknown implementer → raw ids.
- `get_cpu_brand()` returns a non-empty string on the host (smoke test; on x86_64 CI it exercises
  the raw-cpuid branch).

The aarch64 and macOS I/O wrappers are thin and not reachable from x86_64 CI; their parsing
logic is covered by the pure-function tests, and the cfg branch is type-checked by the
`aarch64-linux-android` `cargo check` in step 7.

## Manual Verification
On an aarch64 Linux host (e.g. a Graviton instance), run any instrumented process against a local
ingestion server and query
`micromegas-query "SELECT cpu_brand FROM processes ORDER BY start_time DESC LIMIT 1"` — expect
e.g. `Neoverse-V1 (0x41:0xd40)`. Not automated: CI has no aarch64 runner. If no aarch64 host is
available, this step is skipped and recorded as such.

## Open Questions
