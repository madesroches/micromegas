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

/// Resolves the `process.start_time` attribute (or its deprecated `process.creation.time` alias).
pub fn process_start_string(attrs: &[KeyValue]) -> String {
    let s = attr_raw(attrs, "process.start_time");
    if !s.is_empty() {
        return s;
    }
    attr_raw(attrs, "process.creation.time")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::any_value::Value as AvValue;

    fn kv(key: &str, value: &str) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: Some(AnyValue {
                value: Some(AvValue::StringValue(value.to_string())),
            }),
        }
    }

    fn resource_with(pairs: &[(&str, &str)]) -> Resource {
        Resource {
            attributes: pairs.iter().map(|(k, v)| kv(k, v)).collect(),
            dropped_attributes_count: 0,
            entity_refs: vec![],
        }
    }

    #[test]
    fn process_id_is_stable() {
        let r1 = resource_with(&[
            ("service.name", "claude-code"),
            ("host.name", "macbook-mad"),
            ("process.pid", "1234"),
            ("process.start_time", "2026-04-01T12:00:00Z"),
        ]);
        let r2 = resource_with(&[
            // permuted order
            ("process.start_time", "2026-04-01T12:00:00Z"),
            ("host.name", "macbook-mad"),
            ("process.pid", "1234"),
            ("service.name", "claude-code"),
        ]);
        assert_eq!(
            process_id_from_resource(Some(&r1)),
            process_id_from_resource(Some(&r2))
        );
    }

    #[test]
    fn process_id_differs_per_pid() {
        let a = resource_with(&[("service.name", "claude-code"), ("process.pid", "1")]);
        let b = resource_with(&[("service.name", "claude-code"), ("process.pid", "2")]);
        assert_ne!(
            process_id_from_resource(Some(&a)),
            process_id_from_resource(Some(&b))
        );
    }

    #[test]
    fn process_id_normalizes_host_case() {
        let a = resource_with(&[("host.name", "Foo"), ("service.name", "svc")]);
        let b = resource_with(&[("host.name", "FOO"), ("service.name", "svc")]);
        assert_eq!(
            process_id_from_resource(Some(&a)),
            process_id_from_resource(Some(&b))
        );
    }

    #[test]
    fn process_start_falls_back_to_creation_time() {
        let with_new = resource_with(&[("process.start_time", "abc")]);
        let with_old = resource_with(&[("process.creation.time", "abc")]);
        assert_eq!(process_start_string(&with_new.attributes), "abc");
        assert_eq!(process_start_string(&with_old.attributes), "abc");
    }

    #[test]
    fn stream_id_differs_per_signal() {
        let p = Uuid::new_v4();
        assert_ne!(
            stream_id_from_process_signal(p, SignalKey::Logs),
            stream_id_from_process_signal(p, SignalKey::Metrics)
        );
        assert_ne!(
            stream_id_from_process_signal(p, SignalKey::Logs),
            stream_id_from_process_signal(p, SignalKey::Traces)
        );
    }

    #[test]
    fn block_id_is_content_addressed() {
        let id1 = block_id_from_payload(b"hello");
        let id2 = block_id_from_payload(b"hello");
        let id3 = block_id_from_payload(b"world");
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn degenerate_resource_detected() {
        let empty = resource_with(&[("service.name", "svc")]);
        assert!(is_degenerate_resource(&empty.attributes));
        let with_host = resource_with(&[("service.name", "svc"), ("host.name", "h")]);
        assert!(!is_degenerate_resource(&with_host.attributes));
    }
}
