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
| `PropertySet` | vec of `Property`, used by tagged dispatch functions | `pub struct PropertySet;` |

On wasm32, none of these are ever constructed. They exist purely to make `EventSink` and `dispatch` compile.

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
js-sys = "0.3"

[target.'cfg(windows)'.dependencies]
winapi.workspace = true

[features]
default = ["tokio"]
tokio = ["dep:tokio"]
```

### Phase 2: `time.rs` — add wasm32 implementation

Add wasm32 `now()` alongside the existing arch-specific definitions:

```rust
#[cfg(target_arch = "wasm32")]
pub fn now() -> i64 {
    (js_sys::Date::now() * 1000.0) as i64  // ms → µs
}
```

Gate the existing `frequency()` as native-only and add a wasm32 version. The existing function has no outer `#[cfg]` — its inner `#[cfg]` blocks are eliminated on wasm32 and it falls through to `return 0`, which would conflict with a separate wasm32 definition. Fix by gating:

```rust
#[allow(unreachable_code)]
#[cfg(not(target_arch = "wasm32"))]
pub fn frequency() -> i64 {
    #[cfg(windows)]
    return freq_windows();

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        let cpuid = raw_cpuid::CpuId::new();
        return cpuid
            .get_tsc_info()
            .map(|tsc_info| tsc_info.tsc_frequency().unwrap_or(0))
            .unwrap_or(0) as i64;
    }
    #[cfg(target_arch = "aarch64")]
    {
        let counter_frequency: i64;
        unsafe {
            core::arch::asm!(
                "mrs x0, cntfrq_el0",
                out("x0") counter_frequency
            );
        }
        return counter_frequency;
    }
    0
}

#[cfg(target_arch = "wasm32")]
pub fn frequency() -> i64 {
    1_000_000  // µs per second (matches now() which returns µs)
}
```

`js_sys::Date::now()` is ms-precision — good enough for log timestamps. Works in both browser and web worker contexts. `js-sys` is an explicit dependency of the tracing crate on wasm32.

`DualTime::now()` uses `chrono::Utc::now()` which works on wasm32 with the `wasmbind` feature.

### Phase 3: Stub types in sub-modules

Each of `logs/`, `metrics/`, `spans/` has a `block.rs` (transit-heavy) and `events.rs` (mixed). Each `events.rs` mixes transit-free metadata types with transit-heavy event types. Rather than littering each file with `#[cfg]` on every type, split into separate files: metadata stays in `events.rs`, event types move to a native-only file. The `mod.rs` handles gating and stub types.

**`logs/mod.rs`:**
```rust
#[cfg(not(target_arch = "wasm32"))]
mod block;
#[cfg(not(target_arch = "wasm32"))]
pub use block::*;

#[cfg(target_arch = "wasm32")]
pub struct LogBlock;
#[cfg(target_arch = "wasm32")]
pub struct LogStream;

mod events;       // LogMetadata, FilterState, FILTER_LEVEL_UNSET_VALUE (transit-free, all platforms)
pub use events::*;

#[cfg(not(target_arch = "wasm32"))]
mod log_events;   // LogStaticStrEvent, LogStringEvent, etc. (transit-heavy, native only)
#[cfg(not(target_arch = "wasm32"))]
pub use log_events::*;
```

**`logs/events.rs`** — only metadata, transit-free:
```rust
use crate::levels::{Level, LevelFilter};
use std::sync::atomic::{AtomicU32, Ordering};

pub struct LogMetadata<'a> { ... }  // Level, AtomicU32, &str
pub enum FilterState { ... }
pub const FILTER_LEVEL_UNSET_VALUE: u32 = 0xF;
impl LogMetadata<'_> { ... }       // level_filter, set_level_filter
```

**`logs/log_events.rs`** (**new**, native-only) — all event types, moved from events.rs:
```rust
use crate::{property_set::PropertySet, static_string_ref::StaticStringRef, string_id::StringId};
use micromegas_transit::{DynString, UserDefinedType, prelude::*, read_advance_string, read_consume_pod};
use super::LogMetadata;

const _: () = assert!(std::mem::size_of::<usize>() == 8);

pub struct LogStaticStrEvent { ... }
pub struct LogStringEvent { ... }
// ... etc
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

mod events;       // StaticMetricMetadata (transit-free, all platforms)
pub use events::*;

#[cfg(not(target_arch = "wasm32"))]
mod metric_events;  // IntegerMetricEvent, FloatMetricEvent, etc. (native only)
#[cfg(not(target_arch = "wasm32"))]
pub use metric_events::*;
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

mod events;       // SpanLocation, SpanMetadata (transit-free, all platforms)
pub use events::*;

#[cfg(not(target_arch = "wasm32"))]
mod span_events;  // BeginThreadSpanEvent, SpanRecord, etc. (native only)
#[cfg(not(target_arch = "wasm32"))]
pub use span_events::*;

mod instrumented_future;  // transit-free, always needed
pub use instrumented_future::*;
```

