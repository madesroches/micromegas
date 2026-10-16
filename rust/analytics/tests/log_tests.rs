use micromegas_analytics::log_entry::log_entry_from_value;
use micromegas_analytics::metadata::StreamMetadata;
use micromegas_analytics::payload::parse_block;
use micromegas_analytics::time::ConvertTicks;
use micromegas_telemetry_sink::stream_block::StreamBlock;
use micromegas_telemetry_sink::stream_info::make_stream_info;
use micromegas_tracing::dispatch::make_process_info;
use micromegas_tracing::dyn_deps::merge_properties;
use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::levels::Level;
use micromegas_tracing::logs;
use micromegas_tracing::logs::LogBlock;
use micromegas_tracing::logs::LogStaticStrInteropEvent;
use micromegas_tracing::logs::LogStream;
use micromegas_tracing::logs::LogStringEvent;
use micromegas_tracing::logs::LogStringInteropEvent;
use micromegas_tracing::logs::TaggedLogInteropEvent;
use micromegas_tracing::logs::TaggedLogStaticStrEvent;
use micromegas_tracing::logs::TaggedLogString;
use micromegas_tracing::prelude::*;
use micromegas_tracing::property_set::Property;
use micromegas_tracing::property_set::PropertySet;
use micromegas_tracing::property_set::PropertySetId;
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

    let (events, dyn_deps) = stream.get_block_mut_parts();
    events.push(LogStaticStrInteropEvent {
        time: 1,
        level: 2,
        target: "target_name".into(),
        msg: "my message".into(),
    });
    let target = dyn_deps.intern_str_ref("target_name");
    events.push(LogStringInteropEvent {
        time: 1,
        level: 2,
        target,
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
        properties: PropertySetId::interned(PropertySet::find_or_create(vec![
            Property::new("name", "road_width"),
            Property::new("animal", "chicken"),
        ])),
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

fn parse_entries<T>(
    block: &mut Arc<LogBlock>,
    stream: &LogStream,
    process_id: uuid::Uuid,
    mut convert: impl FnMut(&micromegas_analytics::log_entry::LogEntry<'_>) -> T,
) -> Vec<T> {
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
        entries.push(convert(&entry));
        Ok(true)
    })
    .unwrap();
    entries
}

fn parse_tagged_entries(
    block: &mut Arc<LogBlock>,
    stream: &LogStream,
    process_id: uuid::Uuid,
) -> Vec<ParsedEntry> {
    parse_entries(block, stream, process_id, |entry| {
        let mut props = vec![];
        entry
            .properties
            .for_each_property(|k, v| {
                props.push((k.to_owned(), v.to_owned()));
                Ok(())
            })
            .unwrap();
        (
            entry.msg.to_owned(),
            props,
            entry.properties.object_ptr() as usize,
        )
    })
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
    // Interned sets are mixed with dynamic ones that share their names and static values.
    for (cid, msg) in [
        (None, "zero"),
        (Some("abc"), "one"),
        (Some("abc"), "two"),
        (Some("xyz"), "three"),
        (None, "four"),
    ] {
        let (events, dyn_deps) = stream.get_block_mut_parts();
        let properties = match cid {
            Some(cid) => {
                let cid = String::from(cid);
                let merged =
                    merge_properties(static_set, &[DynProperty::new("correlation_id", &cid)]);
                dyn_deps.intern_set(&merged)
            }
            None => PropertySetId::interned(static_set),
        };
        events.push(TaggedLogString {
            desc: &LOG_DESC,
            properties,
            time: 1,
            msg: DynString(msg.into()),
        });
    }

    let mut block = stream.replace_block(Arc::new(LogBlock::new(8192, process_id, stream_id, 0)));
    let entries = parse_tagged_entries(&mut block, &stream, process_id);
    assert_eq!(entries.len(), 5);
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
    assert_eq!(cid(0), None);
    assert_eq!(cid(1).as_deref(), Some("abc"));
    assert_eq!(cid(2).as_deref(), Some("abc"));
    assert_eq!(cid(3).as_deref(), Some("xyz"));
    assert_eq!(cid(4), None);
    // one wire property set per distinct content within a block
    assert_eq!(entries[1].2, entries[2].2);
    assert_ne!(entries[1].2, entries[3].2);
    assert_eq!(entries[0].2, entries[4].2);
    assert_ne!(entries[0].2, entries[1].2);
}

#[test]
#[serial]
fn test_tagged_log_interop_event() {
    let process_id = uuid::Uuid::new_v4();
    let mut stream = LogStream::new(8192, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();
    let merged = merge_properties(
        PropertySet::empty(),
        &[DynProperty::new("correlation_id", "abc")],
    );
    let (events, dyn_deps) = stream.get_block_mut_parts();
    let target = dyn_deps.intern_str_ref("interop_target");
    let properties = dyn_deps.intern_set(&merged);
    events.push(TaggedLogInteropEvent {
        time: 1,
        level: Level::Warn as u8,
        target,
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

#[test]
#[serial]
fn test_tagged_log_static_str_entries() {
    let process_id = uuid::Uuid::new_v4();
    let mut stream = LogStream::new(8192, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    static LOG_DESC: logs::LogMetadata = logs::LogMetadata {
        level: Level::Info,
        level_filter: std::sync::atomic::AtomicU32::new(logs::FILTER_LEVEL_UNSET_VALUE),
        fmt_str: "static",
        target: "target_name",
        module_path: "module_path",
        file: file!(),
        line: line!(),
    };
    let static_set = PropertySet::find_or_create(vec![Property::new("animal", "chicken")]);
    let override_set = PropertySet::find_or_create(vec![
        Property::new("level", "error"),
        Property::new("target", "overridden"),
    ]);
    let cid = String::from("abc");
    let merged = merge_properties(static_set, &[DynProperty::new("correlation_id", &cid)]);
    let (events, dyn_deps) = stream.get_block_mut_parts();
    let dynamic_set = dyn_deps.intern_set(&merged);
    for properties in [
        PropertySetId::interned(static_set),
        dynamic_set,
        PropertySetId::interned(override_set),
    ] {
        events.push(TaggedLogStaticStrEvent {
            desc: &LOG_DESC,
            properties,
            time: 1,
        });
    }

    let mut block = stream.replace_block(Arc::new(LogBlock::new(8192, process_id, stream_id, 0)));
    let entries = parse_entries(&mut block, &stream, process_id, |entry| {
        let mut props = vec![];
        entry
            .properties
            .for_each_property(|k, v| {
                props.push((k.to_owned(), v.to_owned()));
                Ok(())
            })
            .unwrap();
        (
            entry.msg.to_owned(),
            props,
            entry.level,
            entry.target.to_owned(),
        )
    });
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().all(|e| e.0 == "static"));
    let animal = ("animal".to_owned(), "chicken".to_owned());
    assert_eq!(entries[0].1, vec![animal.clone()]);
    assert!(entries[1].1.contains(&animal));
    assert!(
        entries[1]
            .1
            .contains(&("correlation_id".to_owned(), "abc".to_owned()))
    );
    assert_eq!(entries[0].2, Level::Info as i32);
    assert_eq!(entries[0].3, "target_name");
    assert_eq!(entries[2].2, Level::Error as i32);
    assert_eq!(entries[2].3, "overridden");
}

#[test]
#[serial]
fn test_block_scoped_interop_targets() {
    let process_id = uuid::Uuid::new_v4();
    let mut stream = LogStream::new(8192, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    static LOG_DESC: logs::LogMetadata = logs::LogMetadata {
        level: Level::Info,
        level_filter: std::sync::atomic::AtomicU32::new(logs::FILTER_LEVEL_UNSET_VALUE),
        fmt_str: "static message",
        target: "shared_target",
        module_path: "module_path",
        file: file!(),
        line: line!(),
    };
    let (events, dyn_deps) = stream.get_block_mut_parts();
    events.push(logs::LogStaticStrEvent {
        desc: &LOG_DESC,
        time: 1,
    });
    // a block-scoped target equal to a static log's target
    let target = dyn_deps.intern_str_ref("shared_target");
    events.push(LogStringInteropEvent {
        time: 2,
        level: Level::Warn as u8,
        target,
        msg: DynString("interop one".into()),
    });
    let target = dyn_deps.intern_str_ref("runtime_target");
    events.push(LogStringInteropEvent {
        time: 3,
        level: Level::Error as u8,
        target,
        msg: DynString("interop two".into()),
    });
    let key = String::from("region_key");
    let merged = merge_properties(
        PropertySet::empty(),
        &[DynProperty::with_dyn_name(&key, "eu")],
    );
    let target = dyn_deps.intern_str_ref("tagged_target");
    let properties = dyn_deps.intern_set(&merged);
    events.push(TaggedLogInteropEvent {
        time: 4,
        level: Level::Debug as u8,
        target,
        properties,
        msg: DynString("interop tagged".into()),
    });

    let mut block = stream.replace_block(Arc::new(LogBlock::new(8192, process_id, stream_id, 0)));
    let entries = parse_entries(&mut block, &stream, process_id, |entry| {
        let mut props = vec![];
        entry
            .properties
            .for_each_property(|k, v| {
                props.push((k.to_owned(), v.to_owned()));
                Ok(())
            })
            .unwrap();
        (
            entry.msg.to_owned(),
            entry.target.to_owned(),
            entry.level,
            props,
        )
    });
    let expected = |msg: &str, target: &str, level: Level, props: Vec<(&str, &str)>| {
        (
            msg.to_owned(),
            target.to_owned(),
            level as i32,
            props
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(
        entries,
        vec![
            expected("static message", "shared_target", Level::Info, vec![]),
            expected("interop one", "shared_target", Level::Warn, vec![]),
            expected("interop two", "runtime_target", Level::Error, vec![]),
            expected(
                "interop tagged",
                "tagged_target",
                Level::Debug,
                vec![("region_key", "eu")]
            ),
        ]
    );
}
