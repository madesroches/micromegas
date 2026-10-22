//! Test utilities for in-memory tracing in unit tests

use crate::dispatch::flush_log_buffer;
use crate::dispatch::{force_uninit, init_event_dispatch, shutdown_dispatch};
use crate::event::in_memory_sink::InMemorySink;
use crate::logs::LogMsgQueueAny;
use micromegas_transit::HeterogeneousQueue;
use std::collections::HashMap;
use std::sync::Arc;

/// RAII guard for in-memory tracing that handles cleanup
///
/// This guard automatically calls shutdown_dispatch() and force_uninit()
/// when dropped, ensuring proper cleanup between tests.
///
/// # Important
/// Tests using this guard MUST be marked with `#[serial]` since they
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
        init_event_dispatch(1024, 1024, 1024, sink.clone(), HashMap::new(), true) // Enable CPU tracing for tests
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
/// Tests using this function MUST be marked with `#[serial]` since they
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

/// A log line read back from an [`InMemorySink`].
pub struct CapturedLog {
    pub msg: String,
    /// Dynamic properties of a tagged log; empty for untagged lines.
    pub properties: Vec<(String, String)>,
}

impl CapturedLog {
    /// First property named `name`.
    pub fn property(&self, name: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Flushes the log buffer, then returns every static-string, formatted and tagged log recorded
/// in `sink`, in emission order. Template events are not covered.
pub fn collect_logs(sink: &InMemorySink) -> Vec<CapturedLog> {
    flush_log_buffer();
    let state = sink.state.lock().expect("sink lock");
    let mut out = vec![];
    for block in &state.log_blocks {
        for event in block.events.iter() {
            let (msg, set_id) = match event {
                LogMsgQueueAny::LogStaticStrEvent(e) => (e.desc.fmt_str.to_owned(), None),
                LogMsgQueueAny::LogStringEvent(e) => (e.msg.0.clone(), None),
                LogMsgQueueAny::TaggedLogStaticStrEvent(e) => {
                    (e.desc.fmt_str.to_owned(), Some(e.properties))
                }
                LogMsgQueueAny::TaggedLogString(e) => (e.msg.0.clone(), Some(e.properties)),
                _ => continue,
            };
            let properties = set_id
                .map(|id| {
                    block
                        .dyn_deps
                        .set_properties(id)
                        .into_iter()
                        .map(|(k, v)| (k.to_owned(), v.to_owned()))
                        .collect()
                })
                .unwrap_or_default();
            out.push(CapturedLog { msg, properties });
        }
    }
    out
}
