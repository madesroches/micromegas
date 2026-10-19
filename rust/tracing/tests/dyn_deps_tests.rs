use micromegas_tracing::{
    dispatch::{
        float_metric, flush_log_buffer, flush_metrics_buffer, force_uninit, init_event_dispatch,
        log, log_dyn_tagged, log_tagged, shutdown_dispatch, tagged_float_metric,
        tagged_float_metric_dyn,
    },
    dyn_deps::{DYN_ID_TAG, DynDeps, MergedProperties, content_hash, merge_properties},
    event::{EventSink, ExtractDeps, TracingBlock},
    levels::Level,
    logs::{
        FILTER_LEVEL_UNSET_VALUE, LogBlock, LogDepsQueue, LogMetadata, LogMsgForm, LogMsgQueue,
        LogMsgQueueAny, LogStream, TaggedLogInteropEvent, TaggedLogString,
    },
    metrics::{
        MetricsBlock, MetricsMsgQueueAny, MetricsStream, StaticMetricMetadata,
        TaggedFloatMetricEvent, TaggedIntegerMetricEvent,
    },
    prelude::*,
    process_info::ProcessInfo,
    property_set::{
        DynProperty, Property, PropertySet, PropertySetId, cmp_dyn_properties, dyn_property_get,
    },
    spans::{ThreadBlock, ThreadStream},
    static_string_ref::BlockStringRef,
};
use micromegas_transit::{DynString, HeterogeneousQueue, Reflect, string_codec::StringCodec};
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
    target: "dyn_deps_tests",
    module_path: "dyn_deps_tests",
    file: file!(),
    line: line!(),
};

static LOG_DESC_2: LogMetadata = LogMetadata {
    level: Level::Info,
    level_filter: AtomicU32::new(FILTER_LEVEL_UNSET_VALUE),
    fmt_str: "",
    target: "dyn_deps_tests",
    module_path: "dyn_deps_tests",
    file: file!(),
    line: line!(),
};

static METRIC_DESC: StaticMetricMetadata = StaticMetricMetadata {
    lod: Verbosity::Max,
    name: "dyn_deps_metric",
    unit: "count",
    target: "dyn_deps_tests",
    file: file!(),
    line: line!(),
};

type LogDeps = DynDeps<LogDepsQueue>;

fn owned_props<'a>(props: &[(&'static str, &'a str)]) -> MergedProperties<'a> {
    let dyn_props: Vec<DynProperty<'_>> = props
        .iter()
        .map(|(name, value)| DynProperty::new(name, value))
        .collect();
    merge_properties(PropertySet::empty(), &dyn_props)
}