Each file is clean — either fully platform-independent or fully native-only. The `mod.rs` handles the gating.

Note: On wasm32, the module only re-exports the metadata types. Event types are absent from the wasm public API — this is correct since `dispatch_wasm.rs` never constructs them.

### Phase 4: `property_set` stub for wasm32

`property_set.rs` is transit-heavy (derives `TransitReflect`, `InProcSerialize`, uses `internment`). The `Property` type is referenced by `EventSink::on_log`, and `PropertySet` is referenced by dispatch functions `log_tagged`, `tagged_float_metric`, and `tagged_integer_metric` (called from `log!`, `imetric!`, `fmetric!` macros with `properties:` variants). On wasm32, dispatch passes `&[]` for properties — these types just need to exist.

Gate entire `property_set.rs` native-only, provide stubs in `lib.rs`:
```rust
#[cfg(not(target_arch = "wasm32"))]
pub mod property_set;

#[cfg(target_arch = "wasm32")]
pub mod property_set {
    /// Stub for EventSink trait compatibility — never constructed on wasm32
    pub struct Property;
    /// Stub for dispatch function signatures — never constructed on wasm32
    pub struct PropertySet;
}
```

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

New file `rust/tracing/src/dispatch_wasm.rs`. Must export every public symbol that `dispatch.rs` exports, since `guards.rs`, macros, and `instrumented_future.rs` all import from `crate::dispatch::*`.

**Required re-export** (must match `dispatch.rs` line 2):
```rust
pub use crate::errors::{Error, Result};
```

**Required imports:**
```rust
use crate::event::{EventSink, NullEventSink};
use crate::logs::LogMetadata;
use crate::metrics::StaticMetricMetadata;
use crate::process_info::ProcessInfo;
use crate::property_set::PropertySet;  // wasm stub
use crate::spans::{SpanLocation, SpanMetadata, ThreadStream};
use crate::time::{frequency, now};
```

**Architecture:**
- Global `OnceLock<WasmDispatch>` holds a struct with `process_id: Uuid` and `sink: Arc<dyn EventSink>` (same trait as native)

**Complete function list** (every pub fn must exist for callers to compile):

| Function | Behavior on wasm | Called by |
|----------|-----------------|-----------|
| `init_event_dispatch(logs_buffer_size, metrics_buffer_size, threads_buffer_size, sink, process_properties, cpu_tracing_enabled)` | Store sink in OnceLock, call `sink.on_startup()` | `guards.rs` |
| `log(desc: &'static LogMetadata, args: fmt::Arguments)` | Forward to `sink.on_log(desc, &[], time, args)` | `log!` macro |
| `log_tagged(desc: &'static LogMetadata, properties: &'static PropertySet, args: fmt::Arguments)` | Forward to `sink.on_log(desc, &[], time, args)` | `log!(properties:)` macro |
| `log_interop(metadata: &LogMetadata, args: fmt::Arguments)` | Forward to `sink.on_log(desc, &[], time, args)` | `log_interop` module |
| `log_enabled(metadata: &LogMetadata) -> bool` | Forward to `sink.on_log_enabled()` | `log_enabled!` macro |
| `int_metric(desc: &'static StaticMetricMetadata, value: u64)` | no-op | `imetric!` macro |
| `float_metric(desc: &'static StaticMetricMetadata, value: f64)` | no-op | `fmetric!` macro |
| `tagged_float_metric(desc, properties: &'static PropertySet, value: f64)` | no-op | `fmetric!(properties)` macro |
| `tagged_integer_metric(desc, properties: &'static PropertySet, value: u64)` | no-op | `imetric!(properties)` macro |
| `on_begin_scope(scope: &'static SpanMetadata)` | no-op | `guards::ThreadSpanGuard` |
| `on_end_scope(scope: &'static SpanMetadata)` | no-op | `guards::ThreadSpanGuard` |
| `on_begin_named_scope(location: &'static SpanLocation, name: &'static str)` | no-op | `guards::ThreadNamedSpanGuard` |
| `on_end_named_scope(location: &'static SpanLocation, name: &'static str)` | no-op | `guards::ThreadNamedSpanGuard` |
| `on_begin_async_scope(scope, parent_span_id, depth) -> u64` | Return incrementing span ID | `InstrumentedFuture` |
| `on_end_async_scope(span_id, parent_span_id, scope, depth)` | no-op | `InstrumentedFuture` |
| `on_begin_async_named_scope(location, name, parent_span_id, depth) -> u64` | Return incrementing span ID | `InstrumentedNamedFuture` |
| `on_end_async_named_scope(span_id, parent_span_id, location, name, depth)` | no-op | `InstrumentedNamedFuture` |
| `shutdown_dispatch()` | Call `sink.on_shutdown()` | `guards::shutdown_telemetry` |
| `flush_log_buffer()` | no-op | `guards::shutdown_telemetry` |
| `flush_metrics_buffer()` | no-op | `guards::shutdown_telemetry` |
| `flush_thread_buffer()` | no-op | `guards::TracingThreadGuard::drop` |
| `init_thread_stream()` | no-op | `guards::TracingThreadGuard::new` |
| `process_id() -> Option<uuid::Uuid>` | Return stored process ID | various |
| `cpu_tracing_enabled() -> Option<bool>` | Return `Some(false)` | various |
| `get_sink() -> Option<Arc<dyn EventSink>>` | Return stored sink | various |
| `force_uninit()` | no-op | test code |
| `for_each_thread_stream(fun: &mut dyn FnMut(*mut ThreadStream))` | no-op | native-only callers |
| `unregister_thread_stream()` | no-op | native-only callers |
| `make_process_info(process_id, parent_process_id, properties) -> ProcessInfo` | Stub values (see Phase 7) | `init_event_dispatch` |

