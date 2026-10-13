use micromegas_analytics::log_entry::log_entry_from_value;
use micromegas_analytics::metadata::StreamMetadata;
use micromegas_analytics::payload::parse_block;
use micromegas_analytics::time::ConvertTicks;
use micromegas_telemetry_sink::stream_block::StreamBlock;
use micromegas_telemetry_sink::stream_info::make_stream_info;
use micromegas_tracing::dispatch::{DynProperty, empty_property_set, make_process_info};
use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::intern_string::intern_string;
use micromegas_tracing::levels::Level;
use micromegas_tracing::logs;
use micromegas_tracing::logs::LogBlock;
use micromegas_tracing::logs::LogStaticStrInteropEvent;
use micromegas_tracing::logs::LogStream;
use micromegas_tracing::logs::LogStringEvent;
use micromegas_tracing::logs::LogStringInteropEvent;
use micromegas_tracing::logs::TaggedLogInteropEvent;
use micromegas_tracing::logs::TaggedLogString;
use micromegas_tracing::prelude::*;
use micromegas_tracing::property_arena::merge_properties;
use micromegas_tracing::property_set::Property;
use micromegas_tracing::property_set::PropertySet;
use micromegas_tracing::test_utils::init_in_memory_tracing;
use micromegas_transit::DynString;
use micromegas_transit::value::{Object, Value};
use serial_test::serial;
use std::collections::HashMap;
use std::sync::Arc;

mod test_helpers;
use test_helpers::make_process_metadata;

#[test]
fn test_log_interop_metadata() {
    let stream = LogStream::new(1024, uuid::Uuid::new_v4(), &[], HashMap::new());
    let stream_proto = make_stream_info(&stream);
    let obj_meta = &stream_proto.objects_metadata;
    obj_meta
        .iter()
        .position(|udt| *udt.name == "LogStringInteropEventV3")
        .unwrap();
    obj_meta
        .iter()
        .position(|udt| *udt.name == "LogStaticStrInteropEvent")
        .unwrap();
    obj_meta
        .iter()
        .position(|udt| *udt.name == "StringId")
        .unwrap();
}

