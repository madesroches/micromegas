//! Synthesizing micromegas Process / Stream / Block identity from OTLP `Resource` attributes.
//!
//! OTel has no `Process` object; just a `Resource` carrying `repeated KeyValue attributes`.
//! We hash an OS-honest tuple of identifying attributes to a UUIDv5. Long-term stability of
//! `process_id` values across upgrades is not a design goal — re-deriving existing ids is
//! always acceptable, so the formula can be extended in-place when needed.

use crate::proto::{AnyValue, KeyValue, Resource, any_value};
use micromegas_tracing::prelude::*;
use std::sync::Once;
use uuid::{Uuid, uuid};

/// `AnyValue.string_value_strindex` / `KeyValue.key_strindex` reference a
/// `ProfilesDictionary.string_table` that exists **only** for the Profiling signal. Per the
/// OTLP spec, receivers of logs/metrics/traces MUST treat these as absent. Warn once per
/// process so a misconfigured profiling producer is noticeable without flooding the logs.
fn warn_unexpected_strindex() {
    static WARNED: Once = Once::new();
    WARNED.call_once(|| {
        warn!(
            "ignoring profiling-only string-interning field on a non-profiling OTLP signal; \
             treating it as absent (OTLP spec)"
        );
    });
}

/// Namespace UUID for OTel-derived `process_id`. Generated 2026-05-01 via uuidgen.
/// **Load-bearing — DO NOT change without bumping to `_V2`.**
pub const NS_OTEL_PROCESS_V1: Uuid = uuid!("80a447b8-fcdd-42a6-a613-f6c8719cd5fe");

/// Namespace UUID for OTel-derived `stream_id`.
pub const NS_OTEL_STREAM_V1: Uuid = uuid!("fe93bacf-e851-4cf6-8526-05f8454b3488");

/// Namespace UUID for OTel-derived `block_id`.
pub const NS_OTEL_BLOCK_V1: Uuid = uuid!("5829a6f7-0577-4c8c-862f-cf4fdab445cc");

/// ASCII unit separator — used between concatenated string fields in identity formulas
/// to prevent tuple-boundary collisions like `("abc", "")` vs `("ab", "c")`.
///
/// `pub(crate)`, not private (AbAC Stage 5, #1373, §4): `block.rs` prepends an
/// audience-tagged prefix ahead of its own hash input using this exact separator, rather than a
/// `\x1F` string literal, so the two modules can never drift on what the separator character is.
pub(crate) const SEPARATOR: char = '\x1F';
pub(crate) const SEPARATOR_STR: &str = "\x1F";

/// Identity inputs beyond the OTLP payload itself (AbAC Stage 5, #1373, §4).
///
/// `Default` reproduces pre-Stage-5 ids byte for byte -- both fields are no-ops when absent, so
/// an unstamped deployment (or any call site that hasn't been threaded with a real audience yet)
/// sees zero id churn.
#[derive(Debug, Clone, Copy, Default)]
pub struct IdentityContext<'a> {
    /// Authenticated write audience. Folded into `process_id` and `block_id` so two audiences
    /// posting identical resources/payloads never collapse onto one process or dedup against
    /// each other. `None` reproduces pre-Stage-5 ids byte for byte.
    pub audience: Option<&'a str>,
    /// Webhook-only: canonicalized incoming header bytes (formerly `extra_hash_input`).
    pub extra_hash_input: &'a [u8],
}

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
///
/// Matches on `kv.key` only; `kv.key_strindex` (profiling-only) is intentionally ignored, so an
/// interned key (empty `key`) is treated as absent — the OTLP spec behavior for non-profiling
/// signals.
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
        // Profiling-only string-table reference: no dictionary exists for this signal, so the
        // index is meaningless. Treat as absent (per OTLP spec). NOTE: this value feeds the
        // load-bearing UUIDv5 identity hash — never stringify the index back into it.
        Some(any_value::Value::StringValueStrindex(_)) => {
            warn_unexpected_strindex();
            String::new()
        }
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
///
/// Public so `cloudwatch_metrics::is_cloudwatch_metric_stream_resource` can share the exact
/// same emptiness semantics `is_degenerate_resource` uses for its own fields, rather than
/// reimplementing trim+lowercase locally and risking the two checks drifting apart.
pub fn attr_norm(attrs: &[KeyValue], key: &str) -> String {
    attr(attrs, key)
        .map(|v| norm(&attr_to_string(v)))
        .unwrap_or_default()
}

/// Reads `attr` as-is (no case folding) — used for opaque values like `process.start_time`.
fn attr_raw(attrs: &[KeyValue], key: &str) -> String {
    attr(attrs, key).map(attr_to_string).unwrap_or_default()
}

