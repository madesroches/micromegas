# `parse_block` OTLP Support Plan

Issue: [#1467](https://github.com/madesroches/micromegas/issues/1467)

## Overview

`parse_block(block_id)` today refuses any block whose `streams.format` is not
`micromegas-transit`, and points the user at `log_entries`/`measures`/`otel_spans` instead.
That advice is circular when the problem being diagnosed *is* the materialization of those
views: an OTLP block can be present and healthy in blob storage while the derived view is
empty, and there is no SQL-reachable way to tell "the JIT/daemon pipeline is stalled" from
"the payload doesn't contain what we think it does."

This plan replaces the hard-coded format check with a **format → decoder registry**
(mirroring the existing `BlockProcessorMap` pattern) and adds decoders for `otlp/v1/logs`,
`otlp/v1/metrics`, and `otlp/v1/traces`. Each OTLP block decodes into the same
`(object_index, type_name, value JSONB)` shape transit blocks already produce — one row per
leaf record (log record / span / metric data point), with resource and scope context and a
flattened attribute map injected into every row. The output schema and the transit path's
behavior are unchanged.

## Current State

### `parse_block`

`rust/analytics/src/lakehouse/parse_block_table_function.rs`:

- `output_schema()` (:37) — `object_index Int64`, `type_name Utf8`, `value Binary` (JSONB).
- `fetch_block_metadata()` (:74) — queries the `blocks` view for one block, decodes
  `streams.dependencies_metadata` / `streams.objects_metadata` (CBOR `Vec<UserDefinedType>`)
  into a `StreamMetadata`, and **bails on any non-transit format** (:110-117). Returns
  `None` when the block is not in the `blocks` view.
- `parse_block_objects()` (:159) — drives `payload::parse_block` and builds the RecordBatch,
  converting each transit `Value::Object` via `transit_value_to_jsonb` (:46). Non-Object
  values are warned and skipped, but still consume an `object_index` (:181-187).
- `ParseBlockProvider::scan` (:274) — resolves metadata, fetches the payload
  (`payload::fetch_block_payload`), parses, and wraps the batch in a `DataSourceExec`.
  When the block is absent from the `blocks` view it silently returns **zero rows** (:293-299).
- Early limit: `limit` is pushed into the parse loop only when `filters.is_empty()` (:313).

Registered in `rust/analytics/src/lakehouse/query.rs:129-137` — one construction site.

### OTLP blocks

- Format constants live in `rust/ingestion/src/web_ingestion_service.rs:34-43`
  (`FORMAT_TRANSIT`, `FORMAT_OTLP_LOGS`, `FORMAT_OTLP_METRICS`, `FORMAT_OTLP_TRACES`).
- `rust/otel-ingestion/src/block.rs:241-247` builds one block per resource with
  `BlockPayload { dependencies: vec![], objects: <raw proto bytes> }` and `object_offset: 0`.
  **The proto bytes are not compressed** — `OtelLogsBlockProcessor` decodes
  `payload.objects.as_slice()` directly (`otel/logs_block_processor.rs:52`). An OTLP decoder
  must therefore *not* call `compression::decompress`, unlike the transit path
  (`payload.rs:69-74`).
- `streams.dependencies_metadata` / `objects_metadata` for OTLP streams are the empty-CBOR
  sentinel (`web_ingestion_service.rs:25-31`), so `fetch_block_metadata`'s existing decode
  already yields empty `Vec<UserDefinedType>`s and needs no special-casing.
- `block.nb_objects` counts leaf records for logs/traces, and for metrics counts data points
  with Summary points over-counted at `SUMMARY_MAX_ROWS_PER_POINT = 4`
  (`otel-ingestion/src/block.rs:104-180`) — it is deliberately an upper bound, not an exact
  object count.

### Existing OTel decode helpers (to reuse)

`rust/analytics/src/lakehouse/otel/attrs.rs`:
- `any_value_to_jsonb(&AnyValue) -> JsonbValue<'static>` (:39)
- `attrs_to_jsonb(&[KeyValue], &[(String, JsonbValue)]) -> Vec<u8>` (:87)
- `to_jsonb_bytes` (:79), `scope_extras` (:173)

`opentelemetry-proto` is already a workspace dependency **with the `with-serde` feature**
(`rust/Cargo.toml:70`), so every OTLP message implements `serde::Serialize` in OTLP/JSON
form: camelCase field names, 64-bit nanos as quoted strings, `trace_id`/`span_id` as hex
(`opentelemetry.proto.logs.v1.rs:75-200`). `jsonb::Value` implements
`From<&serde_json::Value>` (jsonb 0.5.5 `src/from.rs:161`), so a faithful proto → JSONB dump
is a two-line conversion with no hand-written per-field mapping.

### Format dispatch precedent

`rust/analytics/src/lakehouse/block_partition_spec.rs:29-32` defines
`BlockProcessorMap = HashMap<&'static str, Arc<dyn BlockProcessor>>`, keyed by
`streams.format`; `log_view.rs:43-54` registers `micromegas-transit` + `otlp/v1/logs` into
one map. The design below mirrors that shape exactly.

## Design

### 1. `BlockObjectDecoder` trait + registry

New module `rust/analytics/src/lakehouse/block_object_decoder.rs`:

```rust
/// Receives objects decoded from a block payload, in payload order.
pub trait ObjectVisitor {
    /// `value` is the JSONB encoding of one object. Returns false to stop iteration.
    fn visit(&mut self, type_name: &str, value: &[u8]) -> Result<bool>;

    /// Consumes an ordinal without emitting a row (a payload entry the decoder
    /// recognizes but cannot represent). Keeps `object_index` aligned with the
    /// block's true ordinals. Returns false to stop iteration.
    fn skip(&mut self) -> Result<bool>;
}

/// Decodes one block payload of a given `streams.format` into generic objects.
pub trait BlockObjectDecoder: Send + Sync + Debug {
    fn decode(
        &self,
        stream: &StreamMetadata,
        payload: &BlockPayload,
        visitor: &mut dyn ObjectVisitor,
    ) -> Result<()>;
}

pub type BlockObjectDecoderMap = HashMap<&'static str, Arc<dyn BlockObjectDecoder>>;

/// Registry covering every format shipped in-tree.
pub fn default_block_object_decoders() -> Arc<BlockObjectDecoderMap>;
```

The visitor — not the decoder — owns the Arrow builders, the running `object_index`, and the
early-limit check, so row construction stays in one place (DRY) and a new format only has to
produce `(type_name, jsonb_bytes)` pairs (open/closed).

`TransitBlockDecoder` lives in the same module and is a straight lift of the body of
`parse_block_objects` (`parse_block_table_function.rs:159-204`): `payload::parse_block` →
`transit_value_to_jsonb` → `visitor.visit(obj.type_name, &buf)`, with the existing
non-Object `warn!` path calling `visitor.skip()` so index fidelity is preserved.

### 2. OTLP decoders

New module `rust/analytics/src/lakehouse/otel/block_decoders.rs` with
`OtelLogsBlockDecoder`, `OtelMetricsBlockDecoder`, `OtelTracesBlockDecoder`. Each:

1. `ResourceLogs|ResourceMetrics|ResourceSpans::decode(payload.objects.as_slice())`
   (no decompression), with an `error!` log on failure mirroring `payload.rs:64-66` — a
   corrupt payload is a potential attack indicator regardless of what the caller does with
   the `Err`.
2. Walks `scope_*` → leaf records, emitting one row per leaf, in payload order.

Row granularity and `type_name`:

| Format | Row | `type_name` |
|---|---|---|
| `otlp/v1/logs` | one per `LogRecord` | `otlp.LogRecord` |
| `otlp/v1/traces` | one per `Span` (events/links nested in the value) | `otlp.Span` |
| `otlp/v1/metrics` | one per data point | `otlp.NumberDataPoint`, `otlp.HistogramDataPoint`, `otlp.ExponentialHistogramDataPoint`, `otlp.SummaryDataPoint` |

Metrics are per-data-point rather than per-`Metric`: for Sum and Gauge, `measures`
materialization already emits one row per data point (`otel/metrics_block_processor.rs:275`);
that correspondence is not universal, though — `append_summary` fans one `SummaryDataPoint`
into up to `SUMMARY_MAX_ROWS_PER_POINT = 4` `measures` rows, and Histogram/ExponentialHistogram
data points emit zero `measures` rows at all. `metrics_bounds` nonetheless counts data points
as its `nb_objects` basis across every kind, so per-data-point is still the granularity already
implied by the rest of the pipeline, even where `measures`'s own row count doesn't match it
one-for-one.

A `Metric` with `data: None` emits nothing and consumes no ordinal (a `debug!` is enough):
`metrics_bounds` (`otel-ingestion/src/block.rs:178`) already contributes zero to `nb_objects`
for this case, and `object_index` is purely positional over emitted leaves for OTLP blocks
(`object_offset` is always 0), so calling `visitor.skip()` here would invent a gap rather than
preserve one. `skip()` is reserved for the transit decoder, whose payload genuinely holds an
entry the decoder recognizes but cannot represent.

### 3. JSONB value shape

One shared helper builds every OTLP row:

```rust
fn leaf_jsonb<T: Serialize>(
    type_name: &str,
    leaf: &T,
    leaf_attrs: &[KeyValue],
    resource: Option<&Resource>,
    scope: Option<&InstrumentationScope>,
    schema_url: &str,
    extras: &[(String, JsonbValue<'static>)],
) -> Result<Vec<u8>>
```

It serializes the leaf with `serde_json::to_value` (OTLP/JSON), converts with
`jsonb::Value::from(&json)`, and injects synthesized keys. The `__`-prefix marks synthesized
envelope fields — it matches the existing `__type` convention from `transit_value_to_jsonb`
(`parse_block_table_function.rs:51-54`) and cannot collide with OTLP/JSON's camelCase names.

```jsonc
{
  // synthesized envelope
  "__type":       "otlp.LogRecord",
  "__attributes": { "ecs.event_id": "...", "aws.region": "..." },        // flattened leaf attrs, bare keys
  "__resource":   { "service.name": "...", "host.name": "..." },         // flattened resource attrs, bare keys
  "__scope":      { "otel.scope.name": "...", "otel.scope.version": "...",
                     "otel.scope.attr.<key>": "...", "otel.scope.schema_url": "..." },
  "__metric":     { "name": "...", "unit": "...", "description": "...",   // parent `Metric`
                     "otel.metric.kind": "sum", "otel.metric.aggregation_temporality": 2,
                     "otel.metric.is_monotonic": true },   // metrics only; `name`/`unit`/
                                                            // `description`/`otel.metric.kind`
                                                            // are synthesized for every leaf
                                                            // kind (no data point carries its
                                                            // parent metric's identity), and
                                                            // `otel.metric.aggregation_temporality`
                                                            // is copied from the parent
                                                            // Sum/Histogram/ExponentialHistogram
                                                            // message for those three kinds;
                                                            // `otel.metric.is_monotonic` is
                                                            // Sum-only, per `metrics_block_processor.rs`

  // faithful OTLP/JSON dump of the leaf record itself
  "timeUnixNano": "1700000000000000000",
  "severityNumber": 9,
  "body": { "stringValue": "..." },
  "attributes": [ { "key": "ecs.event_id", "value": { "stringValue": "..." } } ],
  "traceId": "0af7651916cd43dd8448eb211c80319c",
  ...
}
```

`__attributes` and `__resource` are both built with `attrs_to_jsonb_value` (extracted below,
no extras) over the leaf's and the resource's attribute lists respectively, using bare keys —
the same helper `log_entries.properties` uses for record attributes, so `__attributes`
compares key-for-key with the record-attribute portion of `properties`. `__resource` has no
`properties` counterpart to compare against: resource attrs never appear there, only in
`process_properties` under `otel.resource.*`-prefixed keys (a separate, simpler loop in
`otel-ingestion/src/block.rs`) — `__resource` is bare-keyed instead, matching `__attributes`'s
convention rather than `process_properties`'s. `__resource` is attributes only: the
resource-level `schema_url` and `dropped_attributes_count` fields on `ResourceLogs` /
`ResourceMetrics` / `ResourceSpans` are intentionally omitted from every row (the single
`schema_url` parameter to `leaf_jsonb` is the *scope's* schema URL, feeding
`otel.scope.schema_url` below, not the resource's — there is no resource-level counterpart in
the envelope). `__scope` is a nested object holding `scope_extras`'s entries under their
original `otel.scope.*` key names. For `log_entries`/`otel_spans` this compares key-for-key
with the `otel.scope.*` entries inside `properties`; `metrics_block_processor.rs` never calls
`scope_extras`, so `measures.properties` carries no scope keys to compare against. `__metric` always
synthesizes `otel.metric.kind` (one of `sum`, `gauge`, `histogram`, `exponential_histogram`,
`summary`) for every leaf kind,
since `metrics_block_processor.rs` only builds an `otel.metric.*` `extras` array for Sum and
Gauge and Summary builds none at all (`append_summary`'s doc comment: "No derived
`otel.metric.*` extras … are added for Summary rows") — leaving Histogram, ExponentialHistogram,
and Summary rows with no kind marker otherwise. `otel.metric.aggregation_temporality` is a
field of the parent `Sum`/`Histogram`/`ExponentialHistogram` message, not the data point
(`opentelemetry.proto.metrics.v1.rs:288,305`), so `leaf_jsonb` carries it through for those
three kinds regardless of what `extras` would otherwise contain; `otel.metric.is_monotonic`
stays Sum-only, matching `metrics_block_processor.rs`. No renaming to camelCase, and no
`properties` counterpart since metrics extras normally ride on `measures.properties`, not a
nested object; `name`/`unit`/`description` are added on top because the parent `Metric` — not
the leaf data point — is where OTLP carries them, and no properties-building helper covers
that gap. The raw
`attributes` array is kept alongside `__attributes`: the point of this tool is a faithful
dump, and the flattened view is a lossy convenience (duplicate keys collapse). This is the
shape that answers the issue's question directly:

```sql
SELECT jsonb_as_string(jsonb_get(jsonb_get(value, '__attributes'), 'my.event.id'))
FROM parse_block('<block_id>');
```

Known lossy case: `serde_json`'s `f64` serializer maps NaN/±Infinity to JSON `null`
(`impl From<f64> for Value`), and `jsonb::Value::from(&JsonValue)` carries that `Null` straight
through, so a non-finite `asDouble`/histogram `sum`/`min`/`max` renders as `null` —
indistinguishable from an absent field — even though OTLP/JSON itself encodes NaN as the string
`"NaN"`. Not fixed; noted as a known deviation from true OTLP/JSON fidelity.

To build the flattened maps without duplicating `attrs.rs`, extract from `attrs_to_jsonb`
(`otel/attrs.rs:87`) a `pub fn attrs_to_jsonb_value(attrs, extras) -> JsonbValue<'static>`
and make the existing byte-returning function a one-line wrapper — no behavior change for
its current callers.

### 4. Table-function changes

`parse_block_table_function.rs`:

- `ParseBlockTableFunction` / `ParseBlockProvider` gain a `decoders: Arc<BlockObjectDecoderMap>`
  field. `new()` keeps its current signature and defaults to `default_block_object_decoders()`
  (`query.rs:129-137` is unchanged).
- `fetch_block_metadata` drops the format check and returns the format string alongside the
  rest: `Option<(Uuid, i64, String, StreamMetadata)>`.
- `scan` looks the format up in the map. On a miss, the "known formats" list is built from the
  decoder map's own keys — not hardcoded — sorted for deterministic output (the map is a
  `HashMap`, so unsorted iteration would print in nondeterministic order):

  ```
  parse_block: no decoder for streams.format='<fmt>' (known formats: micromegas-transit,
  otlp/v1/logs, otlp/v1/metrics, otlp/v1/traces)
  ```

  This keeps registering a new format a purely additive change: the message updates itself
  from `default_block_object_decoders()` with no second string to edit.

- `parse_block_objects` becomes `ParseBlockRowBuilder`, an `ObjectVisitor` implementation
  holding the three builders, `object_offset`, `local_index`, `nb_objects`, and `early_limit`.

### 5. Block metadata lookup — lookup path unchanged, missing-block behavior fixed

`fetch_block_metadata` first parses `block_id` as a `Uuid` (`Uuid::parse_str`) before touching
the `blocks` view, and returns a distinct "`<block_id>` is not a valid block id" error on
failure — today a malformed argument is interpolated straight into the `WHERE block_id = '…'`
lookup, silently yields zero rows, and would otherwise surface the misleading
range-widening advice added below.

`fetch_block_metadata` otherwise keeps resolving the block through the `blocks` view — same
DataFusion/lakehouse path, no raw-Postgres fallback. What changes is what happens when the
block isn't there: today `scan` silently returns zero rows, which is indistinguishable from
"the block was found but has no matching records." Since a caller's `query_range` is applied
both to `fetch_block_metadata`'s session and to partition pruning (`query.rs:226-251`), and
`micromegas-query` requires either `--begin` or `--all`, a block whose `insert_time` falls
outside the query window is invisible by default — the common case, not an edge case.

`scan` now returns an error instead of an empty batch when `fetch_block_metadata` returns
`None`, naming the query range in client-neutral terms — this error surfaces through
`DataFusionError::External` to every FlightSQL caller (Grafana, the Python API, the web app),
not just `micromegas-query`, so it must not name CLI-specific flags:

```
parse_block: block '<block_id>' not found in `blocks` for the queried range
[<begin>, <end>]. The block may be outside the query's time range — widen the range to include it.
```

When `query_range` is `None` (no time restriction applied), the message drops the range clause since the block
genuinely isn't in `blocks` for any window. The metadata partition still catches up within
about a second of ingestion — a block posted moments ago and queried immediately may need a
retry, which the same error/guidance already covers.

## Implementation Steps

### Phase 1 — decoder abstraction (no behavior change for transit blocks)

Non-transit blocks (including OTLP, before Phase 2 registers decoders for them) already
change behavior in this phase: the old `parse_block does not support format=…` message is
replaced by the "no decoder for streams.format=…" message from §4, and the missing-block
error from §5 also takes effect here since it lives in `scan`, not in a decoder.

1. Create `rust/analytics/src/lakehouse/block_object_decoder.rs` with `ObjectVisitor`,
   `BlockObjectDecoder`, `BlockObjectDecoderMap`, `TransitBlockDecoder`, and
   `default_block_object_decoders()` (transit only, for now). Register the module in
   `rust/analytics/src/lakehouse/mod.rs`.
2. Rework `parse_block_table_function.rs`: `ParseBlockRowBuilder` (stays private to the crate —
   no need to make it or its constructor `pub`) implements `ObjectVisitor`; `fetch_block_metadata`
   returns the format; `scan` dispatches through the map. Behavior for transit blocks —
   including the early-limit path and non-Object index gaps — must be identical. Add the
   regression test in `parse_block_tests.rs` that drives
   `TransitBlockDecoder::decode` over a real transit `BlockPayload` with a limiting test
   `ObjectVisitor` (see Testing Strategy) to exercise the early-limit stop point through an
   actual walk; the non-Object index-gap branch is unreachable via any in-tree transit reader
   and is deliberately left uncovered (see Testing Strategy).

### Phase 2 — OTLP decoders

3. Add `serde.workspace = true` and `serde_json.workspace = true` to `rust/analytics/Cargo.toml`
   (alphabetical order) — `leaf_jsonb`'s `T: Serialize` bound (§3) names `serde` directly.
4. `otel/attrs.rs`: extract `attrs_to_jsonb_value`; keep `attrs_to_jsonb` as a wrapper.
5. Create `rust/analytics/src/lakehouse/otel/block_decoders.rs` with the shared `leaf_jsonb`
   helper and the three decoders; export from `otel/mod.rs`.
6. Register the three formats in `default_block_object_decoders()`.

### Phase 3 — docs, tests

7. Tests (see Testing Strategy).
8. Documentation updates (see Documentation).

## Files to Modify

**New**
- `rust/analytics/src/lakehouse/block_object_decoder.rs`
- `rust/analytics/src/lakehouse/otel/block_decoders.rs`
- `rust/analytics/tests/parse_block_otel_tests.rs`

**Modified**
- `rust/analytics/Cargo.toml` — add `serde`, `serde_json`
- `rust/analytics/src/lakehouse/mod.rs` — register `block_object_decoder`
- `rust/analytics/src/lakehouse/otel/mod.rs` — register `block_decoders`
- `rust/analytics/src/lakehouse/otel/attrs.rs` — extract `attrs_to_jsonb_value`
- `rust/analytics/src/lakehouse/parse_block_table_function.rs` — registry dispatch, visitor,
  unknown-format error message
- `rust/analytics/tests/parse_block_tests.rs` — add the `TransitBlockDecoder` early-limit
  regression test
- `python/micromegas/tests/test_otlp_e2e.py` — e2e coverage
- `mkdocs/docs/query-guide/functions-reference.md` — `parse_block` section
- `mkdocs/docs/otlp/index.md` — remove the limitation, add a troubleshooting recipe
- `CHANGELOG.md` — Unreleased / Analytics

## Trade-offs

**Registry vs. an `if format == ...` chain in `scan`.** The registry costs one trait and one
map but matches `BlockProcessorMap`, keeps `parse_block_table_function.rs` from growing a
per-format branch, and makes a new format a purely additive change. Chosen.

**A sibling UDF (issue option 2) vs. teaching `parse_block`.** A separate
`parse_otlp_block()` would avoid touching the transit path, but it splits one concept across
two function names and forces the user to know a block's format before querying it — exactly
the knowledge they lack while debugging. `get_payload()` already covers the "give me the raw
bytes" niche.

**`serde_json` round-trip vs. hand-written proto → JSONB.** Hand-writing gives full control
over the shape but is three signal-specific walkers over dozens of proto fields, and silently
goes stale when `opentelemetry-proto` adds fields. The serde path is generic, matches OTLP/JSON
field naming and 64-bit/ID encoding (the wire form users already know), and is free — though
enum fields (`severityNumber`, `aggregationTemporality`, …) come through as their raw `i32`
values rather than the OTLP/JSON enum *names*, and non-finite `f64`s (NaN/±Infinity) collapse
to JSON `null` (see §3) rather than OTLP/JSON's `"NaN"`/`"Infinity"` strings, so "faithful to
OTLP/JSON" overstates it slightly. The cost is also a verbose value: with no
`skip_serializing_if` attributes on the generated types, empty strings and zero fields are
always present. Acceptable for a debug tool. Chosen.

**One row per leaf vs. one row per block.** One row per block would be near-zero code, but
`object_index` would be meaningless, `LIMIT` useless, and a single row could hold thousands
of records — defeating the point of SQL-level inspection. Per-leaf mirrors the transit path
(one row per event) and the views (one row per record/data point).

**Per-data-point vs. per-`Metric` rows for metrics.** Per-data-point lines up with `measures`
and with `nb_objects`'s counting basis, at the cost of repeating the parent metric's
name/unit/type in `__metric` on every row.

**`object_index` fidelity for OTLP.** It is a positional index within the block
(`object_offset` is always 0 for OTLP blocks). It is *not* guaranteed to match `nb_objects`,
which over-counts Summary data points by design. Documented rather than worked around.

## Security

No new data exposure: `parse_block` is not admin-gated today, and `get_payload()`
(`lakehouse/get_payload_function.rs`) already returns the raw bytes of any block to any
querying user. This change only decodes bytes that were already reachable, into content
`log_entries`/`otel_spans` already surface. Access-control posture is unchanged and stays
subject to whatever the data-isolation work layers on the query path.

## Performance

Unchanged for transit blocks (same parse loop, same early-limit rule). For OTLP, prost
decodes the whole `Resource*` message up front, so the early limit only avoids the JSONB
conversion of the remaining leaves, not the proto decode — worth a sentence in the docs
alongside the existing early-limit note. A block is a single HTTP export batch, so the decoded
proto message is bounded by the ingestion body cap, but the output is larger than that cap
suggests: `__resource`, `__scope`, and (for metrics) `__metric` are re-serialized into every
row rather than shared once, and `scan` materializes the whole result as one `RecordBatch` in
memory, so a capped export batch can expand to a multiple of that cap by the time it reaches
the client. The mitigation is the same one that already exists for any query: a filter-free
`LIMIT`.

## Documentation

- `mkdocs/docs/query-guide/functions-reference.md:196-246` — rewrite the `parse_block`
  section: supported formats table, the `type_name` values per format, the `__`-prefixed
  envelope keys, and an OTLP example extracting one attribute via
  `jsonb_get(value, '__attributes')`. Notes: (1) for OTLP blocks `object_index` is a positional
  index only and is not guaranteed to match `nb_objects` from `list_partitions()`/`blocks`,
  since Summary data points are over-counted there by `SUMMARY_MAX_ROWS_PER_POINT = 4`; (2)
  non-finite `f64` values (NaN/±Infinity) in the source payload render as JSON `null` in
  `value`, indistinguishable from an absent field, because the `serde_json` conversion path
  does not preserve OTLP/JSON's `"NaN"`/`"Infinity"` string encoding; (3) a block absent from
  `blocks` for the queried time range now errors instead of returning zero rows — widen
  `--begin` or use `--all` — and a `streams.format` with no registered decoder errors with the
  list of known formats.
- `mkdocs/docs/otlp/index.md:617` — delete the "`parse_block` does not decode OTel payloads"
  limitation and add a Troubleshooting entry: *"`log_entries` is empty but ingestion
  succeeded"* → find the block in `blocks` by `process_id` / `"streams.format"`, run
  `parse_block` on it, and conclude stalled-materialization vs. bad-payload from whether the
  records are there. Note that `parse_block` is subject to the same query range as any other
  query: a block outside `--begin`/`--all` now errors rather than returning empty rows, so the
  recipe should tell the reader to widen `--begin` or pass `--all` if the block itself isn't
  found.
- `CHANGELOG.md` — Unreleased → Analytics bullet.
- `README.md:131` mentions `parse_block` only in the v0.24 history section; leave it.

## Testing Strategy

**Rust unit tests** — `rust/analytics/tests/parse_block_otel_tests.rs`, driving the decoders
directly through a `Vec<(String, Vec<u8>)>`-collecting `ObjectVisitor` (no services, no DB;
matches the existing `parse_block_tests.rs` style):

- logs: 2 scopes × N records → correct row count, `type_name = "otlp.LogRecord"`,
  `__attributes` carries a record attribute, `__resource` carries a resource attribute,
  `__scope["otel.scope.name"]` is the scope name, `timeUnixNano` is the quoted-string form,
  `traceId` is hex.
- metrics: one Sum + one Gauge + one Summary + one Histogram + one ExponentialHistogram →
  per-data-point rows with the right `type_name`s and `__metric.name`, plus — for the
  Histogram and ExponentialHistogram data points — `__metric.otel.metric.kind` (`histogram` /
  `exponential_histogram`) and `__metric.otel.metric.aggregation_temporality` carried from the
  parent `Histogram`/`ExponentialHistogram` message.
- traces: one span with an event and a link → `otlp.Span` with `events`/`links` nested.
- early limit: a visitor returning `false` after 2 objects stops the walk at 2 rows.
- garbage bytes never panic: decoding returns `Ok` or `Err` (matching
  `parse_corrupt_block_tests.rs`'s "never panics" contract) — `prost::Message::decode` skips
  unrecognized tags, so arbitrary bytes frequently decode `Ok` into a message with zero scopes
  rather than erroring.
- a truncated *valid* `Resource*` encoding (a valid message cut short mid-field) returns `Err`.

**Rust regression** — existing `parse_block_tests.rs` and `parse_corrupt_block_tests.rs` must
pass unchanged (transit path untouched). Neither drives a real early-limit stop through the
lifted decoder loop, so Phase 1 also adds a new test in `parse_block_tests.rs` that builds a
real transit `(StreamMetadata, BlockPayload)` (as in `parse_corrupt_block_tests.rs`) and calls
`TransitBlockDecoder::decode` with a test `ObjectVisitor` that returns `false` after N objects:
asserts the walk actually stops at N rows. This exercises `TransitBlockDecoder`'s lifted loop
itself (not just the boolean return of `visit`), so it catches a regression where `local_index`
keeps advancing past a `false` return. The non-Object `object_index` gap branch (the `skip()`
call on a payload entry the decoder recognizes but cannot represent) has no counterpart in any
in-tree transit reader — `parse_pod_instance` and every custom reader in
`rust/tracing/src/parsing.rs` always return `Value::Object` — so it is defensive dead code
against real data and is deliberately left uncovered rather than forced via a synthetic
non-Object payload. `ParseBlockRowBuilder` and its constructor stay private; the test only
needs the already-public `TransitBlockDecoder`, `BlockObjectDecoder`, and `ObjectVisitor`
items. §4's unknown-format branch in `scan` is, similarly, deliberately left uncovered: once
all four in-tree formats are registered in `default_block_object_decoders()`, no in-tree
caller can construct a `streams.format` value that misses the map, so there is no way to reach
that branch without a synthetic decoder map — not worth adding for a message that is otherwise
covered by inspection.

**Python e2e** — extend `python/micromegas/tests/test_otlp_e2e.py`: after
`test_otlp_logs_e2e` posts its batch, poll (`assert_eventually`, as elsewhere in that file)
`blocks` for the block of that `process_id` with `"streams.format" = 'otlp/v1/logs'`, then
assert `SELECT type_name, jsonb_as_string(jsonb_get(jsonb_get(value,'__attributes'), '<key>'))
FROM parse_block('<block_id>')` returns the 5 records with the expected attribute value. Also
assert that `parse_block('<a random UUID not present in `blocks`>')` raises — this is the one
place in the test suite that exercises §5's missing-block error instead of the pre-existing
empty-result behavior. Two assertion paths — the unit tests cover the shape.

**Manual** — `python3 local_test_env/ai_scripts/start_services.py`, post an OTLP batch, then
`micromegas-query "SELECT type_name, jsonb_format_json(value) FROM parse_block('<id>')" --begin 1h`
(the CLI errors without `--begin` or `--all`).

## Open Questions

None outstanding.
