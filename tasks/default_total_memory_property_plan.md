# Add Total System Memory to Process Properties by Default

**Issue:** [#380](https://github.com/madesroches/micromegas/issues/380)

## Overview

Add total system memory as a default process property so every process automatically reports how much RAM is available on its host. This is a static system characteristic that belongs in process metadata rather than being emitted only as a periodic metric.

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

Add total system memory to `process_properties` inside `TelemetryGuardBuilder::build()`, just before passing properties to `TracingSystemGuard::new()`. This ensures:

- It's included by default without any user action
- Users can still override it via `with_process_property("total_memory", ...)` before calling `build()` (their value would be overwritten — see Trade-offs)
- It uses the existing `sysinfo` dependency already in the crate
- No changes needed to `ProcessInfo`, ingestion, or the database schema

The property key will be `"total_memory"` with the value as bytes (string-encoded u64), matching the existing `imetric!("total_memory", "bytes", ...)` convention in `system_monitor.rs`.

## Implementation Steps

1. **Modify `TelemetryGuardBuilder::build()`** in `rust/telemetry-sink/src/lib.rs`
   - Change `build(self)` to `build(mut self)` to allow mutation
   - Before the `TracingSystemGuard::new()` call (line 375), insert total memory:
     ```rust
     let system = sysinfo::System::new_with_specifics(
         sysinfo::RefreshKind::nothing()
             .with_memory(sysinfo::MemoryRefreshKind::nothing().with_ram()),
     );
     self.process_properties
         .entry("total_memory".to_string())
         .or_insert_with(|| system.total_memory().to_string());
     ```
   - Using `entry().or_insert_with()` preserves any user-specified override

2. **Remove redundant one-shot metric from `system_monitor.rs`** (optional)
   - Line 14: `imetric!("total_memory", "bytes", system.total_memory());` could be removed since total memory is now a process property
   - However, keeping it maintains backward compatibility for queries that look for the metric — defer this to a follow-up

## Files to Modify

| File | Change |
|------|--------|
| `rust/telemetry-sink/src/lib.rs` | Add total memory to properties in `build()` |

## Trade-offs

**Approach chosen: Insert in `build()` with `entry().or_insert_with()`**
- Allows user override if set before `build()`
- Minimal code change (3-4 lines)
- Uses existing `sysinfo` dependency

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
- Manual verification: start a service with telemetry, query `SELECT properties FROM processes` to confirm `total_memory` appears
- Verify on both Linux and macOS (sysinfo is cross-platform)

## Open Questions

None — this is a straightforward addition using existing infrastructure.
