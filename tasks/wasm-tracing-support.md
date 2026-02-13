# WASM Support for micromegas-tracing

## Goal

Make `micromegas-tracing` compile on `wasm32-unknown-unknown` so that analytics code (and eventually other crates) can be ported to WASM without removing tracing instrumentation.

**Short-term scope**: Log entries print to the JS console via the `EventSink` trait. Spans and metrics are silently dropped. No telemetry transport. No transit dependency on wasm32.

## Context

- `datafusion-wasm` already compiles DataFusion to WASM but doesn't depend on `micromegas-tracing`
- The `analytics` crate uses `micromegas_tracing::prelude::*` in ~50 source files
- To port analytics code into WASM, tracing must compile for `wasm32`
- `telemetry-sink` (HTTP transport, tokio, reqwest, sysinfo) is **not needed** for WASM
- `micromegas-transit` (binary serialization, `HeterogeneousQueue`, `InProcSerialize`) is **not needed** for WASM since we're not buffering or sending events

## Design: Same EventSink trait, stub types on wasm32

Keep the `EventSink` trait as the single sink interface on both native and wasm. The trait references several transit-dependent types (`LogBlock`, `LogStream`, `MetricsBlock`, `MetricsStream`, `ThreadBlock`, `ThreadStream`, `Property`). On wasm32, these are never constructed — the `ConsoleEventSink` no-ops all block/stream methods and receives `&[]` for properties. So we provide **empty struct stubs** that satisfy the type signatures without pulling in transit.

### What the EventSink trait needs (from `event/sink.rs`)

```rust
pub trait EventSink {
    fn on_startup(&self, process_info: Arc<ProcessInfo>);       // transit-free
    fn on_shutdown(&self);
    fn on_log_enabled(&self, metadata: &LogMetadata) -> bool;   // transit-free
    fn on_log(&self, desc: &LogMetadata, properties: &[Property], time: i64, args: fmt::Arguments<'_>);
    fn on_init_log_stream(&self, log_stream: &LogStream);       // stub on wasm
    fn on_process_log_block(&self, log_block: Arc<LogBlock>);   // stub on wasm
    fn on_init_metrics_stream(&self, metrics_stream: &MetricsStream);
    fn on_process_metrics_block(&self, metrics_block: Arc<MetricsBlock>);
    fn on_init_thread_stream(&self, thread_stream: &ThreadStream);
    fn on_process_thread_block(&self, thread_block: Arc<ThreadBlock>);
    fn is_busy(&self) -> bool;
}
```

### Stub types needed on wasm32

| Type | Native definition | Wasm stub |
|------|------------------|-----------|
| `LogBlock` | `EventBlock<LogMsgQueue>` (type alias, transit-heavy) | `pub struct LogBlock;` |
| `LogStream` | `EventStream<LogBlock>` (generic over transit queues) | `pub struct LogStream;` |
| `MetricsBlock` | `EventBlock<MetricsMsgQueue>` | `pub struct MetricsBlock;` |
| `MetricsStream` | `EventStream<MetricsBlock>` | `pub struct MetricsStream;` |
| `ThreadBlock` | `EventBlock<ThreadEventQueue>` | `pub struct ThreadBlock;` |
| `ThreadStream` | `EventStream<ThreadBlock>` | `pub struct ThreadStream;` |
| `Property` | struct with `StaticStringRef` fields, derives `TransitReflect` | `pub struct Property;` |

On wasm32, none of these are ever constructed. They exist purely to make `EventSink` compile.

## Plan

### Phase 1: Cargo.toml dependency gating

**`rust/tracing/Cargo.toml`**:

