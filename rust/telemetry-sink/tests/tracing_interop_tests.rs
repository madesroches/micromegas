use micromegas_telemetry_sink::tracing_interop::TracingCaptureLayer;
use micromegas_tracing::{
    dispatch::flush_log_buffer, levels::LevelFilter, logs::LogMsgQueueAny,
    test_utils::init_in_memory_tracing,
};
use micromegas_tracing::{
    event::{EventSink, in_memory_sink::InMemorySink},
    logs::{LogBlock, LogMetadata, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    spans::{ThreadBlock, ThreadStream},
};
use micromegas_transit::{DynString, HeterogeneousQueue};
use serial_test::serial;
use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex},
};
use tracing_subscriber::prelude::*;

type Captured = (String, Vec<(String, String)>);

fn captured_interop_events(
    guard: &micromegas_tracing::test_utils::InMemoryTracingGuard,
) -> (Vec<Captured>, usize) {
    let state = guard.sink.state.lock().unwrap();
    let mut tagged = vec![];
    let mut untagged = 0;
    for block in &state.log_blocks {
        for evt in block.events.iter() {
            match evt {
                LogMsgQueueAny::TaggedLogInteropEvent(e) => {
                    let DynString(msg) = &e.msg;
                    // SAFETY: the sink still holds the block, which owns the property set.
                    let props = unsafe { &*e.properties }
                        .get_properties()
                        .iter()
                        .map(|p| (p.name.as_str().to_owned(), p.value.as_str().to_owned()))
                        .collect();
                    tagged.push((msg.clone(), props));
                }
                LogMsgQueueAny::LogStringInteropEvent(_) => untagged += 1,
                _ => {}
            }
        }
    }
    (tagged, untagged)
}

#[test]
#[serial]
fn event_fields_become_properties() {
    let guard = init_in_memory_tracing();
    let subscriber = tracing_subscriber::registry().with(TracingCaptureLayer {
        max_level: LevelFilter::Trace,
    });
    tracing::subscriber::with_default(subscriber, || {
        let id = String::from("abc-123");
        tracing::info!(correlation_id = %id, n = 3, user = "bob", log.module_path = "ignored", "hello");
        tracing::info!("no fields");
    });
    flush_log_buffer();

    let (tagged, untagged) = captured_interop_events(&guard);
    assert_eq!(untagged, 1);
    assert_eq!(tagged.len(), 1);
    let (msg, props) = &tagged[0];
    assert!(msg.contains("correlation_id=abc-123"), "{msg}");
    assert!(msg.contains("n=3"), "{msg}");
    assert!(msg.contains("user=\"bob\""), "{msg}");
    assert!(msg.contains("hello"), "{msg}");
    let get = |name: &str| -> Vec<&str> {
        props
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .collect()
    };
    assert_eq!(get("correlation_id"), vec!["abc-123"]);
    assert_eq!(get("n"), vec!["3"]);
    assert_eq!(get("user"), vec!["bob"]);
    assert!(get("message").is_empty());
    assert!(props.iter().all(|(k, _)| !k.starts_with("log.")));
}

struct PropertyCaptureSink {
    inner: InMemorySink,
    on_log_props: Mutex<Vec<Vec<(String, String)>>>,
}

impl EventSink for PropertyCaptureSink {
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
        properties: &[DynProperty<'_>],
        _time: i64,
        _args: fmt::Arguments<'_>,
    ) {
        self.on_log_props.lock().unwrap().push(
            properties
                .iter()
                .map(|p| (p.name.to_owned(), p.value.to_owned()))
                .collect(),
        );
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
#[serial]
fn target_and_level_fields_are_not_delivered_to_on_log() {
    let sink = Arc::new(PropertyCaptureSink {
        inner: InMemorySink::new(),
        on_log_props: Mutex::new(vec![]),
    });
    micromegas_tracing::dispatch::init_event_dispatch(
        1024,
        1024,
        1024,
        sink.clone(),
        HashMap::new(),
        false,
    )
    .unwrap();
    let subscriber = tracing_subscriber::registry().with(TracingCaptureLayer {
        max_level: LevelFilter::Trace,
    });
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(target = "x", level = "debug", user = "bob", "m");
    });
    micromegas_tracing::dispatch::shutdown_dispatch();
    unsafe { micromegas_tracing::dispatch::force_uninit() };

    let captured = sink.on_log_props.lock().unwrap();
    assert_eq!(captured.len(), 1);
    let props = &captured[0];
    assert!(props.iter().all(|(k, _)| k != "target" && k != "level"));
    assert!(props.iter().any(|(k, v)| k == "user" && v == "bob"));
}
