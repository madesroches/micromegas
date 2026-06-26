//! Tests for OTel `AnyValue` → JSONB / string conversion, focused on the profiling-only
//! string-interning fields that leaked into the shared `common.v1` types in proto v1.10.0
//! (opentelemetry-proto 0.32). Logs/metrics/traces are not the Profiling signal, so per the
//! OTLP spec these references MUST be treated as absent — never as data.

use jsonb::Value as JsonbValue;
use micromegas_analytics::lakehouse::otel::attrs::{any_value_to_jsonb, any_value_to_string};
use opentelemetry_proto::tonic::common::v1::{
    AnyValue, KeyValue, KeyValueList, any_value::Value as Av,
};

fn av(value: Av) -> AnyValue {
    AnyValue { value: Some(value) }
}

#[test]
fn string_value_strindex_converts_to_jsonb_null_not_the_index() {
    let v = av(Av::StringValueStrindex(5));
    // Must be Null (absent), NOT the string "5".
    assert!(matches!(any_value_to_jsonb(&v), JsonbValue::Null));
}

#[test]
fn string_value_strindex_converts_to_empty_string_not_the_index() {
    let v = av(Av::StringValueStrindex(5));
    assert_eq!(any_value_to_string(&v), "");
}

#[test]
fn nested_strindex_inside_kvlist_is_null() {
    // A kvlist whose inner value is an interned string-table reference. Recursion must treat the
    // inner value as absent, not stringify the index.
    let inner = KeyValue {
        key: "inner".to_string(),
        key_strindex: 0,
        value: Some(av(Av::StringValueStrindex(9))),
    };
    let v = av(Av::KvlistValue(KeyValueList {
        values: vec![inner],
    }));
    match any_value_to_jsonb(&v) {
        JsonbValue::Object(map) => {
            assert!(matches!(map.get("inner"), Some(JsonbValue::Null)));
        }
        other => panic!("expected object, got {other:?}"),
    }
}

#[test]
fn interned_key_is_treated_as_absent_not_as_the_index() {
    // `key_strindex` is profiling-only. Keying off `kv.key` means an interned key (empty `key`,
    // nonzero `key_strindex`) yields an empty-key entry — never a "7" key.
    let kv = KeyValue {
        key: String::new(),
        key_strindex: 7,
        value: Some(av(Av::StringValue("x".to_string()))),
    };
    let v = av(Av::KvlistValue(KeyValueList { values: vec![kv] }));
    match any_value_to_jsonb(&v) {
        JsonbValue::Object(map) => {
            assert!(!map.contains_key("7"), "key_strindex must not become a key");
            assert!(map.contains_key(""), "interned key collapses to empty key");
        }
        other => panic!("expected object, got {other:?}"),
    }
}