```toml
[dependencies]
# Shared deps (transit-free)
anyhow.workspace = true
chrono = { workspace = true, features = ["wasmbind"] }  # wasmbind is no-op on non-wasm
lazy_static.workspace = true
pin-project.workspace = true
serde.workspace = true
thiserror.workspace = true
uuid.workspace = true
micromegas-tracing-proc-macros = { path = "./proc-macros", version = "^0.21" }

# Optional runtime integration
tokio = { workspace = true, optional = true }

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
# Native-only: transit + platform deps
micromegas-transit.workspace = true
internment.workspace = true
memoffset.workspace = true
raw-cpuid.workspace = true
thread-id.workspace = true
whoami.workspace = true

[target.'cfg(target_arch = "wasm32")'.dependencies]
getrandom = { version = "0.3", features = ["wasm_js"] }
web-sys = { version = "0.3", features = ["console"] }

[target.'cfg(windows)'.dependencies]
winapi.workspace = true

[features]
default = ["tokio"]
tokio = ["dep:tokio"]
```

### Phase 2: `time.rs` — add wasm32 implementation

```rust
#[cfg(target_arch = "wasm32")]
pub fn now() -> i64 {
    (js_sys::Date::now() * 1000.0) as i64  // ms → µs
}

#[cfg(target_arch = "wasm32")]
pub fn frequency() -> i64 {
    1_000_000  // µs per second
}
```

`js_sys::Date::now()` is ms-precision — good enough for log timestamps. Works in both browser and web worker contexts. `js-sys` is a transitive dependency of `web-sys`.

`DualTime::now()` uses `chrono::Utc::now()` which works on wasm32 with the `wasmbind` feature.

### Phase 3: Stub types in sub-modules

Each of `logs/`, `metrics/`, `spans/` has a `block.rs` (transit-heavy) and `events.rs` (mixed). On wasm32, provide stub types in the mod.rs files.

**`logs/mod.rs`:**
```rust
#[cfg(not(target_arch = "wasm32"))]
mod block;
#[cfg(not(target_arch = "wasm32"))]
pub use block::*;

// Stubs for EventSink trait signature
#[cfg(target_arch = "wasm32")]
pub struct LogBlock;
#[cfg(target_arch = "wasm32")]
pub struct LogStream;

mod events;
pub use events::*;
```

**`metrics/mod.rs`:**
```rust
#[cfg(not(target_arch = "wasm32"))]
mod block;
#[cfg(not(target_arch = "wasm32"))]
pub use block::*;

#[cfg(target_arch = "wasm32")]
pub struct MetricsBlock;
#[cfg(target_arch = "wasm32")]
pub struct MetricsStream;

mod events;
pub use events::*;
```

**`spans/mod.rs`:**
```rust
#[cfg(not(target_arch = "wasm32"))]
mod block;
#[cfg(not(target_arch = "wasm32"))]
pub use block::*;

#[cfg(target_arch = "wasm32")]
pub struct ThreadBlock;
#[cfg(target_arch = "wasm32")]
pub struct ThreadStream;

mod events;
pub use events::*;

mod instrumented_future;  // transit-free, always needed
pub use instrumented_future::*;
```

**`logs/events.rs`**, **`metrics/events.rs`**, **`spans/events.rs`** — gate transit imports and event types, keep metadata:
```rust
// Always available (used by macros):
//   LogMetadata, FilterState, FILTER_LEVEL_UNSET_VALUE  (logs)
//   StaticMetricMetadata                                (metrics)
//   SpanLocation, SpanMetadata                          (spans)

// Native only (behind #[cfg(not(target_arch = "wasm32"))]):
//   All event types (LogStaticStrEvent, BeginThreadSpanEvent, etc.)
//   All transit imports (DynString, InProcSerialize, TransitReflect, etc.)
```

### Phase 4: `property_set` stub for wasm32

`property_set.rs` is transit-heavy (derives `TransitReflect`, `InProcSerialize`, uses `internment`). The `Property` type is referenced by `EventSink::on_log`. On wasm32, dispatch passes `&[]` — the type just needs to exist.

**Option A**: Gate entire `property_set.rs` native-only, provide stub in `lib.rs`:
```rust
#[cfg(not(target_arch = "wasm32"))]
pub mod property_set;

#[cfg(target_arch = "wasm32")]
pub mod property_set {
    /// Stub for EventSink trait compatibility — never constructed on wasm32
    pub struct Property;
}
```

