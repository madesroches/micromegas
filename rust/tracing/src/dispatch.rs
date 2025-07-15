//! Where events are recorded and eventually sent to a sink
pub use crate::errors::{Error, Result};
use crate::intern_string::intern_string;
use crate::logs::TaggedLogString;
use crate::metrics::{TaggedFloatMetricEvent, TaggedIntegerMetricEvent};
use crate::prelude::*;
use crate::property_set::PropertySet;
use crate::{
    event::{EventSink, NullEventSink, TracingBlock},
    info,
    logs::{
        LogBlock, LogMetadata, LogStaticStrEvent, LogStaticStrInteropEvent, LogStream,
        LogStringEvent, LogStringInteropEvent,
    },
    metrics::{
        FloatMetricEvent, IntegerMetricEvent, MetricsBlock, MetricsStream, StaticMetricMetadata,
    },
    spans::{
        BeginAsyncNamedSpanEvent, BeginAsyncSpanEvent, BeginThreadNamedSpanEvent,
        BeginThreadSpanEvent, EndAsyncNamedSpanEvent, EndAsyncSpanEvent, EndThreadNamedSpanEvent,
        EndThreadSpanEvent, SpanLocation, SpanMetadata, ThreadBlock, ThreadEventQueueTypeIndex,
        ThreadStream,
    },
    warn,
};
use chrono::Utc;
use std::cell::OnceCell;
use std::collections::HashMap;
use std::fmt;
use std::sync::RwLock;
use std::{
    cell::Cell,
    sync::{Arc, Mutex},
};

pub fn init_event_dispatch(
    logs_buffer_size: usize,
    metrics_buffer_size: usize,
    threads_buffer_size: usize,
    sink: Arc<dyn EventSink>,
    process_properties: HashMap<String, String>,
) -> Result<()> {
    lazy_static::lazy_static! {
        static ref INIT_MUTEX: Mutex<()> = Mutex::new(());
    }
    let _guard = INIT_MUTEX.lock().unwrap();
    let dispatch_ref = &G_DISPATCH.inner;
    if dispatch_ref.get().is_none() {
        dispatch_ref
            .set(Dispatch::new(
                logs_buffer_size,
                metrics_buffer_size,
                threads_buffer_size,
                sink,
                process_properties,
            ))
            .map_err(|_| Error::AlreadyInitialized())
    } else {
        info!("event dispatch already initialized");
        Err(Error::AlreadyInitialized())
    }
}

