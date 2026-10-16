//! Where events are recorded and eventually sent to a sink
use crate::dyn_deps::merge_properties;
pub use crate::errors::{Error, Result};
use crate::images::{ImageBlock, ImageEvent, ImageStream};
use crate::intern_string::intern_string;
use crate::logs::{TaggedLogInteropEvent, TaggedLogString};
use crate::metrics::{TaggedFloatMetricEvent, TaggedIntegerMetricEvent};
use crate::prelude::*;
use crate::property_set::{DynProperty, PropertySet, PropertySetId};
use crate::{
    event::{EventBlock, EventSink, EventStream, ExtractDeps, NullEventSink, TracingBlock},
    info,
    logs::{
        LogMetadata, LogStaticStrEvent, LogStaticStrInteropEvent, LogStream, LogStringEvent,
        LogStringInteropEvent,
    },
    metrics::{FloatMetricEvent, IntegerMetricEvent, MetricsStream, StaticMetricMetadata},
    spans::{
        BeginAsyncNamedSpanEvent, BeginAsyncSpanEvent, BeginThreadNamedSpanEvent,
        BeginThreadSpanEvent, EndAsyncNamedSpanEvent, EndAsyncSpanEvent, EndThreadNamedSpanEvent,
        EndThreadSpanEvent, SpanLocation, SpanMetadata, ThreadBlock, ThreadEventQueueTypeIndex,
        ThreadStream,
    },
    warn,
};

const IMAGE_BUFFER_SIZE: usize = 1024 * 1024;
use chrono::Utc;
use std::cell::OnceCell;
use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::fmt;
use std::sync::RwLock;
use std::{
    cell::Cell,
    sync::{Arc, Mutex, MutexGuard},
};

pub fn init_event_dispatch(
    logs_buffer_size: usize,
    metrics_buffer_size: usize,
    threads_buffer_size: usize,
    sink: Arc<dyn EventSink>,
    process_properties: HashMap<String, String>,
    cpu_tracing_enabled: bool,
) -> Result<()> {
    lazy_static::lazy_static! {
        static ref INIT_MUTEX: Mutex<()> = Mutex::new(());
    }
    let _guard = INIT_MUTEX.lock().unwrap();
    let dispatch_ref = &G_DISPATCH.inner;
    unsafe {
        if (*dispatch_ref.get()).get().is_none() {
            (*dispatch_ref.get())
                .set(Dispatch::new(
                    logs_buffer_size,
                    metrics_buffer_size,
                    threads_buffer_size,
                    sink,
                    process_properties,
                    cpu_tracing_enabled,
                ))
                .map_err(|_| Error::AlreadyInitialized())
        } else {
            info!("event dispatch already initialized");
            Err(Error::AlreadyInitialized())
        }
    }
}

#[inline]
pub fn process_id() -> Option<uuid::Uuid> {
    G_DISPATCH.get().map(Dispatch::get_process_id)
}

#[inline]
pub fn cpu_tracing_enabled() -> Option<bool> {
    G_DISPATCH.get().map(Dispatch::get_cpu_tracing_enabled)
}

pub fn get_sink() -> Option<Arc<dyn EventSink>> {
    G_DISPATCH.get().map(Dispatch::get_sink)
}

pub fn shutdown_dispatch() {
    G_DISPATCH.get().map(Dispatch::shutdown);
}

#[inline(always)]
pub fn int_metric(metric_desc: &'static StaticMetricMetadata, value: u64) {
    if let Some(d) = G_DISPATCH.get() {
        d.int_metric(metric_desc, value);
    }
}

#[inline(always)]
pub fn float_metric(metric_desc: &'static StaticMetricMetadata, value: f64) {
    if let Some(d) = G_DISPATCH.get() {
        d.float_metric(metric_desc, value);
    }
}

#[inline(always)]
pub fn tagged_float_metric(
    desc: &'static StaticMetricMetadata,
    properties: &'static PropertySet,
    value: f64,
) {
    if let Some(d) = G_DISPATCH.get() {
        d.tagged_float_metric(desc, properties, value);
    }
}

#[inline(always)]
pub fn tagged_integer_metric(
    desc: &'static StaticMetricMetadata,
    properties: &'static PropertySet,
    value: u64,
) {
    if let Some(d) = G_DISPATCH.get() {
        d.tagged_integer_metric(desc, properties, value);
    }
}

