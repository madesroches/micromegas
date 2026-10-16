use micromegas_tracing::{
    dispatch::{
        flush_log_buffer, force_uninit, init_event_dispatch, log_interop, log_interop_dyn_tagged,
        shutdown_dispatch,
    },
    event::EventSink,
    levels::{Level, LevelFilter, set_max_level},
    logs::{FILTER_LEVEL_UNSET_VALUE, LogBlock, LogMetadata, LogMsgQueueAny, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    process_info::ProcessInfo,
    property_set::DynProperty,
    spans::{ThreadBlock, ThreadStream},
};
use micromegas_transit::HeterogeneousQueue;
use serial_test::serial;
use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, atomic::AtomicU32},
};

#[derive(Default)]
struct RecordingSink {
    log_blocks: Mutex<Vec<Arc<LogBlock>>>,
    // Names of the properties each `on_log` call received.
    on_log_names: Mutex<Vec<Vec<String>>>,
}

impl EventSink for RecordingSink {
    fn on_startup(&self, _: Arc<ProcessInfo>) {}
    fn on_shutdown(&self) {}
    fn on_log_enabled(&self, _: &LogMetadata) -> bool {
        true
    }
    fn on_log(
        &self,
        _: &LogMetadata,
        properties: &[DynProperty<'_>],
        _: i64,
        _: fmt::Arguments<'_>,
    ) {
        self.on_log_names
            .lock()
            .unwrap()
            .push(properties.iter().map(|p| p.name().to_owned()).collect());
    }
    fn on_init_log_stream(&self, _: &LogStream) {}
    fn on_process_log_block(&self, block: Arc<LogBlock>) {
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

struct DispatchGuard {
    sink: Arc<RecordingSink>,
}

impl DispatchGuard {
    fn new() -> Self {
        let sink = Arc::new(RecordingSink::default());
        init_event_dispatch(
            64 * 1024,
            64 * 1024,
            1024,
            sink.clone(),
            HashMap::new(),
            false,
        )
        .unwrap();
        set_max_level(LevelFilter::Trace);
        Self { sink }
    }

    fn blocks(&self) -> Vec<Arc<LogBlock>> {
        flush_log_buffer();
        self.sink.log_blocks.lock().unwrap().clone()
    }
}

impl Drop for DispatchGuard {
    fn drop(&mut self) {
        shutdown_dispatch();
        unsafe { force_uninit() };
    }
}

fn interop_desc(target: &str, level: Level) -> LogMetadata<'_> {
    LogMetadata {
        level,
        level_filter: AtomicU32::new(FILTER_LEVEL_UNSET_VALUE),
        fmt_str: "",
        target,
        module_path: "",
        file: "",
        line: 0,
    }
}

#[test]
#[serial]
fn runtime_target_is_a_string_record_shared_by_equal_targets() {
    let guard = DispatchGuard::new();
    let target = format!("runtime::{}", 42);
    let desc = interop_desc(&target, Level::Info);
    let runtime_msg = String::from("first");
    log_interop(&desc, format_args!("{runtime_msg}"));
    log_interop(&desc, format_args!("second {}", runtime_msg));

    let blocks = guard.blocks();
    let block = blocks.last().unwrap();
    let ids: Vec<u64> = block
        .events
        .iter()
        .filter_map(|evt| match evt {
            LogMsgQueueAny::LogStringInteropEvent(e) => Some(e.target.id),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], ids[1]);
    assert_eq!(block.dyn_deps.str(ids[0]), target);
    assert_eq!(block.dyn_deps.queue().nb_objects(), 1);
}

#[test]
#[serial]
fn the_next_block_writes_the_target_again() {
    let guard = DispatchGuard::new();
    let desc = interop_desc("again", Level::Info);
    log_interop(&desc, format_args!("one"));
    flush_log_buffer();
    log_interop(&desc, format_args!("two"));
    let blocks = guard.blocks();
    assert_eq!(blocks.len(), 2);
    for block in blocks {
        assert_eq!(block.dyn_deps.queue().nb_objects(), 1);
        let target_ids: Vec<u64> = block
            .events
            .iter()
            .filter_map(|evt| match evt {
                LogMsgQueueAny::LogStringInteropEvent(e) => Some(e.target.id),
                _ => None,
            })
            .collect();
        assert_eq!(target_ids.len(), 1);
        assert_eq!(block.dyn_deps.str(target_ids[0]), "again");
    }
}

#[test]
#[serial]
fn static_message_is_stored_as_a_string_interop_event() {
    let guard = DispatchGuard::new();
    let desc = interop_desc("static_msg", Level::Warn);
    log_interop(&desc, format_args!("a static message"));
    let blocks = guard.blocks();
    let block = blocks.last().unwrap();
    let mut seen = 0;
    for evt in block.events.iter() {
        match evt {
            LogMsgQueueAny::LogStringInteropEvent(e) => {
                seen += 1;
                assert_eq!(e.msg.0, "a static message");
                assert_eq!(e.level, Level::Warn as u8);
                assert_eq!(block.dyn_deps.str(e.target.id), "static_msg");
            }
            LogMsgQueueAny::LogStaticStrInteropEvent(_) => {
                panic!("static interop messages are no longer written as static events")
            }
            _ => {}
        }
    }
    assert_eq!(seen, 1);
}

#[test]
#[serial]
fn tagged_interop_log_with_a_block_scoped_key() {
    let guard = DispatchGuard::new();
    let desc = interop_desc("tagged_target", Level::Error);
    let key = format!("req{}", "uest_id");
    let properties = [
        DynProperty::with_dyn_name(&key, "abc"),
        DynProperty::with_dyn_name("target", "overridden"),
        DynProperty::with_dyn_name("level", "debug"),
    ];
    log_interop_dyn_tagged(&desc, &properties, format_args!("tagged message"));

    let on_log_names = guard.sink.on_log_names.lock().unwrap().clone();
    assert_eq!(on_log_names.len(), 1);
    assert_eq!(on_log_names[0], vec!["request_id".to_owned()]);

    let blocks = guard.blocks();
    let block = blocks.last().unwrap();
    let mut seen = 0;
    for evt in block.events.iter() {
        if let LogMsgQueueAny::TaggedLogInteropEvent(e) = evt {
            seen += 1;
            assert_eq!(block.dyn_deps.str(e.target.id), "tagged_target");
            assert_eq!(e.level, Level::Error as u8);
            let mut pairs = block.dyn_deps.set_properties(e.properties);
            pairs.sort();
            assert_eq!(
                pairs,
                vec![
                    ("level", "debug"),
                    ("request_id", "abc"),
                    ("target", "overridden")
                ]
            );
        }
    }
    assert_eq!(seen, 1);
}