No `DispatchCell`, no `Mutex<LogStream>`, no transit types. Same `EventSink` interface as native.

Since the init signature is the same (`Arc<dyn EventSink>`), **`guards.rs` needs no cfg-gating for the sink type** — only `panic_hook` needs a wasm fix (see Phase 7).

### Phase 7: `guards.rs` and `panic_hook.rs` adjustments

`guards.rs`: `init_telemetry()` and `TracingSystemGuard::new()` keep the same signature. All dispatch functions they call (`init_event_dispatch`, `flush_log_buffer`, `flush_metrics_buffer`, `shutdown_dispatch`, `init_thread_stream`, `flush_thread_buffer`, `on_begin_scope`, `on_end_scope`, etc.) exist in `dispatch_wasm.rs` as no-ops. No changes needed to `guards.rs` itself.

`panic_hook.rs`: `std::io::stdout().flush()` will panic on wasm32 (no stdout). Gate the stdout flush:
```rust
// in panic_hook.rs, inside the panic hook closure:
#[cfg(not(target_arch = "wasm32"))]
std::io::stdout().flush().unwrap();
```

The rest of `panic_hook.rs` works on wasm32: `fatal!` expands to `dispatch::log()` (routed to `dispatch_wasm`), `shutdown_telemetry()` calls dispatch no-ops, and `std::panic::set_hook` is supported on wasm.

`make_process_info` in `dispatch_wasm.rs` — stub values for `whoami` fields:
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

### Phase 8: `process_info.rs` — remove transit uuid_utils dependency

Copy the 4 trivial uuid serde helpers (~15 lines) from `micromegas_transit::uuid_utils` into `process_info.rs` locally. This avoids conditional `#[cfg_attr]` on serde derives and keeps `ProcessInfo` identical on both platforms:

```rust
// Local uuid serde helpers (copied from micromegas_transit::uuid_utils)
mod uuid_serde {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn uuid_to_string<S>(id: &uuid::Uuid, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer {
        serializer.serialize_str(&id.to_string())
    }

    pub fn uuid_from_string<'de, D>(deserializer: D) -> Result<uuid::Uuid, D::Error>
    where D: Deserializer<'de> {
        let s = String::deserialize(deserializer)?;
        uuid::Uuid::try_parse(&s).map_err(serde::de::Error::custom)
    }

    pub fn opt_uuid_to_string<S>(id: &Option<uuid::Uuid>, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer {
        match id {
            Some(id) => serializer.serialize_some(&id.to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn opt_uuid_from_string<'de, D>(deserializer: D) -> Result<Option<uuid::Uuid>, D::Error>
    where D: Deserializer<'de> {
        let s: Option<String> = Option::deserialize(deserializer)?;
        match s {
            Some(s) => uuid::Uuid::try_parse(&s).map(Some).map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    #[serde(
        deserialize_with = "uuid_serde::uuid_from_string",
        serialize_with = "uuid_serde::uuid_to_string"
    )]
    pub process_id: uuid::Uuid,
    // ... all other fields unchanged ...
    #[serde(
        deserialize_with = "uuid_serde::opt_uuid_from_string",
        serialize_with = "uuid_serde::opt_uuid_to_string"
    )]
    pub parent_process_id: Option<uuid::Uuid>,
    // ...
}
```