#[inline]
pub fn process_id() -> Option<uuid::Uuid> {
    G_DISPATCH.get().map(Dispatch::get_process_id)
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
pub fn on_begin_async_scope(scope: &'static SpanMetadata) -> u64 {
    let id = G_ASYNC_SPAN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    on_thread_event(BeginAsyncSpanEvent {
        span_desc: scope,
        span_id: id as u64,
        time: now(),
    });
    id as u64
}

#[inline(always)]
pub fn on_end_async_scope(span_id: u64, scope: &'static SpanMetadata) {
    on_thread_event(EndAsyncSpanEvent {
        span_desc: scope,
        span_id,
        time: now(),
    });
}

#[inline(always)]
pub fn on_begin_async_named_scope(span_location: &'static SpanLocation, name: &'static str) -> u64 {
    let id = G_ASYNC_SPAN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    on_thread_event(BeginAsyncNamedSpanEvent {
        span_location,
        name: name.into(),
        span_id: id as u64,
        time: now(),
    });
    id as u64
}

#[inline(always)]
pub fn on_end_async_named_scope(
    span_id: u64,
    span_location: &'static SpanLocation,
    name: &'static str,
) {
    on_thread_event(EndAsyncNamedSpanEvent {
        span_location,
        name: name.into(),
        span_id,
        time: now(),
    });
}

pub struct DispatchCell {
    inner: OnceCell<Dispatch>,
}

impl DispatchCell {
    const fn new() -> Self {
        Self {
            inner: OnceCell::new(),
        }
    }

    fn get(&self) -> Option<&Dispatch> {
        self.inner.get()
    }
}

// very unsafe indeed - we don't want to pay for locking every time we need to record an event
unsafe impl Sync for DispatchCell {}

static G_DISPATCH: DispatchCell = DispatchCell::new();
static G_ASYNC_SPAN_COUNTER: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

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
    ) -> Self {
        let process_id = uuid::Uuid::new_v4();
        let obj = Self {
            process_id,
            logs_buffer_size,
            metrics_buffer_size,
            threads_buffer_size,
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
        obj.init_log_stream();
        obj.init_metrics_stream();
        obj
    }

    pub fn get_process_id(&self) -> uuid::Uuid {
        self.process_id
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

        if let Ok(parent_process_guid) = std::env::var("MICROMEGAS_TELEMETRY_PARENT_PROCESS") {
            if let Ok(parent_process_id) = uuid::Uuid::try_parse(&parent_process_guid) {
                parent_process = Some(parent_process_id);
            }
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

    fn init_metrics_stream(&self) {
        let metrics_stream = self.metrics_stream.lock().unwrap();
        self.get_sink().on_init_metrics_stream(&metrics_stream);
    }

    fn init_thread_stream(&self, cell: &Cell<Option<ThreadStream>>) {
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

    #[inline]
    fn int_metric(&self, desc: &'static StaticMetricMetadata, value: u64) {
        let time = now();
        let mut metrics_stream = self.metrics_stream.lock().unwrap();
        metrics_stream
            .get_events_mut()
            .push(IntegerMetricEvent { desc, value, time });
        if metrics_stream.is_full() {
            // Release the lock before calling flush_metrics_buffer
            drop(metrics_stream);
            self.flush_metrics_buffer();
        }
    }

    #[inline]
    fn float_metric(&self, desc: &'static StaticMetricMetadata, value: f64) {
        let time = now();
        let mut metrics_stream = self.metrics_stream.lock().unwrap();
        metrics_stream
            .get_events_mut()
            .push(FloatMetricEvent { desc, value, time });
        if metrics_stream.is_full() {
            drop(metrics_stream);
            // Release the lock before calling flush_metrics_buffer
            self.flush_metrics_buffer();
        }
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
                properties,
                value,
                time,
            });
        if metrics_stream.is_full() {
            drop(metrics_stream);
            // Release the lock before calling flush_metrics_buffer
            self.flush_metrics_buffer();
        }
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
                properties,
                value,
                time,
            });
        if metrics_stream.is_full() {
            drop(metrics_stream);
            // Release the lock before calling flush_metrics_buffer
            self.flush_metrics_buffer();
        }
    }

    #[inline]
    fn flush_metrics_buffer(&self) {
        let mut metrics_stream = self.metrics_stream.lock().unwrap();
        if metrics_stream.is_empty() {
            return;
        }
        let stream_id = metrics_stream.stream_id();
        let next_offset = metrics_stream.get_block_ref().object_offset()
            + metrics_stream.get_block_ref().nb_objects();
        let mut old_event_block = metrics_stream.replace_block(Arc::new(MetricsBlock::new(
            self.metrics_buffer_size,
            self.process_id,
            stream_id,
            next_offset,
        )));
        assert!(!metrics_stream.is_full());
        Arc::get_mut(&mut old_event_block).unwrap().close();
        self.get_sink().on_process_metrics_block(old_event_block);
    }

    fn log_enabled(&self, metadata: &LogMetadata) -> bool {
        self.get_sink().on_log_enabled(metadata)
    }

    #[inline]
    fn log(&self, metadata: &'static LogMetadata, args: fmt::Arguments<'_>) {
        if !self.log_enabled(metadata) {
            return;
        }
        let time = now();
        self.get_sink().on_log(metadata, &[], time, args);
        let mut log_stream = self.log_stream.lock().unwrap();
        if args.as_str().is_some() {
            log_stream.get_events_mut().push(LogStaticStrEvent {
                desc: metadata,
                time,
            });
        } else {
            log_stream.get_events_mut().push(LogStringEvent {
                desc: metadata,
                time,
                msg: micromegas_transit::DynString(args.to_string()),
            });
        }
        if log_stream.is_full() {
            // Release the lock before calling flush_log_buffer
            drop(log_stream);
            self.flush_log_buffer();
        }
    }

    #[inline]
    fn log_tagged(
        &self,
        desc: &'static LogMetadata,
        properties: &'static PropertySet,
        args: fmt::Arguments<'_>,
    ) {
        if !self.log_enabled(desc) {
            return;
        }
        let time = now();
        self.get_sink()
            .on_log(desc, properties.get_properties(), time, args);
        let mut log_stream = self.log_stream.lock().unwrap();
        log_stream.get_events_mut().push(TaggedLogString {
            desc,
            properties,
            time,
            msg: micromegas_transit::DynString(args.to_string()),
        });
        if log_stream.is_full() {
            // Release the lock before calling flush_log_buffer
            drop(log_stream);
            self.flush_log_buffer();
        }
    }

    #[inline]
    fn log_interop(&self, desc: &LogMetadata, args: fmt::Arguments<'_>) {
        let time = now();
        self.get_sink().on_log(desc, &[], time, args);
        let mut log_stream = self.log_stream.lock().unwrap();
        if let Some(msg) = args.as_str() {
            log_stream.get_events_mut().push(LogStaticStrInteropEvent {
                time,
                level: desc.level as u32,
                target: intern_string(desc.target).into(),
                msg: msg.into(),
            });
        } else {
            log_stream.get_events_mut().push(LogStringInteropEvent {
                time,
                level: desc.level as u8,
                target: intern_string(desc.target).into(),
                msg: micromegas_transit::DynString(args.to_string()),
            });
        }
        if log_stream.is_full() {
            // Release the lock before calling flush_log_buffer
            drop(log_stream);
            self.flush_log_buffer();
        }
    }

    #[inline]
    fn flush_log_buffer(&self) {
        let mut log_stream = self.log_stream.lock().unwrap();
        if log_stream.is_empty() {
            return;
        }
        let stream_id = log_stream.stream_id();
        let next_offset =
            log_stream.get_block_ref().object_offset() + log_stream.get_block_ref().nb_objects();
        let mut old_event_block = log_stream.replace_block(Arc::new(LogBlock::new(
            self.logs_buffer_size,
            self.process_id,
            stream_id,
            next_offset,
        )));
        assert!(!log_stream.is_full());
        Arc::get_mut(&mut old_event_block).unwrap().close();
        self.get_sink().on_process_log_block(old_event_block);
    }

    #[inline]
    fn flush_thread_buffer(&self, stream: &mut ThreadStream) {
        if stream.is_empty() {
            return;
        }
        let next_offset =
            stream.get_block_ref().object_offset() + stream.get_block_ref().nb_objects();
        let mut old_block = stream.replace_block(Arc::new(ThreadBlock::new(
            self.threads_buffer_size,
            self.process_id,
            stream.stream_id(),
            next_offset,
        )));
        assert!(!stream.is_full());
        Arc::get_mut(&mut old_block).unwrap().close();
        self.get_sink().on_process_thread_block(old_block);
    }
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
