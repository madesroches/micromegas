use micromegas_telemetry_sink::tracing_interop::TracingCaptureLayer;
use micromegas_tracing::{
    dispatch::{flush_log_buffer, flush_thread_buffer},
    levels::LevelFilter,
    logs::LogMsgQueueAny,
    test_utils::init_in_memory_tracing,
};
use micromegas_tracing::{
    event::{EventSink, in_memory_sink::InMemorySink},
    logs::{LogBlock, LogMetadata, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    spans::{ThreadBlock, ThreadEventQueueAny, ThreadStream},
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
                    let props = block
                        .dyn_deps
                        .set_properties(e.properties)
                        .into_iter()
                        .map(|(name, value)| (name.to_owned(), value.to_owned()))
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
        capture_spans: true,
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

#[test]
#[serial]
fn consecutive_events_do_not_share_fields() {
    let guard = init_in_memory_tracing();
    let subscriber = tracing_subscriber::registry().with(TracingCaptureLayer {
        max_level: LevelFilter::Trace,
        capture_spans: true,
    });
    let large = "x".repeat(10_000);
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(a = 1, "first");
        tracing::info!(big = %large, "second");
        tracing::info!(c = "3", "third");
    });
    flush_log_buffer();

    let (tagged, _) = captured_interop_events(&guard);
    assert_eq!(tagged.len(), 3);
    assert_eq!(tagged[0].0, "first a=1 ");
    assert_eq!(tagged[0].1, vec![("a".to_owned(), "1".to_owned())]);
    assert_eq!(tagged[1].0, format!("second big={large} "));
    assert_eq!(tagged[1].1, vec![("big".to_owned(), large.clone())]);
    assert_eq!(tagged[2].0, "third c=\"3\" ");
    assert_eq!(tagged[2].1, vec![("c".to_owned(), "3".to_owned())]);
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
                .map(|p| (p.name().to_owned(), p.value().to_owned()))
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
        capture_spans: true,
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

#[derive(Debug, PartialEq)]
struct AsyncSpan {
    span_id: u64,
    parent_span_id: u64,
    depth: u32,
    name: String,
    location: usize,
    properties: Vec<(String, String)>,
}

/// The begin and end events of the async spans in the flushed thread blocks.
fn captured_async_spans(
    guard: &micromegas_tracing::test_utils::InMemoryTracingGuard,
) -> (Vec<AsyncSpan>, Vec<AsyncSpan>) {
    let state = guard.sink.state.lock().unwrap();
    let mut begins = vec![];
    let mut ends = vec![];
    for block in &state.thread_blocks {
        for evt in block.events.iter() {
            match evt {
                ThreadEventQueueAny::BeginAsyncDynNamedSpanEvent(e) => begins.push(AsyncSpan {
                    span_id: e.span_id,
                    parent_span_id: e.parent_span_id,
                    depth: e.depth,
                    name: block.dyn_deps.str(e.name.id).to_owned(),
                    location: e.span_location as *const _ as usize,
                    properties: vec![],
                }),
                ThreadEventQueueAny::EndAsyncDynNamedSpanEvent(e) => ends.push(AsyncSpan {
                    span_id: e.span_id,
                    parent_span_id: e.parent_span_id,
                    depth: e.depth,
                    name: block.dyn_deps.str(e.name.id).to_owned(),
                    location: e.span_location as *const _ as usize,
                    properties: vec![],
                }),
                ThreadEventQueueAny::TaggedEndAsyncNamedSpanEvent(e) => {
                    let mut properties: Vec<(String, String)> = block
                        .dyn_deps
                        .set_properties(e.properties)
                        .into_iter()
                        .map(|(name, value)| (name.to_owned(), value.to_owned()))
                        .collect();
                    properties.sort();
                    ends.push(AsyncSpan {
                        span_id: e.span_id,
                        parent_span_id: e.parent_span_id,
                        depth: e.depth,
                        name: block.dyn_deps.str(e.name.id).to_owned(),
                        location: e.span_location as *const _ as usize,
                        properties,
                    });
                }
                _ => {}
            }
        }
    }
    (begins, ends)
}

fn layer(capture_spans: bool) -> TracingCaptureLayer {
    TracingCaptureLayer {
        max_level: LevelFilter::Trace,
        capture_spans,
    }
}

fn props(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
#[serial]
fn a_span_yields_one_matched_begin_and_end() {
    let guard = init_in_memory_tracing();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer(true)), || {
        let _span = tracing::info_span!("solo").entered();
    });
    flush_thread_buffer();

    let (begins, ends) = captured_async_spans(&guard);
    assert_eq!(begins.len(), 1);
    assert_eq!(ends.len(), 1);
    assert_eq!(begins[0].name, "solo");
    assert_eq!(ends[0].name, "solo");
    assert_eq!(begins[0].span_id, ends[0].span_id);
    assert_eq!(begins[0].parent_span_id, 0);
    assert_eq!(begins[0].depth, 0);
    assert_eq!(ends[0].parent_span_id, 0);
    assert_eq!(ends[0].depth, 0);
}