**Option B**: cfg-gate within `property_set.rs` keeping only the `Property` struct definition on wasm. Option A is simpler.

### Phase 5: `event` module — make sink available on all platforms

The `event/` module currently has:
- `sink.rs` — `EventSink` trait + `NullEventSink` (needs to be available on wasm)
- `block.rs` — `EventBlock`, `TracingBlock`, `ExtractDeps` (transit-heavy, native only)
- `stream.rs` — `EventStream`, `StreamDesc` (transit-heavy, native only)
- `in_memory_sink.rs` — (native only)

Split the module so `EventSink` compiles on wasm:

**`event/mod.rs`:**
```rust
#[cfg(not(target_arch = "wasm32"))]
mod block;
#[cfg(not(target_arch = "wasm32"))]
pub use block::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod in_memory_sink;

mod sink;
pub use sink::*;

#[cfg(not(target_arch = "wasm32"))]
mod stream;
#[cfg(not(target_arch = "wasm32"))]
pub use stream::*;
```

`event/sink.rs` compiles on both platforms because all its referenced types exist (real on native, stubs on wasm).

### Phase 6: `dispatch_wasm.rs` — minimal wasm dispatch

New file `rust/tracing/src/dispatch_wasm.rs`. Same public function signatures as `dispatch.rs`, uses `EventSink` trait.

**Architecture:**
- Global `OnceLock<Arc<dyn EventSink>>` holds the sink (same trait as native)
- `init_event_dispatch()` stores the sink, calls `sink.on_startup()`
- `log()`, `log_tagged()`, `log_interop()` → forward to `sink.on_log(desc, &[], time, args)`
- `log_enabled()` → forward to `sink.on_log_enabled()`
- `int_metric()`, `float_metric()`, `tagged_*_metric()` → no-op
- `on_begin_scope()`, `on_end_scope()`, `on_begin_named_scope()`, `on_end_named_scope()` → no-op
- `on_begin_async_scope()` → returns incrementing span ID (needed by `InstrumentedFuture`)
- `on_end_async_scope()`, `on_begin_async_named_scope()`, `on_end_async_named_scope()` → no-op
- `shutdown_dispatch()` → calls `sink.on_shutdown()`
- `flush_*()`, `init_thread_stream()` → no-op
- `process_id()`, `cpu_tracing_enabled()`, `get_sink()` → work normally (sink stored in OnceLock)
- `force_uninit()`, `for_each_thread_stream()`, `unregister_thread_stream()` → no-op

No `DispatchCell`, no `Mutex<LogStream>`, no transit types. Same `EventSink` interface as native.

Since the init signature is the same (`Arc<dyn EventSink>`), **`guards.rs` needs no cfg-gating for the sink type** — only for the `make_process_info` call path.

### Phase 7: `guards.rs` adjustments

`init_telemetry()` and `TracingSystemGuard::new()` keep the same signature. Only the `make_process_info` function (currently in `dispatch.rs`) needs to exist in `dispatch_wasm.rs` too — with stub values for `whoami` fields.

```rust
// in dispatch_wasm.rs
pub fn make_process_info(
    process_id: uuid::Uuid,
    parent_process_id: Option<uuid::Uuid>,
    properties: HashMap<String, String>,
) -> ProcessInfo {
    ProcessInfo {
        process_id,
        username: String::from("wasm"),
        realname: String::from("wasm"),
        exe: String::from("wasm"),
        computer: String::from("wasm"),
        distro: String::from("wasm"),
        cpu_brand: String::from("wasm32"),
        tsc_frequency: frequency(),
        start_time: chrono::Utc::now(),
        start_ticks: now(),
        parent_process_id,
        properties,
    }
}
```

### Phase 8: `process_info.rs` — gate transit uuid_utils

```rust
#[cfg(not(target_arch = "wasm32"))]
use micromegas_transit::uuid_utils;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    #[cfg_attr(not(target_arch = "wasm32"), serde(
        deserialize_with = "uuid_utils::uuid_from_string",
        serialize_with = "uuid_utils::uuid_to_string"
    ))]
    pub process_id: uuid::Uuid,
    // ... same for parent_process_id ...
}
```