No `#[cfg]` needed — `ProcessInfo` compiles identically on all platforms.

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
[dependencies]
# Shared (transit-free)
anyhow.workspace = true
chrono.workspace = true
lazy_static.workspace = true
micromegas-tracing.workspace = true
serde.workspace = true
uuid.workspace = true

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
# Native-only: transport, runtime, system info, interop
micromegas-telemetry.workspace = true
micromegas-transit.workspace = true
async-trait.workspace = true
bytes.workspace = true
colored.workspace = true
ctrlc.workspace = true
log.workspace = true
reqwest.workspace = true
sysinfo.workspace = true
tokio.workspace = true
tokio-retry2.workspace = true
tracing.workspace = true
tracing-core.workspace = true
tracing-subscriber.workspace = true

[target.'cfg(target_arch = "wasm32")'.dependencies]
web-sys = { version = "0.3", features = ["console"] }
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
| `rust/tracing/Cargo.toml` | Gate `micromegas-transit`, `internment`, `memoffset`, `raw-cpuid`, `thread-id`, `whoami` as native-only; add `getrandom`+`js-sys` for wasm32; add `wasmbind` to chrono |
| `rust/tracing/src/lib.rs` | cfg-gate dispatch path swap + native-only modules |
| `rust/tracing/src/dispatch_wasm.rs` | **New** — minimal dispatch using `EventSink`, no transit |
| `rust/tracing/src/time.rs` | Add `#[cfg(target_arch = "wasm32")]` `now()` and `frequency()`; gate existing `frequency()` with `#[cfg(not(target_arch = "wasm32"))]` to avoid duplicate definition |
| `rust/tracing/src/event/mod.rs` | Gate `block`, `stream`, `in_memory_sink` native-only; keep `sink` on all platforms |
| `rust/tracing/src/logs/mod.rs` | Gate `block`, add `LogBlock`/`LogStream` stubs on wasm32 |
| `rust/tracing/src/logs/events.rs` | Keep only metadata: `LogMetadata`, `FilterState`, `FILTER_LEVEL_UNSET_VALUE` (remove transit imports + event types) |
| `rust/tracing/src/logs/log_events.rs` | **New** — native-only event types moved from events.rs: `LogStaticStrEvent`, `LogStringEvent`, etc. (includes 64-bit assert) |
| `rust/tracing/src/metrics/mod.rs` | Gate `block`, add `MetricsBlock`/`MetricsStream` stubs on wasm32 |
| `rust/tracing/src/metrics/events.rs` | Keep only metadata: `StaticMetricMetadata` (remove transit imports + event types) |
| `rust/tracing/src/metrics/metric_events.rs` | **New** — native-only event types moved from events.rs: `IntegerMetricEvent`, `FloatMetricEvent`, etc. |
| `rust/tracing/src/spans/mod.rs` | Gate `block`, add `ThreadBlock`/`ThreadStream` stubs on wasm32 |
| `rust/tracing/src/spans/events.rs` | Keep only metadata: `SpanLocation`, `SpanMetadata` (remove transit imports + event/record types) |
| `rust/tracing/src/spans/span_events.rs` | **New** — native-only event types moved from events.rs: `BeginThreadSpanEvent`, `SpanRecord`, etc. |
| `rust/tracing/src/property_set.rs` | Full on native; on wasm32, stubs: `pub struct Property;` + `pub struct PropertySet;` |
| `rust/tracing/src/process_info.rs` | Copy uuid serde helpers locally, remove `micromegas_transit::uuid_utils` import |
| `rust/tracing/src/panic_hook.rs` | Gate `stdout().flush()` behind `cfg(not(wasm32))` |
| `rust/tracing/src/guards.rs` | Unchanged — same signatures, all dispatch imports resolve on both platforms |

### `micromegas-telemetry-sink` crate

