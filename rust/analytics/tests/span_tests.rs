use std::{collections::HashMap, sync::Arc};

use micromegas_analytics::async_block_processing::{
    AsyncBlockProcessor, parse_async_block_payload,
};
use micromegas_analytics::metadata::StreamMetadata;
use micromegas_analytics::payload::parse_block;
use micromegas_analytics::properties::property_set::PropertySet as AnalyticsPropertySet;
use micromegas_analytics::scope::BorrowedScopeDesc;
use micromegas_analytics::thread_block_processor::{
    ThreadBlockProcessor, parse_thread_block_payload,
};
use micromegas_telemetry_sink::{stream_block::StreamBlock, stream_info::make_stream_info};
use micromegas_tracing::{
    dispatch::make_process_info,
    event::TracingBlock,
    prelude::*,
    spans::{
        BeginAsyncDynNamedSpanEvent, BeginAsyncNamedSpanEvent, BeginThreadDynNamedSpanEvent,
        BeginThreadNamedSpanEvent, BeginThreadSpanEvent, EndAsyncDynNamedSpanEvent,
        EndAsyncNamedSpanEvent, EndAsyncSpanEvent, EndThreadDynNamedSpanEvent,
        EndThreadNamedSpanEvent, EndThreadSpanEvent, SpanLocation, SpanMetadata,
        TaggedEndAsyncNamedSpanEvent, TaggedEndAsyncSpanEvent, TaggedEndThreadNamedSpanEvent,
        TaggedEndThreadSpanEvent, ThreadBlock, ThreadStream,
    },
    test_utils::init_in_memory_tracing,
};
use serial_test::serial;

