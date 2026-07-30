# CloudWatch Metric Streams: Per-Namespace Process Identity Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1387
**Related**: #1386 (CloudWatch Logs Firehose `exe` naming — closed as a misdiagnosis of this
issue; same class of bug, much lower severity, different route)
**Depends on**: #1299 (closed/shipped — see `tasks/completed/1299_firehose_otlp_metrics_ingestion_plan.md`),
which built the metrics Firehose route this plan modifies.

## Overview

CloudWatch Metric Streams delivered via `POST /ingestion/otlp/v1/metrics/firehose` carry a
`Resource` with no `service.*`/`host.*`/`process.*` identity fields at all — only
`cloud.account.id`, `cloud.provider`, `cloud.region`, and `aws.exporter.arn`. Every stream from
every AWS account/region therefore hashes to the same fully-degenerate `process_id`
(`3f62581b-ec0e-535d-8d84-4a528c2d77cb`, per the issue's dev data), and `processes.exe` is empty.

This plan adds a CloudWatch-Metric-Streams-specific rewrite step — mirroring what
`cloudwatch_logs.rs` already does for the Logs Firehose route — that detects this fingerprint and
partitions each incoming `ResourceMetrics` into one synthetic `ResourceMetrics` per CloudWatch
**namespace** (`AWS/RDS`, `AWS/ECS`, `ECS/ContainerInsights`, `AWS/S3`, …), giving `exe` a bounded,
meaningful value (Option B from the issue) while folding the exporter ARN into
`service.instance.id` so different accounts/regions never collide onto the same process.

## Current State

- **Route**: `rust/public/src/servers/firehose.rs` → `ingest_firehose_metrics`
  (`rust/otel-ingestion/src/handler.rs:370-388`) decodes each Firehose record as one-or-more
  length-delimited `ExportMetricsServiceRequest` messages (`decode_next_length_delimited`,
  `handler.rs:58-73`) and feeds each straight into `ingest_parsed_metrics`
  (`handler.rs:168-181`) → `split_metrics` (`block.rs:365-387`) → `write_blocks`
  (`handler.rs:94-144`). **No resource rewriting happens anywhere on this path** — whatever
  `Resource` AWS sends is what gets hashed.
- **Identity hash**: `process_id_from_resource` (`rust/otel-ingestion/src/identity.rs:184-227`)
  hashes a fixed 31-field tuple (`host.*`, `process.*`, `service.*`, `os.*`, `telemetry.sdk.*`,
  in `SEPARATOR`-joined order). None of these are `cloud.*` or `aws.*` — a CloudWatch Metric
  Streams resource (only `cloud.account.id`/`cloud.provider`/`cloud.region`/`aws.exporter.arn`
  populated) hashes identically to a totally empty resource. `is_degenerate_resource`
  (`identity.rs:158-163`) already flags exactly this (checks `host.id`/`host.name`/`process.pid`/
  `service.instance.id`, all empty here) and logs a `debug!` (`block.rs:218-227`), but nothing
  corrects it.
- **`exe` derivation**: `ProcessFromResource::build` (`block.rs:443-455`) —
  `exe = if svc_ns.is_empty() { svc_name } else { format!("{svc_ns}/{svc_name}") }` — both empty
  here, so `exe = ""`.
- **Split granularity**: `split_metrics` (`block.rs:365-387`) already creates one `PreparedBlock`
  (and therefore one `process_id`/`stream_id` pair) **per `ResourceMetrics` entry** in
  `req.resource_metrics`. CloudWatch Metric Streams' OTel 1.0.0 output emits one `ResourceMetrics`
  per delivered message (confirmed by the dev data: a single degenerate process absorbs
  RDS+ECS+S3+ContainerInsights metrics together), so **splitting further requires rewriting
  inside one `ResourceMetrics`**, not just passing more `ResourceMetrics` through unchanged.
- **Metric naming**: `mkdocs/docs/otlp/index.md:154` documents `Metric.name` as
  `amazonaws.com/<Namespace>/<MetricName>` (e.g. `amazonaws.com/AWS/EC2/CPUUtilization`), but
  **nothing in the repo parses this string** — grep across `rust/analytics/src`,
  `rust/otel-ingestion/src`, and all tests turns up zero references to `Namespace`, `MetricName`,
  or `Dimensions`.
- **Datapoint attributes are the more reliable source.** `metrics_block_processor.rs`
  (`rust/analytics/src/lakehouse/otel/metrics_block_processor.rs:275,306`) passes
  `dp.attributes` (the OTLP datapoint's own `Vec<KeyValue>` — `NumberDataPoint`/
  `HistogramDataPoint`/`ExponentialHistogramDataPoint`/`SummaryDataPoint` per
  `opentelemetry-proto`'s `metrics.v1` message set) straight through `attrs_to_jsonb` into the
  `measures.properties` column, **verbatim, with no relabeling**. The issue's dev-data snippet —
  `properties = {Namespace: AWS/ECS, MetricName: CPUUtilization, Dimensions: {...}}` — is
  therefore proof that AWS attaches a literal `Namespace` (and `MetricName`, `Dimensions`)
  `KeyValue` attribute to **every datapoint**, not just an artifact of the metric-name prefix.
  This is a more robust namespace signal than string-parsing `Metric.name` — parsing is
  ambiguous anyway, since the namespace itself contains a `/` (`AWS/RDS`), so
  `amazonaws.com/<namespace-with-a-slash>/<metric-name>` can't be split unambiguously by
  position alone.
- `crate::identity::attr(attrs: &[KeyValue], key: &str) -> Option<&AnyValue>`
  (`identity.rs:65-70`) is generic over any `&[KeyValue]` slice — already reusable for reading a
  datapoint's `Namespace` attribute, not just resource attributes.
- No existing test fixture (`rust/otel-ingestion/tests/fixtures.rs:161-199`'s `sum_metric`/
  `gauge_metric`/`summary_metric` helpers) sets datapoint `attributes` — all built with
  `attributes: vec![]`. This is genuinely new territory; realistic CloudWatch-shaped fixtures
  need to be added from scratch.

## Design

### Fingerprint detection

A resource is treated as a CloudWatch Metric Stream resource when it carries `aws.exporter.arn`
and has no `service.name`/`service.namespace` — i.e., it would otherwise be the exact
degenerate case `is_degenerate_resource` already flags, plus the AWS-specific marker attribute
that lets us safely limit the rewrite to this one producer:

```rust
fn is_cloudwatch_metric_stream_resource(attrs: &[KeyValue]) -> bool {
    identity::attr(attrs, "aws.exporter.arn").is_some()
        && identity::attr(attrs, "service.name").is_none()
        && identity::attr(attrs, "service.namespace").is_none()
}
```

Any `ResourceMetrics` that doesn't match passes through completely untouched — this rewrite is
purely additive for this one AWS-specific shape, not a change to the shared OTLP/metrics path.

### Per-`Metric` namespace lookup

Read the `Namespace` attribute off a `Metric`'s **first** data point, across whichever OTel data
type it holds (`Sum`/`Gauge`/`Histogram`/`ExponentialHistogram`/`Summary` — same match shape as
`metrics_bounds` in `block.rs:114-189`):

```rust
fn metric_namespace(metric: &Metric) -> Option<String> {
    use crate::proto::metric::Data;
    let attrs: &[KeyValue] = match metric.data.as_ref()? {
        Data::Sum(s) => s.data_points.first()?.attributes.as_slice(),
        Data::Gauge(g) => g.data_points.first()?.attributes.as_slice(),
        Data::Histogram(h) => h.data_points.first()?.attributes.as_slice(),
        Data::ExponentialHistogram(h) => h.data_points.first()?.attributes.as_slice(),
        Data::Summary(s) => s.data_points.first()?.attributes.as_slice(),
    };
    identity::attr(attrs, "Namespace").map(identity::attr_to_string)
}
```

A `Metric` with no first data point, or no `Namespace` attribute on it, falls back to a shared
"unknown" bucket (below) rather than being dropped — CloudWatch always sets this per the observed
data, but a missing attribute must never lose the metric.

### Partitioning a matching `ResourceMetrics`

For a `ResourceMetrics` that matches the fingerprint, walk its `scope_metrics`, and for each
`ScopeMetrics`, bucket its `metrics` by `metric_namespace(...)` (`None` → the "unknown" bucket
key). Collect per-namespace `ScopeMetrics` (same `scope`/`schema_url`, filtered `metrics`) into a
`BTreeMap<Option<String>, Vec<ScopeMetrics>>` keyed across the whole `ResourceMetrics` (so a
namespace that appears in more than one original `ScopeMetrics` still ends up as a single
resource's `scope_metrics` list). `BTreeMap` (not `HashMap`) for deterministic iteration order —
matters for reproducible test assertions and stable `block_id`/`payload_bytes` ordering across
runs of the same input.

For each `(namespace, scope_metrics)` entry, build one new `ResourceMetrics`:

```rust
let arn = identity::attr(&original_attrs, "aws.exporter.arn")
    .map(identity::attr_to_string)
    .unwrap_or_default();
let mut resource_attrs = original_attrs.clone(); // cloud.account.id, cloud.provider, cloud.region, aws.exporter.arn
resource_attrs.push(kv("service.instance.id", &arn));
if let Some(ns) = &namespace {
    resource_attrs.push(kv("service.name", ns));
}
```

- `namespace = Some("AWS/RDS")` → `service.name = "AWS/RDS"`, `service.instance.id = <arn>` →
  `exe = "AWS/RDS"` (Option B, exactly as recommended in the issue).
- `namespace = None` (the "unknown" bucket, e.g. Histogram/ExponentialHistogram metrics — which
  the analytics layer doesn't materialize into `measures` yet anyway per
  `mkdocs/docs/otlp/index.md:144`) → only `service.instance.id = <arn>` is added, no `service.name`
  → `exe = ""` still, but `is_degenerate_resource` no longer trips (service.instance.id is set)
  and the process no longer collapses across accounts/regions (Option A's fix, applied as the
  fallback for whatever isn't confidently namespace-attributed).

The resulting `ResourceMetrics` list replaces the original single entry in
`req.resource_metrics` before the request reaches `ingest_parsed_metrics`/`split_metrics` — every
other line of `split_metrics`, `write_blocks`, `process_id_from_resource`, and
`ProcessFromResource::build` handles the rest unchanged, exactly like the CloudWatch Logs
precedent.

### Where the rewrite runs

Inside `ingest_firehose_metrics` only (`handler.rs:370-388`), immediately after
`decode_next_length_delimited` decodes each `ExportMetricsServiceRequest` and before it's handed
to `ingest_parsed_metrics`:

```rust
while let Some(req) =
    decode_next_length_delimited::<ExportMetricsServiceRequest>(&mut buf, Signal::Metrics)...
{
    let req = cloudwatch_metrics::rewrite_cloudwatch_metric_streams(req);
    ingest_parsed_metrics(&service, req).await...
}
```

**Not** inside the shared `ingest_parsed_metrics`/`ingest_metrics` (`handler.rs:168-192`, also
used by the plain `/v1/metrics` OTLP endpoint): CloudWatch Metric Streams can only physically
arrive via Firehose delivery — there is no other transport for this exact resource shape — so
scoping the rewrite to the Firehose-specific call site keeps the shared, generic OTLP metrics path
untouched and avoids any chance of a real (non-CloudWatch) OTLP producer's resource being mutated
by a fingerprint match on the standard endpoint.

### New module: `rust/otel-ingestion/src/cloudwatch_metrics.rs`

Pure, unit-testable, no HTTP/framework dependency — same shape as `cloudwatch_logs.rs`:

- `is_cloudwatch_metric_stream_resource(attrs: &[KeyValue]) -> bool`
- `metric_namespace(metric: &Metric) -> Option<String>`
- `rewrite_cloudwatch_metric_streams(req: ExportMetricsServiceRequest) -> ExportMetricsServiceRequest`
  (the public entry point; iterates `req.resource_metrics`, leaves non-matching entries as-is,
  replaces matching ones in place with their partitioned set)

## Implementation Steps

1. **New module** — add `rust/otel-ingestion/src/cloudwatch_metrics.rs` with
   `is_cloudwatch_metric_stream_resource`, `metric_namespace`, and
   `rewrite_cloudwatch_metric_streams` per Design above. Add `pub mod cloudwatch_metrics;` to
   `rust/otel-ingestion/src/lib.rs`.
2. **Wire into the Firehose metrics path** — call `rewrite_cloudwatch_metric_streams` in
   `ingest_firehose_metrics` (`handler.rs:370-388`), right after
   `decode_next_length_delimited` and before `ingest_parsed_metrics`.
3. **Test fixtures** — add CloudWatch-shaped fixture builders in a new
   `rust/otel-ingestion/tests/cloudwatch_metrics_tests.rs` (not the shared `fixtures.rs`, to keep
   its general-purpose helpers' signatures unchanged): a resource with only
   `cloud.account.id`/`cloud.provider`/`cloud.region`/`aws.exporter.arn`, and metrics whose
   datapoints carry `Namespace`/`MetricName`/`Dimensions` attributes, mirroring the issue's dev
   data (`AWS/RDS`, `AWS/ECS`, `ECS/ContainerInsights`, `AWS/S3`).
4. **Unit tests** (see Testing Strategy) in `cloudwatch_metrics_tests.rs`.
5. **Docs** — extend `mkdocs/docs/otlp/index.md`'s CloudWatch Metric Streams section (around
   line 154) to document the per-namespace process split, the `service.instance.id` = ARN
   folding, and the "unknown"-bucket fallback for metrics without a `Namespace` attribute.
6. **CI** — `cargo fmt`, `cargo clippy --workspace -- -D warnings`,
   `cargo test -p micromegas-otel-ingestion`, then `python3 build/rust_ci.py`.

## Files to Modify

- `rust/otel-ingestion/src/cloudwatch_metrics.rs` — **new**: fingerprint detection, namespace
  lookup, request rewrite.
- `rust/otel-ingestion/src/lib.rs` — `pub mod cloudwatch_metrics;`.
- `rust/otel-ingestion/src/handler.rs` — call the rewrite in `ingest_firehose_metrics`.
- `rust/otel-ingestion/tests/cloudwatch_metrics_tests.rs` — **new** unit tests + fixtures.
- `mkdocs/docs/otlp/index.md` — document the per-namespace split and its fallback.

## Trade-offs

- **Reading the datapoint `Namespace` attribute vs. parsing `Metric.name`.** The issue's
  suggested fix parses the `amazonaws.com/<Namespace>/<MetricName>` prefix; this plan instead
  reads the `Namespace` `KeyValue` attribute already present on every datapoint. Chosen because
  (a) it's already-structured data, not a string format that could change, and (b) the namespace
  itself contains a `/` (`AWS/RDS`), making prefix-parsing genuinely ambiguous without also
  knowing the metric-name suffix scheme for every AWS namespace. Falls back to the "unknown"
  bucket (Option A shape) rather than attempting name-parsing as a secondary source — one
  well-defined extraction path, not two.
- **Partition granularity: per-namespace (Option B) vs. per-stream (A) or per-resource (C).**
  Matches the issue's own recommendation. Per-stream (A) still merges unrelated services (RDS +
  ECS + S3) into one process; per-resource (C) requires regrouping individual datapoints by
  `Dimensions` (a datapoint-level attribute, not resource- or metric-level), tracks fleet size
  1:1, and is deferred as a possible future refinement if per-namespace processes prove too
  coarse in practice.
- **Rewrite scoped to `ingest_firehose_metrics`, not the shared `ingest_parsed_metrics`.** Keeps
  the generic `/v1/metrics` OTLP endpoint's behavior completely unchanged; CloudWatch Metric
  Streams cannot arrive any other way, so there's no coverage gap from scoping it here.
- **`BTreeMap` over `HashMap` for namespace bucketing.** Deterministic iteration order matters
  for reproducible test assertions and stable output ordering across otherwise-identical inputs;
  the extra ordering cost is negligible at CloudWatch's namespace cardinality (single digits per
  stream in practice).
- **`process_id` changes for existing CloudWatch-metrics processes.** Every process derived from
  this route gets a new `process_id` once this ships (old rows keep their old id; new ingestion
  uses the new one) — accepted per `process_id_from_resource`'s own doc comment
  (`identity.rs:1-6`): "Long-term stability of `process_id` values across upgrades is not a
  design goal." Same trade-off already taken for #1386's logs-side fix and prior field additions
  to the hash.

## Documentation

- `mkdocs/docs/otlp/index.md` — CloudWatch Metric Streams section: document the per-namespace
  process split (`exe` = CloudWatch namespace, `service.instance.id` = exporter ARN), and the
  fallback behavior for metrics without a `Namespace` datapoint attribute.

## Testing Strategy

- **Unit (`rust/otel-ingestion/tests/cloudwatch_metrics_tests.rs`, no DB):**
  - `is_cloudwatch_metric_stream_resource`: true for `aws.exporter.arn`-only resource; false when
    `service.name` or `service.namespace` is also present (regression guard against rewriting a
    resource that isn't actually this producer).
  - `metric_namespace`: extracts `Namespace` from a `Sum`/`Gauge` metric's first data point;
    returns `None` when the metric has no data points or the attribute is absent.
  - `rewrite_cloudwatch_metric_streams` on a `ResourceMetrics` with metrics from two namespaces
    (`AWS/RDS`, `AWS/ECS`) → two output `ResourceMetrics`, correct `service.name`/
    `service.instance.id` on each, metrics partitioned correctly (no metric duplicated or
    dropped), a metric with no `Namespace` attribute lands in the "unknown" bucket alongside
    only `service.instance.id`.
  - A non-CloudWatch `ResourceMetrics` (has `service.name` already, or no `aws.exporter.arn`) →
    passed through byte-for-byte unchanged.
  - Full pipeline: rewritten request → `split_metrics` → one `PreparedBlock` per namespace,
    distinct `process_id`s; `ProcessFromResource::build` on each → `exe` equals the expected
    namespace string; two requests with the same `aws.exporter.arn` + same namespace but
    different data → identical `process_id` (idempotent process identity across records/retries).
    Two requests with the same namespace but *different* `aws.exporter.arn` (simulating two
    accounts/regions) → distinct `process_id`s — the specific collapse this issue reports.
- **Regression check:** existing `rust/otel-ingestion/tests/firehose_tests.rs` (synthetic, non-
  CloudWatch-shaped fixtures) must keep passing unmodified — those resources always carry
  `service.name` (or no attributes at all), so the fingerprint never matches and the rewrite is a
  no-op for them.
- **CI:** `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`,
  `python3 build/rust_ci.py`.

## Open Questions

- Is the "unknown" bucket's exact shape (only `service.instance.id` = ARN, no `service.name`) the
  right fallback, or would a placeholder `service.name` (e.g. `"aws.cloudwatch.metrics.unknown"`)
  be more useful for spotting these rows in practice? Low-stakes either way since (per
  `mkdocs/docs/otlp/index.md:144`) Histogram/ExponentialHistogram metrics — the most likely
  reason a `Namespace` attribute would be missing — aren't materialized into `measures` at all
  yet.
- Should `aws.exporter.arn` also be added to `is_degenerate_resource`'s check / a documented
  identity field, so *other* AWS-shaped producers (not just Metric Streams) that key off it get
  the same non-degenerate treatment automatically? Out of scope here — no other such producer
  currently exists in this codebase — but worth flagging if a similar CloudWatch integration is
  added later.
