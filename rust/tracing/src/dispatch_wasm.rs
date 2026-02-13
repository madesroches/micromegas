//! Minimal wasm32 dispatch — forwards logs to EventSink, no-ops everything else
pub use crate::errors::{Error, Result};
use crate::event::EventSink;
use crate::logs::LogMetadata;
use crate::metrics::StaticMetricMetadata;
use crate::process_info::ProcessInfo;
use crate::property_set::PropertySet;
use crate::spans::{SpanLocation, SpanMetadata, ThreadStream};
use crate::time::{frequency, now};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, OnceLock};

struct WasmDispatch {
    process_id: uuid::Uuid,
    sink: Arc<dyn EventSink>,
}

static G_DISPATCH: OnceLock<WasmDispatch> = OnceLock::new();
static G_ASYNC_SPAN_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn init_event_dispatch(
    _logs_buffer_size: usize,
    _metrics_buffer_size: usize,
    _threads_buffer_size: usize,
    sink: Arc<dyn EventSink>,
    process_properties: HashMap<String, String>,
    _cpu_tracing_enabled: bool,
) -> Result<()> {
    let process_id = uuid::Uuid::new_v4();
    let process_info = Arc::new(make_process_info(process_id, None, process_properties));
    let dispatch = WasmDispatch {
        process_id,
        sink: sink.clone(),
    };
    G_DISPATCH
        .set(dispatch)
        .map_err(|_| Error::AlreadyInitialized())?;
    crate::levels::set_max_level(crate::levels::LevelFilter::Trace);
    sink.on_startup(process_info);
    Ok(())
}

#[inline]
pub fn process_id() -> Option<uuid::Uuid> {
    G_DISPATCH.get().map(|d| d.process_id)
}

#[inline]
pub fn cpu_tracing_enabled() -> Option<bool> {
    G_DISPATCH.get().map(|_| false)
}

pub fn get_sink() -> Option<Arc<dyn EventSink>> {
    G_DISPATCH.get().map(|d| d.sink.clone())
}

pub fn shutdown_dispatch() {
    if let Some(d) = G_DISPATCH.get() {
        d.sink.on_shutdown();
    }
}

#[inline(always)]
pub fn int_metric(_metric_desc: &'static StaticMetricMetadata, _value: u64) {}

#[inline(always)]
pub fn float_metric(_metric_desc: &'static StaticMetricMetadata, _value: f64) {}

#[inline(always)]
pub fn tagged_float_metric(
    _desc: &'static StaticMetricMetadata,
    _properties: &'static PropertySet,
    _value: f64,
) {
}

#[inline(always)]
pub fn tagged_integer_metric(
    _desc: &'static StaticMetricMetadata,
    _properties: &'static PropertySet,
    _value: u64,
) {
}

#[inline(always)]
pub fn log(desc: &'static LogMetadata, args: fmt::Arguments<'_>) {
    if let Some(d) = G_DISPATCH.get() {
        if d.sink.on_log_enabled(desc) {
            d.sink.on_log(desc, &[], now(), args);
        }
    }
}

#[inline(always)]
pub fn log_tagged(
    desc: &'static LogMetadata,
    _properties: &'static PropertySet,
    args: fmt::Arguments<'_>,
) {
    if let Some(d) = G_DISPATCH.get() {
        if d.sink.on_log_enabled(desc) {
            d.sink.on_log(desc, &[], now(), args);
        }
    }
}

#[inline(always)]
pub fn log_interop(metadata: &LogMetadata, args: fmt::Arguments<'_>) {
    if let Some(d) = G_DISPATCH.get() {
        d.sink.on_log(metadata, &[], now(), args);
    }
}

#[inline(always)]
pub fn log_enabled(metadata: &LogMetadata) -> bool {
    if let Some(d) = G_DISPATCH.get() {
        d.sink.on_log_enabled(metadata)
    } else {
        false
    }
}

#[inline(always)]
pub fn flush_log_buffer() {}

#[inline(always)]
pub fn flush_metrics_buffer() {}

#[inline(always)]
pub fn init_thread_stream() {}

pub fn for_each_thread_stream(_fun: &mut dyn FnMut(*mut ThreadStream)) {}

#[inline(always)]
pub fn flush_thread_buffer() {}

#[inline(always)]
pub fn unregister_thread_stream() {}

#[inline(always)]
pub fn on_begin_scope(_scope: &'static SpanMetadata) {}

#[inline(always)]
pub fn on_end_scope(_scope: &'static SpanMetadata) {}

#[inline(always)]
pub fn on_begin_named_scope(_thread_span_location: &'static SpanLocation, _name: &'static str) {}

#[inline(always)]
pub fn on_end_named_scope(_thread_span_location: &'static SpanLocation, _name: &'static str) {}

#[inline(always)]
pub fn on_begin_async_scope(
    _scope: &'static SpanMetadata,
    _parent_span_id: u64,
    _depth: u32,
) -> u64 {
    G_ASYNC_SPAN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[inline(always)]
pub fn on_end_async_scope(
    _span_id: u64,
    _parent_span_id: u64,
    _scope: &'static SpanMetadata,
    _depth: u32,
) {
}

#[inline(always)]
pub fn on_begin_async_named_scope(
    _span_location: &'static SpanLocation,
    _name: &'static str,
    _parent_span_id: u64,
    _depth: u32,
) -> u64 {
    G_ASYNC_SPAN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[inline(always)]
pub fn on_end_async_named_scope(
    _span_id: u64,
    _parent_span_id: u64,
    _span_location: &'static SpanLocation,
    _name: &'static str,
    _depth: u32,
) {
}

/// # Safety
/// No-op on wasm32
pub unsafe fn force_uninit() {}

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