#[test]
#[serial]
fn nested_spans_link_parent_and_increase_depth() {
    let guard = init_in_memory_tracing();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer(true)), || {
        let _outer = tracing::info_span!("outer").entered();
        let _inner = tracing::info_span!("inner").entered();
        let _leaf = tracing::info_span!("leaf").entered();
    });
    flush_thread_buffer();

    let (begins, ends) = captured_async_spans(&guard);
    assert_eq!(begins.len(), 3);
    assert_eq!(ends.len(), 3);
    assert_eq!(begins[1].parent_span_id, begins[0].span_id);
    assert_eq!(begins[2].parent_span_id, begins[1].span_id);
    assert_eq!(
        begins.iter().map(|b| b.depth).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    for begin in &begins {
        let end = ends.iter().find(|e| e.span_id == begin.span_id).unwrap();
        assert_eq!(end.parent_span_id, begin.parent_span_id);
        assert_eq!(end.depth, begin.depth);
        assert_eq!(end.name, begin.name);
    }
}

#[test]
#[serial]
fn span_fields_become_end_properties_and_later_records_win() {
    let guard = init_in_memory_tracing();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer(true)), || {
        let span = tracing::info_span!(
            "work",
            model = "m1",
            tokens = 3,
            late = tracing::field::Empty,
            log.target = "ignored"
        );
        span.record("late", "set");
        span.record("tokens", 7);
        drop(span);
    });
    flush_thread_buffer();

    let (_, ends) = captured_async_spans(&guard);
    assert_eq!(ends.len(), 1);
    assert_eq!(
        ends[0].properties,
        props(&[("late", "set"), ("model", "m1"), ("tokens", "7")])
    );
}

#[test]
#[serial]
fn children_of_a_level_filtered_span_attach_to_the_nearest_captured_ancestor() {
    let guard = init_in_memory_tracing();
    let layer = TracingCaptureLayer {
        max_level: LevelFilter::Info,
        capture_spans: true,
    };
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        let _top = tracing::info_span!("top").entered();
        let _hidden = tracing::trace_span!("hidden").entered();
        let _contextual = tracing::info_span!("contextual").entered();
        let _explicit = tracing::info_span!(parent: tracing::Span::current(), "explicit").entered();
        let _root = tracing::info_span!(parent: None, "root").entered();
    });
    flush_thread_buffer();

    let (begins, ends) = captured_async_spans(&guard);
    let names: Vec<&str> = begins.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, vec!["top", "contextual", "explicit", "root"]);
    assert_eq!(ends.len(), 4);
    let top = &begins[0];
    assert_eq!(
        (begins[1].parent_span_id, begins[1].depth),
        (top.span_id, 1)
    );
    // Explicit parent resolves through the same walk as a contextual one.
    assert_eq!(
        (begins[2].parent_span_id, begins[2].depth),
        (begins[1].span_id, 2)
    );
    assert_eq!((begins[3].parent_span_id, begins[3].depth), (0, 0));
}

#[test]
#[serial]
fn a_child_of_a_filtered_root_becomes_a_root() {
    let guard = init_in_memory_tracing();
    let layer = TracingCaptureLayer {
        max_level: LevelFilter::Info,
        capture_spans: true,
    };
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        let _hidden = tracing::debug_span!("hidden").entered();
        let _child = tracing::info_span!("child").entered();
    });
    flush_thread_buffer();

    let (begins, _) = captured_async_spans(&guard);
    assert_eq!(begins.len(), 1);
    assert_eq!(begins[0].name, "child");
    assert_eq!((begins[0].parent_span_id, begins[0].depth), (0, 0));
}

#[test]
#[serial]
fn disabled_span_capture_emits_nothing() {
    let guard = init_in_memory_tracing();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer(false)), || {
        let _span = tracing::info_span!("quiet", k = 1).entered();
        tracing::info!("still logged");
    });
    flush_thread_buffer();
    flush_log_buffer();

    let (begins, ends) = captured_async_spans(&guard);
    assert!(begins.is_empty());
    assert!(ends.is_empty());
    let (tagged, untagged) = captured_interop_events(&guard);
    assert_eq!(tagged.len() + untagged, 1);
}

#[test]
#[serial]
fn a_span_closed_on_another_thread_still_ends() {
    let guard = init_in_memory_tracing();
    let span = Arc::new(Mutex::new(None));
    let subscriber = tracing_subscriber::registry().with(layer(true));
    let dispatch = tracing::Dispatch::new(subscriber);
    tracing::dispatcher::with_default(&dispatch, || {
        *span.lock().unwrap() = Some(tracing::info_span!("migrating", k = "v"));
    });
    flush_thread_buffer();
    let closer = span.clone();
    std::thread::spawn(move || {
        drop(closer.lock().unwrap().take());
        flush_thread_buffer();
    })
    .join()
    .unwrap();

    let (begins, ends) = captured_async_spans(&guard);
    assert_eq!(begins.len(), 1);
    assert_eq!(ends.len(), 1);
    assert_eq!(begins[0].span_id, ends[0].span_id);
    assert_eq!(ends[0].properties, props(&[("k", "v")]));
}

#[test]
#[serial]
fn spans_from_one_callsite_share_a_location() {
    let guard = init_in_memory_tracing();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer(true)), || {
        for _ in 0..2 {
            let _span = tracing::info_span!("repeated").entered();
        }
        let _other = tracing::info_span!("other").entered();
    });
    flush_thread_buffer();

    let (begins, ends) = captured_async_spans(&guard);
    assert_eq!(begins.len(), 3);
    assert_eq!(begins[0].location, begins[1].location);
    assert_ne!(begins[0].location, begins[2].location);
    assert_eq!(begins[0].location, ends[0].location);
}
