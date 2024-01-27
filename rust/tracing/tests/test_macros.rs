use std::sync::{Arc, Mutex};
use std::thread;

mod utils;
use lgn_tracing::dispatch::{
    flush_log_buffer, flush_metrics_buffer, flush_thread_buffer, init_event_dispatch,
    init_thread_stream, process_id,
};
use lgn_tracing::{fmetric, frequency, imetric, info, set_max_level, span_scope, LevelFilter};
use lgn_tracing_proc_macros::{log_fn, span_fn};
use utils::{DebugEventSink, LogDispatch, SharedState, State};

fn test_log_str(state: &SharedState) {
    for x in 0..5 {
        info!("test");
        expect_state!(state, Some(State::Log(String::from("test"))));
        info!("test {}", x);
        expect_state!(state, Some(State::Log(format!("test {}", x))));
    }
    flush_log_buffer();
    expect_state!(state, Some(State::ProcessLogBlock(10)));
}

fn test_log_interop_str(state: &SharedState) {
    for x in 0..5 {
        log::info!("test");
        expect_state!(state, Some(State::Log(String::from("test"))));
        log::info!("test {}", x);
        expect_state!(state, Some(State::Log(format!("test {}", x))));
    }
    flush_log_buffer();
    expect_state!(state, Some(State::ProcessLogBlock(10)));
}

fn test_thread_spans(state: &SharedState) {
    println!("TSC frequency: {}", frequency());
    let mut threads = Vec::new();
    for _ in 0..5 {
        threads.push(thread::spawn(move || {
            init_thread_stream();
            for _ in 0..1024 {
                span_scope!("test");
            }
            flush_thread_buffer();
        }));
    }
    for t in threads {
        t.join().unwrap();
    }

    init_thread_stream();
    for _ in 0..1024 {
        span_scope!("test");
    }
    flush_thread_buffer();
    expect_state!(state, Some(State::ProcessThreadBlock(2048)));
}

fn test_metrics(state: &SharedState) {
    imetric!("Frame Time", "ticks", 1000);
    fmetric!("Frame Time", "ticks", 1.0);
    flush_metrics_buffer();
    expect_state!(state, Some(State::ProcessMetricsBlock(2)));
}

#[span_fn]
fn trace_func() {}

#[span_fn("foo")]
fn trace_func_named() {}

#[log_fn]
fn log_func() {}

fn test_proc_macros(state: &SharedState) {
    trace_func();
    trace_func_named();
    flush_thread_buffer();
    expect_state!(&state.clone(), Some(utils::State::ProcessThreadBlock(4)));

    log_func();
    expect_state!(state, Some(State::Log(String::from("log_func"))));
}

#[test]
fn test_log() {
    static LOG_DISPATCHER: LogDispatch = LogDispatch;
    log::set_logger(&LOG_DISPATCHER).unwrap();

    let state = Arc::new(Mutex::new(None));
    init_event_dispatch(
        10 * 1024,
        1024,
        64 * 1024,
        Arc::new(DebugEventSink::new(state.clone())),
    )
    .unwrap();
    set_max_level(LevelFilter::Trace);
    log::set_max_level(log::LevelFilter::Trace);
    assert!(process_id().is_some());
    test_log_str(&state);
    test_log_interop_str(&state);
    test_thread_spans(&state);
    test_proc_macros(&state);
    test_metrics(&state);
}
