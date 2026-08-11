use jsonb::Value as JsonbValue;
use micromegas_analytics::lakehouse::block_object_decoder::{
    BlockObjectDecoder, ObjectVisitor, TransitBlockDecoder,
};
use micromegas_analytics::metadata::StreamMetadata;
use micromegas_telemetry::block_wire_format::{Block, BlockPayload};
use micromegas_telemetry_sink::stream_block::StreamBlock;
use micromegas_telemetry_sink::stream_info::make_stream_info;
use micromegas_tracing::dispatch::make_process_info;
use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::logs::{LogBlock, LogStaticStrInteropEvent, LogStream};
use micromegas_transit::value::{Object, Value as TransitValue};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

// Re-export the conversion function for testing
use micromegas_analytics::lakehouse::parse_block_table_function::transit_value_to_jsonb;

const BUF: usize = 256 * 1024;

/// Builds a small, real transit `(StreamMetadata, BlockPayload)` carrying `n` log
/// events — same block-builder pattern as `parse_corrupt_block_tests.rs`.
fn build_log_block(n: usize) -> (StreamMetadata, BlockPayload) {
    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let mut stream = LogStream::new(BUF, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    for i in 0..n {
        stream.get_events_mut().push(LogStaticStrInteropEvent {
            time: i as i64,
            level: 2,
            target: "target_name".into(),
            msg: "my message".into(),
        });
    }

    let mut block = stream.replace_block(Arc::new(LogBlock::new(BUF, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info).unwrap();
    let meta = StreamMetadata::from_stream_info(&make_stream_info(&stream)).unwrap();
    let received: Block = ciborium::from_reader(&encoded[..]).unwrap();
    (meta, received.payload)
}

/// Test double: collects `(type_name, value)` pairs and stops once `limit` rows
/// have been visited — exercises `TransitBlockDecoder`'s lifted parse loop
/// itself, not just the boolean return of `visit`.
struct StoppingVisitor {
    limit: usize,
    rows: Vec<(String, Vec<u8>)>,
}

impl ObjectVisitor for StoppingVisitor {
    fn visit(&mut self, type_name: &str, value: &[u8]) -> anyhow::Result<bool> {
        self.rows.push((type_name.to_string(), value.to_vec()));
        Ok(self.rows.len() < self.limit)
    }

    fn skip(&mut self) -> anyhow::Result<bool> {
        Ok(true)
    }
}

#[test]
fn test_transit_block_decoder_stops_at_early_limit() {
    let (meta, payload) = build_log_block(5);
    let mut visitor = StoppingVisitor {
        limit: 2,
        rows: Vec::new(),
    };
    TransitBlockDecoder
        .decode(&meta, &payload, &mut visitor)
        .expect("decoding a valid transit block");
    assert_eq!(visitor.rows.len(), 2);
}

#[test]
fn test_transit_string_to_jsonb() {
    let val = TransitValue::String("hello");
    let jsonb = transit_value_to_jsonb(val);
    assert!(matches!(jsonb, JsonbValue::String(s) if s == "hello"));
}

#[test]
fn test_transit_u8_to_jsonb() {
    let val = TransitValue::U8(42);
    let jsonb = transit_value_to_jsonb(val);
    assert!(matches!(
        jsonb,
        JsonbValue::Number(jsonb::Number::UInt64(42))
    ));
}

#[test]
fn test_transit_u32_to_jsonb() {
    let val = TransitValue::U32(100_000);
    let jsonb = transit_value_to_jsonb(val);
    assert!(matches!(
        jsonb,
        JsonbValue::Number(jsonb::Number::UInt64(100_000))
    ));
}

#[test]
fn test_transit_u64_to_jsonb() {
    let val = TransitValue::U64(999_999_999);
    let jsonb = transit_value_to_jsonb(val);
    assert!(matches!(
        jsonb,
        JsonbValue::Number(jsonb::Number::UInt64(999_999_999))
    ));
}

#[test]
fn test_transit_i64_to_jsonb() {
    let val = TransitValue::I64(-42);
    let jsonb = transit_value_to_jsonb(val);
    assert!(matches!(
        jsonb,
        JsonbValue::Number(jsonb::Number::Int64(-42))
    ));
}

#[test]
fn test_transit_f64_to_jsonb() {
    let val = TransitValue::F64(std::f64::consts::PI);
    let jsonb = transit_value_to_jsonb(val);
    match jsonb {
        JsonbValue::Number(jsonb::Number::Float64(v)) => {
            assert!((v - std::f64::consts::PI).abs() < f64::EPSILON);
        }
        _ => panic!("expected Float64"),
    }
}

#[test]
fn test_transit_none_to_jsonb() {
    let val = TransitValue::None;
    let jsonb = transit_value_to_jsonb(val);
    assert!(matches!(jsonb, JsonbValue::Null));
}

#[test]
fn test_transit_object_to_jsonb() {
    let members = [
        ("msg", TransitValue::String("hello")),
        ("level", TransitValue::U8(3)),
    ];
    let obj = Object {
        type_name: "TestEvent",
        members: &members,
    };
    let val = TransitValue::Object(&obj);
    let jsonb = transit_value_to_jsonb(val);
    match jsonb {
        JsonbValue::Object(map) => {
            assert_eq!(
                map.get("__type"),
                Some(&JsonbValue::String(Cow::Borrowed("TestEvent")))
            );
            assert!(matches!(map.get("msg"), Some(JsonbValue::String(s)) if s == "hello"));
            assert!(matches!(
                map.get("level"),
                Some(JsonbValue::Number(jsonb::Number::UInt64(3)))
            ));
        }
        _ => panic!("expected Object"),
    }
}

#[test]
fn test_transit_nested_object_to_jsonb() {
    let inner_members = [("x", TransitValue::I64(99))];
    let inner = Object {
        type_name: "Inner",
        members: &inner_members,
    };
    let outer_members = [("child", TransitValue::Object(&inner))];
    let outer = Object {
        type_name: "Outer",
        members: &outer_members,
    };
    let val = TransitValue::Object(&outer);
    let jsonb = transit_value_to_jsonb(val);
    match jsonb {
        JsonbValue::Object(map) => {
            assert_eq!(
                map.get("__type"),
                Some(&JsonbValue::String(Cow::Borrowed("Outer")))
            );
            match map.get("child") {
                Some(JsonbValue::Object(inner_map)) => {
                    assert_eq!(
                        inner_map.get("__type"),
                        Some(&JsonbValue::String(Cow::Borrowed("Inner")))
                    );
                    assert!(matches!(
                        inner_map.get("x"),
                        Some(JsonbValue::Number(jsonb::Number::Int64(99)))
                    ));
                }
                _ => panic!("expected nested Object"),
            }
        }
        _ => panic!("expected Object"),
    }
}

#[test]
fn test_transit_value_roundtrip_to_jsonb_bytes() {
    let members = [
        ("msg", TransitValue::String("test")),
        ("count", TransitValue::U64(42)),
        ("empty", TransitValue::None),
    ];
    let obj = Object {
        type_name: "LogEvent",
        members: &members,
    };
    let val = TransitValue::Object(&obj);
    let jsonb = transit_value_to_jsonb(val);
    let mut buf = Vec::new();
    jsonb.write_to_vec(&mut buf);
    // Verify we get non-empty JSONB bytes
    assert!(!buf.is_empty());
}
