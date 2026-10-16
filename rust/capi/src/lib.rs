//! C ABI for the Micromegas telemetry producer stack.
//!
//! Exposes init/shutdown/log/metric/flush as flat `extern "C"` functions that any
//! non-Rust process can load via dlopen (cdylib) or link at build time (staticlib).
//!
//! Threading model: all functions are safe to call from any thread.  The transport
//! runs on its own OS thread inside the library; callers need no async runtime.
//!
//! Everything a caller passes (metric names, units, log targets, property keys and values) is
//! copied into the block being filled and freed with it; nothing is retained for the life of the
//! process.  Cardinality still matters for queries: every distinct metric name is a series and
//! every distinct property set is a dictionary entry, so keep them drawn from bounded sets.

#![allow(unsafe_code, clippy::missing_safety_doc)]

use micromegas_telemetry_sink::{TelemetryGuard, TelemetryGuardBuilder};
use micromegas_tracing::dispatch;
use micromegas_tracing::levels::{Level, Verbosity};
use micromegas_tracing::logs::{FILTER_LEVEL_UNSET_VALUE, LogMetadata};
use micromegas_tracing::metrics::StaticMetricMetadata;
use micromegas_tracing::property_set::{DynProperty, PropertySet};
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_uint};
use std::sync::atomic::AtomicU32;

// Name and unit travel as `name` and `unit` property overrides, so one descriptor serves both paths.
static METRIC_DESC: StaticMetricMetadata = StaticMetricMetadata {
    lod: Verbosity::Min,
    name: "",
    unit: "",
    target: "capi",
    file: "micromegas-capi",
    line: 0,
};

/// Reads pair `i` of parallel key/value arrays.  A pair is skipped when its key or value is
/// null, or when the key is empty or not UTF-8; a value that is not UTF-8 reads as "".
unsafe fn read_pair<'a>(
    keys: *const *const c_char,
    values: *const *const c_char,
    i: usize,
) -> Option<(&'a str, &'a str)> {
    let key_ptr = unsafe { *keys.add(i) };
    let val_ptr = unsafe { *values.add(i) };
    if key_ptr.is_null() || val_ptr.is_null() {
        return None;
    }
    let key = match unsafe { CStr::from_ptr(key_ptr) }.to_str() {
        Ok(s) if !s.is_empty() => s,
        _ => return None,
    };
    let val = unsafe { CStr::from_ptr(val_ptr) }.to_str().unwrap_or("");
    Some((key, val))
}

/// Reads the property arrays of the `*_tagged` functions.  Keys are block-scoped names.
#[doc(hidden)]
pub unsafe fn read_properties<'a>(
    keys: *const *const c_char,
    values: *const *const c_char,
    count: c_uint,
) -> Vec<DynProperty<'a>> {
    if keys.is_null() || values.is_null() {
        return vec![];
    }
    (0..count as usize)
        .filter_map(|i| unsafe { read_pair(keys, values, i) })
        .map(|(key, val)| DynProperty::with_dyn_name(key, val))
        .collect()
}

/// The properties of a metric: the caller's pairs, then `name` and `unit`.  They come last
/// because `merge_properties` keeps the last entry of a name, so the arguments win over keys of
/// the same name.
#[doc(hidden)]
pub unsafe fn metric_properties<'a>(
    keys: *const *const c_char,
    values: *const *const c_char,
    count: c_uint,
    name: &'a str,
    unit: &'a str,
) -> Vec<DynProperty<'a>> {
    let mut properties = unsafe { read_properties(keys, values, count) };
    properties.push(DynProperty::new("name", name));
    properties.push(DynProperty::new("unit", unit));
    properties
}

