use micromegas_tracing::dispatch::flush_thread_buffer;
use micromegas_tracing::prelude::*;
use micromegas_tracing::spans::ThreadEventQueueAny;
use micromegas_tracing::test_utils::init_in_memory_tracing;
use micromegas_transit::HeterogeneousQueue;
use serial_test::serial;
use std::collections::HashMap;
use std::time::Duration;

/// Simple test to validate that async instrumentation with depth tracking compiles and runs
#[test]
fn test_basic_async_instrumentation() {
    // Use a runtime to test async functionality
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        static_span_desc!(LEVEL_0_DESC, "level_0_operation");

        async fn level_0_operation() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // This should work without errors
        level_0_operation().instrument(&LEVEL_0_DESC).await;
    });
}

#[test]
fn test_nested_async_instrumentation() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        static_span_desc!(OUTER_DESC, "outer_operation");
        static_span_desc!(INNER_DESC, "inner_operation");

        async fn inner_operation() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        async fn outer_operation() {
            inner_operation().instrument(&INNER_DESC).await;
        }

        outer_operation().instrument(&OUTER_DESC).await;
    });
}

#[test]
fn test_parallel_async_operations() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        static_span_desc!(WORKER_DESC, "worker");

        async fn worker() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        let tasks = (0..3).map(|_| worker().instrument(&WORKER_DESC));

        futures::future::join_all(tasks).await;
    });
}

#[test]
fn test_deeply_nested_async() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        static_span_desc!(LEVEL_0_DESC, "level_0");
        static_span_desc!(LEVEL_1_DESC, "level_1");
        static_span_desc!(LEVEL_2_DESC, "level_2");
        static_span_desc!(LEVEL_3_DESC, "level_3");

        async fn level_3() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        async fn level_2() {
            level_3().instrument(&LEVEL_3_DESC).await;
        }

        async fn level_1() {
            level_2().instrument(&LEVEL_2_DESC).await;
        }

        async fn level_0() {
            level_1().instrument(&LEVEL_1_DESC).await;
        }

        level_0().instrument(&LEVEL_0_DESC).await;
    });
}

#[test]
fn test_error_handling_with_instrumentation() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        static_span_desc!(ERROR_DESC, "error_operation");
        static_span_desc!(RECOVERY_DESC, "recovery_operation");

        async fn error_operation() -> Result<(), &'static str> {
            tokio::time::sleep(Duration::from_millis(1)).await;
            Err("test error")
        }

        async fn recovery_operation() {
            let _result = error_operation().instrument(&ERROR_DESC).await;
            // Error should not break instrumentation
        }

        recovery_operation().instrument(&RECOVERY_DESC).await;
    });
}

#[test]
#[serial]
fn test_depth_consistency_begin_end() {
    let guard = init_in_memory_tracing();

    let rt = tokio::runtime::Runtime::new().expect("failed to create runtime");
    rt.block_on(async {
        let _thread_guard = micromegas_tracing::guards::TracingThreadGuard::new();

        static_span_desc!(OUTER_DESC, "depth_outer");
        static_span_desc!(INNER_DESC, "depth_inner");

        async fn inner_work() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        async fn outer_work() {
            inner_work().instrument(&INNER_DESC).await;
        }

        outer_work().instrument(&OUTER_DESC).await;

        flush_thread_buffer();
    });

    // Collect begin/end depths keyed by span_id
    let mut begin_depths: HashMap<u64, u32> = HashMap::new();
    let mut end_depths: HashMap<u64, u32> = HashMap::new();

    let state = guard.sink.state.lock().expect("failed to lock sink state");
    for block in &state.thread_blocks {
        for event in block.events.iter() {
            match event {
                ThreadEventQueueAny::BeginAsyncSpanEvent(evt) => {
                    begin_depths.insert(evt.span_id, evt.depth);
                }
                ThreadEventQueueAny::EndAsyncSpanEvent(evt) => {
                    end_depths.insert(evt.span_id, evt.depth);
                }
                _ => {}
            }
        }
    }

    // Verify we captured events
    assert!(
        begin_depths.len() >= 2,
        "Expected at least 2 begin events (outer + inner), got {}",
        begin_depths.len()
    );

    // Verify begin/end depths match for each span_id
    for (span_id, begin_depth) in &begin_depths {
        let end_depth = end_depths
            .get(span_id)
            .unwrap_or_else(|| panic!("missing end event for span_id {span_id}"));
        assert_eq!(
            begin_depth, end_depth,
            "depth mismatch for span_id {span_id}: begin={begin_depth}, end={end_depth}"
        );
    }

    // Verify nested spans have increasing depth
    let mut depths: Vec<u32> = begin_depths.values().copied().collect();
    depths.sort();
    depths.dedup();
    assert!(
        depths.len() >= 2,
        "Expected at least 2 distinct depth levels, got {depths:?}"
    );
    // Depths should be consecutive starting from 0
    for (i, &d) in depths.iter().enumerate() {
        assert_eq!(
            d, i as u32,
            "Expected depth {i} but got {d} in sorted depths {depths:?}"
        );
    }
}

#[test]
#[serial]
fn test_named_depth_consistency_begin_end() {
    let guard = init_in_memory_tracing();

    let rt = tokio::runtime::Runtime::new().expect("failed to create runtime");
    rt.block_on(async {
        let _thread_guard = micromegas_tracing::guards::TracingThreadGuard::new();

        async fn inner_work() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        async fn outer_work() {
            instrument_named!(inner_work(), "named_inner").await;
        }

        instrument_named!(outer_work(), "named_outer").await;

        flush_thread_buffer();
    });

    // Collect begin/end depths keyed by span_id
    let mut begin_depths: HashMap<u64, u32> = HashMap::new();
    let mut end_depths: HashMap<u64, u32> = HashMap::new();

    let state = guard.sink.state.lock().expect("failed to lock sink state");
    for block in &state.thread_blocks {
        for event in block.events.iter() {
            match event {
                ThreadEventQueueAny::BeginAsyncNamedSpanEvent(evt) => {
                    begin_depths.insert(evt.span_id, evt.depth);
                }
                ThreadEventQueueAny::EndAsyncNamedSpanEvent(evt) => {
                    end_depths.insert(evt.span_id, evt.depth);
                }
                _ => {}
            }
        }
    }

    assert!(
        begin_depths.len() >= 2,
        "Expected at least 2 named begin events (outer + inner), got {}",
        begin_depths.len()
    );

    for (span_id, begin_depth) in &begin_depths {
        let end_depth = end_depths
            .get(span_id)
            .unwrap_or_else(|| panic!("missing end event for span_id {span_id}"));
        assert_eq!(
            begin_depth, end_depth,
            "depth mismatch for span_id {span_id}: begin={begin_depth}, end={end_depth}"
        );
    }

    let mut depths: Vec<u32> = begin_depths.values().copied().collect();
    depths.sort();
    depths.dedup();
    assert!(
        depths.len() >= 2,
        "Expected at least 2 distinct depth levels, got {depths:?}"
    );
    for (i, &d) in depths.iter().enumerate() {
        assert_eq!(
            d, i as u32,
            "Expected depth {i} but got {d} in sorted depths {depths:?}"
        );
    }
}
