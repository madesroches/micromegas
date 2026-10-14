use micromegas_tracing::{
    dispatch::{
        flush_log_buffer, flush_metrics_buffer, force_uninit, init_event_dispatch, log_dyn_tagged,
        log_tagged, shutdown_dispatch, tagged_float_metric, tagged_float_metric_dyn,
    },
    event::{EventSink, TracingBlock},
    levels::Level,
    logs::{
        FILTER_LEVEL_UNSET_VALUE, LogBlock, LogMetadata, LogMsgQueueAny, LogStream, TaggedLogString,
    },
    metrics::{
        MetricsBlock, MetricsMsgQueueAny, MetricsStream, StaticMetricMetadata,
        TaggedFloatMetricEvent,
    },
    prelude::*,
    process_info::ProcessInfo,
    property_arena::PropertyArena,
    property_arena::merge_properties,
    property_set::{Property, PropertySet, dyn_property_get},
    spans::{ThreadBlock, ThreadStream},
};
use micromegas_transit::{DynString, HeterogeneousQueue};
use serial_test::serial;
use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, atomic::AtomicU32},
};

static LOG_DESC: LogMetadata = LogMetadata {
    level: Level::Info,
    level_filter: AtomicU32::new(FILTER_LEVEL_UNSET_VALUE),
    fmt_str: "",
    target: "arena_tests",
    module_path: "arena_tests",
    file: file!(),
    line: line!(),
};

static METRIC_DESC: StaticMetricMetadata = StaticMetricMetadata {
    lod: Verbosity::Max,
    name: "arena_metric",
    unit: "count",
    target: "arena_tests",
    file: file!(),
    line: line!(),
};

fn owned_props<'a>(props: &[(&'static str, &'a str)]) -> Vec<DynProperty<'a>> {
    let dyn_props: Vec<DynProperty<'_>> = props
        .iter()
        .map(|(name, value)| DynProperty { name, value })
        .collect();
    merge_properties(PropertySet::empty(), &dyn_props)
}

fn set_as_pairs(set: *const PropertySet) -> Vec<(String, String)> {
    // SAFETY: callers pass sets from the arena, which outlives this call.
    let set = unsafe { &*set };
    set.get_properties()
        .iter()
        .map(|p| (p.name.as_str().to_owned(), p.value.as_str().to_owned()))
        .collect()
}

#[test]
fn same_content_same_pointer() {
    let mut arena = PropertyArena::default();
    let props = owned_props(&[("correlation_id", "abc")]);
    let first = arena.get_or_insert(&props);
    let len_after_first = arena.len_bytes();
    let second = arena.get_or_insert(&props);
    assert_eq!(first, second);
    assert_eq!(arena.len_bytes(), len_after_first);
}

#[test]
fn different_values_different_sets() {
    let mut arena = PropertyArena::default();
    let a = arena.get_or_insert(&owned_props(&[("correlation_id", "abc")]));
    let b = arena.get_or_insert(&owned_props(&[("correlation_id", "abd")]));
    assert_ne!(a, b);
    assert_eq!(set_as_pairs(a)[0].1, "abc");
    assert_eq!(set_as_pairs(b)[0].1, "abd");
}

#[test]
fn values_are_copied() {
    let mut arena = PropertyArena::default();
    let ptr = {
        let value = String::from("short lived value");
        arena.get_or_insert(&owned_props(&[("correlation_id", &value)]))
    };
    assert_eq!(
        set_as_pairs(ptr),
        vec![("correlation_id".to_owned(), "short lived value".to_owned())]
    );
}

#[test]
fn len_bytes_accounts_for_set_and_values() {
    let mut arena = PropertyArena::default();
    assert_eq!(arena.len_bytes(), 0);
    arena.get_or_insert(&owned_props(&[("a", "12345"), ("b", "123")]));
    assert_eq!(
        arena.len_bytes(),
        std::mem::size_of::<PropertySet>() + 2 * std::mem::size_of::<Property>() + 5 + 3
    );
}

