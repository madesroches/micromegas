use micromegas_tracing::{
    dispatch::{
        flush_log_buffer, flush_metrics_buffer, force_uninit, init_event_dispatch,
        shutdown_dispatch,
    },
    event::{EventSink, TracingBlock},
    levels::{LevelFilter, set_max_level},
    logs::{LogBlock, LogMetadata, LogMsgQueueAny, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    process_info::ProcessInfo,
    property_set::DynProperty,
    spans::{ThreadBlock, ThreadStream},
    test_utils::init_in_memory_tracing,
};
use micromegas_transit::HeterogeneousQueue;
use serial_test::serial;
use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

const BUFFER_SIZE: usize = 1024;
const HINT_MAX_OBJ_SIZE: usize = 128;

fn assert_contiguous<B: TracingBlock>(blocks: &[Arc<B>], begin_end: impl Fn(&B) -> (i64, i64)) {
    for pair in blocks.windows(2) {
        assert_eq!(
            pair[1].object_offset(),
            pair[0].object_offset() + pair[0].nb_objects()
        );
        assert_eq!(begin_end(&pair[0]).1, begin_end(&pair[1]).0);
    }
}

fn log_span(block: &LogBlock) -> (i64, i64) {
    (block.begin.ticks, block.end.as_ref().unwrap().ticks)
}

fn metrics_span(block: &MetricsBlock) -> (i64, i64) {
    (block.begin.ticks, block.end.as_ref().unwrap().ticks)
}

#[test]
#[serial]
fn blocks_are_contiguous_and_never_reallocated() {
    let guard = init_in_memory_tracing();
    set_max_level(LevelFilter::Trace);
    for i in 0..200 {
        info!("event number {}", i);
        imetric!("test_int_metric", "count", i as u64);
        fmetric!("test_float_metric", "count", i as f64);
    }
    flush_log_buffer();
    flush_metrics_buffer();

    let state = guard.sink.state.lock().unwrap();
    assert!(state.log_blocks.len() > 3);
    assert!(state.metrics_blocks.len() > 3);
    assert_eq!(state.log_blocks[0].object_offset(), 0);
    assert_eq!(state.metrics_blocks[0].object_offset(), 0);
    assert_contiguous(&state.log_blocks, log_span);
    assert_contiguous(&state.metrics_blocks, metrics_span);
    for block in &state.log_blocks {
        assert_eq!(block.events.capacity_bytes(), BUFFER_SIZE);
    }
    for block in &state.metrics_blocks {
        assert_eq!(block.events.capacity_bytes(), BUFFER_SIZE);
    }
    let nb_log_events: usize = state.log_blocks.iter().map(|b| b.nb_objects()).sum();
    assert_eq!(nb_log_events, 200);
}

#[test]
#[serial]
fn concurrent_recording_loses_and_duplicates_nothing() {
    const NB_THREADS: usize = 8;
    const NB_EVENTS: usize = 500;
    let guard = init_in_memory_tracing();
    set_max_level(LevelFilter::Trace);
    std::thread::scope(|s| {
        for t in 0..NB_THREADS {
            s.spawn(move || {
                for i in 0..NB_EVENTS {
                    let id = format!("{t}-{i}");
                    info!(id = id, "concurrent event");
                }
            });
        }
    });
    flush_log_buffer();

    let state = guard.sink.state.lock().unwrap();
    let mut blocks: Vec<_> = state.log_blocks.to_vec();
    blocks.sort_by_key(|b| b.object_offset());
    let nb_events: usize = blocks.iter().map(|b| b.nb_objects()).sum();
    assert_eq!(nb_events, NB_THREADS * NB_EVENTS);
    assert_eq!(blocks[0].object_offset(), 0);
    assert_contiguous(&blocks, log_span);

    let mut ids = std::collections::HashSet::new();
    for block in &blocks {
        for evt in block.events.iter() {
            if let LogMsgQueueAny::TaggedLogString(e) = evt {
                // SAFETY: the set lives in the block's arena, which is alive here.
                let set = unsafe { &*e.properties };
                for p in set.get_properties() {
                    if p.name.as_str() == "id" {
                        assert!(ids.insert(p.value.as_str().to_owned()), "duplicated event");
                    }
                }
            }
        }
        assert_eq!(block.events.capacity_bytes(), BUFFER_SIZE);
    }
    assert_eq!(ids.len(), NB_THREADS * NB_EVENTS);

    // Only the final explicit flush may send a block that is not full.
    let (last, others) = blocks.split_last().unwrap();
    for block in others {
        assert!(
            block.len_bytes() > BUFFER_SIZE - HINT_MAX_OBJ_SIZE,
            "tiny block of {} bytes",
            block.len_bytes()
        );
    }
    assert!(last.nb_objects() > 0);
}

struct ReentrantSink {
    log_blocks: Mutex<Vec<Arc<LogBlock>>>,
    logged_from_sink: AtomicBool,
}

impl EventSink for ReentrantSink {
    fn on_startup(&self, _: Arc<ProcessInfo>) {}
    fn on_shutdown(&self) {}
    fn on_log_enabled(&self, _: &LogMetadata) -> bool {
        true
    }
    fn on_log(&self, _: &LogMetadata, _: &[DynProperty<'_>], _: i64, _: fmt::Arguments<'_>) {}
    fn on_init_log_stream(&self, _: &LogStream) {}
    fn on_process_log_block(&self, block: Arc<LogBlock>) {
        if !self.logged_from_sink.swap(true, Ordering::SeqCst) {
            let origin = String::from("sink");
            info!("nested event from the {}", origin);
        }
        self.log_blocks.lock().unwrap().push(block);
    }
    fn on_init_metrics_stream(&self, _: &MetricsStream) {}
    fn on_process_metrics_block(&self, _: Arc<MetricsBlock>) {}
    fn on_init_thread_stream(&self, _: &ThreadStream) {}
    fn on_process_thread_block(&self, _: Arc<ThreadBlock>) {}
    fn is_busy(&self) -> bool {
        false
    }
}

struct DispatchGuard;

impl Drop for DispatchGuard {
    fn drop(&mut self) {
        shutdown_dispatch();
        unsafe { force_uninit() };
    }
}

#[test]
#[serial]
fn sink_can_log_while_processing_a_block() {
    let sink = Arc::new(ReentrantSink {
        log_blocks: Mutex::new(vec![]),
        logged_from_sink: AtomicBool::new(false),
    });
    init_event_dispatch(
        BUFFER_SIZE,
        BUFFER_SIZE,
        BUFFER_SIZE,
        sink.clone(),
        HashMap::new(),
        false,
    )
    .unwrap();
    let _guard = DispatchGuard;
    set_max_level(LevelFilter::Trace);

    let (tx, rx) = mpsc::channel();
    let body_sink = sink.clone();
    // A regression would deadlock, so run the body where a timeout can fail the test instead.
    std::thread::spawn(move || {
        for i in 0..100 {
            info!("filler event {}", i);
        }
        flush_log_buffer();
        let nb_blocks = body_sink.log_blocks.lock().unwrap().len();
        let _ = tx.send(nb_blocks);
    });
    let nb_blocks_before = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("sink logging from on_process_log_block deadlocked");
    assert!(nb_blocks_before > 0);

    flush_log_buffer();
    let blocks = sink.log_blocks.lock().unwrap();
    let nested = blocks.iter().any(|block| {
        block.events.iter().any(|evt| match evt {
            LogMsgQueueAny::LogStringEvent(e) => e.msg.0 == "nested event from the sink",
            _ => false,
        })
    });
    assert!(nested, "nested event was not recorded");
}
