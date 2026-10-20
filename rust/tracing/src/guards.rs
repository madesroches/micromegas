//! RAII-style guards
use std::{collections::HashMap, marker::PhantomData, sync::Arc};

use crate::{
    dispatch::{
        flush_log_buffer, flush_metrics_buffer, flush_thread_buffer, init_event_dispatch,
        init_thread_stream, on_begin_scope, on_end_named_scope_tagged, on_end_scope,
        on_end_scope_tagged, shutdown_dispatch,
    },
    errors::Result,
    event::EventSink,
    panic_hook::init_panic_hook,
    spans::{SpanLocation, SpanMetadata, SpanName, SpanProperties, StaticSpanName},
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
        cpu_tracing_enabled: bool,
    ) -> Result<Self> {
        init_telemetry(
            logs_buffer_size,
            metrics_buffer_size,
            threads_buffer_size,
            sink,
            process_properties,
            cpu_tracing_enabled,
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
    cpu_tracing_enabled: bool,
) -> Result<()> {
    init_event_dispatch(
        logs_buffer_size,
        metrics_buffer_size,
        threads_buffer_size,
        sink,
        process_properties,
        cpu_tracing_enabled,
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

mod sealed {
    pub trait Sealed {}
    impl Sealed for () {}
    impl Sealed for super::SpanProperties {}
}

/// What a thread span guard carries until its end: nothing (`()`), which ends the span plainly,
/// or [`SpanProperties`], which ends it with the properties that were set.
pub trait SpanEndProperties: sealed::Sealed {
    #[doc(hidden)]
    fn end_thread(&self, desc: &'static SpanMetadata);
    #[doc(hidden)]
    fn end_thread_named<N: SpanName>(&self, location: &'static SpanLocation, name: &N);
}

impl SpanEndProperties for () {
    #[inline(always)]
    fn end_thread(&self, desc: &'static SpanMetadata) {
        on_end_scope(desc);
    }

    #[inline(always)]
    fn end_thread_named<N: SpanName>(&self, location: &'static SpanLocation, name: &N) {
        name.end_thread(location);
    }
}

impl SpanEndProperties for SpanProperties {
    fn end_thread(&self, desc: &'static SpanMetadata) {
        if self.is_empty() {
            on_end_scope(desc);
        } else {
            self.with_dyn(|set, dyn_properties| on_end_scope_tagged(desc, set, dyn_properties));
        }
    }

    fn end_thread_named<N: SpanName>(&self, location: &'static SpanLocation, name: &N) {
        if self.is_empty() {
            name.end_thread(location);
        } else {
            self.with_dyn(|set, dyn_properties| {
                on_end_named_scope_tagged(location, name.as_str(), set, dyn_properties);
            });
        }
    }
}

/// Ends a span when dropped. Bind it to a name: `let _ = ..` drops it at once.
// sync scope guard
#[must_use = "the span ends when the guard is dropped"]
pub struct ThreadSpanGuard<P: SpanEndProperties = ()> {
    thread_span_desc: &'static SpanMetadata,
    properties: P,
    _dummy_ptr: PhantomData<*mut u8>, // to mark the object as !Send
}

impl ThreadSpanGuard {
    pub fn new(thread_span_desc: &'static SpanMetadata) -> Self {
        let guard = Self {
            thread_span_desc,
            properties: (),
            _dummy_ptr: std::marker::PhantomData {},
        };
        on_begin_scope(thread_span_desc);
        guard
    }
}

impl ThreadSpanGuard<SpanProperties> {
    /// Begins a span whose end event carries `properties`, plus the ones set on the guard later.
    pub fn with_properties(
        thread_span_desc: &'static SpanMetadata,
        properties: SpanProperties,
    ) -> Self {
        let guard = Self {
            thread_span_desc,
            properties,
            _dummy_ptr: std::marker::PhantomData {},
        };
        on_begin_scope(thread_span_desc);
        guard
    }

    /// See [`SpanProperties::set_property`].
    pub fn set_property(&mut self, name: &'static str, value: impl std::fmt::Display) {
        self.properties.set_property(name, value);
    }
}

impl<P: SpanEndProperties> Drop for ThreadSpanGuard<P> {
    fn drop(&mut self) {
        self.properties.end_thread(self.thread_span_desc);
    }
}

/// Ends a named span when dropped. Bind it to a name: `let _ = ..` drops it at once.
#[must_use = "the span ends when the guard is dropped"]
pub struct ThreadNamedSpanGuard<N: SpanName = StaticSpanName, P: SpanEndProperties = ()> {
    thread_span_location: &'static SpanLocation,
    name: N,
    properties: P,
    _dummy_ptr: PhantomData<*mut u8>, // to mark the object as !Send
}

impl ThreadNamedSpanGuard {
    pub fn new(thread_span_location: &'static SpanLocation, name: &'static str) -> Self {
        Self::with_name(thread_span_location, StaticSpanName(name))
    }
}

impl<N: SpanName> ThreadNamedSpanGuard<N> {
    pub fn with_name(thread_span_location: &'static SpanLocation, name: N) -> Self {
        let guard = Self {
            thread_span_location,
            name,
            properties: (),
            _dummy_ptr: std::marker::PhantomData {},
        };
        guard.name.begin_thread(guard.thread_span_location);
        guard
    }
}

impl<N: SpanName> ThreadNamedSpanGuard<N, SpanProperties> {
    /// Begins a span whose end event carries `properties`, plus the ones set on the guard later.
    pub fn with_properties(
        thread_span_location: &'static SpanLocation,
        name: N,
        properties: SpanProperties,
    ) -> Self {
        let guard = Self {
            thread_span_location,
            name,
            properties,
            _dummy_ptr: std::marker::PhantomData {},
        };
        guard.name.begin_thread(guard.thread_span_location);
        guard
    }

    /// See [`SpanProperties::set_property`].
    pub fn set_property(&mut self, name: &'static str, value: impl std::fmt::Display) {
        self.properties.set_property(name, value);
    }
}

impl<N: SpanName, P: SpanEndProperties> Drop for ThreadNamedSpanGuard<N, P> {
    fn drop(&mut self) {
        self.properties
            .end_thread_named(self.thread_span_location, &self.name);
    }
}