#[test]
fn merge_dynamic_overrides_static_case_insensitively() {
    let static_set = PropertySet::find_or_create(vec![
        Property::new("Region", "eu"),
        Property::new("kind", "t"),
    ]);
    let merged = merge_properties(
        static_set,
        &[
            DynProperty {
                name: "region",
                value: "us",
            },
            DynProperty {
                name: "correlation_id",
                value: "abc",
            },
        ],
    );
    assert_eq!(merged.len(), 3);
    assert_eq!(dyn_property_get(&merged, "region"), Some("us"));
    assert_eq!(dyn_property_get(&merged, "kind"), Some("t"));
    assert_eq!(dyn_property_get(&merged, "correlation_id"), Some("abc"));
}

#[test]
fn log_budget_counts_arena_bytes() {
    let mut stream = LogStream::new(4096, uuid::Uuid::new_v4(), &[], HashMap::new());
    let big_value = "x".repeat(300);
    let mut nb_events = 0;
    while !stream.is_full() {
        let unique = format!("{nb_events}{big_value}");
        let (events, arena) = stream.get_block_mut_parts();
        let properties = arena.get_or_insert(&owned_props(&[("correlation_id", &unique)]));
        events.push(TaggedLogString {
            desc: &LOG_DESC,
            properties,
            time: 0,
            msg: DynString(String::new()),
        });
        nb_events += 1;
        assert!(nb_events < 1000, "stream never became full");
    }
    let block = stream.get_block_ref();
    assert!(block.events.len_bytes() < 4096 - block.hint_max_obj_size());
    assert!(block.len_bytes() > block.events.len_bytes());
}

#[test]
fn metrics_budget_counts_arena_bytes() {
    let mut stream = MetricsStream::new(4096, uuid::Uuid::new_v4(), &[], HashMap::new());
    let big_value = "x".repeat(300);
    let mut nb_events = 0;
    while !stream.is_full() {
        let unique = format!("{nb_events}{big_value}");
        let (events, arena) = stream.get_block_mut_parts();
        let properties = arena.get_or_insert(&owned_props(&[("correlation_id", &unique)]));
        events.push(TaggedFloatMetricEvent {
            desc: &METRIC_DESC,
            properties,
            value: 1.0,
            time: 0,
        });
        nb_events += 1;
        assert!(nb_events < 1000, "stream never became full");
    }
    let block = stream.get_block_ref();
    assert!(block.events.len_bytes() < 4096 - block.hint_max_obj_size());
}

#[derive(Default)]
struct CapturedState {
    on_log_properties: Vec<Vec<(String, String)>>,
    log_blocks: Vec<Arc<LogBlock>>,
    metrics_blocks: Vec<Arc<MetricsBlock>>,
}

#[derive(Default)]
struct CapturingSink {
    state: Mutex<CapturedState>,
}

impl EventSink for CapturingSink {
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
        self.state.lock().unwrap().on_log_properties.push(
            properties
                .iter()
                .map(|p| (p.name.to_owned(), p.value.to_owned()))
                .collect(),
        );
    }
    fn on_init_log_stream(&self, _: &LogStream) {}
    fn on_process_log_block(&self, block: Arc<LogBlock>) {
        self.state.lock().unwrap().log_blocks.push(block);
    }
    fn on_init_metrics_stream(&self, _: &MetricsStream) {}
    fn on_process_metrics_block(&self, block: Arc<MetricsBlock>) {
        self.state.lock().unwrap().metrics_blocks.push(block);
    }
    fn on_init_thread_stream(&self, _: &ThreadStream) {}
    fn on_process_thread_block(&self, _: Arc<ThreadBlock>) {}
    fn is_busy(&self) -> bool {
        false
    }
}

struct DispatchGuard {
    sink: Arc<CapturingSink>,
}

impl DispatchGuard {
    fn new() -> Self {
        let sink = Arc::new(CapturingSink::default());
        init_event_dispatch(
            64 * 1024,
            64 * 1024,
            1024,
            sink.clone(),
            HashMap::new(),
            false,
        )
        .unwrap();
        Self { sink }
    }
}

impl Drop for DispatchGuard {
    fn drop(&mut self) {
        shutdown_dispatch();
        unsafe { force_uninit() };
    }
}

