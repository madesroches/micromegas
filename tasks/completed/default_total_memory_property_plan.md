# Add Default System Properties to Process Metadata

**Issue:** [#380](https://github.com/madesroches/micromegas/issues/380)

## Overview

Add default system properties to every process so that host characteristics (CPU, memory, OS, user) are automatically captured as process metadata. These are static system characteristics that belong in process metadata rather than being emitted only as periodic metrics.

## Current State

- **Process properties** are a `HashMap<String, String>` initialized empty in `TelemetryGuardBuilder::default()` (`rust/telemetry-sink/src/lib.rs:121`)
- The `#[micromegas_main]` proc macro adds `"version"` as the only default property (`rust/micromegas-proc-macros/src/lib.rs:117`)
- Total memory is currently emitted as an `imetric!` in `system_monitor.rs:14` — this is a one-shot metric at startup, but it's mixed in with the periodic metrics stream rather than being a process-level property
- The `sysinfo` crate is already a dependency of `telemetry-sink` (used in `system_monitor.rs`)
- Properties are stored in PostgreSQL as `micromegas_property[]` on the `processes` table

### Data Flow

```
TelemetryGuardBuilder.process_properties
  → TracingSystemGuard::new()
    → Dispatch::startup()
      → make_process_info()
        → ProcessInfo { properties }
          → HttpEventSink::push_process()
            → POST /ingestion/insert_process
              → PostgreSQL processes.properties
```

## Design

Add a `populate_default_system_properties()` method on `TelemetryGuardBuilder` that inserts system metadata into `process_properties`, called from `build()`. This ensures:

- Properties are included by default without any user action
- Users can override any property via `with_process_property(key, value)` before calling `build()` — user values take precedence via `entry().or_insert_with()`
- A `with_default_system_properties_enabled(false)` escape hatch disables all defaults
- No changes needed to `ProcessInfo`, ingestion, or the database schema

### Default Properties

| Key | Source | Value |
|-----|--------|-------|
| `exe` | `std::env::current_exe()` | Executable path |
| `username` | `whoami::username()` | OS username |
| `realname` | `whoami::realname()` | User's display name |
| `computer` | `whoami::devicename()` | Hostname |
| `distro` | `whoami::distro()` | OS distribution |
| `cpu_brand` | `raw_cpuid::CpuId` (x86_64) / `std::env::consts::ARCH` (other) | CPU brand string |
| `physical_core_count` | `sysinfo::System::physical_core_count()` | Physical CPU cores |
| `logical_cpu_count` | `sysinfo::System::cpus().len()` | Logical CPU count |
| `total_memory` | `sysinfo::System::total_memory()` | Total RAM in bytes |

### New Dependencies

| Crate | Purpose | Platform |
|-------|---------|----------|
| `whoami` | Username, hostname, OS info | All |
| `raw-cpuid` | CPU brand string | x86_64 only |

## Implementation (done)

1. **Added `default_system_properties_enabled` field** to `TelemetryGuardBuilder` (defaults to `true`)
2. **Added `with_default_system_properties_enabled()` builder method** for opting out
3. **Added `populate_default_system_properties()`** method that inserts all defaults using `entry().or_insert_with()` to preserve user overrides
4. **Modified `build()`** to `build(mut self)` and call `populate_default_system_properties()` when enabled
5. **Gated `raw-cpuid`** to `target_arch = "x86_64"` with `std::env::consts::ARCH` fallback on other architectures

## Files Modified

| File | Change |
|------|--------|
| `rust/telemetry-sink/Cargo.toml` | Added `whoami` and `raw-cpuid` (x86_64) dependencies |
| `rust/telemetry-sink/src/lib.rs` | Added builder field, method, and `populate_default_system_properties()` |

## Trade-offs

**Approach chosen: Insert in `build()` with `entry().or_insert_with()`**
- Allows user override if set before `build()`
- Lazy evaluation — property values only computed if not already set
- Uses closures to defer expensive operations (sysinfo, whoami, cpuid)

**Alternative: Add in `Default::default()`**
- Rejected: would instantiate `sysinfo::System` eagerly even if the builder is never built
- Would also make it harder to test with mock values

**Alternative: Add in the `#[micromegas_main]` proc macro**
- Rejected: not all processes use the proc macro; the property should be universal

**Regarding the existing `imetric!` in system_monitor.rs:**
- Keep it for now to avoid breaking existing metric queries
- Can be removed in a future cleanup once consumers migrate to using the process property

## Testing Strategy

- `cargo build` in `rust/` to verify compilation
- `cargo test` in `rust/` for existing tests
- Manual verification: start a service with telemetry, query `SELECT properties FROM processes` to confirm properties appear
- Verify on both Linux and macOS (sysinfo and whoami are cross-platform)
- Test on ARM to verify `cpu_brand` falls back to arch string