fn pairs_of(deps: &LogDeps, id: PropertySetId) -> Vec<(String, String)> {
    deps.set_properties(id)
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

#[test]
fn same_content_same_id() {
    let mut deps = LogDeps::new();
    let props = owned_props(&[("correlation_id", "abc")]);
    let first = deps.intern_set(&props);
    let len_after_first = deps.len_bytes();
    let second = deps.intern_set(&props);
    assert_eq!(first, second);
    assert_eq!(deps.len_bytes(), len_after_first);
}

#[test]
fn split_between_static_and_dynamic_does_not_change_identity() {
    let static_set = PropertySet::find_or_create(vec![Property::new("kind", "t")]);
    let split = merge_properties(static_set, &[DynProperty::new("correlation_id", "abc")]);
    let all_dynamic = owned_props(&[("kind", "t"), ("correlation_id", "abc")]);
    let mut deps = LogDeps::new();
    let first = deps.intern_set(&split);
    let second = deps.intern_set(&all_dynamic);
    assert_eq!(first, second);
    let split_again = deps.intern_set(&split);
    assert_eq!(first, split_again);
}

#[test]
fn different_values_different_sets() {
    let mut deps = LogDeps::new();
    let a = deps.intern_set(&owned_props(&[("correlation_id", "abc")]));
    let b = deps.intern_set(&owned_props(&[("correlation_id", "abd")]));
    assert_ne!(a, b);
    assert_eq!(pairs_of(&deps, a)[0].1, "abc");
    assert_eq!(pairs_of(&deps, b)[0].1, "abd");
}

#[test]
fn values_are_copied() {
    let mut deps = LogDeps::new();
    let id = {
        let value = String::from("short lived value");
        deps.intern_set(&owned_props(&[("correlation_id", &value)]))
    };
    assert_eq!(
        pairs_of(&deps, id),
        vec![("correlation_id".to_owned(), "short lived value".to_owned())]
    );
}

#[test]
fn dynamic_value_shared_by_two_sets_is_one_record() {
    let mut deps = LogDeps::new();
    deps.intern_set(&owned_props(&[("a", "shared")]));
    // one string record and one set record
    assert_eq!(deps.queue().nb_objects(), 2);
    deps.intern_set(&owned_props(&[("b", "shared")]));
    // only a set record was added
    assert_eq!(deps.queue().nb_objects(), 3);
}

#[test]
fn static_values_get_no_string_record() {
    let static_set = PropertySet::find_or_create(vec![Property::new("kind", "t")]);
    let mut deps = LogDeps::new();
    deps.intern_set(&merge_properties(
        static_set,
        &[DynProperty::new("correlation_id", "abc")],
    ));
    // the dynamic value's string record and the set record
    assert_eq!(deps.queue().nb_objects(), 2);
}

#[test]
fn len_bytes_is_the_queue_length() {
    let mut deps = LogDeps::new();
    assert_eq!(deps.len_bytes(), 0);
    deps.intern_set(&owned_props(&[("a", "12345"), ("b", "123")]));
    assert!(deps.len_bytes() > 0);
    assert_eq!(deps.len_bytes(), deps.queue().len_bytes());
    assert_eq!(deps.len_bytes(), deps.queue().as_bytes().len());
}

#[test]
fn ids_are_tagged_and_told_apart() {
    let mut deps = LogDeps::new();
    let string_id = deps.intern_str("some value");
    let set_id = deps.intern_set(&owned_props(&[("a", "some value")]));
    assert!(set_id.as_interned().is_none());

    let queued = records(deps.queue().as_bytes());
    assert_eq!(queued.len(), 2);
    assert_eq!(queued[0].type_index, STRING_RECORD);
    assert_eq!(record_id(&queued[0]), string_id);
    assert_eq!(queued[1].type_index, SET_RECORD);
    for record in &queued {
        assert_ne!(record_id(record) & DYN_ID_TAG, 0);
    }
    assert_eq!(string_id & 1, 0);
    assert_eq!(record_id(&queued[1]) & 1, 1);

    let interned = PropertySetId::interned(PropertySet::empty());
    assert!(std::ptr::eq(
        interned.as_interned().unwrap(),
        PropertySet::empty()
    ));
}

#[test]
fn set_properties_decodes_interned_and_dynamic_sets() {
    let static_set = PropertySet::find_or_create(vec![Property::new("kind", "t")]);
    let mut deps = LogDeps::new();
    let dynamic = deps.intern_set(&merge_properties(
        static_set,
        &[DynProperty::new("correlation_id", "abc")],
    ));
    assert_eq!(
        deps.set_properties(dynamic),
        vec![("kind", "t"), ("correlation_id", "abc")]
    );
    assert_eq!(
        deps.set_properties(PropertySetId::interned(static_set)),
        vec![("kind", "t")]
    );
}

struct Record {
    type_index: u8,
    payload: Vec<u8>,
}

// Walks a queue made only of dynamically sized records.
fn records(bytes: &[u8]) -> Vec<Record> {
    let mut out = vec![];
    let mut at = 0;
    while at < bytes.len() {
        let size = u32::from_ne_bytes(bytes[at + 1..at + 5].try_into().unwrap()) as usize;
        out.push(Record {
            type_index: bytes[at],
            payload: bytes[at + 5..at + 5 + size].to_vec(),
        });
        at += 5 + size;
    }
    out
}

fn record_id(record: &Record) -> u64 {
    u64::from_ne_bytes(record.payload[..8].try_into().unwrap())
}

const STRING_RECORD: u8 = 1;
const SET_RECORD: u8 = 3;

fn extract_interop(deps: &LogDeps, ids: &[PropertySetId]) -> Vec<Record> {
    let mut events = LogMsgQueue::new(4096);
    for id in ids {
        events.push(TaggedLogInteropEvent {
            time: 0,
            level: 0,
            // `extract` doesn't read the target: it is a record of the dynamic queue.
            target: BlockStringRef {
                id: 0,
                len: 0,
                codec: StringCodec::Utf8,
            },
            properties: *id,
            msg: DynString(String::new()),
        });
    }
    records(events.extract(deps).as_bytes())
}

#[test]
fn extract_emits_static_refs_once_before_the_dynamic_queue() {
    let static_set = PropertySet::find_or_create(vec![Property::new("kind", "t")]);
    let kind = &static_set.get_properties()[0];
    let mut deps = LogDeps::new();
    let first = deps.intern_set(&merge_properties(
        static_set,
        &[DynProperty::new("correlation_id", "abc")],
    ));
    let second = deps.intern_set(&merge_properties(
        static_set,
        &[DynProperty::new("correlation_id", "abd")],
    ));
    let records = extract_interop(&deps, &[first, second]);
    let count = |id: u64| {
        records
            .iter()
            .filter(|r| r.type_index == STRING_RECORD && record_id(r) == id)
            .count()
    };
    assert_eq!(count(kind.name.id()), 1);
    // the original pointer, not a copy of the value
    assert_eq!(count(kind.value.id()), 1);
    let first_dynamic = records
        .iter()
        .position(|r| record_id(r) & DYN_ID_TAG != 0)
        .unwrap();
    let last_static = records
        .iter()
        .rposition(|r| record_id(r) & DYN_ID_TAG == 0)
        .unwrap();
    assert!(last_static < first_dynamic);
    // a record only refers to ids that precede it
    let mut seen = std::collections::HashSet::new();
    for record in &records {
        if record.type_index == SET_RECORD {
            let nb = u32::from_ne_bytes(record.payload[8..12].try_into().unwrap()) as usize;
            let size = std::mem::size_of::<Property>();
            for i in 0..nb {
                let prop = &record.payload[12 + i * size..12 + (i + 1) * size];
                let name = u64::from_ne_bytes(prop[..8].try_into().unwrap());
                let value = u64::from_ne_bytes(prop[size / 2..size / 2 + 8].try_into().unwrap());
                assert!(seen.contains(&name));
                assert!(seen.contains(&value));
            }
        }
        seen.insert(record_id(record));
    }
}

#[test]
fn metric_events_keep_the_properties_member_layout() {
    fn check<T: Reflect>(offset: usize) {
        let udt = T::reflect();
        let member = udt
            .members
            .iter()
            .find(|m| &*m.name == "properties")
            .unwrap();
        assert_eq!(member.offset, offset);
        assert_eq!(member.size, std::mem::size_of::<u64>());
        assert!(member.is_reference);
    }
    check::<TaggedFloatMetricEvent>(std::mem::offset_of!(TaggedFloatMetricEvent, properties));
    check::<TaggedIntegerMetricEvent>(std::mem::offset_of!(TaggedIntegerMetricEvent, properties));
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
            DynProperty::new("region", "us"),
            DynProperty::new("correlation_id", "abc"),
        ],
    );
    let merged = merged.properties();
    assert_eq!(merged.len(), 3);
    assert_eq!(dyn_property_get(merged, "region"), Some("us"));
    assert_eq!(dyn_property_get(merged, "kind"), Some("t"));
    assert_eq!(dyn_property_get(merged, "correlation_id"), Some("abc"));
}

