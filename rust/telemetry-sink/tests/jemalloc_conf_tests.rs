//! Asserts that `declare_jemalloc_conf!()` actually reaches jemalloc, guarding
//! against every silent-failure mode of the exported symbol: a wrong symbol
//! prefix, a dropped static, a typo in the conf string, or an upstream
//! default shift. `required-features = ["jemalloc"]` (set on this test's
//! `[[test]]` entry in `Cargo.toml`) gates the whole file; `#![cfg(not(target_os
//! = "windows"))]` makes it compile to an empty harness on Windows, matching
//! `jemalloc_stats_tests.rs`.
#![cfg(not(target_os = "windows"))]

#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

micromegas_telemetry_sink::declare_jemalloc_conf!();

use tikv_jemalloc_ctl::{opt, raw};

/// `_RJEM_MALLOC_CONF` outranks the exported symbol under test (jemalloc's
/// source-precedence 3 vs. 1), so a test run under that env var would be
/// checking the operator's override, not this crate's default -- skip
/// rather than fail.
fn rjem_malloc_conf_override_set() -> bool {
    std::env::var_os("_RJEM_MALLOC_CONF").is_some()
}

#[test]
fn background_thread_is_enabled_by_default() {
    if rjem_malloc_conf_override_set() {
        eprintln!("skipping: _RJEM_MALLOC_CONF is set, outranks the symbol under test");
        return;
    }
    assert!(
        opt::background_thread::read().expect("read opt.background_thread"),
        "declare_jemalloc_conf!() must enable background_thread"
    );
}

#[test]
fn dirty_decay_ms_is_tightened_by_default() {
    if rjem_malloc_conf_override_set() {
        eprintln!("skipping: _RJEM_MALLOC_CONF is set, outranks the symbol under test");
        return;
    }
    let dirty_decay_ms =
        unsafe { raw::read::<isize>(b"opt.dirty_decay_ms\0") }.expect("read opt.dirty_decay_ms");
    assert_eq!(
        dirty_decay_ms, 5000,
        "declare_jemalloc_conf!() must set dirty_decay_ms to 5000"
    );
}

#[test]
fn muzzy_decay_ms_is_pinned_to_zero_by_default() {
    if rjem_malloc_conf_override_set() {
        eprintln!("skipping: _RJEM_MALLOC_CONF is set, outranks the symbol under test");
        return;
    }
    let muzzy_decay_ms =
        unsafe { raw::read::<isize>(b"opt.muzzy_decay_ms\0") }.expect("read opt.muzzy_decay_ms");
    assert_eq!(
        muzzy_decay_ms, 0,
        "declare_jemalloc_conf!() must pin muzzy_decay_ms to 0"
    );
}
