//! RAII-style guards
use std::{collections::HashMap, marker::PhantomData, sync::Arc};

use crate::{
    dispatch::{
        flush_log_buffer, flush_metrics_buffer, flush_thread_buffer, init_event_dispatch,
        init_thread_stream, on_begin_async_named_scope, on_begin_async_scope, on_begin_named_scope,
        on_begin_scope, on_end_async_named_scope, on_end_async_scope, on_end_named_scope,
        on_end_scope, shutdown_dispatch,
    },
    errors::Result,
    event::EventSink,
    panic_hook::init_panic_hook,
    spans::{SpanLocation, SpanMetadata},
};

pub struct TracingSystemGuard {}

impl TracingSystemGuard {
    /// Instantiates a new system guard that initializes the telemetry system.
    pub fn new(
        logs_buffer_size: usize,
        metrics_buffer_size: usize,
        threads_buffer_size: usize,
        sink: Arc<dyn EventSink>,
        process_properties: HashMap<String, String>,
    ) -> Result<Self> {
        init_telemetry(
            logs_buffer_size,
            metrics_buffer_size,
            threads_buffer_size,
            sink,
            process_properties,
        )?;
        Ok(Self {})
    }
}

impl Drop for TracingSystemGuard {
    fn drop(&mut self) {
        shutdown_telemetry();
    }
}

pub fn init_telemetry(
    logs_buffer_size: usize,
    metrics_buffer_size: usize,
    threads_buffer_size: usize,
    sink: Arc<dyn EventSink>,
    process_properties: HashMap<String, String>,
) -> Result<()> {
    init_event_dispatch(
        logs_buffer_size,
        metrics_buffer_size,
        threads_buffer_size,
        sink,
        process_properties,
    )?;
    init_panic_hook();
    Ok(())
}

pub fn shutdown_telemetry() {
    flush_log_buffer();
    flush_metrics_buffer();
    shutdown_dispatch();
}

pub struct TracingThreadGuard {
    _dummy_ptr: *mut u8,
}

impl TracingThreadGuard {
    pub fn new() -> Self {
        init_thread_stream();
        Self {
            _dummy_ptr: std::ptr::null_mut(),
        }
    }
}

impl Drop for TracingThreadGuard {
    fn drop(&mut self) {
        flush_thread_buffer();
    }
}

//not used at the time of writing, but clippy wants it
impl Default for TracingThreadGuard {
    fn default() -> Self {
        Self::new()
    }
}

// sync scope guard
pub struct ThreadSpanGuard {
    thread_span_desc: &'static SpanMetadata,
    _dummy_ptr: PhantomData<*mut u8>, // to mark the object as !Send
}

impl ThreadSpanGuard {
    pub fn new(thread_span_desc: &'static SpanMetadata) -> Self {
        let guard = Self {
            thread_span_desc,
            _dummy_ptr: std::marker::PhantomData {},
        };
        on_begin_scope(thread_span_desc);
        guard
    }
}

impl Drop for ThreadSpanGuard {
    fn drop(&mut self) {
        on_end_scope(self.thread_span_desc);
    }
}

pub struct ThreadNamedSpanGuard {
    thread_span_location: &'static SpanLocation,
    name: &'static str,
    _dummy_ptr: PhantomData<*mut u8>, // to mark the object as !Send
}

impl ThreadNamedSpanGuard {
    pub fn new(thread_span_location: &'static SpanLocation, name: &'static str) -> Self {
        let guard = Self {
            thread_span_location,
            name,
            _dummy_ptr: std::marker::PhantomData {},
        };
        on_begin_named_scope(guard.thread_span_location, guard.name);
        guard
    }
}

impl Drop for ThreadNamedSpanGuard {
    fn drop(&mut self) {
        on_end_named_scope(self.thread_span_location, self.name);
    }
}

// async scope guard : to be removed
pub struct AsyncSpanGuard {
    span_desc: &'static SpanMetadata,
    span_id: u64,
}

impl AsyncSpanGuard {
    pub fn new(span_desc: &'static SpanMetadata) -> Self {
        let span_id = on_begin_async_scope(span_desc, 0);
        Self { span_desc, span_id }
    }
}

impl Drop for AsyncSpanGuard {
    fn drop(&mut self) {
        on_end_async_scope(self.span_id, 0, self.span_desc);
    }
}

pub struct AsyncNamedSpanGuard {
    span_location: &'static SpanLocation,
    name: &'static str,
    span_id: u64,
}

impl AsyncNamedSpanGuard {
    pub fn new(span_location: &'static SpanLocation, name: &'static str) -> Self {
        let span_id = on_begin_async_named_scope(span_location, name, 0);
        Self {
            span_location,
            name,
            span_id,
        }
    }
}

impl Drop for AsyncNamedSpanGuard {
    fn drop(&mut self) {
        on_end_async_named_scope(self.span_id, 0, self.span_location, self.name);
    }
}
