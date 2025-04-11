use micromegas_analytics::log_entry::log_entry_from_value;
use micromegas_analytics::payload::parse_block;
use micromegas_analytics::time::ConvertTicks;
use micromegas_telemetry_sink::stream_block::StreamBlock;
use micromegas_telemetry_sink::stream_info::make_stream_info;
use micromegas_telemetry_sink::TelemetryGuardBuilder;
use micromegas_tracing::dispatch::make_process_info;
use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::levels::Level;
use micromegas_tracing::logs;
use micromegas_tracing::logs::LogBlock;
use micromegas_tracing::logs::LogStaticStrInteropEvent;
use micromegas_tracing::logs::LogStream;
use micromegas_tracing::logs::LogStringEvent;
use micromegas_tracing::logs::LogStringInteropEvent;
use micromegas_tracing::logs::TaggedLogString;
use micromegas_tracing::property_set::Property;
use micromegas_tracing::property_set::PropertySet;
use micromegas_transit::value::{Object, Value};
use micromegas_transit::DynString;
use std::collections::HashMap;
use std::sync::Arc;

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
fn test_log_encode_static() {
    let _telemetry_guard = TelemetryGuardBuilder::default()
        .with_install_tracing_capture(false)
        .build();
    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
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
    let encoded = block.encode_bin(&process_info).unwrap();
    let stream_info = make_stream_info(&stream);
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();
    parse_block(&stream_info, &received_block.payload, |val| {
        if let Value::Object(obj) = val {
            assert_eq!(obj.type_name.as_str(), "LogStaticStrInteropEvent");
            assert_eq!(obj.get::<i64>("time").unwrap(), 1);
            assert_eq!(obj.get::<u32>("level").unwrap(), 2);
            assert_eq!(&*obj.get::<Arc<String>>("target").unwrap(), "target_name");
            assert_eq!(&*obj.get::<Arc<String>>("msg").unwrap(), "my message");
        } else {
            panic!("log entry not an object");
        }
        Ok(true)
    })
    .unwrap();
}

#[test]
fn test_log_encode_dynamic() {
    let _telemetry_guard = TelemetryGuardBuilder::default()
        .with_install_tracing_capture(false)
        .build();
    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
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
    let encoded = block.encode_bin(&process_info).unwrap();
    let stream_info = make_stream_info(&stream);
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();
    parse_block(&stream_info, &received_block.payload, |val| {
        if let Value::Object(obj) = val {
            assert_eq!(obj.type_name.as_str(), "LogStringEventV2");
            assert_eq!(obj.get::<i64>("time").unwrap(), 1);
            let desc = obj.get::<Arc<Object>>("desc").unwrap();
            assert_eq!(desc.get::<u32>("level").unwrap(), 4);
            assert_eq!(&*desc.get::<Arc<String>>("target").unwrap(), "target_name");
            assert_eq!(&*obj.get::<Arc<String>>("msg").unwrap(), "my message");
        } else {
            panic!("log entry not an object");
        }
        Ok(true)
    })
    .unwrap();
}

#[test]
fn test_parse_log_interops() {
    let _telemetry_guard = TelemetryGuardBuilder::default()
        .with_install_tracing_capture(false)
        .build();
    let process_id = uuid::Uuid::new_v4();
    let process_info = Arc::new(make_process_info(
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
    let encoded = block.encode_bin(&process_info).unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();
    let stream_info = make_stream_info(&stream);
    let mut nb_log_entries = 0;
    let convert_ticks = ConvertTicks::from_meta_data(0, 0, 1).unwrap();
    parse_block(&stream_info, &received_block.payload, |val| {
        if log_entry_from_value(
            &convert_ticks,
            process_info.clone(),
            stream_id.to_string().into(),
            received_block.block_id.to_string().into(),
            0,
            &val,
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
}

#[test]
fn test_tagged_log_entries() {
    let _telemetry_guard = TelemetryGuardBuilder::default()
        .with_install_tracing_capture(false)
        .build();
    let process_id = uuid::Uuid::new_v4();
    let process_info = Arc::new(make_process_info(
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
    let encoded = block.encode_bin(&process_info).unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();
    let stream_info = make_stream_info(&stream);
    let convert_ticks = ConvertTicks::from_meta_data(0, 0, 1).unwrap();
    parse_block(&stream_info, &received_block.payload, |val| {
        let _log_entry = log_entry_from_value(
            &convert_ticks,
            process_info.clone(),
            stream_id.to_string().into(),
            received_block.block_id.to_string().into(),
            0,
            &val,
        )
        .unwrap()
        .unwrap();
        Ok(true)
    })
    .unwrap();
}
