use jsonb::Value as JsonbValue;
use micromegas_transit::value::{Object, Value as TransitValue};
use std::{borrow::Cow, sync::Arc};

// Re-export the conversion function for testing
use micromegas_analytics::lakehouse::parse_block_table_function::transit_value_to_jsonb;

#[test]
fn test_transit_string_to_jsonb() {
    let val = TransitValue::String(Arc::new("hello".to_string()));
    let jsonb = transit_value_to_jsonb(&val);
    assert!(matches!(jsonb, JsonbValue::String(s) if s == "hello"));
}

#[test]
fn test_transit_u8_to_jsonb() {
    let val = TransitValue::U8(42);
    let jsonb = transit_value_to_jsonb(&val);
    assert!(matches!(
        jsonb,
        JsonbValue::Number(jsonb::Number::UInt64(42))
    ));
}

#[test]
fn test_transit_u32_to_jsonb() {
    let val = TransitValue::U32(100_000);
    let jsonb = transit_value_to_jsonb(&val);
    assert!(matches!(
        jsonb,
        JsonbValue::Number(jsonb::Number::UInt64(100_000))
    ));
}

#[test]
fn test_transit_u64_to_jsonb() {
    let val = TransitValue::U64(999_999_999);
    let jsonb = transit_value_to_jsonb(&val);
    assert!(matches!(
        jsonb,
        JsonbValue::Number(jsonb::Number::UInt64(999_999_999))
    ));
}

#[test]
fn test_transit_i64_to_jsonb() {
    let val = TransitValue::I64(-42);
    let jsonb = transit_value_to_jsonb(&val);
    assert!(matches!(
        jsonb,
        JsonbValue::Number(jsonb::Number::Int64(-42))
    ));
}

#[test]
fn test_transit_f64_to_jsonb() {
    let val = TransitValue::F64(3.14);
    let jsonb = transit_value_to_jsonb(&val);
    match jsonb {
        JsonbValue::Number(jsonb::Number::Float64(v)) => {
            assert!((v - 3.14).abs() < f64::EPSILON);
        }
        _ => panic!("expected Float64"),
    }
}

#[test]
fn test_transit_none_to_jsonb() {
    let val = TransitValue::None;
    let jsonb = transit_value_to_jsonb(&val);
    assert!(matches!(jsonb, JsonbValue::Null));
}

#[test]
fn test_transit_object_to_jsonb() {
    let obj = Object {
        type_name: Arc::new("TestEvent".to_string()),
        members: vec![
            (
                Arc::new("msg".to_string()),
                TransitValue::String(Arc::new("hello".to_string())),
            ),
            (Arc::new("level".to_string()), TransitValue::U8(3)),
        ],
    };
    let val = TransitValue::Object(Arc::new(obj));
    let jsonb = transit_value_to_jsonb(&val);
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
    let inner = Object {
        type_name: Arc::new("Inner".to_string()),
        members: vec![(Arc::new("x".to_string()), TransitValue::I64(99))],
    };
    let outer = Object {
        type_name: Arc::new("Outer".to_string()),
        members: vec![(
            Arc::new("child".to_string()),
            TransitValue::Object(Arc::new(inner)),
        )],
    };
    let val = TransitValue::Object(Arc::new(outer));
    let jsonb = transit_value_to_jsonb(&val);
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
    let obj = Object {
        type_name: Arc::new("LogEvent".to_string()),
        members: vec![
            (
                Arc::new("msg".to_string()),
                TransitValue::String(Arc::new("test".to_string())),
            ),
            (Arc::new("count".to_string()), TransitValue::U64(42)),
            (Arc::new("empty".to_string()), TransitValue::None),
        ],
    };
    let val = TransitValue::Object(Arc::new(obj));
    let jsonb = transit_value_to_jsonb(&val);
    let mut buf = Vec::new();
    jsonb.write_to_vec(&mut buf);
    // Verify we get non-empty JSONB bytes
    assert!(!buf.is_empty());
}