#[inline(always)]
pub fn tagged_float_metric_dyn(
    desc: &'static StaticMetricMetadata,
    properties: &'static PropertySet,
    dyn_properties: &[DynProperty<'_>],
    value: f64,
) {
    if let Some(d) = G_DISPATCH.get() {
        d.tagged_float_metric_dyn(desc, properties, dyn_properties, value);
    }
}

#[inline(always)]
pub fn tagged_integer_metric_dyn(
    desc: &'static StaticMetricMetadata,
    properties: &'static PropertySet,
    dyn_properties: &[DynProperty<'_>],
    value: u64,
) {
    if let Some(d) = G_DISPATCH.get() {
        d.tagged_integer_metric_dyn(desc, properties, dyn_properties, value);
    }
}

#[inline(always)]
pub fn log_dyn_tagged(
    desc: &'static LogMetadata,
    properties: &'static PropertySet,
    dyn_properties: &[DynProperty<'_>],
    args: fmt::Arguments<'_>,
) {
    if let Some(d) = G_DISPATCH.get() {
        d.log_dyn_tagged(desc, properties, dyn_properties, args);
    }
}

/// A sink that has accepted a log callsite; obtained from `enabled_log_sink`.
#[doc(hidden)]
pub struct EnabledLogSink {
    dispatch: &'static Dispatch,
    sink: Arc<dyn EventSink>,
}

/// Fetches the sink once and checks the callsite against it, so that the gated macro arm
/// neither re-checks nor re-fetches.
#[doc(hidden)]
#[inline(always)]
pub fn enabled_log_sink(desc: &LogMetadata) -> Option<EnabledLogSink> {
    let dispatch = G_DISPATCH.get()?;
    let sink = dispatch.get_sink();
    if sink.on_log_enabled(desc) {
        Some(EnabledLogSink { dispatch, sink })
    } else {
        None
    }
}

#[doc(hidden)]
#[inline(always)]
pub fn log_dyn_tagged_enabled(
    sink: EnabledLogSink,
    desc: &'static LogMetadata,
    properties: &'static PropertySet,
    dyn_properties: &[DynProperty<'_>],
    args: fmt::Arguments<'_>,
) {
    sink.dispatch
        .log_dyn_tagged_to_sink(&*sink.sink, desc, properties, dyn_properties, args);
}

#[inline(always)]
pub fn log_interop_dyn_tagged(
    metadata: &LogMetadata,
    dyn_properties: &[DynProperty<'_>],
    args: fmt::Arguments<'_>,
) {
    if let Some(d) = G_DISPATCH.get() {
        d.log_interop_dyn_tagged(metadata, dyn_properties, args);
    }
}

#[inline(always)]
pub fn log(desc: &'static LogMetadata, args: fmt::Arguments<'_>) {
    if let Some(d) = G_DISPATCH.get() {
        d.log(desc, args);
    }
}

#[inline(always)]
pub fn log_tagged(
    desc: &'static LogMetadata,
    properties: &'static PropertySet,
    args: fmt::Arguments<'_>,
) {
    if let Some(d) = G_DISPATCH.get() {
        d.log_tagged(desc, properties, args);
    }
}

#[inline(always)]
pub fn log_interop(metadata: &LogMetadata, args: fmt::Arguments<'_>) {
    if let Some(d) = G_DISPATCH.get() {
        d.log_interop(metadata, args);
    }
}

#[inline(always)]
pub fn log_enabled(metadata: &LogMetadata) -> bool {
    if let Some(d) = G_DISPATCH.get() {
        d.log_enabled(metadata)
    } else {
        false
    }
}

#[inline(always)]
pub fn flush_log_buffer() {
    if let Some(d) = G_DISPATCH.get() {
        d.flush_log_buffer();
    }
}

pub fn send_image(name: &str, format: &str, data: Vec<u8>) {
    if let Some(d) = G_DISPATCH.get() {
        d.send_image(name, format, data);
    }
}

#[inline(always)]
pub fn flush_image_buffer() {
    if let Some(d) = G_DISPATCH.get() {
        d.flush_image_buffer();
    }
}

#[inline(always)]
pub fn flush_metrics_buffer() {
    if let Some(d) = G_DISPATCH.get() {
        d.flush_metrics_buffer();
    }
}

//todo: should be implicit by default but limit the maximum number of tracked
// threads
#[inline(always)]
pub fn init_thread_stream() {
    LOCAL_THREAD_STREAM.with(|cell| unsafe {
        if (*cell.as_ptr()).is_some() {
            return;
        }
        #[allow(static_mut_refs)]
        if let Some(d) = G_DISPATCH.get() {
            // Check if CPU tracing is enabled before creating thread stream
            if !d.cpu_tracing_enabled {
                return;
            }
            d.init_thread_stream(cell);
        } else {
            warn!("dispatch not initialized, cannot init thread stream, events will be lost for this thread");
        }
    });
}

pub fn for_each_thread_stream(fun: &mut dyn FnMut(*mut ThreadStream)) {
    if let Some(d) = G_DISPATCH.get() {
        d.for_each_thread_stream(fun);
    }
}

#[inline(always)]
pub fn flush_thread_buffer() {
    LOCAL_THREAD_STREAM.with(|cell| unsafe {
        let opt_stream = &mut *cell.as_ptr();
        if let Some(stream) = opt_stream {
            #[allow(static_mut_refs)]
            match G_DISPATCH.get() {
                Some(d) => {
                    d.flush_thread_buffer(stream);
                }
                None => {
                    panic!("threads are recording but there is no event dispatch");
                }
            }
        }
    });
}

/// Unregisters the current thread's stream from the global dispatch to prevent
/// dangling pointers when the thread is destroyed.
#[inline(always)]
pub fn unregister_thread_stream() {
    LOCAL_THREAD_STREAM.with(|cell| unsafe {
        let opt_stream = &mut *cell.as_ptr();
        if let Some(stream) = opt_stream {
            #[allow(static_mut_refs)]
            match G_DISPATCH.get() {
                Some(d) => {
                    // Flush any remaining events before unregistering
                    d.flush_thread_buffer(stream);
                    // Unregister the thread stream
                    d.unregister_thread_stream(stream);
                    // Clear the local thread stream
                    *opt_stream = None;
                }
                None => {
                    // If dispatch is already shut down, just clear the local stream
                    *opt_stream = None;
                }
            }
        }
    });
}

#[inline(always)]
pub fn on_begin_scope(scope: &'static SpanMetadata) {
    on_thread_event(BeginThreadSpanEvent {
        time: now(),
        thread_span_desc: scope,
    });
}

#[inline(always)]
pub fn on_end_scope(scope: &'static SpanMetadata) {
    on_thread_event(EndThreadSpanEvent {
        time: now(),
        thread_span_desc: scope,
    });
}

#[inline(always)]
pub fn on_begin_named_scope(thread_span_location: &'static SpanLocation, name: &'static str) {
    on_thread_event(BeginThreadNamedSpanEvent {
        thread_span_location,
        name: name.into(),
        time: now(),
    });
}

#[inline(always)]
pub fn on_end_named_scope(thread_span_location: &'static SpanLocation, name: &'static str) {
    on_thread_event(EndThreadNamedSpanEvent {
        thread_span_location,
        name: name.into(),
        time: now(),
    });
}

#[inline(always)]
pub fn on_begin_async_scope(scope: &'static SpanMetadata, parent_span_id: u64, depth: u32) -> u64 {
    let id = G_ASYNC_SPAN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    on_thread_event(BeginAsyncSpanEvent {
        span_desc: scope,
        span_id: id as u64,
        parent_span_id,
        depth,
        time: now(),
    });
    id as u64
}

#[inline(always)]
pub fn on_end_async_scope(
    span_id: u64,
    parent_span_id: u64,
    scope: &'static SpanMetadata,
    depth: u32,
) {
    on_thread_event(EndAsyncSpanEvent {
        span_desc: scope,
        span_id,
        parent_span_id,
        depth,
        time: now(),
    });
}

#[inline(always)]
pub fn on_begin_async_named_scope(
    span_location: &'static SpanLocation,
    name: &'static str,
    parent_span_id: u64,
    depth: u32,
) -> u64 {
    let id = G_ASYNC_SPAN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    on_thread_event(BeginAsyncNamedSpanEvent {
        span_location,
        name: name.into(),
        span_id: id as u64,
        parent_span_id,
        depth,
        time: now(),
    });
    id as u64
}

#[inline(always)]
pub fn on_end_async_named_scope(
    span_id: u64,
    parent_span_id: u64,
    span_location: &'static SpanLocation,
    name: &'static str,
    depth: u32,
) {
    on_thread_event(EndAsyncNamedSpanEvent {
        span_location,
        name: name.into(),
        span_id,
        parent_span_id,
        depth,
        time: now(),
    });
}

pub struct DispatchCell {
    // unsafecell is only necessary for force_uninit()
    inner: UnsafeCell<OnceCell<Dispatch>>,
}

impl DispatchCell {
    const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(OnceCell::new()),
        }
    }

    fn get(&self) -> Option<&Dispatch> {
        unsafe { (*self.inner.get()).get() }
    }

    unsafe fn take(&self) -> Option<Dispatch> {
        unsafe { (*self.inner.get()).take() }
    }
}

// very unsafe indeed - we don't want to pay for locking every time we need to record an event
unsafe impl Sync for DispatchCell {}

static G_DISPATCH: DispatchCell = DispatchCell::new();
static G_ASYNC_SPAN_COUNTER: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(1);

/// # Safety
/// very unsafe! make sure there is no thread running that could be sending events
/// normally that global state should not be destroyed
pub unsafe fn force_uninit() {
    unsafe {
        G_DISPATCH.take();
    }
}

thread_local! {
    static LOCAL_THREAD_STREAM: Cell<Option<ThreadStream>> = const { Cell::new(None) };
}

#[inline(always)]
fn on_thread_event<T>(event: T)
where
    T: micromegas_transit::InProcSerialize + ThreadEventQueueTypeIndex,
{
    LOCAL_THREAD_STREAM.with(|cell| unsafe {
        let opt_stream = &mut *cell.as_ptr();
        if let Some(stream) = opt_stream {
            stream.get_events_mut().push(event);
            if stream.is_full() {
                flush_thread_buffer();
            }
        }
    });
}

struct Dispatch {
    process_id: uuid::Uuid,
    logs_buffer_size: usize,
    metrics_buffer_size: usize,
    threads_buffer_size: usize,
    cpu_tracing_enabled: bool,
    image_stream: Mutex<ImageStream>,
    log_stream: Mutex<LogStream>,
    metrics_stream: Mutex<MetricsStream>,
    thread_streams: Mutex<Vec<*mut ThreadStream>>, // very very unsafe - threads would need to be unregistered before they are destroyed
    sink: RwLock<Arc<dyn EventSink>>,
}

impl Dispatch {
    pub fn new(
        logs_buffer_size: usize,
        metrics_buffer_size: usize,
        threads_buffer_size: usize,
        sink: Arc<dyn EventSink>,
        process_properties: HashMap<String, String>,
        cpu_tracing_enabled: bool,
    ) -> Self {
        let process_id = uuid::Uuid::new_v4();
        let obj = Self {
            process_id,
            logs_buffer_size,
            metrics_buffer_size,
            threads_buffer_size,
            cpu_tracing_enabled,
            image_stream: Mutex::new(ImageStream::new(
                IMAGE_BUFFER_SIZE,
                process_id,
                &[String::from("image")],
                HashMap::new(),
            )),
            log_stream: Mutex::new(LogStream::new(
                logs_buffer_size,
                process_id,
                &[String::from("log")],
                HashMap::new(),
            )),
            metrics_stream: Mutex::new(MetricsStream::new(
                metrics_buffer_size,
                process_id,
                &[String::from("metrics")],
                HashMap::new(),
            )),
            thread_streams: Mutex::new(vec![]),
            sink: RwLock::new(sink),
        };
        obj.startup(process_properties);
        obj.init_image_stream();
        obj.init_log_stream();
        obj.init_metrics_stream();
        obj
    }

    pub fn get_process_id(&self) -> uuid::Uuid {
        self.process_id
    }

    pub fn get_cpu_tracing_enabled(&self) -> bool {
        self.cpu_tracing_enabled
    }

    pub fn get_sink(&self) -> Arc<dyn EventSink> {
        if let Ok(guard) = self.sink.try_read() {
            (*guard).clone()
        } else {
            Arc::new(NullEventSink {})
        }
    }

    fn shutdown(&self) {
        let old_sink = self.get_sink();
        let null_sink = Arc::new(NullEventSink {});
        if let Ok(mut guard) = self.sink.write() {
            *guard = null_sink;
            drop(guard)
        }
        old_sink.on_shutdown();
    }

    fn startup(&self, process_properties: HashMap<String, String>) {
        let mut parent_process = None;

        if let Ok(parent_process_guid) = std::env::var("MICROMEGAS_TELEMETRY_PARENT_PROCESS")
            && let Ok(parent_process_id) = uuid::Uuid::try_parse(&parent_process_guid)
        {
            parent_process = Some(parent_process_id);
        }

        unsafe {
            std::env::set_var(
                "MICROMEGAS_TELEMETRY_PARENT_PROCESS",
                self.process_id.to_string(),
            );
        }

        let process_info = Arc::new(make_process_info(
            self.process_id,
            parent_process,
            process_properties,
        ));

        self.get_sink().on_startup(process_info);
    }

    fn init_log_stream(&self) {
        let log_stream = self.log_stream.lock().unwrap();
        self.get_sink().on_init_log_stream(&log_stream);
    }

    fn init_image_stream(&self) {
        let image_stream = self.image_stream.lock().expect("image_stream lock");
        self.get_sink().on_init_image_stream(&image_stream);
    }

    fn init_metrics_stream(&self) {
        let metrics_stream = self.metrics_stream.lock().unwrap();
        self.get_sink().on_init_metrics_stream(&metrics_stream);
    }

    pub fn send_image(&self, name: &str, format: &str, data: Vec<u8>) {
        let time = now();
        let mut image_stream = self.image_stream.lock().expect("image_stream lock");
        image_stream.get_events_mut().push(ImageEvent {
            time,
            name: micromegas_transit::DynString(name.to_owned()),
            format: micromegas_transit::DynString(format.to_owned()),
            data: micromegas_transit::DynBlob(data),
        });
        if image_stream.is_full() {
            drop(image_stream);
            self.flush_image_buffer();
        }
    }

    fn flush_image_buffer(&self) {
        let mut image_stream = self.image_stream.lock().expect("image_stream lock");
        if image_stream.is_empty() {
            return;
        }
        let stream_id = image_stream.stream_id();
        let next_offset = image_stream.get_block_ref().object_offset()
            + image_stream.get_block_ref().nb_objects();
        let new_block = Arc::new(ImageBlock::new(
            IMAGE_BUFFER_SIZE,
            self.process_id,
            stream_id,
            next_offset,
        ));
        let begin = new_block.begin;
        let mut old_event_block = image_stream.replace_block(new_block);
        assert!(!image_stream.is_full());
        Arc::get_mut(&mut old_event_block)
            .expect("image block exclusive ref")
            .close_at(begin);
        drop(image_stream);
        self.get_sink().on_process_image_block(old_event_block);
    }

    fn init_thread_stream(&self, cell: &Cell<Option<ThreadStream>>) {
        // Early return if CPU tracing is disabled
        if !self.cpu_tracing_enabled {
            return;
        }

        let mut properties = HashMap::new();
        properties.insert(String::from("thread-id"), thread_id::get().to_string());
        if let Some(name) = std::thread::current().name() {
            properties.insert("thread-name".to_owned(), name.to_owned());
        }
        let thread_stream = ThreadStream::new(
            self.threads_buffer_size,
            self.process_id,
            &["cpu".to_owned()],
            properties,
        );
        unsafe {
            let opt_ref = &mut *cell.as_ptr();
            self.get_sink().on_init_thread_stream(&thread_stream);
            *opt_ref = Some(thread_stream);
            let mut vec_guard = self.thread_streams.lock().unwrap();
            vec_guard.push(opt_ref.as_mut().unwrap());
        }
    }

    fn for_each_thread_stream(&self, fun: &mut dyn FnMut(*mut ThreadStream)) {
        let mut vec_guard = self.thread_streams.lock().unwrap();
        for stream in &mut *vec_guard {
            fun(*stream);
        }
    }

    fn unregister_thread_stream(&self, stream_to_remove: &mut ThreadStream) {
        let mut vec_guard = self.thread_streams.lock().unwrap();
        let stream_ptr = stream_to_remove as *mut ThreadStream;

        // Find and remove the thread stream pointer from the vector
        if let Some(pos) = vec_guard.iter().position(|&ptr| ptr == stream_ptr) {
            vec_guard.remove(pos);
        }
    }

    #[inline]
    fn int_metric(&self, desc: &'static StaticMetricMetadata, value: u64) {
        let time = now();
        let mut metrics_stream = self.metrics_stream.lock().unwrap();
        metrics_stream
            .get_events_mut()
            .push(IntegerMetricEvent { desc, value, time });
        self.on_metrics_pushed(metrics_stream);
    }

    #[inline]
    fn float_metric(&self, desc: &'static StaticMetricMetadata, value: f64) {
        let time = now();
        let mut metrics_stream = self.metrics_stream.lock().unwrap();
        metrics_stream
            .get_events_mut()
            .push(FloatMetricEvent { desc, value, time });
        self.on_metrics_pushed(metrics_stream);
    }

    #[inline]
    fn tagged_float_metric(
        &self,
        desc: &'static StaticMetricMetadata,
        properties: &'static PropertySet,
        value: f64,
    ) {
        let time = now();
        let mut metrics_stream = self.metrics_stream.lock().unwrap();
        metrics_stream
            .get_events_mut()
            .push(TaggedFloatMetricEvent {
                desc,
                properties: PropertySetId::interned(properties),
                value,
                time,
            });
        self.on_metrics_pushed(metrics_stream);
    }

    #[inline]
    fn tagged_integer_metric(
        &self,
        desc: &'static StaticMetricMetadata,
        properties: &'static PropertySet,
        value: u64,
    ) {
        let time = now();
        let mut metrics_stream = self.metrics_stream.lock().unwrap();
        metrics_stream
            .get_events_mut()
            .push(TaggedIntegerMetricEvent {
                desc,
                properties: PropertySetId::interned(properties),
                value,
                time,
            });
        self.on_metrics_pushed(metrics_stream);
    }

    #[inline]
    fn tagged_float_metric_dyn(
        &self,
        desc: &'static StaticMetricMetadata,
        properties: &'static PropertySet,
        dyn_properties: &[DynProperty<'_>],
        value: f64,
    ) {
        if dyn_properties.is_empty() {
            return self.tagged_float_metric(desc, properties, value);
        }
        let time = now();
        let merged = merge_properties(properties, dyn_properties);
        let mut metrics_stream = self.metrics_stream.lock().unwrap();
        let (events, dyn_deps) = metrics_stream.get_block_mut_parts();
        let properties = dyn_deps.intern_set(&merged);
        events.push(TaggedFloatMetricEvent {
            desc,
            properties,
            value,
            time,
        });
        self.on_metrics_pushed(metrics_stream);
    }

    #[inline]
    fn tagged_integer_metric_dyn(
        &self,
        desc: &'static StaticMetricMetadata,
        properties: &'static PropertySet,
        dyn_properties: &[DynProperty<'_>],
        value: u64,
    ) {
        if dyn_properties.is_empty() {
            return self.tagged_integer_metric(desc, properties, value);
        }
        let time = now();
        let merged = merge_properties(properties, dyn_properties);
        let mut metrics_stream = self.metrics_stream.lock().unwrap();
        let (events, dyn_deps) = metrics_stream.get_block_mut_parts();
        let properties = dyn_deps.intern_set(&merged);
        events.push(TaggedIntegerMetricEvent {
            desc,
            properties,
            value,
            time,
        });
        self.on_metrics_pushed(metrics_stream);
    }

    #[inline]
    fn on_metrics_pushed(&self, mut metrics_stream: MutexGuard<'_, MetricsStream>) {
        if !metrics_stream.is_full() {
            return;
        }
        // Swapped in the critical section that filled the block, so no other thread can push
        // into it past the size margin or flush it a second time.
        let old_block = swap_block(
            &mut metrics_stream,
            self.metrics_buffer_size,
            self.process_id,
        );
        drop(metrics_stream);
        self.get_sink().on_process_metrics_block(old_block);
    }

    #[inline]
    fn flush_metrics_buffer(&self) {
        let mut metrics_stream = self.metrics_stream.lock().unwrap();
        if metrics_stream.is_empty() {
            return;
        }
        let old_block = swap_block(
            &mut metrics_stream,
            self.metrics_buffer_size,
            self.process_id,
        );
        drop(metrics_stream);
        self.get_sink().on_process_metrics_block(old_block);
    }

    fn log_enabled(&self, metadata: &LogMetadata) -> bool {
        self.get_sink().on_log_enabled(metadata)
    }

    #[inline]
    fn log(&self, metadata: &'static LogMetadata, args: fmt::Arguments<'_>) {
        let sink = self.get_sink();
        if !sink.on_log_enabled(metadata) {
            return;
        }
        let time = now();
        sink.on_log(metadata, &[], time, args);
        let msg = match args.as_str() {
            Some(_) => None,
            None => Some(micromegas_transit::DynString(args.to_string())),
        };
        let mut log_stream = self.log_stream.lock().unwrap();
        match msg {
            None => log_stream.get_events_mut().push(LogStaticStrEvent {
                desc: metadata,
                time,
            }),
            Some(msg) => log_stream.get_events_mut().push(LogStringEvent {
                desc: metadata,
                time,
                msg,
            }),
        }
        self.on_log_pushed(log_stream);
    }

    #[inline]
    fn log_tagged(
        &self,
        desc: &'static LogMetadata,
        properties: &'static PropertySet,
        args: fmt::Arguments<'_>,
    ) {
        let sink = self.get_sink();
        if !sink.on_log_enabled(desc) {
            return;
        }
        self.log_tagged_to_sink(&*sink, desc, properties, args);
    }

    #[inline]
    fn log_tagged_to_sink(
        &self,
        sink: &dyn EventSink,
        desc: &'static LogMetadata,
        properties: &'static PropertySet,
        args: fmt::Arguments<'_>,
    ) {
        let time = now();
        let sink_properties: Vec<DynProperty<'static>> = properties.dyn_properties().collect();
        sink.on_log(desc, &sink_properties, time, args);
        let msg = micromegas_transit::DynString(args.to_string());
        let mut log_stream = self.log_stream.lock().unwrap();
        log_stream.get_events_mut().push(TaggedLogString {
            desc,
            properties: PropertySetId::interned(properties),
            time,
            msg,
        });
        self.on_log_pushed(log_stream);
    }

    #[inline]
    fn log_interop(&self, desc: &LogMetadata, args: fmt::Arguments<'_>) {
        let time = now();
        self.get_sink().on_log(desc, &[], time, args);
        let target = intern_string(desc.target);
        let msg = args
            .as_str()
            .ok_or_else(|| micromegas_transit::DynString(args.to_string()));
        let mut log_stream = self.log_stream.lock().unwrap();
        match msg {
            Ok(static_msg) => log_stream.get_events_mut().push(LogStaticStrInteropEvent {
                time,
                level: desc.level as u32,
                target: target.into(),
                msg: static_msg.into(),
            }),
            Err(msg) => log_stream.get_events_mut().push(LogStringInteropEvent {
                time,
                level: desc.level as u8,
                target: target.into(),
                msg,
            }),
        }
        self.on_log_pushed(log_stream);
    }

    #[inline]
    fn log_dyn_tagged(
        &self,
        desc: &'static LogMetadata,
        properties: &'static PropertySet,
        dyn_properties: &[DynProperty<'_>],
        args: fmt::Arguments<'_>,
    ) {
        let sink = self.get_sink();
        if !sink.on_log_enabled(desc) {
            return;
        }
        self.log_dyn_tagged_to_sink(&*sink, desc, properties, dyn_properties, args);
    }

    #[inline]
    fn log_dyn_tagged_to_sink(
        &self,
        sink: &dyn EventSink,
        desc: &'static LogMetadata,
        properties: &'static PropertySet,
        dyn_properties: &[DynProperty<'_>],
        args: fmt::Arguments<'_>,
    ) {
        if dyn_properties.is_empty() {
            return self.log_tagged_to_sink(sink, desc, properties, args);
        }
        let time = now();
        let merged = merge_properties(properties, dyn_properties);
        // Before taking the lock, so that local sinks can log without deadlocking.
        sink.on_log(desc, &merged.properties, time, args);
        let msg = micromegas_transit::DynString(args.to_string());
        let mut log_stream = self.log_stream.lock().unwrap();
        let (events, dyn_deps) = log_stream.get_block_mut_parts();
        let properties = dyn_deps.intern_set(&merged);
        events.push(TaggedLogString {
            desc,
            properties,
            time,
            msg,
        });
        self.on_log_pushed(log_stream);
    }

    #[inline]
    fn log_interop_dyn_tagged(
        &self,
        desc: &LogMetadata,
        dyn_properties: &[DynProperty<'_>],
        args: fmt::Arguments<'_>,
    ) {
        let time = now();
        let merged = merge_properties(PropertySet::empty(), dyn_properties);
        // Sinks treat `target`/`level` properties as overrides; interop logs must keep their own.
        let is_override = |p: &DynProperty<'_>| {
            p.name.eq_ignore_ascii_case("target") || p.name.eq_ignore_ascii_case("level")
        };
        if merged.properties.iter().any(is_override) {
            let filtered: Vec<DynProperty<'_>> = merged
                .properties
                .iter()
                .copied()
                .filter(|p| !is_override(p))
                .collect();
            self.get_sink().on_log(desc, &filtered, time, args);
        } else {
            self.get_sink().on_log(desc, &merged.properties, time, args);
        }
        let msg = micromegas_transit::DynString(args.to_string());
        let target = intern_string(desc.target);
        let mut log_stream = self.log_stream.lock().unwrap();
        let (events, dyn_deps) = log_stream.get_block_mut_parts();
        let properties = dyn_deps.intern_set(&merged);
        events.push(TaggedLogInteropEvent {
            time,
            level: desc.level as u8,
            target: target.into(),
            properties,
            msg,
        });
        self.on_log_pushed(log_stream);
    }

    #[inline]
    fn on_log_pushed(&self, mut log_stream: MutexGuard<'_, LogStream>) {
        if !log_stream.is_full() {
            return;
        }
        let old_block = swap_block(&mut log_stream, self.logs_buffer_size, self.process_id);
        drop(log_stream);
        self.get_sink().on_process_log_block(old_block);
    }

    #[inline]
    fn flush_log_buffer(&self) {
        let mut log_stream = self.log_stream.lock().unwrap();
        if log_stream.is_empty() {
            return;
        }
        let old_block = swap_block(&mut log_stream, self.logs_buffer_size, self.process_id);
        drop(log_stream);
        self.get_sink().on_process_log_block(old_block);
    }

    #[inline]
    fn flush_thread_buffer(&self, stream: &mut ThreadStream) {
        if stream.is_empty() {
            return;
        }
        let next_offset =
            stream.get_block_ref().object_offset() + stream.get_block_ref().nb_objects();
        let new_block = Arc::new(ThreadBlock::new(
            self.threads_buffer_size,
            self.process_id,
            stream.stream_id(),
            next_offset,
        ));
        let begin = new_block.begin;
        let mut old_block = stream.replace_block(new_block);
        assert!(!stream.is_full());
        Arc::get_mut(&mut old_block).unwrap().close_at(begin);
        self.get_sink().on_process_thread_block(old_block);
    }
}

/// Replaces the stream's current block with an empty one and returns the closed old block.
///
/// Must be called with the stream locked: `begin` is stamped here so that consecutive blocks
/// stay contiguous.
fn swap_block<Q>(
    stream: &mut EventStream<EventBlock<Q>>,
    buffer_size: usize,
    process_id: uuid::Uuid,
) -> Arc<EventBlock<Q>>
where
    Q: micromegas_transit::HeterogeneousQueue + ExtractDeps,
    EventBlock<Q>: TracingBlock,
{
    let stream_id = stream.stream_id();
    let next_offset = stream.get_block_ref().object_offset() + stream.get_block_ref().nb_objects();
    let new_block = Arc::new(EventBlock::<Q>::new(
        buffer_size,
        process_id,
        stream_id,
        next_offset,
    ));
    let begin = new_block.begin;
    let mut old_block = stream.replace_block(new_block);
    assert!(!stream.is_full());
    Arc::get_mut(&mut old_block).unwrap().close_at(begin);
    old_block
}

fn get_cpu_brand() -> String {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    return raw_cpuid::CpuId::new()
        .get_processor_brand_string()
        .map_or_else(|| "unknown".to_owned(), |b| b.as_str().to_owned());
    #[cfg(target_arch = "aarch64")]
    return String::from("aarch64");
}

pub fn make_process_info(
    process_id: uuid::Uuid,
    parent_process_id: Option<uuid::Uuid>,
    properties: HashMap<String, String>,
) -> ProcessInfo {
    let start_ticks = now();
    let start_time = Utc::now();
    let cpu_brand = get_cpu_brand();
    ProcessInfo {
        process_id,
        username: whoami::username(),
        realname: whoami::realname(),
        exe: std::env::current_exe()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        computer: whoami::devicename(),
        distro: whoami::distro(),
        cpu_brand,
        tsc_frequency: frequency(),
        start_time,
        start_ticks,
        parent_process_id,
        properties,
    }
}