#[test]
#[serial]
fn test_parse_span_interops() {
    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let mut stream = ThreadStream::new(1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    static SPAN_LOCATION_BEGIN: SpanLocation = SpanLocation {
        lod: Verbosity::Med,
        target: "target",
        module_path: "module_path",
        file: "file",
        line: 123,
    };
    stream.get_events_mut().push(BeginThreadNamedSpanEvent {
        thread_span_location: &SPAN_LOCATION_BEGIN,
        name: "my_function".into(),
        time: 1,
    });
    static SPAN_LOCATION_END: SpanLocation = SpanLocation {
        lod: Verbosity::Med,
        target: "target",
        module_path: "module_path",
        file: "file",
        line: 456,
    };
    stream.get_events_mut().push(BeginThreadNamedSpanEvent {
        thread_span_location: &SPAN_LOCATION_END,
        name: "my_function".into(),
        time: 2,
    });

    let mut block =
        stream.replace_block(Arc::new(ThreadBlock::new(1024, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info).unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();

    let stream_info = make_stream_info(&stream);
    let stream_metadata = StreamMetadata::from_stream_info(&stream_info).unwrap();
    let mut nb_span_entries = 0;
    parse_block(&stream_metadata, &received_block.payload, |_val| {
        nb_span_entries += 1;
        Ok(true)
    })
    .unwrap();
    assert_eq!(nb_span_entries, 2);

    let guard = init_in_memory_tracing();
    micromegas_tracing::dispatch::init_thread_stream();
    span_scope!("test_span");
    micromegas_tracing::dispatch::flush_thread_buffer();

    let state = guard.sink.state.lock().unwrap();
    assert!(state.process_info.is_some());
    assert!(!state.thread_blocks.is_empty());
}

#[derive(Default)]
struct NameCollector {
    thread: Vec<(&'static str, String)>,
    async_spans: Vec<(&'static str, String)>,
    thread_end_properties: Vec<Vec<(String, String)>>,
    async_end_properties: Vec<Vec<(String, String)>>,
}

fn collect_pairs(properties: &AnalyticsPropertySet<'_>) -> Vec<(String, String)> {
    let mut pairs = vec![];
    properties
        .for_each_property(|k, v| {
            pairs.push((k.to_owned(), v.to_owned()));
            Ok(())
        })
        .unwrap();
    pairs.sort();
    pairs
}

impl ThreadBlockProcessor for NameCollector {
    fn on_begin_thread_scope(
        &mut self,
        _block_id: &str,
        _event_id: i64,
        scope: BorrowedScopeDesc<'_>,
        _ts: i64,
    ) -> anyhow::Result<bool> {
        self.thread.push(("begin", scope.name.to_owned()));
        Ok(true)
    }

    fn on_end_thread_scope(
        &mut self,
        _block_id: &str,
        _event_id: i64,
        scope: BorrowedScopeDesc<'_>,
        _ts: i64,
        properties: AnalyticsPropertySet<'_>,
    ) -> anyhow::Result<bool> {
        self.thread_end_properties.push(collect_pairs(&properties));
        self.thread.push(("end", scope.name.to_owned()));
        Ok(true)
    }
}

impl AsyncBlockProcessor for NameCollector {
    fn on_begin_async_scope(
        &mut self,
        _block_id: &str,
        scope: BorrowedScopeDesc<'_>,
        _ts: i64,
        _span_id: i64,
        _parent_span_id: i64,
        _depth: u32,
    ) -> anyhow::Result<bool> {
        self.async_spans.push(("begin", scope.name.to_owned()));
        Ok(true)
    }

    fn on_end_async_scope(
        &mut self,
        _block_id: &str,
        scope: BorrowedScopeDesc<'_>,
        _ts: i64,
        _span_id: i64,
        _parent_span_id: i64,
        _depth: u32,
        properties: AnalyticsPropertySet<'_>,
    ) -> anyhow::Result<bool> {
        self.async_end_properties.push(collect_pairs(&properties));
        self.async_spans.push(("end", scope.name.to_owned()));
        Ok(true)
    }
}

#[test]
#[serial]
fn test_parse_dyn_named_spans() {
    static LOCATION: SpanLocation = SpanLocation {
        lod: Verbosity::Med,
        target: "target",
        module_path: "module_path",
        file: "file",
        line: 1,
    };
    static SCOPE: SpanMetadata = SpanMetadata {
        name: "plain_scope",
        location: SpanLocation {
            lod: Verbosity::Med,
            target: "target",
            module_path: "module_path",
            file: "file",
            line: 2,
        },
    };

    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let mut stream = ThreadStream::new(64 * 1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    let dyn_name = String::from("shared_text");
    let other_name = String::from("other_text");
    stream.push_dyn_named(&dyn_name, |name| BeginThreadDynNamedSpanEvent {
        thread_span_location: &LOCATION,
        name,
        time: 1,
    });
    stream.get_events_mut().push(BeginThreadNamedSpanEvent {
        thread_span_location: &LOCATION,
        name: "shared_text".into(),
        time: 2,
    });
    stream.get_events_mut().push(BeginThreadSpanEvent {
        thread_span_desc: &SCOPE,
        time: 3,
    });
    stream.push_dyn_named(&other_name, |name| EndThreadDynNamedSpanEvent {
        thread_span_location: &LOCATION,
        name,
        time: 4,
    });
    stream.get_events_mut().push(EndThreadNamedSpanEvent {
        thread_span_location: &LOCATION,
        name: "shared_text".into(),
        time: 5,
    });
    stream.push_dyn_named(&dyn_name, |name| BeginAsyncDynNamedSpanEvent {
        span_location: &LOCATION,
        name,
        span_id: 1,
        parent_span_id: 0,
        depth: 0,
        time: 6,
    });
    stream.get_events_mut().push(BeginAsyncNamedSpanEvent {
        span_location: &LOCATION,
        name: "shared_text".into(),
        span_id: 2,
        parent_span_id: 0,
        depth: 0,
        time: 7,
    });
    stream.push_dyn_named(&other_name, |name| EndAsyncDynNamedSpanEvent {
        span_location: &LOCATION,
        name,
        span_id: 1,
        parent_span_id: 0,
        depth: 0,
        time: 8,
    });
    stream.get_events_mut().push(EndAsyncNamedSpanEvent {
        span_location: &LOCATION,
        name: "shared_text".into(),
        span_id: 2,
        parent_span_id: 0,
        depth: 0,
        time: 9,
    });

    let mut block = stream.replace_block(Arc::new(ThreadBlock::new(
        64 * 1024,
        process_id,
        stream_id,
        0,
    )));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info).unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();
    let stream_info = make_stream_info(&stream);
    let stream_metadata = StreamMetadata::from_stream_info(&stream_info).unwrap();

    let mut collector = NameCollector::default();
    parse_thread_block_payload(
        "block",
        0,
        &received_block.payload,
        &stream_metadata,
        &mut collector,
    )
    .unwrap();
    parse_async_block_payload(
        "block",
        0,
        &received_block.payload,
        &stream_metadata,
        &mut collector,
    )
    .unwrap();

    let owned = |v: &[(&'static str, &str)]| -> Vec<(&'static str, String)> {
        v.iter().map(|(k, n)| (*k, n.to_string())).collect()
    };
    assert_eq!(
        collector.thread,
        owned(&[
            ("begin", "shared_text"),
            ("begin", "shared_text"),
            ("begin", "plain_scope"),
            ("end", "other_text"),
            ("end", "shared_text"),
        ])
    );
    assert_eq!(
        collector.async_spans,
        owned(&[
            ("begin", "shared_text"),
            ("begin", "shared_text"),
            ("end", "other_text"),
            ("end", "shared_text"),
        ])
    );
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
#[serial]
fn test_parse_tagged_end_events() {
    use micromegas_tracing::property_set::{DynProperty, Property, PropertySet};

    static LOCATION: SpanLocation = SpanLocation {
        lod: Verbosity::Med,
        target: "target",
        module_path: "module_path",
        file: "file",
        line: 1,
    };
    static SCOPE: SpanMetadata = SpanMetadata {
        name: "plain_scope",
        location: SpanLocation {
            lod: Verbosity::Med,
            target: "target",
            module_path: "module_path",
            file: "file",
            line: 2,
        },
    };

    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let mut stream = ThreadStream::new(64 * 1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    let interned = PropertySet::find_or_create(vec![Property::new("region", "eu")]);
    let rows = String::from("42");

    stream.get_events_mut().push(BeginThreadSpanEvent {
        thread_span_desc: &SCOPE,
        time: 1,
    });
    // an interned set
    stream.push_tagged(interned, &[], |properties| TaggedEndThreadSpanEvent {
        thread_span_desc: &SCOPE,
        properties,
        time: 2,
    });
    stream.get_events_mut().push(BeginThreadSpanEvent {
        thread_span_desc: &SCOPE,
        time: 3,
    });
    // a dynamic set that references a 'static name
    stream.push_tagged(interned, &[DynProperty::new("rows", &rows)], |properties| {
        TaggedEndThreadSpanEvent {
            thread_span_desc: &SCOPE,
            properties,
            time: 4,
        }
    });
    stream.get_events_mut().push(BeginThreadNamedSpanEvent {
        thread_span_location: &LOCATION,
        name: "named".into(),
        time: 5,
    });
    stream.push_tagged_named(
        "named",
        PropertySet::empty(),
        &[DynProperty::new("rows", &rows)],
        |name, properties| TaggedEndThreadNamedSpanEvent {
            thread_span_location: &LOCATION,
            name,
            properties,
            time: 6,
        },
    );
    stream.get_events_mut().push(BeginThreadSpanEvent {
        thread_span_desc: &SCOPE,
        time: 7,
    });
    stream.get_events_mut().push(EndThreadSpanEvent {
        thread_span_desc: &SCOPE,
        time: 8,
    });
    stream.get_events_mut().push(EndAsyncSpanEvent {
        span_desc: &SCOPE,
        span_id: 1,
        parent_span_id: 0,
        depth: 0,
        time: 9,
    });
    stream.push_tagged(interned, &[DynProperty::new("rows", &rows)], |properties| {
        TaggedEndAsyncSpanEvent {
            span_desc: &SCOPE,
            properties,
            span_id: 2,
            parent_span_id: 0,
            depth: 0,
            time: 10,
        }
    });
    stream.push_tagged_named("named", interned, &[], |name, properties| {
        TaggedEndAsyncNamedSpanEvent {
            span_location: &LOCATION,
            name,
            properties,
            span_id: 3,
            parent_span_id: 0,
            depth: 0,
            time: 11,
        }
    });

    let mut block = stream.replace_block(Arc::new(ThreadBlock::new(
        64 * 1024,
        process_id,
        stream_id,
        0,
    )));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info).unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();
    let stream_info = make_stream_info(&stream);
    let stream_metadata = StreamMetadata::from_stream_info(&stream_info).unwrap();

    let mut collector = NameCollector::default();
    parse_thread_block_payload(
        "block",
        0,
        &received_block.payload,
        &stream_metadata,
        &mut collector,
    )
    .unwrap();
    parse_async_block_payload(
        "block",
        0,
        &received_block.payload,
        &stream_metadata,
        &mut collector,
    )
    .unwrap();

    assert_eq!(
        collector.thread_end_properties,
        vec![
            pairs(&[("region", "eu")]),
            pairs(&[("region", "eu"), ("rows", "42")]),
            pairs(&[("rows", "42")]),
            pairs(&[]),
        ]
    );
    assert_eq!(
        collector.async_end_properties,
        vec![
            pairs(&[]),
            pairs(&[("region", "eu"), ("rows", "42")]),
            pairs(&[("region", "eu")]),
        ]
    );
}