/// Resolves the process owner's username.
///
/// `process.owner` is the OTel semantic-conventions attribute emitted by process resource
/// detectors. When it is absent we fall back, in decreasing semantic closeness, to the
/// process-scoped effective (`process.user.name`) and real (`process.real_user.name`) user
/// names, then to the generic `user.name` for producers that only set that.
pub fn process_owner_string(attrs: &[KeyValue]) -> String {
    for key in [
        "process.owner",
        "process.user.name",
        "process.real_user.name",
        "user.name",
    ] {
        let s = attr_raw(attrs, key);
        if !s.is_empty() {
            return s;
        }
    }
    String::new()
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

/// Derives `process_id` from a resource by hashing the identifying tuple under `NS_OTEL_PROCESS_V1`.
///
/// All fields pass through `attr_norm` (lower-case + trim) except `process.pid` and
/// `process.creation.time` which are `attr_raw`. Field order (separated by `\x1F`):
///
///   host.id · host.name · process.pid · process.creation.time ·
///   service.namespace · service.name · service.instance.id · process.owner ·
///   os.type · os.version · os.name · os.description · os.build_id ·
///   host.arch · host.type ·
///   host.image.id · host.image.name · host.image.version ·
///   host.cpu.model.id · host.cpu.model.name · host.cpu.family ·
///   host.cpu.vendor.id · host.cpu.stepping · host.cpu.cache.l2.size ·
///   service.version ·
///   telemetry.sdk.name · telemetry.sdk.language · telemetry.sdk.version ·
///   process.runtime.name · process.runtime.version · process.runtime.description
///
/// Fields are appended in-place under the same namespace UUID rather than bumping to `_V2` —
/// the same pattern used when `process.owner` was added. Long-term stability of `process_id`
/// values is not a design goal; re-deriving existing ids is always acceptable.
///
/// `ctx.audience` (AbAC Stage 5, #1373, §4) is appended as a 32nd field, **only when `Some`**:
/// appending unconditionally would add a trailing separator and re-derive every existing id even
/// for unstamped deployments. With `ctx.audience: None` the joined key -- and therefore the
/// resulting id -- is byte-identical to before this stage. This is what stops two audiences
/// sending identical resource attributes (the same containerized app in two tenants, a
/// degenerate resource, a CloudWatch namespace) from silently collapsing onto one `process_id`.
pub fn process_id_from_resource(resource: Option<&Resource>, ctx: IdentityContext) -> Uuid {
    let attrs = resource.map(|r| r.attributes.as_slice()).unwrap_or(&[]);

    let fields = [
        attr_norm(attrs, "host.id"),
        attr_norm(attrs, "host.name"),
        attr_raw(attrs, "process.pid"),
        process_start_string(attrs),
        attr_norm(attrs, "service.namespace"),
        attr_norm(attrs, "service.name"),
        attr_norm(attrs, "service.instance.id"),
        norm(&process_owner_string(attrs)),
        // OS identity
        attr_norm(attrs, "os.type"),
        attr_norm(attrs, "os.version"),
        attr_norm(attrs, "os.name"),
        attr_norm(attrs, "os.description"),
        attr_norm(attrs, "os.build_id"),
        // Host hardware
        attr_norm(attrs, "host.arch"),
        attr_norm(attrs, "host.type"),
        attr_norm(attrs, "host.image.id"),
        attr_norm(attrs, "host.image.name"),
        attr_norm(attrs, "host.image.version"),
        attr_norm(attrs, "host.cpu.model.id"),
        attr_norm(attrs, "host.cpu.model.name"),
        attr_norm(attrs, "host.cpu.family"),
        attr_norm(attrs, "host.cpu.vendor.id"),
        attr_norm(attrs, "host.cpu.stepping"),
        attr_norm(attrs, "host.cpu.cache.l2.size"),
        // Service / SDK
        attr_norm(attrs, "service.version"),
        attr_norm(attrs, "telemetry.sdk.name"),
        attr_norm(attrs, "telemetry.sdk.language"),
        attr_norm(attrs, "telemetry.sdk.version"),
        // Runtime
        attr_norm(attrs, "process.runtime.name"),
        attr_norm(attrs, "process.runtime.version"),
        attr_norm(attrs, "process.runtime.description"),
    ];

    let mut key = fields.join(SEPARATOR_STR);
    if let Some(audience) = ctx.audience {
        key.push(SEPARATOR);
        key.push_str(audience);
    }
    Uuid::new_v5(&NS_OTEL_PROCESS_V1, key.as_bytes())
}

/// Derives `stream_id` from `(process_id, signal)`. Max three streams per process.
pub fn stream_id_from_process_signal(process_id: Uuid, signal: SignalKey) -> Uuid {
    let key = format!("{process_id}{}{}", SEPARATOR, signal.as_str());
    Uuid::new_v5(&NS_OTEL_STREAM_V1, key.as_bytes())
}

/// Derives `block_id` by hashing whatever bytes the caller hands it. `Uuid::new_v5` SHA-1s its
/// input internally, so we don't pre-hash.
///
/// This is no longer just "the re-encoded protobuf bytes of one Resource submessage" -- the
/// signature is unchanged, but callers now routinely concatenate up to three inputs ahead of
/// calling this: the webhook path's canonicalized header set (`extra_hash_input`, since before
/// this doc was corrected), and, as of AbAC Stage 5 (#1373, §4), an
/// `"aud{SEPARATOR}{audience}{SEPARATOR}"` prefix (`block.rs`) so two audiences posting
/// byte-identical payloads never dedup against each other. See `block.rs`'s `split_logs` /
/// `split_metrics` / `split_traces` for the actual hash-input assembly.
pub fn block_id_from_payload(payload: &[u8]) -> Uuid {
    Uuid::new_v5(&NS_OTEL_BLOCK_V1, payload)
}