Alternatively, copy the 2 trivial uuid serde helpers locally to avoid the conditional attrs.

### Phase 9: cfg-gate remaining native-only modules in `lib.rs`

```rust
// dispatch: use wasm version on wasm32
#[cfg(not(target_arch = "wasm32"))]
pub mod dispatch;
#[cfg(target_arch = "wasm32")]
#[path = "dispatch_wasm.rs"]
pub mod dispatch;

// Available on all platforms (with stubs on wasm where needed)
pub mod errors;
pub mod event;         // sink.rs always, block/stream/in_memory native-only
pub mod guards;
pub mod levels;
pub mod logs;          // metadata always, block native-only, stubs on wasm
pub mod metrics;       // same pattern
pub mod panic_hook;
pub mod process_info;
pub mod property_set;  // full on native, stub Property on wasm
pub mod spans;         // same pattern as logs/metrics
pub mod time;

// Native-only modules (entirely transit-dependent)
#[cfg(not(target_arch = "wasm32"))]
pub mod flush_monitor;
#[cfg(not(target_arch = "wasm32"))]
pub mod intern_string;
#[cfg(not(target_arch = "wasm32"))]
pub mod parsing;
#[cfg(not(target_arch = "wasm32"))]
pub mod static_string_ref;
#[cfg(not(target_arch = "wasm32"))]
pub mod string_id;
#[cfg(not(target_arch = "wasm32"))]
pub mod test_utils;

#[cfg(feature = "tokio")]
pub mod runtime;
```

### Phase 10: `telemetry-sink` — wasm support

On native, applications initialize tracing via `TelemetryGuardBuilder` from `micromegas-telemetry-sink` — not by calling `TracingSystemGuard::new()` directly. The same crate should own the wasm initialization path.

The native and wasm sides share almost nothing today (the native side is HTTP transport, tokio, reqwest, sysinfo, etc.), but they live in the same crate with `#[cfg]` gating — same pattern as `dispatch.rs` / `dispatch_wasm.rs` in `micromegas-tracing`. When we later add data sending from wasm, more code can be shared.

**`rust/telemetry-sink/Cargo.toml`** — add wasm deps, gate native deps:

```toml
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
# existing native deps: reqwest, tokio, sysinfo, colored, ctrlc,
# tokio-retry2, tracing, tracing-core, tracing-subscriber, log,
# micromegas-transit, micromegas-telemetry, bytes, async-trait
...

[target.'cfg(target_arch = "wasm32")'.dependencies]
web-sys = { version = "0.3", features = ["console"] }

[dependencies]
# shared: micromegas-tracing, anyhow, chrono, lazy_static, serde, uuid
...
```

**New file `rust/telemetry-sink/src/console_event_sink.rs`** (wasm32-only):

```rust
use std::{fmt, sync::Arc};
use micromegas_tracing::event::EventSink;
use micromegas_tracing::logs::{LogBlock, LogMetadata, LogStream};
use micromegas_tracing::metrics::{MetricsBlock, MetricsStream};
use micromegas_tracing::process_info::ProcessInfo;
use micromegas_tracing::property_set::Property;
use micromegas_tracing::spans::{ThreadBlock, ThreadStream};

pub struct ConsoleEventSink;

impl EventSink for ConsoleEventSink {
    fn on_startup(&self, _: Arc<ProcessInfo>) {}
    fn on_shutdown(&self) {}
    fn on_log_enabled(&self, _: &LogMetadata) -> bool { true }

    fn on_log(&self, desc: &LogMetadata, _properties: &[Property], _time: i64, args: fmt::Arguments<'_>) {
        let msg = format!("[{}] {args}", desc.level);
        web_sys::console::log_1(&msg.into());
    }

    fn on_init_log_stream(&self, _: &LogStream) {}
    fn on_process_log_block(&self, _: Arc<LogBlock>) {}
    fn on_init_metrics_stream(&self, _: &MetricsStream) {}
    fn on_process_metrics_block(&self, _: Arc<MetricsBlock>) {}
    fn on_init_thread_stream(&self, _: &ThreadStream) {}
    fn on_process_thread_block(&self, _: Arc<ThreadBlock>) {}
    fn is_busy(&self) -> bool { false }
}
```

