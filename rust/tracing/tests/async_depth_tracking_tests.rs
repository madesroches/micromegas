use micromegas_tracing::prelude::*;
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
