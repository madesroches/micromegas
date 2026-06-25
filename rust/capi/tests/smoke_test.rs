use micromegas_capi::{
    MmConfig, MmHandle, mm_flush, mm_init, mm_log, mm_metric_f, mm_metric_i, mm_shutdown,
};
use std::ffi::CString;
use std::ptr;

/// Make a handle isolated from any MICROMEGAS_TELEMETRY_URL env var.
/// Passing an empty-string URL tells the builder not to fall back to the env var.
fn make_isolated_handle() -> *mut MmHandle {
    let url = CString::new("").expect("CString");
    let cfg = MmConfig {
        sink_url: url.as_ptr(), // non-null empty → no HTTP sink, no env-var pickup
        property_keys: ptr::null(),
        property_values: ptr::null(),
        property_count: 0,
    };
    unsafe { mm_init(&cfg) }
}

#[test]
fn null_config_does_not_crash() {
    // null cfg pointer → must return null without panicking
    let handle = unsafe { mm_init(ptr::null()) };
    assert!(handle.is_null());
}

#[test]
fn init_shutdown_no_server() {
    let handle = make_isolated_handle();
    // Guard builder succeeds even without an HTTP sink.
    assert!(!handle.is_null(), "expected non-null handle without server");
    unsafe { mm_shutdown(handle) };
}

#[test]
fn null_safety_on_log_and_metrics() {
    let handle = make_isolated_handle();
    if handle.is_null() {
        return;
    }
    unsafe {
        mm_log(handle, 4, ptr::null(), ptr::null());
        mm_metric_i(handle, ptr::null(), ptr::null(), 0);
        mm_metric_f(handle, ptr::null(), ptr::null(), 0.0);
        mm_flush(handle);
        mm_shutdown(handle);
    }
}

#[test]
fn null_handle_shutdown_is_safe() {
    unsafe { mm_shutdown(ptr::null_mut()) };
}

#[test]
fn null_handle_flush_is_safe() {
    unsafe { mm_flush(ptr::null_mut()) };
}

#[test]
fn log_and_metrics_with_valid_strings() {
    let handle = make_isolated_handle();
    if handle.is_null() {
        return;
    }
    let target = CString::new("smoke.test").expect("CString");
    let msg = CString::new("hello from capi smoke test").expect("CString");
    let name_counter = CString::new("smoke.counter").expect("CString");
    let name_latency = CString::new("smoke.latency_ms").expect("CString");
    let unit_count = CString::new("count").expect("CString");
    let unit_ms = CString::new("ms").expect("CString");

    unsafe {
        mm_log(handle, 4, target.as_ptr(), msg.as_ptr());
        mm_metric_i(handle, name_counter.as_ptr(), unit_count.as_ptr(), 42);
        mm_metric_f(handle, name_latency.as_ptr(), unit_ms.as_ptr(), 16.7);
        mm_flush(handle);
        mm_shutdown(handle);
    }
}

#[test]
fn metric_interner_deduplication() {
    let handle = make_isolated_handle();
    if handle.is_null() {
        return;
    }
    let name = CString::new("dedup.metric").expect("CString");
    let unit = CString::new("bytes").expect("CString");

    // Same (name, unit) emitted many times — the interner must not grow unboundedly.
    for i in 0..100_u64 {
        unsafe { mm_metric_i(handle, name.as_ptr(), unit.as_ptr(), i) };
    }
    unsafe {
        mm_flush(handle);
        mm_shutdown(handle);
    }
}

#[test]
fn all_log_levels() {
    let handle = make_isolated_handle();
    if handle.is_null() {
        return;
    }
    let target = CString::new("smoke.levels").expect("CString");
    for level in 1..=6_i32 {
        let msg = CString::new(format!("level {level}")).expect("CString");
        unsafe { mm_log(handle, level, target.as_ptr(), msg.as_ptr()) };
    }
    unsafe {
        mm_flush(handle);
        mm_shutdown(handle);
    }
}
