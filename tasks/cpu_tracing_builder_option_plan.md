# CPU Tracing Builder Option Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1705

## Overview
Async span events only flow through thread streams, which exist only when CPU tracing is enabled. Today the only switch is the `MICROMEGAS_ENABLE_CPU_TRACING` env var read inside `TelemetryGuardBuilder::build()`, so an embedder must mutate its own process environment at startup. Add a builder option so the choice can be made in code, keeping the env var as the fallback.

## Current State
- `rust/telemetry-sink/src/lib.rs` (~line 666): `build()` passes `std::env::var("MICROMEGAS_ENABLE_CPU_TRACING").map(|v| v == "true").unwrap_or(false)` to `TracingSystemGuard::new` (`rust/tracing/src/guards.rs:20`).
- `rust/micromegas-proc-macros/src/lib.rs` (~line 348): `#[micromegas_main]` reads the same env var to decide whether to call `with_tracing_callbacks()` on the tokio runtime builder.
- The builder already has a precedent for "code pins the value, otherwise env var, otherwise default" (`with_local_sink_max_level`).

## Design
- Add `cpu_tracing_enabled: Option<bool>` to `TelemetryGuardBuilder` (default `None`).
- Add `#[must_use] pub fn with_cpu_tracing_enabled(mut self, enabled: bool) -> Self`, doc-commented to say it pins the value and that `MICROMEGAS_ENABLE_CPU_TRACING` then no longer applies.
- In `build()`, resolve: explicit builder value, else the env var (`== "true"`), else `false`. Extract the resolution into a small pure function taking the optional builder value and the optional env string, so it is unit-testable without touching the process environment.
- Name the env var as a `pub const` next to `LOCAL_SINK_MAX_LEVEL_ENV_VAR`, and use it in `build()`.

### Proc macro
`#[micromegas_main]` decides about tokio callbacks before the builder runs, from the env var alone. If a user pins CPU tracing in code the macro cannot see it, so the runtime would lack callbacks. Out of scope here: users of the macro keep using the env var. Document that the builder option is for manual `TelemetryGuardBuilder` use.

## Implementation Steps
1. `rust/telemetry-sink/src/lib.rs`: add field, default, env-var const, builder method, resolution function, and use it in `build()`.
2. Unit tests for the resolution function.
3. Docs and CHANGELOG.

## Files to Modify
- `rust/telemetry-sink/src/lib.rs`
- `CHANGELOG.md`
- `mkdocs/docs/when-to-use/index.md` (mention the builder option next to the env var)

## Trade-offs
- A span stream decoupled from CPU tracing was also proposed in the issue. It is a much larger change to the stream architecture and the builder option alone resolves the embedder's need.
- Plain `bool` instead of `Option<bool>` would make the builder silently override the env var in both directions, breaking existing env-based deployments.

## Documentation
Update `mkdocs/docs/when-to-use/index.md` (lines ~195, 220, 252) to mention `with_cpu_tracing_enabled`. Add a CHANGELOG entry.

## Testing Strategy
Unit tests of the resolution function: builder `Some(true)` wins over env `"false"`, `Some(false)` wins over env `"true"`, `None` falls back to env, `None` with env absent or not `"true"` gives `false`. No live-DB tests.

## Open Questions
- Should `#[micromegas_main]` gain an attribute to pin CPU tracing so the tokio callbacks stay consistent? Deferred; not needed by the issue.