#[test]
fn log_budget_counts_dyn_deps_bytes() {
    let mut stream = LogStream::new(4096, uuid::Uuid::new_v4(), &[], HashMap::new());
    let big_value = "x".repeat(300);
    let mut nb_events = 0;
    while !stream.is_full() {
        let unique = format!("{nb_events}{big_value}");
        let (events, dyn_deps) = stream.get_block_mut_parts();
        let properties = dyn_deps.intern_set(&owned_props(&[("correlation_id", &unique)]));
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
fn metrics_budget_counts_dyn_deps_bytes() {
    let mut stream = MetricsStream::new(4096, uuid::Uuid::new_v4(), &[], HashMap::new());
    let big_value = "x".repeat(300);
    let mut nb_events = 0;
    while !stream.is_full() {
        let unique = format!("{nb_events}{big_value}");
        let (events, dyn_deps) = stream.get_block_mut_parts();
        let properties = dyn_deps.intern_set(&owned_props(&[("correlation_id", &unique)]));
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
                .map(|p| (p.name().to_owned(), p.value().to_owned()))
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
                    LogMsgQueueAny::TaggedLogString(e) => {
                        Some(pairs_of(&block.dyn_deps, e.properties))
                    }
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
    let dyn_props = [DynProperty::new("correlation_id", &correlation_id)];

    log_dyn_tagged(
        &LOG_DESC,
        static_set,
        &dyn_props,
        LogMsgForm::Formatted,
        format_args!("first"),
    );
    flush_log_buffer();
    log_dyn_tagged(
        &LOG_DESC,
        static_set,
        &dyn_props,
        LogMsgForm::Formatted,
        format_args!("second"),
    );
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
            .all(|b| b.dyn_deps.len_bytes() > 0)
    );
}

#[test]
#[serial]
fn static_path_does_not_touch_dyn_deps() {
    let guard = DispatchGuard::new();
    let static_set = PropertySet::find_or_create(vec![Property::new("kind", "static")]);
    log_tagged(
        &LOG_DESC,
        static_set,
        LogMsgForm::Formatted,
        format_args!("static"),
    );
    tagged_float_metric(&METRIC_DESC, static_set, 1.0);
    flush_log_buffer();
    flush_metrics_buffer();

    let state = guard.sink.state.lock().unwrap();
    let mut nb_tagged_logs = 0;
    for block in &state.log_blocks {
        assert_eq!(block.dyn_deps.len_bytes(), 0);
        for evt in block.events.iter() {
            if let LogMsgQueueAny::TaggedLogString(e) = evt {
                assert_eq!(e.properties, PropertySetId::interned(static_set));
                nb_tagged_logs += 1;
            }
        }
    }
    assert_eq!(nb_tagged_logs, 1);
    let mut nb_tagged_metrics = 0;
    for block in &state.metrics_blocks {
        assert_eq!(block.dyn_deps.len_bytes(), 0);
        for evt in block.events.iter() {
            if let MetricsMsgQueueAny::TaggedFloatMetricEvent(e) = evt {
                assert_eq!(e.properties, PropertySetId::interned(static_set));
                nb_tagged_metrics += 1;
            }
        }
    }
    assert_eq!(nb_tagged_metrics, 1);
}

#[test]
#[serial]
fn dynamic_metric_lands_in_dyn_deps() {
    let guard = DispatchGuard::new();
    let value = String::from("req-1");
    tagged_float_metric_dyn(
        &METRIC_DESC,
        PropertySet::empty(),
        &[DynProperty::new("request_id", &value)],
        2.0,
    );
    flush_metrics_buffer();
    let state = guard.sink.state.lock().unwrap();
    let mut found = false;
    for block in &state.metrics_blocks {
        for evt in block.events.iter() {
            if let MetricsMsgQueueAny::TaggedFloatMetricEvent(e) = evt {
                assert_eq!(
                    block.dyn_deps.set_properties(e.properties),
                    vec![("request_id", "req-1")]
                );
                assert!(block.dyn_deps.len_bytes() > 0);
                found = true;
            }
        }
    }
    assert!(found);
}

fn log_deps(guard: &DispatchGuard, record: impl FnOnce()) -> usize {
    guard.sink.state.lock().unwrap().log_blocks.clear();
    record();
    flush_log_buffer();
    let state = guard.sink.state.lock().unwrap();
    let blocks: Vec<_> = state
        .log_blocks
        .iter()
        .filter(|b| b.events.nb_objects() > 0)
        .collect();
    assert_eq!(blocks.len(), 1, "events must fit in a single block");
    blocks[0].events.extract(&blocks[0].dyn_deps).nb_objects()
}

fn metric_deps(guard: &DispatchGuard, record: impl FnOnce()) -> usize {
    guard.sink.state.lock().unwrap().metrics_blocks.clear();
    record();
    flush_metrics_buffer();
    let state = guard.sink.state.lock().unwrap();
    let blocks: Vec<_> = state
        .metrics_blocks
        .iter()
        .filter(|b| b.events.nb_objects() > 0)
        .collect();
    assert_eq!(blocks.len(), 1, "events must fit in a single block");
    blocks[0].events.extract(&blocks[0].dyn_deps).nb_objects()
}

fn dyn_log(desc: &'static LogMetadata, set: &'static PropertySet, value: &str) {
    log_dyn_tagged(
        desc,
        set,
        &[DynProperty::new("k", value)],
        LogMsgForm::Formatted,
        format_args!("m"),
    );
}

#[test]
#[serial]
fn tagged_events_emit_dependencies() {
    let guard = DispatchGuard::new();
    let set = PropertySet::find_or_create(vec![Property::new("x", "1")]);
    let untagged = log_deps(&guard, || {
        log(&LOG_DESC, LogMsgForm::Formatted, format_args!("m"))
    });
    let tagged = log_deps(&guard, || {
        log_tagged(&LOG_DESC, set, LogMsgForm::Formatted, format_args!("m"))
    });
    assert!(tagged > untagged);

    let untagged = metric_deps(&guard, || float_metric(&METRIC_DESC, 1.0));
    let tagged = metric_deps(&guard, || tagged_float_metric(&METRIC_DESC, set, 1.0));
    assert!(tagged > untagged);
}

#[test]
#[serial]
fn repeated_events_emit_dependencies_once() {
    let guard = DispatchGuard::new();
    let set = PropertySet::find_or_create(vec![Property::new("x", "1")]);
    let one = log_deps(&guard, || dyn_log(&LOG_DESC, PropertySet::empty(), "v"));
    let many = log_deps(&guard, || {
        for _ in 0..200 {
            dyn_log(&LOG_DESC, PropertySet::empty(), "v");
        }
    });
    assert_eq!(one, many);

    let one = log_deps(&guard, || {
        log_tagged(&LOG_DESC, set, LogMsgForm::Formatted, format_args!("m"))
    });
    let many = log_deps(&guard, || {
        for _ in 0..200 {
            log_tagged(&LOG_DESC, set, LogMsgForm::Formatted, format_args!("m"));
        }
    });
    assert_eq!(one, many);

    let one = metric_deps(&guard, || tagged_float_metric(&METRIC_DESC, set, 1.0));
    let many = metric_deps(&guard, || {
        for _ in 0..200 {
            tagged_float_metric(&METRIC_DESC, set, 1.0);
        }
    });
    assert_eq!(one, many);

    let dyn_metric = || {
        tagged_float_metric_dyn(
            &METRIC_DESC,
            PropertySet::empty(),
            &[DynProperty::new("k", "v")],
            1.0,
        )
    };
    let one = metric_deps(&guard, dyn_metric);
    let many = metric_deps(&guard, || {
        for _ in 0..200 {
            dyn_metric();
        }
    });
    assert_eq!(one, many);
}

#[test]
#[serial]
fn interleaved_sets_still_dedup() {
    let guard = DispatchGuard::new();
    let a = log_deps(&guard, || dyn_log(&LOG_DESC, PropertySet::empty(), "A"));
    let interleaved = log_deps(&guard, || {
        for v in ["A", "B", "A", "B", "A"] {
            dyn_log(&LOG_DESC, PropertySet::empty(), v);
        }
    });
    assert_eq!(interleaved, a + 2);
}

#[test]
#[serial]
fn interleaved_callsites_still_dedup() {
    let guard = DispatchGuard::new();
    let set = PropertySet::find_or_create(vec![Property::new("x", "1")]);
    let one = log_deps(&guard, || {
        log_tagged(&LOG_DESC, set, LogMsgForm::Formatted, format_args!("m"))
    });
    let each = log_deps(&guard, || {
        log_tagged(&LOG_DESC, set, LogMsgForm::Formatted, format_args!("m"));
        log_tagged(&LOG_DESC_2, set, LogMsgForm::Formatted, format_args!("m"));
    });
    let alternating = log_deps(&guard, || {
        for _ in 0..50 {
            log_tagged(&LOG_DESC, set, LogMsgForm::Formatted, format_args!("m"));
            log_tagged(&LOG_DESC_2, set, LogMsgForm::Formatted, format_args!("m"));
        }
    });
    assert!(each > one);
    assert_eq!(alternating, each);
}

#[test]
fn dyn_properties_mirror_the_set() {
    let set = PropertySet::find_or_create(vec![
        Property::new("region", "eu"),
        Property::new("kind", "x"),
        Property::new("zone", "z1"),
    ]);
    let props = set.get_properties();
    let view = set.dyn_properties();
    assert_eq!(props.len(), view.len());
    for (p, d) in props.iter().zip(view) {
        assert_eq!(p.name.as_str(), d.name());
        assert_eq!(p.value.as_str(), d.value());
        assert_eq!(p.value.id(), d.value().as_ptr() as u64);
    }
    let again = PropertySet::find_or_create(vec![
        Property::new("zone", "z1"),
        Property::new("region", "eu"),
        Property::new("kind", "x"),
    ]);
    assert!(std::ptr::eq(set, again));
}

fn dyn_named<'a>(props: &[(&'a str, &'a str)]) -> MergedProperties<'a> {
    let dyn_props: Vec<DynProperty<'_>> = props
        .iter()
        .map(|(name, value)| DynProperty::with_dyn_name(name, value))
        .collect();
    merge_properties(PropertySet::empty(), &dyn_props)
}

#[test]
fn dynamic_name_is_a_string_record_and_set_is_deduplicated() {
    let mut deps = LogDeps::new();
    let key = String::from("runtime_key");
    let props = dyn_named(&[(key.as_str(), "v")]);
    let first = deps.intern_set(&props);
    // two string records and one set record
    assert_eq!(deps.queue().nb_objects(), 3);
    let len_after_first = deps.len_bytes();
    let second = deps.intern_set(&props);
    assert_eq!(first, second);
    assert_eq!(deps.len_bytes(), len_after_first);
}

#[test]
fn dynamic_names_share_string_records_with_values_and_other_names() {
    let mut deps = LogDeps::new();
    deps.intern_set(&dyn_named(&[("a", "shared")]));
    // "a", "shared" and the set
    assert_eq!(deps.queue().nb_objects(), 3);
    // a name equal to an earlier value
    deps.intern_set(&dyn_named(&[("shared", "x")]));
    // "x" and the set
    assert_eq!(deps.queue().nb_objects(), 5);
    // a name equal to an earlier name
    deps.intern_set(&dyn_named(&[("a", "y")]));
    // "y" and the set
    assert_eq!(deps.queue().nb_objects(), 7);
}

#[test]
fn static_and_dynamic_names_of_equal_text_have_equal_content() {
    let mut deps = LogDeps::new();
    let key = String::from("correlation_id");
    let dynamic = deps.intern_set(&dyn_named(&[(key.as_str(), "abc")]));
    let statically = deps.intern_set(&owned_props(&[("correlation_id", "abc")]));
    assert_eq!(dynamic, statically);
    assert_eq!(pairs_of(&deps, dynamic), pairs_of(&deps, statically));
    assert_eq!(
        pairs_of(&deps, dynamic),
        vec![("correlation_id".to_owned(), "abc".to_owned())]
    );
}

#[test]
fn set_properties_decodes_dynamic_names_and_extract_emits_only_static_ones() {
    let static_set = PropertySet::find_or_create(vec![Property::new("kind", "t")]);
    let kind = &static_set.get_properties()[0];
    let key = String::from("runtime_key");
    let mut deps = LogDeps::new();
    let id = deps.intern_set(&merge_properties(
        static_set,
        &[DynProperty::with_dyn_name(&key, "abc")],
    ));
    let mut decoded = pairs_of(&deps, id);
    decoded.sort();
    assert_eq!(
        decoded,
        vec![
            ("kind".to_owned(), "t".to_owned()),
            ("runtime_key".to_owned(), "abc".to_owned())
        ]
    );
    let records = extract_interop(&deps, &[id]);
    let static_strings: Vec<u64> = records
        .iter()
        .filter(|r| r.type_index == STRING_RECORD && record_id(r) & DYN_ID_TAG == 0)
        .map(record_id)
        .collect();
    // the static name and value of `kind`, and nothing for the runtime key
    assert_eq!(static_strings.len(), 2);
    assert!(static_strings.contains(&kind.name.id()));
    assert!(static_strings.contains(&kind.value.id()));
}

/// The algorithm `merge_properties` replaced, kept as the specification of its output.
fn reference_merge(
    static_set: &'static PropertySet,
    dyn_properties: &[DynProperty<'static>],
) -> (Vec<(String, String, bool, bool)>, u64) {
    let mut merged: Vec<DynProperty<'static>> = static_set.dyn_properties().to_vec();
    for dyn_prop in dyn_properties {
        merged.retain(|p| !p.name().eq_ignore_ascii_case(dyn_prop.name()));
        merged.push(*dyn_prop);
    }
    merged.sort_by(cmp_dyn_properties);
    let entries = merged
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let is_static_value = i < 64
                && static_set.get_properties().iter().any(|s| {
                    s.value.id() == p.value().as_ptr() as u64
                        && s.value.len as usize == p.value().len()
                        && s.name.as_str() == p.name()
                });
            let is_static_name = i < 64 && p.is_static_name();
            (
                p.name().to_owned(),
                p.value().to_owned(),
                is_static_value,
                is_static_name,
            )
        })
        .collect();
    let hash = content_hash(merged.iter().map(|p| (p.name(), p.value())));
    (entries, hash)
}

fn assert_merge_matches_reference(
    label: &str,
    static_set: &'static PropertySet,
    dyn_properties: &[DynProperty<'static>],
) {
    let (expected, expected_hash) = reference_merge(static_set, dyn_properties);
    let merged = merge_properties(static_set, dyn_properties);
    let actual: Vec<_> = merged
        .properties()
        .iter()
        .enumerate()
        .map(|(i, p)| {
            (
                p.name().to_owned(),
                p.value().to_owned(),
                merged.is_static_value(i),
                merged.is_static_name(i),
            )
        })
        .collect();
    assert_eq!(actual, expected, "{label}");
    let actual_hash = content_hash(merged.properties().iter().map(|p| (p.name(), p.value())));
    assert_eq!(actual_hash, expected_hash, "{label}");
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

#[test]
fn merge_matches_the_reference_algorithm() {
    let abc = PropertySet::find_or_create(vec![
        Property::new("b", "static_b"),
        Property::new("d", "static_d"),
        Property::new("f", "static_f"),
    ]);
    let case_variants = PropertySet::find_or_create(vec![
        Property::new("Key", "upper"),
        Property::new("key", "lower"),
        Property::new("other", "x"),
    ]);
    let static_names = ["N0", "N1", "N2", "N3", "N4", "N5", "N6", "N7", "N8", "N9"];
    let many_static = PropertySet::find_or_create(
        (0..70)
            .map(|i| Property::new(leak(format!("s{i:03}")), leak(format!("sv{i}"))))
            .collect(),
    );
    let many_dyn: Vec<DynProperty<'static>> = (0..70)
        .map(|i| DynProperty::new(leak(format!("d{i:03}")), leak(format!("dv{i}"))))
        .collect();
    let owned_name = leak(String::from("owned_name"));

    assert_merge_matches_reference("both empty", PropertySet::empty(), &[]);
    assert_merge_matches_reference(
        "empty static",
        PropertySet::empty(),
        &[DynProperty::new("a", "1")],
    );
    assert_merge_matches_reference("empty dynamic", abc, &[]);
    assert_merge_matches_reference(
        "sorts before, between and after",
        abc,
        &[
            DynProperty::new("a", "1"),
            DynProperty::new("c", "2"),
            DynProperty::new("g", "3"),
        ],
    );
    assert_merge_matches_reference(
        "case-insensitive override",
        abc,
        &[DynProperty::new("D", "override")],
    );
    assert_merge_matches_reference(
        "case-insensitive duplicates, last wins",
        abc,
        &[
            DynProperty::new("x", "1"),
            DynProperty::new("X", "2"),
            DynProperty::new("y", "3"),
            DynProperty::new("x", "4"),
        ],
    );
    assert_merge_matches_reference("case-variant statics kept", case_variants, &[]);
    assert_merge_matches_reference(
        "case-variant statics with other dynamic",
        case_variants,
        &[DynProperty::new("zzz", "1"), DynProperty::new("a", "2")],
    );
    assert_merge_matches_reference(
        "case-variant statics overridden",
        case_variants,
        &[DynProperty::new("KEY", "new")],
    );
    assert_merge_matches_reference(
        "dyn names",
        abc,
        &[
            DynProperty::with_dyn_name(owned_name, "v"),
            DynProperty::with_dyn_name("B", "over"),
        ],
    );
    let nine: Vec<DynProperty<'static>> = static_names
        .iter()
        .map(|n| DynProperty::new(n, "v"))
        .collect();
    assert_merge_matches_reference("spills past 8 entries", abc, &nine);
    assert_merge_matches_reference("past 64 entries", many_static, &many_dyn);
    assert_merge_matches_reference(
        "past 64 with overrides",
        many_static,
        &[DynProperty::new("S005", "o"), DynProperty::new("d000", "o")],
    );
}
