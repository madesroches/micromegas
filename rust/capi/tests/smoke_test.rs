use micromegas_capi::{
    MmConfig, MmHandle, mm_flush, mm_init, mm_log, mm_metric_f, mm_metric_i, mm_shutdown,
};
use std::ffi::CString;
use std::ptr;

// The telemetry dispatch is a process-global that can only be initialized once
// (see `micromegas_tracing::dispatch::G_DISPATCH`, a write-once cell). Rust's test
// harness runs every test in a single process, so only the first `mm_init` can
// succeed — any later one returns null. All tests that need a live handle are
// therefore consolidated into one test below; the remaining tests exercise the
// null-pointer paths, which never touch the global dispatch.

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
fn null_handle_shutdown_is_safe() {
    unsafe { mm_shutdown(ptr::null_mut()) };
}

#[test]
fn null_handle_flush_is_safe() {
    unsafe { mm_flush(ptr::null_mut()) };
}

/// Single test that owns the one allowed dispatch initialization and exercises
/// the full handle-based API: init, null-safety on a valid handle, valid log and
/// metric emission across all levels, interner deduplication, flush, and shutdown.
#[test]
fn init_and_exercise_api() {
    let handle = make_isolated_handle();
    // Guard builder succeeds even without an HTTP sink.
    assert!(!handle.is_null(), "expected non-null handle without server");

    let target = CString::new("smoke.test").expect("CString");
    let msg = CString::new("hello from capi smoke test").expect("CString");
    let name_counter = CString::new("smoke.counter").expect("CString");
    let name_latency = CString::new("smoke.latency_ms").expect("CString");
    let name_dedup = CString::new("dedup.metric").expect("CString");
    let unit_count = CString::new("count").expect("CString");
    let unit_ms = CString::new("ms").expect("CString");
    let unit_bytes = CString::new("bytes").expect("CString");
    let levels_target = CString::new("smoke.levels").expect("CString");

    unsafe {
        // Null args on a valid handle must be no-ops, not crashes.
        mm_log(handle, 4, ptr::null(), ptr::null());
        mm_metric_i(handle, ptr::null(), ptr::null(), 0);
        mm_metric_f(handle, ptr::null(), ptr::null(), 0.0);

        // Valid log and metric emission.
        mm_log(handle, 4, target.as_ptr(), msg.as_ptr());
        mm_metric_i(handle, name_counter.as_ptr(), unit_count.as_ptr(), 42);
        mm_metric_f(handle, name_latency.as_ptr(), unit_ms.as_ptr(), 16.7);

        // All log levels (1=Fatal … 6=Trace).
        for level in 1..=6_i32 {
            let lvl_msg = CString::new(format!("level {level}")).expect("CString");
            mm_log(handle, level, levels_target.as_ptr(), lvl_msg.as_ptr());
        }

        // Same (name, unit) emitted many times — the interner must not grow unboundedly.
        for i in 0..100_u64 {
            mm_metric_i(handle, name_dedup.as_ptr(), unit_bytes.as_ptr(), i);
        }

        mm_flush(handle);
        mm_shutdown(handle);
    }
}