fn tagged_log_sets(blocks: &[Arc<LogBlock>]) -> Vec<Vec<Vec<(String, String)>>> {
    blocks
        .iter()
        .map(|block| {
            block
                .events
                .iter()
                .filter_map(|evt| match evt {
                    LogMsgQueueAny::TaggedLogString(e) => Some(set_as_pairs(e.properties)),
                    _ => None,
                })
                .collect()
        })
        .collect()
}

#[test]
#[serial]
fn dispatch_delivers_merged_properties_and_isolates_blocks() {
    let guard = DispatchGuard::new();
    let static_set = PropertySet::find_or_create(vec![Property::new("kind", "test")]);
    let correlation_id = String::from("abc-123");
    let dyn_props = [DynProperty {
        name: "correlation_id",
        value: &correlation_id,
    }];

    log_dyn_tagged(&LOG_DESC, static_set, &dyn_props, format_args!("first"));
    flush_log_buffer();
    log_dyn_tagged(&LOG_DESC, static_set, &dyn_props, format_args!("second"));
    flush_log_buffer();

    let state = guard.sink.state.lock().unwrap();
    assert_eq!(state.on_log_properties.len(), 2);
    for props in &state.on_log_properties {
        assert!(props.contains(&("correlation_id".to_owned(), "abc-123".to_owned())));
        assert!(props.contains(&("kind".to_owned(), "test".to_owned())));
    }

    let per_block: Vec<_> = tagged_log_sets(&state.log_blocks)
        .into_iter()
        .filter(|sets| !sets.is_empty())
        .collect();
    assert_eq!(per_block.len(), 2);
    assert_eq!(per_block[0], per_block[1]);
    assert_eq!(per_block[0].len(), 1);
    assert!(
        state
            .log_blocks
            .iter()
            .filter(|b| b.events.nb_objects() > 0)
            .all(|b| b.properties.len_bytes() > 0)
    );
}

#[test]
#[serial]
fn static_path_does_not_touch_arena() {
    let guard = DispatchGuard::new();
    let static_set = PropertySet::find_or_create(vec![Property::new("kind", "static")]);
    log_tagged(&LOG_DESC, static_set, format_args!("static"));
    tagged_float_metric(&METRIC_DESC, static_set, 1.0);
    flush_log_buffer();
    flush_metrics_buffer();

    let state = guard.sink.state.lock().unwrap();
    let mut nb_tagged_logs = 0;
    for block in &state.log_blocks {
        assert_eq!(block.properties.len_bytes(), 0);
        for evt in block.events.iter() {
            if let LogMsgQueueAny::TaggedLogString(e) = evt {
                assert_eq!(e.properties, static_set as *const PropertySet);
                nb_tagged_logs += 1;
            }
        }
    }
    assert_eq!(nb_tagged_logs, 1);
    let mut nb_tagged_metrics = 0;
    for block in &state.metrics_blocks {
        assert_eq!(block.properties.len_bytes(), 0);
        for evt in block.events.iter() {
            if let MetricsMsgQueueAny::TaggedFloatMetricEvent(e) = evt {
                assert_eq!(e.properties, static_set as *const PropertySet);
                nb_tagged_metrics += 1;
            }
        }
    }
    assert_eq!(nb_tagged_metrics, 1);
}

#[test]
#[serial]
fn dynamic_metric_lands_in_arena() {
    let guard = DispatchGuard::new();
    let value = String::from("req-1");
    tagged_float_metric_dyn(
        &METRIC_DESC,
        PropertySet::empty(),
        &[DynProperty {
            name: "request_id",
            value: &value,
        }],
        2.0,
    );
    flush_metrics_buffer();
    let state = guard.sink.state.lock().unwrap();
    let mut found = false;
    for block in &state.metrics_blocks {
        for evt in block.events.iter() {
            if let MetricsMsgQueueAny::TaggedFloatMetricEvent(e) = evt {
                assert_eq!(
                    set_as_pairs(e.properties),
                    vec![("request_id".to_owned(), "req-1".to_owned())]
                );
                assert!(block.properties.len_bytes() > 0);
                found = true;
            }
        }
    }
    assert!(found);
}