unsafe fn metric_name_and_unit<'a>(
    handle: *mut MmHandle,
    name: *const c_char,
    unit: *const c_char,
) -> Option<(&'a str, &'a str)> {
    if handle.is_null() || name.is_null() {
        return None;
    }
    let name_str = match unsafe { CStr::from_ptr(name) }.to_str() {
        Ok(s) if !s.is_empty() => s,
        _ => return None,
    };
    let unit_str = if unit.is_null() {
        ""
    } else {
        unsafe { CStr::from_ptr(unit) }.to_str().unwrap_or("")
    };
    Some((name_str, unit_str))
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
            if let Some((key, val)) =
                unsafe { read_pair(cfg.property_keys, cfg.property_values, i) }
            {
                props.insert(key.to_owned(), val.to_owned());
            }
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
    unsafe {
        mm_log_tagged(
            handle,
            level,
            target,
            msg,
            std::ptr::null(),
            std::ptr::null(),
            0,
        )
    }
}

/// Emit a log event with properties.
///
/// Same as `mm_log`, plus `property_count` key/value pairs read from the parallel arrays, the
/// way `mm_init` reads process properties: a pair with a null key or value, or an empty or
/// non-UTF-8 key, is skipped.  Keys and values are copied into the current block.  `target` and
/// `level` keys are not applied: the arguments decide those.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mm_log_tagged(
    handle: *mut MmHandle,
    level: c_int,
    target: *const c_char,
    msg: *const c_char,
    property_keys: *const *const c_char,
    property_values: *const *const c_char,
    property_count: c_uint,
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
    let properties = unsafe { read_properties(property_keys, property_values, property_count) };
    if properties.is_empty() {
        dispatch::log_interop(&metadata, format_args!("{msg_str}"));
    } else {
        dispatch::log_interop_dyn_tagged(&metadata, &properties, format_args!("{msg_str}"));
    }
}

/// Emit an integer metric.
///
/// `name` must not be null or empty; a null `unit` is "".  The strings are copied into the
/// current block.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mm_metric_i(
    handle: *mut MmHandle,
    name: *const c_char,
    unit: *const c_char,
    value: u64,
) {
    unsafe {
        mm_metric_i_tagged(
            handle,
            name,
            unit,
            std::ptr::null(),
            std::ptr::null(),
            0,
            value,
        )
    }
}

/// Emit an integer metric with properties.
///
/// Same as `mm_metric_i`, plus `property_count` key/value pairs read as in `mm_log_tagged`.
/// A `target` key replaces the default "capi" target; `name` and `unit` keys are overridden by
/// the arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mm_metric_i_tagged(
    handle: *mut MmHandle,
    name: *const c_char,
    unit: *const c_char,
    property_keys: *const *const c_char,
    property_values: *const *const c_char,
    property_count: c_uint,
    value: u64,
) {
    let Some((name_str, unit_str)) = (unsafe { metric_name_and_unit(handle, name, unit) }) else {
        return;
    };
    let properties = unsafe {
        metric_properties(
            property_keys,
            property_values,
            property_count,
            name_str,
            unit_str,
        )
    };
    dispatch::tagged_integer_metric_dyn(&METRIC_DESC, PropertySet::empty(), &properties, value);
}

/// Emit a floating-point metric.
///
/// Same contract as `mm_metric_i`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mm_metric_f(
    handle: *mut MmHandle,
    name: *const c_char,
    unit: *const c_char,
    value: f64,
) {
    unsafe {
        mm_metric_f_tagged(
            handle,
            name,
            unit,
            std::ptr::null(),
            std::ptr::null(),
            0,
            value,
        )
    }
}

/// Emit a floating-point metric with properties.
///
/// Same contract as `mm_metric_i_tagged`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mm_metric_f_tagged(
    handle: *mut MmHandle,
    name: *const c_char,
    unit: *const c_char,
    property_keys: *const *const c_char,
    property_values: *const *const c_char,
    property_count: c_uint,
    value: f64,
) {
    let Some((name_str, unit_str)) = (unsafe { metric_name_and_unit(handle, name, unit) }) else {
        return;
    };
    let properties = unsafe {
        metric_properties(
            property_keys,
            property_values,
            property_count,
            name_str,
            unit_str,
        )
    };
    dispatch::tagged_float_metric_dyn(&METRIC_DESC, PropertySet::empty(), &properties, value);
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