**Wasm guard builder** (in `lib.rs` or a new `wasm.rs`, behind `#[cfg(target_arch = "wasm32")]`):

```rust
use micromegas_tracing::guards::TracingSystemGuard;
use std::sync::Arc;

pub struct TelemetryGuard {
    _guard: Arc<TracingSystemGuard>,
}

pub fn init_telemetry() -> anyhow::Result<TelemetryGuard> {
    let guard = Arc::new(TracingSystemGuard::new(
        0, 0, 0,
        Arc::new(ConsoleEventSink),
        HashMap::new(),
        false,
    )?);
    Ok(TelemetryGuard { _guard: guard })
}
```

The native `TelemetryGuardBuilder` and all its machinery (`HttpEventSink`, `CompositeSink`, `LocalEventSink`, interop layers, system monitor, etc.) stay behind `#[cfg(not(target_arch = "wasm32"))]`.

**`rust/telemetry-sink/src/lib.rs`** gating:
```rust
#[cfg(not(target_arch = "wasm32"))]
mod http_event_sink;
#[cfg(not(target_arch = "wasm32"))]
mod composite_event_sink;
#[cfg(not(target_arch = "wasm32"))]
mod local_event_sink;
// ... etc for all native modules ...

#[cfg(target_arch = "wasm32")]
mod console_event_sink;
#[cfg(target_arch = "wasm32")]
pub use console_event_sink::*;

// Native builder
#[cfg(not(target_arch = "wasm32"))]
pub struct TelemetryGuardBuilder { ... }

// Wasm init
#[cfg(target_arch = "wasm32")]
pub fn init_telemetry() -> anyhow::Result<TelemetryGuard> { ... }
```

### Phase 11: Wire up in `datafusion-wasm`

**`rust/datafusion-wasm/Cargo.toml`**:
```toml
micromegas-tracing = { path = "../tracing", default-features = false }
micromegas-telemetry-sink = { path = "../telemetry-sink", default-features = false }
```

**`rust/datafusion-wasm/src/lib.rs`**:
```rust
use std::sync::Once;

static INIT: Once = Once::new();

fn ensure_tracing() {
    INIT.call_once(|| {
        let guard = micromegas_telemetry_sink::init_telemetry()
            .expect("failed to init telemetry");
        std::mem::forget(guard);  // leak — WASM module lives for page lifetime
    });
}
```

`dispatch_wasm.rs` forwards `on_log` directly to the sink — no buffering, no copying into streams.

## Files Summary

### `micromegas-tracing` crate

| File | Action |
|------|--------|
| `rust/tracing/Cargo.toml` | Gate `micromegas-transit`, `internment`, `memoffset`, `raw-cpuid`, `thread-id`, `whoami` as native-only; add `getrandom`+`web-sys` for wasm32; add `wasmbind` to chrono |
| `rust/tracing/src/lib.rs` | cfg-gate dispatch path swap + native-only modules |
| `rust/tracing/src/dispatch_wasm.rs` | **New** — minimal dispatch using `EventSink`, no transit |
| `rust/tracing/src/time.rs` | Add `#[cfg(target_arch = "wasm32")]` `now()` and `frequency()` |
| `rust/tracing/src/event/mod.rs` | Gate `block`, `stream`, `in_memory_sink` native-only; keep `sink` on all platforms |
| `rust/tracing/src/logs/mod.rs` | Gate `block`, add `LogBlock`/`LogStream` stubs on wasm32 |
| `rust/tracing/src/logs/events.rs` | Gate transit imports and event types, keep `LogMetadata` |
| `rust/tracing/src/metrics/mod.rs` | Gate `block`, add `MetricsBlock`/`MetricsStream` stubs on wasm32 |
| `rust/tracing/src/metrics/events.rs` | Gate transit imports and event types, keep `StaticMetricMetadata` |
| `rust/tracing/src/spans/mod.rs` | Gate `block`, add `ThreadBlock`/`ThreadStream` stubs on wasm32 |
| `rust/tracing/src/spans/events.rs` | Gate transit imports and event types, keep `SpanMetadata`/`SpanLocation` |
| `rust/tracing/src/property_set.rs` | Full on native; on wasm32, just `pub struct Property;` |
| `rust/tracing/src/process_info.rs` | Gate `uuid_utils` import, handle serde attrs |
| `rust/tracing/src/guards.rs` | Unchanged — same `TracingSystemGuard::new()` signature works on both platforms |

