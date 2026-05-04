//! Synthesizing micromegas Process / Stream / Block identity from OTLP `Resource` attributes.
//!
//! OTel has no `Process` object; just a `Resource` carrying `repeated KeyValue attributes`.
//! We hash an OS-honest tuple of identifying attributes to a stable UUIDv5. Once the
//! formula ships it cannot change without a `_V2` namespace UUID — that constraint is
//! load-bearing for cross-pod consistency.

use crate::proto::{AnyValue, KeyValue, Resource, any_value};
use uuid::{Uuid, uuid};

/// Namespace UUID for OTel-derived `process_id`. Generated 2026-05-01 via uuidgen.
/// **Load-bearing — DO NOT change without bumping to `_V2`.**
pub const NS_OTEL_PROCESS_V1: Uuid = uuid!("80a447b8-fcdd-42a6-a613-f6c8719cd5fe");

/// Namespace UUID for OTel-derived `stream_id`.
pub const NS_OTEL_STREAM_V1: Uuid = uuid!("fe93bacf-e851-4cf6-8526-05f8454b3488");

/// Namespace UUID for OTel-derived `block_id`.
pub const NS_OTEL_BLOCK_V1: Uuid = uuid!("5829a6f7-0577-4c8c-862f-cf4fdab445cc");

/// ASCII unit separator — used between concatenated string fields in identity formulas
/// to prevent tuple-boundary collisions like `("abc", "")` vs `("ab", "c")`.
const SEPARATOR: char = '\x1F';

/// OTel signal label used in stream-id derivation.
#[derive(Debug, Clone, Copy)]
pub enum SignalKey {
    Logs,
    Metrics,
    Traces,
}

impl SignalKey {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Logs => "logs",
            Self::Metrics => "metrics",
            Self::Traces => "traces",
        }
    }
}

/// Convenience accessor — fetch one resource attribute by key, returning `None` when absent.
pub fn attr<'a>(attrs: &'a [KeyValue], key: &str) -> Option<&'a AnyValue> {
    attrs
        .iter()
        .find(|kv| kv.key == key)
        .and_then(|kv| kv.value.as_ref())
}

/// Returns the attribute's value rendered as a stable string.
///
/// - `string_value` → as-is
/// - `int_value`    → decimal (some SDKs emit timestamps as int nanos)
/// - `bool_value` / `double_value` / `bytes_value` / `array_value` / `kvlist_value` →
///   their `Debug` form is unstable, so we just stringify with `format!`. In practice
///   the resource attributes that feed identity are always strings or ints, but having
///   a fallback prevents identity drift if an SDK emits something exotic.
pub fn attr_to_string(v: &AnyValue) -> String {
    match v.value.as_ref() {
        Some(any_value::Value::StringValue(s)) => s.clone(),
        Some(any_value::Value::IntValue(i)) => i.to_string(),
        Some(any_value::Value::BoolValue(b)) => b.to_string(),
        Some(any_value::Value::DoubleValue(d)) => d.to_string(),
        Some(any_value::Value::BytesValue(b)) => format!("{b:?}"),
        Some(any_value::Value::ArrayValue(_)) | Some(any_value::Value::KvlistValue(_)) => {
            // Structured values shouldn't appear in identity-bearing fields. If one does,
            // hashing the Debug form is at least deterministic for a given prost version.
            format!("{:?}", v.value)
        }
        None => String::new(),
    }
}

/// Lower-case + trim. Applied to free-form string fields where the SDK may render
/// the same logical value with different casing.
fn norm(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

/// Reads `attr` and returns the lower-cased + trimmed string value (or empty).
fn attr_norm(attrs: &[KeyValue], key: &str) -> String {
    attr(attrs, key)
        .map(|v| norm(&attr_to_string(v)))
        .unwrap_or_default()
}

/// Reads `attr` as-is (no case folding) — used for opaque values like `process.start_time`.
fn attr_raw(attrs: &[KeyValue], key: &str) -> String {
    attr(attrs, key).map(attr_to_string).unwrap_or_default()
}

/// Resolves the OTel process-creation timestamp.
///
/// `process.creation.time` is the stable OTel semantic-conventions attribute and is
/// what real SDKs emit; `process.start_time` is accepted as a fallback for any
/// non-standard producer that still uses the older name.
pub fn process_start_string(attrs: &[KeyValue]) -> String {
    let s = attr_raw(attrs, "process.creation.time");
    if !s.is_empty() {
        return s;
    }
    attr_raw(attrs, "process.start_time")
}

/// Returns true when none of the four identifying fields are populated. Caller may want
/// to log a warning so a degenerate-resource scenario doesn't silently collapse multiple
/// processes onto one `process_id`.
pub fn is_degenerate_resource(attrs: &[KeyValue]) -> bool {
    attr_norm(attrs, "host.id").is_empty()
        && attr_norm(attrs, "host.name").is_empty()
        && attr_raw(attrs, "process.pid").is_empty()
        && attr_norm(attrs, "service.instance.id").is_empty()
}

/// Derives `process_id` from a resource. Stable for the lifetime of the formula —
/// shipping a change here requires bumping `NS_OTEL_PROCESS_V1` to `_V2`.
pub fn process_id_from_resource(resource: Option<&Resource>) -> Uuid {
    let attrs = resource.map(|r| r.attributes.as_slice()).unwrap_or(&[]);

    let key = format!(
        "{host_id}{s}{host_name}{s}{pid}{s}{start}{s}{ns}{s}{name}{s}{instance}",
        s = SEPARATOR,
        host_id = attr_norm(attrs, "host.id"),
        host_name = attr_norm(attrs, "host.name"),
        pid = attr_raw(attrs, "process.pid"),
        start = process_start_string(attrs),
        ns = attr_norm(attrs, "service.namespace"),
        name = attr_norm(attrs, "service.name"),
        instance = attr_norm(attrs, "service.instance.id"),
    );
    Uuid::new_v5(&NS_OTEL_PROCESS_V1, key.as_bytes())
}

/// Derives `stream_id` from `(process_id, signal)`. Max three streams per process.
pub fn stream_id_from_process_signal(process_id: Uuid, signal: SignalKey) -> Uuid {
    let key = format!("{process_id}{}{}", SEPARATOR, signal.as_str());
    Uuid::new_v5(&NS_OTEL_STREAM_V1, key.as_bytes())
}

/// Derives `block_id` from the re-encoded protobuf bytes of one Resource submessage.
/// `Uuid::new_v5` SHA-1s its input internally, so we don't pre-hash.
pub fn block_id_from_payload(payload: &[u8]) -> Uuid {
    Uuid::new_v5(&NS_OTEL_BLOCK_V1, payload)
}
