use micromegas_telemetry_sink::tracing_interop::TracingCaptureLayer;
use micromegas_tracing::{
    event::{EventSink, in_memory_sink::InMemorySink},
    levels::LevelFilter,
    logs::{LogBlock, LogMetadata, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    spans::{ThreadBlock, ThreadEventQueueAny, ThreadStream},
};
use micromegas_transit::HeterogeneousQueue;
use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tracing_subscriber::prelude::*;

struct ReentrantSink {
    inner: InMemorySink,
    messages: Mutex<Vec<String>>,
    nested_emitted: AtomicBool,
}

impl EventSink for ReentrantSink {
    fn on_startup(&self, process_info: Arc<ProcessInfo>) {
        self.inner.on_startup(process_info)
    }
    fn on_shutdown(&self) {
        self.inner.on_shutdown()
    }
    fn on_log_enabled(&self, metadata: &LogMetadata) -> bool {
        self.inner.on_log_enabled(metadata)
    }
    fn on_log(
        &self,
        _desc: &LogMetadata,
        _properties: &[DynProperty<'_>],
        _time: i64,
        args: fmt::Arguments<'_>,
    ) {
        self.messages.lock().unwrap().push(args.to_string());
        if !self.nested_emitted.swap(true, Ordering::SeqCst) {
            tracing::info!(inner = 2, "nested");
            drop(tracing::info_span!("sink_span", inner = 3).entered());
        }
    }
    fn on_init_log_stream(&self, log_stream: &LogStream) {
        self.inner.on_init_log_stream(log_stream)
    }
    fn on_process_log_block(&self, log_block: Arc<LogBlock>) {
        self.inner.on_process_log_block(log_block)
    }
    fn on_init_metrics_stream(&self, metrics_stream: &MetricsStream) {
        self.inner.on_init_metrics_stream(metrics_stream)
    }
    fn on_process_metrics_block(&self, metrics_block: Arc<MetricsBlock>) {
        self.inner.on_process_metrics_block(metrics_block)
    }
    fn on_init_thread_stream(&self, thread_stream: &ThreadStream) {
        self.inner.on_init_thread_stream(thread_stream)
    }
    fn on_process_thread_block(&self, thread_block: Arc<ThreadBlock>) {
        self.inner.on_process_thread_block(thread_block)
    }
    fn is_busy(&self) -> bool {
        false
    }
}

#[test]
fn event_emitted_by_sink_during_capture_is_not_dropped() {
    let sink = Arc::new(ReentrantSink {
        inner: InMemorySink::new(),
        messages: Mutex::new(vec![]),
        nested_emitted: AtomicBool::new(false),
    });
    micromegas_tracing::dispatch::init_event_dispatch(
        1024,
        1024,
        1024,
        sink.clone(),
        HashMap::new(),
        true,
    )
    .unwrap();
    micromegas_tracing::dispatch::init_thread_stream();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(
        TracingCaptureLayer {
            max_level: LevelFilter::Trace,
            capture_spans: true,
            span_field_hook: None,
        },
    ))
    .expect("no global tracing subscriber is installed yet");

    tracing::info!(outer = 1, "outer");
    micromegas_tracing::dispatch::flush_thread_buffer();

    let span_names: Vec<String> = {
        let state = sink.inner.state.lock().unwrap();
        state
            .thread_blocks
            .iter()
            .flat_map(|block| {
                block.events.iter().filter_map(|evt| match evt {
                    ThreadEventQueueAny::BeginAsyncDynNamedSpanEvent(e) => {
                        Some(block.dyn_deps.str(e.name.id).to_owned())
                    }
                    _ => None,
                })
            })
            .collect()
    };

    micromegas_tracing::dispatch::shutdown_dispatch();
    unsafe { micromegas_tracing::dispatch::force_uninit() };

    let messages = sink.messages.lock().unwrap();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert_eq!(messages[0], "outer outer=1 ");
    assert_eq!(messages[1], "nested inner=2 ");
    assert_eq!(span_names, vec!["sink_span"]);
}