#[test]
#[serial]
fn test_log_encode_static() {
    let process_id = uuid::Uuid::new_v4();
    let process_info_for_encode =
        make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let mut stream = LogStream::new(1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    stream.get_events_mut().push(LogStaticStrInteropEvent {
        time: 1,
        level: 2,
        target: "target_name".into(),
        msg: "my message".into(),
    });

    let mut block = stream.replace_block(Arc::new(LogBlock::new(1024, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info_for_encode).unwrap();
    let stream_info = make_stream_info(&stream);
    let stream_metadata = StreamMetadata::from_stream_info(&stream_info).unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();

    parse_block(&stream_metadata, &received_block.payload, |val| {
        if let Value::Object(obj) = val {
            assert_eq!(obj.type_name, "LogStaticStrInteropEvent");
            assert_eq!(obj.get::<i64>("time").unwrap(), 1);
            assert_eq!(obj.get::<u32>("level").unwrap(), 2);
            assert_eq!(obj.get::<&str>("target").unwrap(), "target_name");
            assert_eq!(obj.get::<&str>("msg").unwrap(), "my message");
        } else {
            panic!("log entry not an object");
        }
        Ok(true)
    })
    .unwrap();

    let guard = init_in_memory_tracing();
    info!("integration test message");
    micromegas_tracing::dispatch::flush_log_buffer();

    let state = guard.sink.state.lock().unwrap();
    assert!(state.process_info.is_some());
    assert!(state.log_stream_desc.is_some());
}

#[test]
#[serial]
fn test_log_encode_dynamic() {
    let process_id = uuid::Uuid::new_v4();
    let process_info_for_encode =
        make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let mut stream = LogStream::new(1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    static LOG_DESC: logs::LogMetadata = logs::LogMetadata {
        level: Level::Info,
        level_filter: std::sync::atomic::AtomicU32::new(logs::FILTER_LEVEL_UNSET_VALUE),
        fmt_str: "",
        target: "target_name",
        module_path: "module_path",
        file: file!(),
        line: line!(),
    };

    stream.get_events_mut().push(LogStringEvent {
        desc: &LOG_DESC,
        time: 1,
        msg: micromegas_transit::DynString(String::from("my message")),
    });

    let mut block = stream.replace_block(Arc::new(LogBlock::new(1024, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info_for_encode).unwrap();
    let stream_info = make_stream_info(&stream);
    let stream_metadata = StreamMetadata::from_stream_info(&stream_info).unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();

    parse_block(&stream_metadata, &received_block.payload, |val| {
        if let Value::Object(obj) = val {
            assert_eq!(obj.type_name, "LogStringEventV2");
            assert_eq!(obj.get::<i64>("time").unwrap(), 1);
            let desc = obj.get::<&Object>("desc").unwrap();
            assert_eq!(desc.get::<u32>("level").unwrap(), 4);
            assert_eq!(desc.get::<&str>("target").unwrap(), "target_name");
            assert_eq!(obj.get::<&str>("msg").unwrap(), "my message");
        } else {
            panic!("log entry not an object");
        }
        Ok(true)
    })
    .unwrap();

    let guard = init_in_memory_tracing();
    let dynamic_content = "integration test message";
    info!("{}", dynamic_content);
    micromegas_tracing::dispatch::flush_log_buffer();

    let state = guard.sink.state.lock().unwrap();
    assert!(state.process_info.is_some());
    assert!(state.log_stream_desc.is_some());
}

#[test]
#[serial]
fn test_parse_log_interops() {
    let process_id = uuid::Uuid::new_v4();
    let process_info_for_encode =
        make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let process_info = Arc::new(make_process_metadata(
        process_id,
        Some(uuid::Uuid::new_v4()),
        HashMap::new(),
    ));
    let mut stream = LogStream::new(1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    stream.get_events_mut().push(LogStaticStrInteropEvent {
        time: 1,
        level: 2,
        target: "target_name".into(),
        msg: "my message".into(),
    });
    stream.get_events_mut().push(LogStringInteropEvent {
        time: 1,
        level: 2,
        target: "target_name".into(),
        msg: micromegas_transit::DynString(String::from("my message")),
    });

    let mut block = stream.replace_block(Arc::new(LogBlock::new(1024, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info_for_encode).unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();
    let stream_info = make_stream_info(&stream);
    let stream_metadata = StreamMetadata::from_stream_info(&stream_info).unwrap();
    let mut nb_log_entries = 0;
    let convert_ticks = ConvertTicks::from_meta_data(0, 0, 1).unwrap();

    parse_block(&stream_metadata, &received_block.payload, |val| {
        if log_entry_from_value(
            &convert_ticks,
            process_info.clone(),
            stream_id.to_string().into(),
            received_block.block_id.to_string().into(),
            0,
            val,
        )
        .unwrap()
        .is_some()
        {
            nb_log_entries += 1;
        }
        Ok(true)
    })
    .unwrap();
    assert_eq!(nb_log_entries, 2);

    let guard = init_in_memory_tracing();
    info!("static message");
    let dynamic_msg = "dynamic message";
    info!("{}", dynamic_msg);
    micromegas_tracing::dispatch::flush_log_buffer();

    let state = guard.sink.state.lock().unwrap();
    assert!(state.process_info.is_some());
    assert!(state.log_stream_desc.is_some());
}

#[test]
#[serial]
fn test_tagged_log_entries() {
    let process_id = uuid::Uuid::new_v4();
    let process_info_for_encode =
        make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let process_info = Arc::new(make_process_metadata(
        process_id,
        Some(uuid::Uuid::new_v4()),
        HashMap::new(),
    ));
    let mut stream = LogStream::new(1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    static LOG_DESC: logs::LogMetadata = logs::LogMetadata {
        level: Level::Info,
        level_filter: std::sync::atomic::AtomicU32::new(logs::FILTER_LEVEL_UNSET_VALUE),
        fmt_str: "",
        target: "target_name",
        module_path: "module_path",
        file: file!(),
        line: line!(),
    };

    stream.get_events_mut().push(TaggedLogString {
        desc: &LOG_DESC,
        properties: PropertySet::find_or_create(vec![
            Property::new("name", "road_width"),
            Property::new("animal", "chicken"),
        ]),
        time: 1,
        msg: DynString("my message".into()),
    });

    let mut block = stream.replace_block(Arc::new(LogBlock::new(1024, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info_for_encode).unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();
    let stream_info = make_stream_info(&stream);
    let stream_metadata = StreamMetadata::from_stream_info(&stream_info).unwrap();
    let convert_ticks = ConvertTicks::from_meta_data(0, 0, 1).unwrap();

    parse_block(&stream_metadata, &received_block.payload, |val| {
        let _log_entry = log_entry_from_value(
            &convert_ticks,
            process_info.clone(),
            stream_id.to_string().into(),
            received_block.block_id.to_string().into(),
            0,
            val,
        )
        .unwrap()
        .unwrap();
        Ok(true)
    })
    .unwrap();

    let guard = init_in_memory_tracing();
    info!("tagged message");
    micromegas_tracing::dispatch::flush_log_buffer();

    let state = guard.sink.state.lock().unwrap();
    assert!(state.process_info.is_some());
    assert!(state.log_stream_desc.is_some());
}

/// `(msg, properties, wire property set identity)` per parsed entry
type ParsedEntry = (String, Vec<(String, String)>, usize);

fn parse_tagged_entries(
    block: &mut Arc<LogBlock>,
    stream: &LogStream,
    process_id: uuid::Uuid,
) -> Vec<ParsedEntry> {
    let process_info_for_encode =
        make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let process_info = Arc::new(make_process_metadata(
        process_id,
        Some(uuid::Uuid::new_v4()),
        HashMap::new(),
    ));
    Arc::get_mut(block).unwrap().close();
    let encoded = block.encode_bin(&process_info_for_encode).unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();
    let stream_info = make_stream_info(stream);
    let stream_metadata = StreamMetadata::from_stream_info(&stream_info).unwrap();
    let convert_ticks = ConvertTicks::from_meta_data(0, 0, 1).unwrap();
    let mut entries = vec![];
    parse_block(&stream_metadata, &received_block.payload, |val| {
        let entry = log_entry_from_value(
            &convert_ticks,
            process_info.clone(),
            stream.stream_id().to_string().into(),
            received_block.block_id.to_string().into(),
            0,
            val,
        )
        .unwrap()
        .unwrap();
        let mut props = vec![];
        entry
            .properties
            .for_each_property(|k, v| {
                props.push((k.to_owned(), v.to_owned()));
                Ok(())
            })
            .unwrap();
        entries.push((
            entry.msg.to_owned(),
            props,
            entry.properties.object_ptr() as usize,
        ));
        Ok(true)
    })
    .unwrap();
    entries
}

#[test]
#[serial]
fn test_dynamic_tagged_log_entries() {
    let process_id = uuid::Uuid::new_v4();
    let mut stream = LogStream::new(8192, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    static LOG_DESC: logs::LogMetadata = logs::LogMetadata {
        level: Level::Info,
        level_filter: std::sync::atomic::AtomicU32::new(logs::FILTER_LEVEL_UNSET_VALUE),
        fmt_str: "",
        target: "target_name",
        module_path: "module_path",
        file: file!(),
        line: line!(),
    };
    let static_set = PropertySet::find_or_create(vec![Property::new("animal", "chicken")]);
    for (cid, msg) in [("abc", "one"), ("abc", "two"), ("xyz", "three")] {
        let cid = String::from(cid);
        // SAFETY: `cid` outlives `merged`, which is never interned.
        let merged = unsafe {
            merge_properties(
                static_set,
                &[DynProperty {
                    name: "correlation_id",
                    value: &cid,
                }],
            )
        };
        let (events, arena) = stream.get_block_mut_parts();
        let properties = arena.get_or_insert(&merged);
        events.push(TaggedLogString {
            desc: &LOG_DESC,
            properties,
            time: 1,
            msg: DynString(msg.into()),
        });
    }

    let mut block = stream.replace_block(Arc::new(LogBlock::new(8192, process_id, stream_id, 0)));
    let entries = parse_tagged_entries(&mut block, &stream, process_id);
    assert_eq!(entries.len(), 3);
    for (_, props, _) in &entries {
        assert!(props.contains(&("animal".to_owned(), "chicken".to_owned())));
    }
    let cid = |i: usize| {
        entries[i]
            .1
            .iter()
            .find(|(k, _)| k == "correlation_id")
            .map(|(_, v)| v.clone())
    };
    assert_eq!(cid(0).as_deref(), Some("abc"));
    assert_eq!(cid(1).as_deref(), Some("abc"));
    assert_eq!(cid(2).as_deref(), Some("xyz"));
    // one wire property set per distinct content within a block
    assert_eq!(entries[0].2, entries[1].2);
    assert_ne!(entries[0].2, entries[2].2);
}

#[test]
#[serial]
fn test_tagged_log_interop_event() {
    let process_id = uuid::Uuid::new_v4();
    let mut stream = LogStream::new(8192, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();
    // SAFETY: the value is a string literal and `merged` is never interned.
    let merged = unsafe {
        merge_properties(
            empty_property_set(),
            &[DynProperty {
                name: "correlation_id",
                value: "abc",
            }],
        )
    };
    let (events, arena) = stream.get_block_mut_parts();
    let properties = arena.get_or_insert(&merged);
    events.push(TaggedLogInteropEvent {
        time: 1,
        level: Level::Warn as u8,
        target: intern_string("interop_target").into(),
        properties,
        msg: DynString("interop message".into()),
    });

    let mut block = stream.replace_block(Arc::new(LogBlock::new(8192, process_id, stream_id, 0)));
    let entries = parse_tagged_entries(&mut block, &stream, process_id);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, "interop message");
    assert_eq!(
        entries[0].1,
        vec![("correlation_id".to_owned(), "abc".to_owned())]
    );
}
