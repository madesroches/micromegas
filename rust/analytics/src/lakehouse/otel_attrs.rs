//! Converts OTel `KeyValue` arrays + scalar `AnyValue` instances to JSONB bytes.
//!
//! Mapping follows the plan's "Attribute value encoding" table:
//!  - string → JSON string
//!  - int / double / bool → JSON number / bool
//!  - bytes → base64-encoded string (existing properties consumers expect text)
//!  - array / kvlist → recursive JSON
//!
//! The output is a JSONB-encoded `{key → value}` blob suitable for the
//! `properties` columns across `log_entries`, `measures`, and `otel_spans`.

use base64::Engine;
use jsonb::{Number as JsonbNumber, Value as JsonbValue};
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value::Value as Av};
use std::borrow::Cow;
use std::collections::BTreeMap;

/// Converts an `AnyValue` to a `jsonb::Value`. Recursively handles arrays and kvlists.
pub fn any_value_to_jsonb(v: &AnyValue) -> JsonbValue<'static> {
    match v.value.as_ref() {
        Some(Av::StringValue(s)) => JsonbValue::String(Cow::Owned(s.clone())),
        Some(Av::BoolValue(b)) => JsonbValue::Bool(*b),
        Some(Av::IntValue(i)) => JsonbValue::Number(JsonbNumber::Int64(*i)),
        Some(Av::DoubleValue(d)) => JsonbValue::Number(JsonbNumber::Float64(*d)),
        Some(Av::BytesValue(b)) => {
            // existing JSONB readers (`jsonb_extract_path`, etc.) expect strings,
            // so we base64-encode bytes rather than emitting a JSON binary type.
            let encoded = base64::engine::general_purpose::STANDARD.encode(b);
            JsonbValue::String(Cow::Owned(encoded))
        }
        Some(Av::ArrayValue(arr)) => {
            JsonbValue::Array(arr.values.iter().map(any_value_to_jsonb).collect())
        }
        Some(Av::KvlistValue(kvs)) => {
            let mut map: BTreeMap<String, JsonbValue<'static>> = BTreeMap::new();
            for kv in &kvs.values {
                let value = kv
                    .value
                    .as_ref()
                    .map(any_value_to_jsonb)
                    .unwrap_or(JsonbValue::Null);
                map.insert(kv.key.clone(), value);
            }
            JsonbValue::Object(map)
        }
        None => JsonbValue::Null,
    }
}

/// Serializes a flat `(key → value)` map (with optional extra entries layered on top)
/// to JSONB bytes. Output ordering is alphabetical, matching `serialize_properties_to_jsonb`.
pub fn attrs_to_jsonb(attrs: &[KeyValue], extras: &[(String, JsonbValue<'static>)]) -> Vec<u8> {
    let mut map: BTreeMap<String, JsonbValue<'static>> = BTreeMap::new();
    for kv in attrs {
        let value = kv
            .value
            .as_ref()
            .map(any_value_to_jsonb)
            .unwrap_or(JsonbValue::Null);
        map.insert(kv.key.clone(), value);
    }
    for (k, v) in extras {
        map.insert(k.clone(), v.clone());
    }
    let mut bytes = Vec::new();
    JsonbValue::Object(map).write_to_vec(&mut bytes);
    bytes
}

/// Renders `AnyValue` to a flat string for fields that need a textual form
/// (e.g., the `msg` column when an OTel log body is structured).
pub fn any_value_to_string(v: &AnyValue) -> String {
    match v.value.as_ref() {
        Some(Av::StringValue(s)) => s.clone(),
        Some(Av::IntValue(i)) => i.to_string(),
        Some(Av::DoubleValue(d)) => d.to_string(),
        Some(Av::BoolValue(b)) => b.to_string(),
        Some(Av::BytesValue(b)) => base64::engine::general_purpose::STANDARD.encode(b),
        Some(Av::ArrayValue(arr)) => {
            // Render via JSONB to keep round-trippable representations.
            let v = JsonbValue::Array(arr.values.iter().map(any_value_to_jsonb).collect());
            let mut bytes = Vec::new();
            v.write_to_vec(&mut bytes);
            jsonb::RawJsonb::new(&bytes).to_string()
        }
        Some(Av::KvlistValue(kvs)) => {
            let mut map: BTreeMap<String, JsonbValue<'static>> = BTreeMap::new();
            for kv in &kvs.values {
                let value = kv
                    .value
                    .as_ref()
                    .map(any_value_to_jsonb)
                    .unwrap_or(JsonbValue::Null);
                map.insert(kv.key.clone(), value);
            }
            let mut bytes = Vec::new();
            JsonbValue::Object(map).write_to_vec(&mut bytes);
            jsonb::RawJsonb::new(&bytes).to_string()
        }
        None => String::new(),
    }
}

/// Maps OTel `severity_number` (1–24) to micromegas `Level` (1–6).
///
/// Per the plan:
///  - TRACE   1–4   → 6
///  - DEBUG   5–8   → 5
///  - INFO    9–12  → 4
///  - WARN    13–16 → 3
///  - ERROR   17–20 → 2
///  - FATAL   21–24 → 1
///
/// `severity_number = 0` (UNSPECIFIED) → 6 (least-severe so default `WHERE level <= 4`
/// queries don't drop them). `> 24` → clamp to 1.
pub fn severity_number_to_level(sev: i32) -> i32 {
    match sev {
        0 => 6,       // UNSPECIFIED → Trace
        1..=4 => 6,   // TRACE
        5..=8 => 5,   // DEBUG
        9..=12 => 4,  // INFO
        13..=16 => 3, // WARN
        17..=20 => 2, // ERROR
        21..=24 => 1, // FATAL
        _ => 1,       // out of range → Fatal
    }
}
