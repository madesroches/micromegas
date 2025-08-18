//! Test utilities for in-memory tracing in unit tests

use crate::dispatch::{force_uninit, init_event_dispatch, shutdown_dispatch};
use crate::event::in_memory_sink::InMemorySink;
use std::collections::HashMap;
use std::sync::Arc;

/// RAII guard for in-memory tracing that handles cleanup
///
/// This guard automatically calls shutdown_dispatch() and force_uninit()
/// when dropped, ensuring proper cleanup between tests.
///
/// # Important
/// Tests using this guard MUST be marked with #[serial] since they
/// share global state through init_event_dispatch.
pub struct InMemoryTracingGuard {
    pub sink: Arc<InMemorySink>,
}

impl Default for InMemoryTracingGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryTracingGuard {
    pub fn new() -> Self {
        let sink = Arc::new(InMemorySink::new());
        init_event_dispatch(1024, 1024, 1024, sink.clone(), HashMap::new())
            .expect("Failed to initialize event dispatch");
        Self { sink }
    }
}

impl Drop for InMemoryTracingGuard {
    fn drop(&mut self) {
        shutdown_dispatch();
        unsafe { force_uninit() };
    }
}

/// Initialize in-memory tracing for unit tests
///
/// # Important
/// Tests using this function MUST be marked with #[serial] since they
/// share global state through init_event_dispatch.
///
/// # Example
/// ```rust
/// use micromegas_tracing::test_utils::init_in_memory_tracing;
/// use serial_test::serial;
///
/// // In your test file:
/// // #[test]
/// // #[serial]
/// fn test_example() {
///     let guard = init_in_memory_tracing();
///     // Use tracing macros: info!(), debug!(), span_scope!(), etc.
///     // Verify results in guard.sink.state
///     // Automatic cleanup when guard is dropped
/// }
/// ```
pub fn init_in_memory_tracing() -> InMemoryTracingGuard {
    InMemoryTracingGuard::new()
}