### `micromegas-telemetry-sink` crate

| File | Action |
|------|--------|
| `rust/telemetry-sink/Cargo.toml` | Gate native deps (`reqwest`, `tokio`, `sysinfo`, etc.) behind `cfg(not(wasm32))`; add `web-sys` for wasm32; keep shared deps (`micromegas-tracing`, `anyhow`, etc.) |
| `rust/telemetry-sink/src/lib.rs` | Gate all native modules (`http_event_sink`, `composite_event_sink`, etc.); expose wasm `init_telemetry()` + `ConsoleEventSink` |
| `rust/telemetry-sink/src/console_event_sink.rs` | **New** — `ConsoleEventSink` impl (wasm32-only) |

### `datafusion-wasm` crate

| File | Action |
|------|--------|
| `rust/datafusion-wasm/Cargo.toml` | Add `micromegas-tracing` + `micromegas-telemetry-sink` deps |
| `rust/datafusion-wasm/src/lib.rs` | Call `micromegas_telemetry_sink::init_telemetry()` at startup |

## What Does NOT Change

- `dispatch.rs` — untouched, native path unchanged
- `event/sink.rs` — `EventSink` trait unchanged, compiles on both platforms
- `telemetry-sink` native code — all existing modules untouched, just cfg-gated
- Proc macros (`span_fn`, `log_fn`) — generate code that calls `dispatch::*`, works with either dispatch module
- All macros (`info!`, `imetric!`, `span_scope!`, etc.) — unchanged
- `guards.rs` — same signature, same dispatch function calls
- `spans/instrumented_future.rs` — unchanged (transit-free, calls dispatch functions)
- `levels.rs` — unchanged (transit-free)

## Verification

1. Native build still works: `cd rust && cargo build -p micromegas-tracing`
2. Native tests pass: `cd rust && cargo test -p micromegas-tracing`
3. WASM build: `cargo build --target wasm32-unknown-unknown -p micromegas-tracing --no-default-features`
4. datafusion-wasm builds: `cd rust/datafusion-wasm && wasm-pack build --target web`
5. Manual test: log output appears in browser console when using datafusion-wasm

## Risks & Notes

- **Prelude on wasm32**: Re-exports `ProcessInfo` (transit-free after gating `uuid_utils`) and `DualTime`/`now()`/`frequency()` (transit-free). Works fine.
- **`lazy_static`**: Used in dispatch.rs for `INIT_MUTEX` and elsewhere. Works on wasm32 (single-threaded, no contention).
- **`internment` crate**: Used by `property_set.rs` for `PropertySet` interning. Gated with native-only code.
- **Stub type safety**: The stub types (`LogBlock`, `ThreadStream`, etc.) are empty and unconstructable in normal use. `dispatch_wasm.rs` never creates them — it only calls the `on_log` path. If someone tried to call `on_process_log_block` with a stub `Arc<LogBlock>`, it would compile but be meaningless. This is fine — it's the same guarantee as `NullEventSink`.
- **Future: re-enabling transit on wasm32**: If needed later (e.g., to buffer and batch-send events from WASM), transit could be made wasm-compatible and the stubs replaced with real types. The `EventSink` interface is already correct.