| File | Action |
|------|--------|
| `rust/telemetry-sink/Cargo.toml` | Gate native deps (`reqwest`, `tokio`, `sysinfo`, `micromegas-telemetry`, `micromegas-transit`, etc.) behind `cfg(not(wasm32))`; add `web-sys` for wasm32; keep shared deps (`micromegas-tracing.workspace = true`, `anyhow`, etc.) — note: uses workspace inheritance without `default-features = false` due to Cargo limitation |
| `rust/telemetry-sink/src/lib.rs` | Gate all native modules (`http_event_sink`, `composite_event_sink`, etc.); expose wasm `init_telemetry()` + `ConsoleEventSink` |
| `rust/telemetry-sink/src/console_event_sink.rs` | **New** — `ConsoleEventSink` impl (wasm32-only) |

### `datafusion-wasm` crate

| File | Action |
|------|--------|
| `rust/datafusion-wasm/Cargo.toml` | Add `micromegas-tracing` + `micromegas-telemetry-sink` deps (both `default-features = false`); add `micromegas-tracing` to cargo-machete ignored list |
| `rust/datafusion-wasm/src/lib.rs` | Call `micromegas_telemetry_sink::init_telemetry()` at startup |

## What Does NOT Change

- `dispatch.rs` — untouched, native path unchanged
- `event/sink.rs` — `EventSink` trait unchanged, compiles on both platforms
- `telemetry-sink` native code — all existing modules untouched, just cfg-gated
- Proc macros (`span_fn`, `log_fn`) — generate code that calls `dispatch::*`, works with either dispatch module
- All macros (`info!`, `imetric!`, `span_scope!`, etc.) — unchanged
- `guards.rs` — same signatures, all dispatch imports resolve on both platforms
- `spans/instrumented_future.rs` — unchanged (transit-free, `thread_local!` works on wasm32 as single-threaded global)
- `levels.rs` — unchanged (transit-free)
- `errors.rs` — unchanged (transit-free)

## Implementation Status

All phases implemented. Native verification complete:
- `cargo build --workspace` — passes
- `cargo test --workspace` — all tests pass, zero failures
- `cargo clippy -p micromegas-tracing -p micromegas-telemetry-sink -- -D warnings` — clean
- `cargo fmt --check` — clean

### Phases completed:
1. **Phase 1**: Gated native-only deps in `tracing/Cargo.toml`, added wasm32 deps
2. **Phase 2**: Added wasm32 `now()` and `frequency()` in `time.rs`
3. **Phase 3**: Split event files — metadata types in `events.rs`, transit-heavy types in `*_events.rs` (native-only)
4. **Phase 4-5**: Added property_set stubs, gated event submodules
5. **Phase 6**: Created `dispatch_wasm.rs` with OnceLock-based dispatch
6. **Phase 7-9**: Gated `panic_hook.rs` flush, made `process_info.rs` transit-free, cfg-gated `lib.rs` modules
7. **Phase 10**: Gated telemetry-sink native modules, created `ConsoleEventSink`, added wasm `init_telemetry()`
8. **Phase 11**: Wired up `datafusion-wasm` with `ensure_tracing()` init

### Implementation note:
- `telemetry-sink/Cargo.toml` uses `micromegas-tracing.workspace = true` (not `default-features = false`) because workspace dependency inheritance doesn't allow overriding default-features. This is fine — the `tokio` feature being enabled doesn't affect wasm builds since `datafusion-wasm` is excluded from the workspace and resolves its own dependency tree.

### Remaining verification:
- WASM build: `cargo build --target wasm32-unknown-unknown -p micromegas-tracing --no-default-features`
- datafusion-wasm builds: `cd rust/datafusion-wasm && wasm-pack build --target web`
- Manual test: log output appears in browser console when using datafusion-wasm

## Risks & Notes

- **Prelude on wasm32**: Re-exports `ProcessInfo` (transit-free after gating `uuid_utils`) and `DualTime`/`now()`/`frequency()` (transit-free). Works fine.
- **`lazy_static`**: Used in dispatch.rs for `INIT_MUTEX` and elsewhere. Works on wasm32 (single-threaded, no contention).
- **`internment` crate**: Used by `property_set.rs` for `PropertySet` interning. Gated with native-only code.
- **Stub type safety**: The stub types (`LogBlock`, `ThreadStream`, etc.) are empty and unconstructable in normal use. `dispatch_wasm.rs` never creates them — it only calls the `on_log` path. If someone tried to call `on_process_log_block` with a stub `Arc<LogBlock>`, it would compile but be meaningless. This is fine — it's the same guarantee as `NullEventSink`.
- **Future: re-enabling transit on wasm32**: If needed later (e.g., to buffer and batch-send events from WASM), transit could be made wasm-compatible and the stubs replaced with real types. The `EventSink` interface is already correct.
