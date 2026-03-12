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

/// Validates that spawn_with_context maintains correct depth across yield points.
///
/// Reproduces the bug where SpanScope (RAII guard) only pushed to the thread-local
/// stack once at creation during the first poll, but never re-pushed on subsequent
/// polls. This caused futures created after a yield point to see a shorter stack
/// and report incorrect depth.
///
/// The fix replaces SpanScope with SpanContextFuture, which pushes/pops on every poll.
#[test]
#[serial]
fn test_depth_across_spawn_with_context_and_yield() {
    use micromegas_tracing::runtime::TracingRuntimeExt;

    let guard = init_in_memory_tracing();

    // Use multi-threaded runtime with tracing callbacks so worker threads flush
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .with_tracing_callbacks()
        .build()
        .expect("failed to create runtime");

    rt.block_on(async {
        static_span_desc!(PARENT_DESC, "parent_op");
        static_span_desc!(CHILD_A_DESC, "child_a");
        static_span_desc!(CHILD_B_DESC, "child_b");

        async fn child_a() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        async fn child_b() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // Futures are created INSIDE the spawned async block (like CronTask::spawn does).
        // This means their InstrumentedFuture::new() runs with SpanContextFuture's
        // parent span on the stack, giving correct depth.
        let handle = spawn_with_context(async {
            // parent_op's InstrumentedFuture is created inside the spawn context
            async {
                // child_a is created during the first poll
                child_a().instrument(&CHILD_A_DESC).await;
                // child_b is created after child_a completes (after yield points) —
                // with the old SpanScope bug, this would get a wrong (lower) depth
                child_b().instrument(&CHILD_B_DESC).await;
            }
            .instrument(&PARENT_DESC)
            .await;
        });
        handle.await.expect("spawned task panicked");
    });

    // Drop the runtime so worker threads stop and flush their buffers
    drop(rt);

    // Collect span events: (span_id) → (parent_span_id, depth, name)
    struct SpanInfo {
        #[allow(dead_code)]
        parent_span_id: u64,
        depth: u32,
        name: String,
    }
    let mut spans: HashMap<u64, SpanInfo> = HashMap::new();

    let state = guard.sink.state.lock().expect("failed to lock sink state");
    for block in &state.thread_blocks {
        for event in block.events.iter() {
            if let ThreadEventQueueAny::BeginAsyncSpanEvent(evt) = event {
                spans.insert(
                    evt.span_id,
                    SpanInfo {
                        parent_span_id: evt.parent_span_id,
                        depth: evt.depth,
                        name: evt.span_desc.name.to_string(),
                    },
                );
            }
        }
    }

    // We should have 3 spans: parent_op, child_a, child_b
    assert!(
        spans.len() >= 3,
        "Expected at least 3 spans (parent + 2 children), got {}",
        spans.len()
    );

    // Find parent and children
    let parent = spans
        .values()
        .find(|s| s.name == "parent_op")
        .expect("missing parent_op span");
    let child_a = spans
        .values()
        .find(|s| s.name == "child_a")
        .expect("missing child_a span");
    let child_b = spans
        .values()
        .find(|s| s.name == "child_b")
        .expect("missing child_b span");

    // Both children must have depth = parent.depth + 1
    assert_eq!(
        child_a.depth,
        parent.depth + 1,
        "child_a depth should be parent+1: child_a.depth={}, parent.depth={}",
        child_a.depth,
        parent.depth
    );
    assert_eq!(
        child_b.depth,
        parent.depth + 1,
        "child_b depth should be parent+1 (created after yield): child_b.depth={}, parent.depth={}",
        child_b.depth,
        parent.depth
    );

    // child_a and child_b should have the same depth
    assert_eq!(
        child_a.depth, child_b.depth,
        "child_a and child_b should have same depth: child_a={}, child_b={}",
        child_a.depth, child_b.depth
    );
}
