//! C ABI for the Micromegas telemetry producer stack.
//!
//! Exposes init/shutdown/log/metric/flush as flat `extern "C"` functions that any
//! non-Rust process can load via dlopen (cdylib) or link at build time (staticlib).
//!
//! Threading model: all functions are safe to call from any thread.  The transport
//! runs on its own OS thread inside the library; callers need no async runtime.
//!
//! String cardinality contract: `mm_metric_i` / `mm_metric_f` intern each unique
//! `(name, unit)` pair via `Box::leak`.  Keep metric names low-cardinality and
//! bounded — do NOT pass unbounded values (session IDs, asset names) as metric names.

#![allow(unsafe_code, clippy::missing_safety_doc)]

use micromegas_telemetry_sink::{TelemetryGuard, TelemetryGuardBuilder};
use micromegas_tracing::dispatch;
use micromegas_tracing::levels::{Level, Verbosity};
use micromegas_tracing::logs::{FILTER_LEVEL_UNSET_VALUE, LogMetadata};
use micromegas_tracing::metrics::StaticMetricMetadata;
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_uint};
use std::sync::atomic::AtomicU32;
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Metric string interner — bounded by cardinality of (name, unit) pairs.
// ---------------------------------------------------------------------------

type MetricKey = (String, String); // (name, unit); target is always "capi"

fn metric_interner() -> &'static Mutex<HashMap<MetricKey, &'static StaticMetricMetadata>> {
    static INTERNER: OnceLock<Mutex<HashMap<MetricKey, &'static StaticMetricMetadata>>> =
        OnceLock::new();
    INTERNER.get_or_init(|| Mutex::new(HashMap::new()))
}

fn intern_metric(name: &str, unit: &str) -> &'static StaticMetricMetadata {
    let key = (name.to_owned(), unit.to_owned());
    let mut map = metric_interner().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(meta) = map.get(&key) {
        return meta;
    }
    let meta: &'static StaticMetricMetadata = Box::leak(Box::new(StaticMetricMetadata {
        lod: Verbosity::Min,
        name: Box::leak(name.to_owned().into_boxed_str()),
        unit: Box::leak(unit.to_owned().into_boxed_str()),
        target: "capi",
        file: "micromegas-capi",
        line: 0,
    }));
    map.insert(key, meta);
    meta
}

// ---------------------------------------------------------------------------
// Public C types
// ---------------------------------------------------------------------------

/// Configuration passed to `mm_init`.
///
/// All pointer fields may be null:
/// - `sink_url` null → reads `MICROMEGAS_TELEMETRY_URL` env var.
/// - `property_keys` / `property_values` null → no extra process properties.
/// - Auth is always taken from env: `MICROMEGAS_INGESTION_API_KEY` or OIDC vars.
#[repr(C)]
pub struct MmConfig {
    /// HTTP endpoint of the Micromegas ingestion server, e.g. "http://host:9000".
    pub sink_url: *const c_char,
    /// Parallel arrays of NUL-terminated key/value process property strings.
    pub property_keys: *const *const c_char,
    pub property_values: *const *const c_char,
    /// Number of key/value pairs in the arrays above.
    pub property_count: c_uint,
}

/// Opaque handle returned by `mm_init` and consumed by `mm_shutdown`.
pub struct MmHandle {
    _guard: TelemetryGuard,
}

// ---------------------------------------------------------------------------
// C API functions
// ---------------------------------------------------------------------------

/// Initialize the telemetry system and return an opaque handle.
///
/// Returns null on failure (e.g., invalid sink URL, internal error).
/// The caller must eventually call `mm_shutdown` to flush buffers and free resources.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mm_init(cfg: *const MmConfig) -> *mut MmHandle {
    let cfg = if cfg.is_null() {
        return std::ptr::null_mut();
    } else {
        unsafe { &*cfg }
    };

    let mut builder = TelemetryGuardBuilder::default()
        .with_local_sink_enabled(false)
        .with_install_tracing_capture(false)
        .with_auth_from_env();

    if !cfg.sink_url.is_null() {
        // Always call with_telemetry_sink_url even for empty strings so the
        // caller can explicitly opt out of the MICROMEGAS_TELEMETRY_URL env var.
        // The builder will filter empty URLs internally.
        let url = unsafe { CStr::from_ptr(cfg.sink_url) }
            .to_str()
            .unwrap_or_default()
            .to_owned();
        builder = builder.with_telemetry_sink_url(url);
    }

    if cfg.property_count > 0 && !cfg.property_keys.is_null() && !cfg.property_values.is_null() {
        let mut props = HashMap::new();
        for i in 0..cfg.property_count as usize {
            let key_ptr = unsafe { *cfg.property_keys.add(i) };
            let val_ptr = unsafe { *cfg.property_values.add(i) };
            if key_ptr.is_null() || val_ptr.is_null() {
                continue;
            }
            let key = match unsafe { CStr::from_ptr(key_ptr) }.to_str() {
                Ok(s) if !s.is_empty() => s.to_owned(),
                _ => continue,
            };
            let val = unsafe { CStr::from_ptr(val_ptr) }
                .to_str()
                .unwrap_or("")
                .to_owned();
            props.insert(key, val);
        }
        if !props.is_empty() {
            builder = builder.with_process_properties(props);
        }
    }

    match builder.build() {
        Ok(guard) => Box::into_raw(Box::new(MmHandle { _guard: guard })),
        Err(e) => {
            eprintln!("mm_init: failed to initialize telemetry: {e}");
            std::ptr::null_mut()
        }
    }
}

