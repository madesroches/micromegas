//! Best-effort extraction of the claimed OTLP `Resource` identity from a possibly truncated
//! request body, for logging who sent a request that authentication refused.
//!
//! Only an allowlist of scalar resource attributes is ever retained, so an unexpected custom
//! attribute (which could hold a secret) cannot reach a log through this module. Values are
//! returned as sent: sanitizing them for a log line is the caller's job.

use crate::handler::Encoding;
use crate::identity::{attr, attr_to_string, exe_from_service, process_owner_string};
use crate::proto::{KeyValue, any_value};
use prost::Message;
use prost::encoding::{WireType, decode_key, decode_varint};
use serde::de::{DeserializeSeed, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::cell::RefCell;
use std::fmt;

/// Resource attributes read from the wire. Everything else is dropped on sight.
const ALLOWED_ATTRIBUTES: [&str; 23] = [
    "service.namespace",
    "service.name",
    "host.name",
    "telemetry.sdk.language",
    "telemetry.sdk.name",
    "telemetry.sdk.version",
    "service.version",
    "service.instance.id",
    "deployment.environment.name",
    "deployment.environment",
    "k8s.namespace.name",
    "k8s.pod.name",
    "k8s.deployment.name",
    "k8s.cluster.name",
    "container.name",
    "cloud.region",
    "process.pid",
    "process.runtime.name",
    "process.runtime.version",
    "process.owner",
    "process.user.name",
    "process.real_user.name",
    "user.name",
];

const DONE: &str = "resource attributes complete";

/// Identity a rejected caller claims, as `(log property name, value)` pairs. Only non-empty
/// values are present.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaimedResource {
    properties: Vec<(&'static str, String)>,
}

impl ClaimedResource {
    pub fn properties(&self) -> &[(&'static str, String)] {
        &self.properties
    }

    /// Value of a property, or `""` when the caller did not claim it.
    pub fn get(&self, name: &str) -> &str {
        self.properties
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
            .unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.properties.is_empty()
    }

    fn from_attributes(attrs: &[KeyValue]) -> Self {
        let text = |key: &str| attr(attrs, key).map(attr_to_string).unwrap_or_default();
        let first_of = |keys: &[&str]| {
            keys.iter()
                .map(|k| text(k))
                .find(|v| !v.is_empty())
                .unwrap_or_default()
        };
        let runtime_name = text("process.runtime.name");
        let runtime_version = text("process.runtime.version");
        let runtime = [runtime_name, runtime_version]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let name = text("service.name");
        let namespace = text("service.namespace");
        let exe = if name.is_empty() && namespace.is_empty() {
            String::new()
        } else {
            exe_from_service(&namespace, &name)
        };
        let candidates: [(&'static str, String); 16] = [
            ("claimed_exe", exe),
            ("claimed_computer", text("host.name")),
            ("claimed_sdk_language", text("telemetry.sdk.language")),
            ("claimed_sdk_name", text("telemetry.sdk.name")),
            ("claimed_sdk_version", text("telemetry.sdk.version")),
            ("claimed_service_version", text("service.version")),
            ("claimed_service_instance_id", text("service.instance.id")),
            (
                "claimed_deployment_environment",
                first_of(&["deployment.environment.name", "deployment.environment"]),
            ),
            ("claimed_k8s_namespace", text("k8s.namespace.name")),
            ("claimed_k8s_pod", text("k8s.pod.name")),
            ("claimed_k8s_deployment", text("k8s.deployment.name")),
            ("claimed_k8s_cluster", text("k8s.cluster.name")),
            ("claimed_container", text("container.name")),
            ("claimed_cloud_region", text("cloud.region")),
            ("claimed_pid", text("process.pid")),
            ("claimed_runtime", runtime),
        ];
        let mut properties: Vec<_> = candidates
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .collect();
        let username = process_owner_string(attrs);
        if !username.is_empty() {
            properties.push(("claimed_username", username));
        }
        Self { properties }
    }
}

fn is_wanted(kv: &KeyValue) -> bool {
    ALLOWED_ATTRIBUTES.contains(&kv.key.as_str())
        && matches!(
            kv.value.as_ref().and_then(|v| v.value.as_ref()),
            Some(
                any_value::Value::StringValue(_)
                    | any_value::Value::IntValue(_)
                    | any_value::Value::BoolValue(_)
                    | any_value::Value::DoubleValue(_)
            )
        )
}

/// Reads the first `Resource` of the first `ResourceLogs`/`ResourceMetrics`/`ResourceSpans` of
/// an OTLP export request. `prefix` may be cut anywhere: attributes complete within it are
/// used. All three signals share the layout `request.field1 -> Resource*.field1 -> Resource`,
/// so no signal parameter is needed. Returns `None` when nothing could be extracted.
pub fn claimed_resource_from_prefix(prefix: &[u8], encoding: Encoding) -> Option<ClaimedResource> {
    let attrs = match encoding {
        Encoding::Protobuf => protobuf_attributes(prefix),
        Encoding::Json => json_attributes(prefix),
    };
    let claimed = ClaimedResource::from_attributes(&attrs);
    (!claimed.is_empty()).then_some(claimed)
}

/// Returns the first length-delimited field numbered `field` in `buf`, clipped to what is
/// present when the message is cut short. Unknown fields are skipped.
fn first_len_delimited(mut buf: &[u8], field: u32) -> Option<&[u8]> {
    while !buf.is_empty() {
        let (tag, wire_type) = decode_key(&mut buf).ok()?;
        match wire_type {
            WireType::LengthDelimited => {
                let len = usize::try_from(decode_varint(&mut buf).ok()?).ok()?;
                let end = len.min(buf.len());
                if tag == field {
                    return Some(&buf[..end]);
                }
                if len > buf.len() {
                    return None;
                }
                buf = &buf[end..];
            }
            WireType::Varint => {
                decode_varint(&mut buf).ok()?;
            }
            WireType::SixtyFourBit => buf = buf.get(8..)?,
            WireType::ThirtyTwoBit => buf = buf.get(4..)?,
            WireType::StartGroup | WireType::EndGroup => return None,
        }
    }
    None
}

fn protobuf_attributes(prefix: &[u8]) -> Vec<KeyValue> {
    let mut out = vec![];
    let Some(resource_x) = first_len_delimited(prefix, 1) else {
        return out;
    };
    let Some(mut resource) = first_len_delimited(resource_x, 1) else {
        return out;
    };
    while !resource.is_empty() {
        let Ok((tag, wire_type)) = decode_key(&mut resource) else {
            break;
        };
        match wire_type {
            WireType::LengthDelimited => {
                let Some(len) = decode_varint(&mut resource)
                    .ok()
                    .and_then(|l| usize::try_from(l).ok())
                else {
                    break;
                };
                // An attribute cut by the prefix is dropped rather than decoded half-way.
                if len > resource.len() {
                    break;
                }
                let (body, rest) = resource.split_at(len);
                resource = rest;
                if tag == 1
                    && let Ok(kv) = KeyValue::decode(body)
                    && is_wanted(&kv)
                {
                    out.push(kv);
                }
            }
            WireType::Varint => {
                if decode_varint(&mut resource).is_err() {
                    break;
                }
            }
            WireType::SixtyFourBit => match resource.get(8..) {
                Some(rest) => resource = rest,
                None => break,
            },
            WireType::ThirtyTwoBit => match resource.get(4..) {
                Some(rest) => resource = rest,
                None => break,
            },
            WireType::StartGroup | WireType::EndGroup => break,
        }
    }
    out
}

type Collected<'a> = &'a RefCell<Vec<KeyValue>>;

fn json_attributes(prefix: &[u8]) -> Vec<KeyValue> {
    let collected = RefCell::new(vec![]);
    let mut de = serde_json::Deserializer::from_slice(prefix);
    // Truncation and the deliberate early exit both surface as errors; what was collected
    // before the error is what we want either way.
    let _ = RequestSeed(&collected).deserialize(&mut de);
    collected.into_inner()
}

/// Defines a seed whose map visitor hands the value of `$wanted` to `$child` and skips the
/// other keys. Keys are matched case-sensitively as the OTLP/JSON mapping specifies.
macro_rules! map_seed {
    ($seed:ident, $visitor:ident, $wanted:expr, $child:ident) => {
        struct $seed<'a>(Collected<'a>);

        impl<'de> DeserializeSeed<'de> for $seed<'_> {
            type Value = ();
            fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
                d.deserialize_map($visitor(self.0))
            }
        }

        struct $visitor<'a>(Collected<'a>);

        impl<'de> Visitor<'de> for $visitor<'_> {
            type Value = ();
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an OTLP JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
                while let Some(key) = map.next_key::<String>()? {
                    if $wanted(key.as_str()) {
                        map.next_value_seed($child(self.0))?;
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(())
            }
        }
    };
}

map_seed!(
    RequestSeed,
    RequestVisitor,
    |k: &str| matches!(k, "resourceLogs" | "resourceMetrics" | "resourceSpans"),
    ResourceListSeed
);
map_seed!(
    EntrySeed,
    EntryVisitor,
    |k: &str| k == "resource",
    ResourceSeed
);
map_seed!(
    ResourceSeed,
    ResourceVisitor,
    |k: &str| k == "attributes",
    AttributesSeed
);

struct ResourceListSeed<'a>(Collected<'a>);

impl<'de> DeserializeSeed<'de> for ResourceListSeed<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for ResourceListSeed<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a list of resource entries")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        if seq.next_element_seed(EntrySeed(self.0))?.is_some() {
            // Only the first entry is wanted; stop before reading (or failing on) the rest.
            return Err(A::Error::custom(DONE));
        }
        Ok(())
    }
}

struct AttributesSeed<'a>(Collected<'a>);

impl<'de> DeserializeSeed<'de> for AttributesSeed<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for AttributesSeed<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a list of attributes")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(kv) = seq.next_element::<KeyValue>()? {
            if is_wanted(&kv) {
                self.0.borrow_mut().push(kv);
            }
        }
        Ok(())
    }
}