/// Flush all pending events and shut down the telemetry system.
///
/// After this call `handle` is invalid and must not be used.
/// Safe to call with a null handle (no-op).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mm_shutdown(handle: *mut MmHandle) {
    if !handle.is_null() {
        unsafe { drop(Box::from_raw(handle)) };
    }
}

/// Emit a log event.
///
/// `level` maps to the Micromegas level constants defined in `micromegas.h`
/// (1=Fatal … 6=Trace).  Null `target` defaults to "capi".  Null `msg` is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mm_log(
    handle: *mut MmHandle,
    level: c_int,
    target: *const c_char,
    msg: *const c_char,
) {
    if handle.is_null() {
        return;
    }
    if msg.is_null() {
        return;
    }
    let msg_str = match unsafe { CStr::from_ptr(msg) }.to_str() {
        Ok(s) => s,
        Err(_) => return,
    };
    let target_str = if target.is_null() {
        "capi"
    } else {
        unsafe { CStr::from_ptr(target) }.to_str().unwrap_or("capi")
    };
    let level = match level {
        1 => Level::Fatal,
        2 => Level::Error,
        3 => Level::Warn,
        4 => Level::Info,
        5 => Level::Debug,
        _ => Level::Trace,
    };
    let metadata = LogMetadata {
        level,
        level_filter: AtomicU32::new(FILTER_LEVEL_UNSET_VALUE),
        fmt_str: "",
        target: target_str,
        module_path: "",
        file: "",
        line: 0,
    };
    dispatch::log_interop(&metadata, format_args!("{msg_str}"));
}

/// Emit an integer metric.
///
/// `name` and `unit` must not be null.  Each unique `(name, unit)` pair is
/// interned on first use — keep cardinality bounded.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mm_metric_i(
    handle: *mut MmHandle,
    name: *const c_char,
    unit: *const c_char,
    value: u64,
) {
    if handle.is_null() {
        return;
    }
    if name.is_null() {
        return;
    }
    let name_str = match unsafe { CStr::from_ptr(name) }.to_str() {
        Ok(s) if !s.is_empty() => s,
        _ => return,
    };
    let unit_str = if unit.is_null() {
        ""
    } else {
        unsafe { CStr::from_ptr(unit) }.to_str().unwrap_or("")
    };
    let meta = intern_metric(name_str, unit_str);
    dispatch::int_metric(meta, value);
}

/// Emit a floating-point metric.
///
/// Same cardinality contract as `mm_metric_i`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mm_metric_f(
    handle: *mut MmHandle,
    name: *const c_char,
    unit: *const c_char,
    value: f64,
) {
    if handle.is_null() {
        return;
    }
    if name.is_null() {
        return;
    }
    let name_str = match unsafe { CStr::from_ptr(name) }.to_str() {
        Ok(s) if !s.is_empty() => s,
        _ => return,
    };
    let unit_str = if unit.is_null() {
        ""
    } else {
        unsafe { CStr::from_ptr(unit) }.to_str().unwrap_or("")
    };
    let meta = intern_metric(name_str, unit_str);
    dispatch::float_metric(meta, value);
}

/// Flush all in-memory log and metric buffers to the transport.
///
/// The transport thread will then ship them to the ingestion server.
/// Safe to call with a null handle (operates on the global dispatch).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mm_flush(_handle: *mut MmHandle) {
    dispatch::flush_log_buffer();
    dispatch::flush_metrics_buffer();
}
