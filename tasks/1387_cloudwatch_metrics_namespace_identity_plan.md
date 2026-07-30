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
  `req.resource_metrics`. All namespaces in the dev data (RDS+ECS+S3+ContainerInsights) land on
  one `process_id` because they share one degenerate resource — how many `ResourceMetrics` AWS
  packs per delivered message doesn't change that, since every entry with the same degenerate
  resource hashes to the same `process_id` regardless of grouping. So **splitting further
  requires rewriting inside a `ResourceMetrics`**, not just passing more `ResourceMetrics`
  through unchanged.
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
- No existing test fixture (`rust/otel-ingestion/tests/fixtures.rs:161-222`'s `sum_metric`/
  `gauge_metric`/`summary_metric` helpers) sets datapoint `attributes` — all built with
  `attributes: vec![]`. This is genuinely new territory; realistic CloudWatch-shaped fixtures
  need to be added from scratch.

## Design

### Fingerprint detection

A resource is treated as a CloudWatch Metric Stream resource when `is_degenerate_resource`
already flags it (no `host.id`/`host.name`/`process.pid`/`service.instance.id`) **and** it carries
the AWS-specific marker attribute `aws.exporter.arn` with no `service.name`/`service.namespace` —
the combination that lets us safely limit the rewrite to this one producer. All three conjuncts —
`aws.exporter.arn`, `service.name`, `service.namespace` — are gated on emptiness (via
`identity::attr_norm`, trim + lowercase), not on `Option::is_some()`/`is_none()`, to match the
same-strength check `is_degenerate_resource` already applies to its own fields
(`identity.rs:158-163`) — a present-but-empty `StringValue("")` must not be treated as "has a
value" for any of the three: an empty `aws.exporter.arn` is not a real marker (and would otherwise
still leave the resource degenerate, collapsing every account/region onto one `process_id` again —
the exact bug this rewrite exists to fix), and a present-but-empty `service.name`/
`service.namespace` must not be treated as "has a service name" either:

```rust
pub fn is_cloudwatch_metric_stream_resource(attrs: &[KeyValue]) -> bool {
    !identity::attr_norm(attrs, "aws.exporter.arn").is_empty()
        && identity::attr_norm(attrs, "service.name").is_empty()
        && identity::attr_norm(attrs, "service.namespace").is_empty()
        && identity::is_degenerate_resource(attrs)
}
```

`attr_norm` is currently module-private (`fn attr_norm`, `identity.rs:110-114`), so this requires
promoting it to `pub` — a one-word change that makes it a peer of the already-`pub` `attr` /
`attr_to_string` / `is_degenerate_resource` it sits beside. Reimplementing trim+lowercase locally
instead would risk the two checks drifting apart, which is exactly the bug this conjunct fixes.

Any `ResourceMetrics` that doesn't match passes through completely untouched — this rewrite is
purely additive for this one AWS-specific shape, not a change to the shared OTLP/metrics path.

### Per-`Metric` namespace lookup

Read the `Namespace` attribute off a `Metric`'s **first** data point, across whichever OTel data
type it holds (`Sum`/`Gauge`/`Histogram`/`ExponentialHistogram`/`Summary` — same match shape as
`metrics_bounds` in `block.rs:114-189`). Sampling only the first data point is sufficient because
the namespace is constant per `Metric` — CloudWatch encodes it directly in `Metric.name`
(`amazonaws.com/<Namespace>/<MetricName>`, `mkdocs/docs/otlp/index.md:154`), so every data point
under one `Metric` carries the same `Namespace` value in practice; if a first data point ever
lacked the attribute while later ones had it, the whole `Metric` would still route to the
`"AWS/Unknown"` fallback bucket rather than being inspected point-by-point:

```rust
pub fn metric_namespace(metric: &Metric) -> Option<String> {
    use crate::proto::metric::Data;
    let attrs: &[KeyValue] = match metric.data.as_ref()? {
        Data::Sum(s) => s.data_points.first()?.attributes.as_slice(),
        Data::Gauge(g) => g.data_points.first()?.attributes.as_slice(),
        Data::Histogram(h) => h.data_points.first()?.attributes.as_slice(),
        Data::ExponentialHistogram(h) => h.data_points.first()?.attributes.as_slice(),
        Data::Summary(s) => s.data_points.first()?.attributes.as_slice(),
    };
    identity::attr(attrs, "Namespace")
        .map(identity::attr_to_string)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
```

A `Metric` with no first data point, no `Namespace` attribute on it, or an empty/whitespace-only
`Namespace` value falls back to a shared fallback bucket (below) rather than being dropped —
CloudWatch always sets this per the observed data, but a missing attribute must never lose the
metric. The trim-then-filter matters twice over: `identity::attr` returns `Some` whenever the
`KeyValue` has a value at all and `attr_to_string` maps an empty `StringValue` to `""`
(`identity.rs:65-70,80-101`), so without the `.filter(...)` an empty `Namespace` would produce
`Some("")` — a `BTreeMap` key distinct from `None`, yielding a separate block that nonetheless
hashes to the same `process_id` (`attr_norm`, `identity.rs:192-194`) and renders as an empty
`exe`. And the returned value is trimmed (not just tested for emptiness) because it flows
untouched into both the bucket key and `service.name` below — `process_id_from_resource` folds
`service.name` through `attr_norm` (trim + lowercase) before hashing, so an untrimmed
`" AWS/RDS"` and a trimmed `"AWS/RDS"` would otherwise land in two different `BTreeMap` buckets
(two blocks, two conflicting `exe` values) while still hashing to the same `process_id`.

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
set_attr(&mut resource_attrs, "service.instance.id", &arn); // replace-if-present, not push
set_attr(
    &mut resource_attrs,
    "service.name",
    namespace.as_deref().unwrap_or(UNKNOWN_NAMESPACE),
);
clear_attr_if_present(&mut resource_attrs, "service.namespace"); // so exe = service.name, not "{ns}/{service.name}"
```

where `UNKNOWN_NAMESPACE` is the module constant `"AWS/Unknown"` — `service.name` is **always**
set, so `exe` is never empty on this route. The `service.namespace` clear matters even though the
fingerprint already requires `attr_norm(service.namespace)` to be empty to match: `attr_norm` trims
and lowercases before comparing, so a present-but-whitespace-only value (e.g. `"  "`) satisfies the
fingerprint's emptiness gate but is not itself empty. `ProcessFromResource::build`
(`block.rs:444-455`) reads `service.namespace` with raw `attr_to_string` — no trim — and builds
`exe = format!("{svc_ns}/{svc_name}")` whenever `svc_ns` is a non-empty raw string. Left uncleared,
that whitespace-only value would survive into the synthetic resource and produce
`exe = "  /AWS/RDS"` instead of `exe = "AWS/RDS"`. Setting it to `""` guarantees `svc_ns.is_empty()`
is true downstream regardless of what (if anything) the original resource carried.

`clear_attr_if_present` mutates the existing `KeyValue`'s value to `""` in place when the key is
already present and is a no-op otherwise — unlike `set_attr`, it never pushes a new attribute, so
the normal case (CloudWatch resources carry no `service.namespace` at all) leaves `resource_attrs`
without a spurious empty `otel.resource.service.namespace` property. Either outcome still
guarantees `svc_ns.is_empty()` is true downstream.

Every other field on the synthetic `ResourceMetrics`/`Resource` is carried over from the
original unchanged — only `attributes` differs. Concretely: the new `ResourceMetrics.schema_url`
is the original `ResourceMetrics.schema_url`, and the new `Resource`'s
`dropped_attributes_count`/`entity_refs` are the original `Resource`'s
`dropped_attributes_count`/`entity_refs`. `split_metrics` stores `rm.encode_to_vec()` verbatim as
the block payload (`block.rs:376`), so any non-attribute field left as `Default::default()`
instead of copied would be silently lost from the stored proto's fidelity/round-trippability —
not from what the analytics layer reads back: `OtelMetricsBlockProcessor::process`
(`rust/analytics/src/lakehouse/otel/metrics_block_processor.rs:69-85`) only ever touches
`resource_metrics.scope_metrics` when decoding this payload; process identity/properties come
from `src_block.process` metadata written at ingestion, not from the payload's `Resource`.

`set_attr` replaces an existing key's value in place, pushing only when the key is absent — belt
and suspenders alongside the tightened fingerprint above, since `identity::attr` (`identity.rs:
65-70`) is first-match-wins (a duplicate key's second value would be silently ignored on read) and
`ProcessFromResource::build` (`block.rs:483-490`) emits one `otel.resource.*` property per attribute
in the slice with no deduplication (a plain `push` on an already-present key would emit two
`otel.resource.service.instance.id` properties).

- `namespace = Some("AWS/RDS")` → `service.name = "AWS/RDS"`, `service.instance.id = <arn>` →
  `exe = "AWS/RDS"` (Option B, exactly as recommended in the issue).
- `namespace = None` (the "unknown" bucket — e.g. AWS changes or omits the `Namespace` attribute
  on some future data point shape, or a non-CloudWatch OTLP producer happens to set
  `aws.exporter.arn` without setting per-datapoint `Namespace`) → `service.instance.id = <arn>`
  **and** `service.name = "AWS/Unknown"` → `exe = "AWS/Unknown"`. `exe` is never left empty:
  these rows *are* user-visible — CloudWatch Metric Streams' `opentelemetry1.0` output encodes
  every data point as an OTLP `Summary` (`mkdocs/docs/otlp/index.md:407`) and Summary metrics are
  materialized into `measures` (`mkdocs/docs/otlp/index.md:144`) — so an empty `exe` would put
  unattributed metrics behind a blank, unsearchable process name. `"AWS/Unknown"` keeps the same
  `<Prefix>/<Name>` shape as the real namespaces (`AWS/RDS`, `ECS/ContainerInsights`), so the
  bucket sorts and reads naturally next to them in a process list while still being obviously
  distinguishable — AWS publishes no `AWS/Unknown` namespace, so there is no collision with a
  real one. The `service.instance.id` = ARN folding still applies, so `is_degenerate_resource`
  does not trip and the bucket does not collapse across accounts/regions (Option A's fix, applied
  as the fallback for whatever isn't confidently namespace-attributed), and the originating stream
  stays queryable via `otel.resource.aws.exporter.arn` in `processes.properties`
  (`block.rs:483-490`).

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
used by the plain `/v1/metrics` OTLP endpoint): Firehose delivery is the only transport
micromegas supports for this exact resource shape — a collector that relayed a CloudWatch
Metric Streams record over OTLP (e.g. via `awsfirehosereceiver`) to `/v1/metrics` would land
the identical degenerate resource there unfixed, but that path is out of scope for this plan.
Scoping the rewrite to the Firehose-specific call site keeps the shared, generic OTLP metrics
path untouched and avoids any chance of a real (non-CloudWatch) OTLP producer's resource being
mutated by a fingerprint match on the standard endpoint.

### New module: `rust/otel-ingestion/src/cloudwatch_metrics.rs`

Pure, unit-testable, no HTTP/framework dependency — same shape as `cloudwatch_logs.rs`:

- `pub const UNKNOWN_NAMESPACE: &str = "AWS/Unknown"` — the `service.name` fallback so `exe` is
  never empty. Public (rather than private) so `tests/cloudwatch_metrics_tests.rs` can assert
  against it directly, matching the `cloudwatch_logs.rs` precedent (`cloudwatch_logs.rs:21-22`:
  "Public (rather than private) so `tests/cloudwatch_logs_tests.rs` can assert ... directly,
  matching the `build_webhook_request` precedent in `handler.rs`"). This project prefers `pub`
  over `pub(crate)`.
- `pub fn is_cloudwatch_metric_stream_resource(attrs: &[KeyValue]) -> bool` — same rationale:
  public so the unit tests below can call it directly from `tests/cloudwatch_metrics_tests.rs`.
- `pub fn metric_namespace(metric: &Metric) -> Option<String>` — same rationale: public so the
  unit tests below can call it directly from `tests/cloudwatch_metrics_tests.rs`.
- `set_attr(attrs: &mut Vec<KeyValue>, key: &str, value: &str)` — private, replace-if-present;
  builds/replaces with `KeyValue { key: key.to_string(), key_strindex: 0, value: Some(AnyValue
  { value: Some(any_value::Value::StringValue(value.to_string())) }) }`, the same shape as
  `cloudwatch_logs.rs:153-161`'s `kv` helper
- `clear_attr_if_present(attrs: &mut Vec<KeyValue>, key: &str)` — private; sets the existing
  entry's value to `""` in place when `key` is already present, and is a no-op otherwise (never
  pushes) — used to clear `service.namespace` without adding a spurious empty attribute to
  resources that never carried one
- `rewrite_cloudwatch_metric_streams(req: ExportMetricsServiceRequest) -> ExportMetricsServiceRequest`
  (the public entry point; iterates `req.resource_metrics`, leaves non-matching entries as-is,
  replaces matching ones in place with their partitioned set)

## Implementation Steps

1. **New module** — add `rust/otel-ingestion/src/cloudwatch_metrics.rs` with
   `UNKNOWN_NAMESPACE`, `is_cloudwatch_metric_stream_resource`, `metric_namespace`, the private
   replace-if-present `set_attr`, and `rewrite_cloudwatch_metric_streams` per Design above. Add
   `pub mod cloudwatch_metrics;` to `rust/otel-ingestion/src/lib.rs`. Promote `attr_norm` in
   `rust/otel-ingestion/src/identity.rs:110-114` from private to `pub` so the fingerprint can
   reuse it.
2. **Wire into the Firehose metrics path** — call `rewrite_cloudwatch_metric_streams` in
   `ingest_firehose_metrics` (`handler.rs:370-388`), right after
   `decode_next_length_delimited` and before `ingest_parsed_metrics`. Also correct the two
   doc comments this falsifies: `rust/public/src/servers/firehose.rs:13-16`'s module doc
   ("no new identity, block, split, or write logic") and `ingest_firehose_metrics`' own doc
   comment (`handler.rs:362-364`, "Identity, content-addressed `block_id`, and idempotent
   writes are inherited unchanged from the shared split/write path") — both need to describe
   the CloudWatch-specific resource rewrite now inserted at that call site.
3. **Test fixtures** — add CloudWatch-shaped fixture builders in a new
   `rust/otel-ingestion/tests/cloudwatch_metrics_tests.rs` (not the shared `fixtures.rs`),
   following the `cloudwatch_logs_tests.rs` / `block_tests.rs` precedent of test files that
   keep their own fixtures local rather than `mod fixtures;`, keeping CloudWatch-shaped fixtures
   self-contained alongside the CloudWatch-specific tests that use them: a resource with only
   `cloud.account.id`/`cloud.provider`/`cloud.region`/`aws.exporter.arn`, and metrics built as
   `Summary` data points (the shape CloudWatch Metric Streams' `opentelemetry1.0` output actually
   produces, per `mkdocs/docs/otlp/index.md:407`) carrying `Namespace`/`MetricName`/`Dimensions`
   attributes, mirroring the issue's dev data (`AWS/RDS`, `AWS/ECS`, `ECS/ContainerInsights`,
   `AWS/S3`); include one `Sum`/`Gauge`-shaped fixture as well to cover `metric_namespace`'s other
   match arms.
4. **Unit tests** (see Testing Strategy) in `cloudwatch_metrics_tests.rs`.
5. **E2E test** — add a CloudWatch-shaped Firehose delivery test to
   `python/micromegas/tests/test_otlp_e2e.py` (alongside `test_firehose_metrics_e2e` and
   friends, using `FIREHOSE_ENDPOINT`): POST a record whose resource matches the
   fingerprint and whose datapoints carry `Namespace` attributes for two or more
   namespaces, using a synthetic `aws.exporter.arn` that is unique to this test run
   (e.g. suffixed with a fresh uuid, the same run-isolation approach
   `_fresh_resource_attrs()` uses for `service.instance.id` elsewhere in this file — since
   this route overwrites `service.instance.id` with the ARN, the ARN itself must be the
   per-run-unique value). Look up the resulting processes by wrapping the query for
   `processes` filtered on `property_get(properties, 'otel.resource.aws.exporter.arn') =
   '<the run's arn>'` in `assert_eventually` (`python/micromegas/tests/otlp_helpers.py:58+`),
   polling until the expected per-namespace row count is present — a single immediate query can
   race the write path and return 0 or a partial set, the same reason every other e2e test in
   `test_otlp_e2e.py` polls after the 200 ack — rather than `discover_process_id`, which resolves
   `service.instance.id` to a single first-match row and can't disambiguate the N processes this
   one ARN now maps to; once the row count matches, assert the full returned set has one row per
   namespace with `exe` equal to that namespace and distinct `process_id`s. Then, for each
   discovered per-namespace `process_id`, poll (via `assert_eventually`, matching the
   `_post_firehose_and_assert_measures` precedent at `test_otlp_e2e.py:664-698`) that
   `SELECT count(*) FROM measures WHERE process_id = '<that process_id>'` is `>= 1`, so the
   partitioned Summary datapoints' materialization into `measures` and their per-namespace
   process attribution are both covered, not just the `processes` row set.
6. **Docs** — two separate deliverables under `mkdocs/docs/otlp/index.md`'s
   `## CloudWatch Metric Streams (Kinesis Firehose)` section (around line 391 — *not* the
   metric-name paragraph at line 154, which is in `### Metrics → measures`):
   (a) add a **new** `### How CloudWatch namespaces surface` subsection, mirroring the
   `### How logGroup/logStream/owner surface` style (line 526), documenting the per-namespace
   process split, the `service.instance.id` = ARN folding, and the `"AWS/Unknown"` fallback for
   metrics without a `Namespace` attribute. Cross-reference the resource→`processes` mapping
   table (lines 95-103), which documents `exe` derivation.
   (b) **in place**, edit the existing `### Idempotency` subsection (lines 461-472) to note that
   dedup granularity is now per-namespace-block, not per-message, since partitioning turns one
   delivered message into N blocks.
   (c) **in place**, amend the route-intro paragraph at `index.md:402-405` ("hands each one to
   the same split/write path; records land in `measures`, same as native OTLP metrics") with a
   one-line pointer to the new `### How CloudWatch namespaces surface` subsection, since a
   CloudWatch-specific resource rewrite is now inserted before that path runs — the same
   "unchanged shared path" claim step 2 corrects in the two Rust doc comments.
7. **CI** — `cargo fmt`, `cargo clippy --workspace -- -D warnings`,
   `cargo test -p micromegas-otel-ingestion`, then `python3 build/rust_ci.py`.

## Files to Modify

- `rust/otel-ingestion/src/cloudwatch_metrics.rs` — **new**: fingerprint detection, namespace
  lookup, request rewrite.
- `rust/otel-ingestion/src/lib.rs` — `pub mod cloudwatch_metrics;`.
- `rust/otel-ingestion/src/identity.rs` — make `attr_norm` `pub` (currently module-private) so the
  fingerprint's emptiness checks share `is_degenerate_resource`'s exact semantics. No behavior
  change; `is_degenerate_resource` itself is untouched.
- `rust/otel-ingestion/src/handler.rs` — call the rewrite in `ingest_firehose_metrics`; fix
  its doc comment's now-false "inherited unchanged" claim.
- `rust/public/src/servers/firehose.rs` — fix the module doc's now-false "no new identity,
  block, split, or write logic" claim.
- `rust/otel-ingestion/tests/cloudwatch_metrics_tests.rs` — **new** unit tests + fixtures.
- `python/micromegas/tests/test_otlp_e2e.py` — add an e2e test for the CloudWatch-shaped
  Firehose delivery.
- `mkdocs/docs/otlp/index.md` — document the per-namespace split and its fallback.

## Trade-offs

- **Reading the datapoint `Namespace` attribute vs. parsing `Metric.name`.** The issue's
  suggested fix parses the `amazonaws.com/<Namespace>/<MetricName>` prefix; this plan instead
  reads the `Namespace` `KeyValue` attribute already present on every datapoint. Chosen because
  (a) it's already-structured data, not a string format that could change, and (b) the namespace
  itself contains a `/` (`AWS/RDS`), making prefix-parsing genuinely ambiguous without also
  knowing the metric-name suffix scheme for every AWS namespace. Falls back to the `"AWS/Unknown"`
  bucket rather than attempting name-parsing as a secondary source — one well-defined extraction
  path, not two.
- **Partition granularity: per-namespace (Option B) vs. per-stream (A) or per-resource (C).**
  Matches the issue's own recommendation. Per-stream (A) still merges unrelated services (RDS +
  ECS + S3) into one process; per-resource (C) requires regrouping individual datapoints by
  `Dimensions` (a datapoint-level attribute, not resource- or metric-level), tracks fleet size
  1:1, and is deferred as a possible future refinement if per-namespace processes prove too
  coarse in practice.
- **Rewrite scoped to `ingest_firehose_metrics`, not the shared `ingest_parsed_metrics`.** Keeps
  the generic `/v1/metrics` OTLP endpoint's behavior completely unchanged; Firehose is the only
  transport micromegas itself supports for this shape, so within micromegas there's no coverage
  gap from scoping it here — a collector-relayed copy of the same data arriving on `/v1/metrics`
  would keep the degenerate identity, which is accepted as out of scope.
- **`BTreeMap` over `HashMap` for namespace bucketing.** Deterministic iteration order matters
  for reproducible test assertions and stable output ordering across otherwise-identical inputs;
  the extra ordering cost is negligible at CloudWatch's namespace cardinality (single digits per
  stream in practice).
- **Per-message write fan-out.** Partitioning multiplies `write_blocks`' per-`PreparedBlock` work
  (`handler.rs:94-144`) by the namespace count: one message that used to yield a single block
  (one object-store `put` + `register_otel_process`/`register_otel_stream`/`insert_block_typed`)
  now yields one block per namespace bucket, so a message spanning the dev data's 4 namespaces
  becomes 4 object-store PUTs + 12 sequential SQL round-trips instead of 1 + 3. Accepted because
  CloudWatch namespace cardinality per stream is single digits in practice, bounding the
  multiplier; revisit if Firehose delivery-timeout pressure shows up under real load.
- **`process_id` changes for existing CloudWatch-metrics processes.** Every process derived from
  this route gets a new `process_id` once this ships (old rows keep their old id; new ingestion
  uses the new one) — accepted per `process_id_from_resource`'s own doc comment
  (`identity.rs:1-6`): "Long-term stability of `process_id` values across upgrades is not a
  design goal." Same trade-off already taken for prior in-place hash-field additions (e.g. when
  `process.owner` was added, per `identity.rs:181-183`). `block_id` churns too, since
  `split_metrics` derives it from `rm.encode_to_vec()` (`block.rs:376-377`) on the now-rewritten
  `ResourceMetrics`: a batch that partially failed pre-deploy and is retried post-deploy no
  longer dedups against its already-written blocks, producing transient duplicate `measures`
  rows under a new process until the old batch's retry window passes.
- **`is_degenerate_resource` stays unchanged (not extended with `aws.exporter.arn`).** No other
  producer in this codebase keys off `aws.exporter.arn` today (confirmed by grep across
  `rust/*/src`), so there is nothing else to generalize the degenerate-resource check for right
  now; revisit only if a second AWS-shaped producer with the same degenerate-resource problem
  appears.

## Documentation

- `mkdocs/docs/otlp/index.md` — **new** `### How CloudWatch namespaces surface` subsection under
  `## CloudWatch Metric Streams (Kinesis Firehose)` (line 391+), mirroring
  `### How logGroup/logStream/owner surface` (line 526): document the per-namespace process
  split (`exe` = CloudWatch namespace, `service.instance.id` = exporter ARN), and the
  `"AWS/Unknown"` fallback for metrics without a usable `Namespace` datapoint attribute.
- `mkdocs/docs/otlp/index.md` — **in-place edit** to the existing `### Idempotency` subsection
  (lines 461-472): dedup granularity is now per-namespace-block rather than per-message, since
  partitioning turns one delivered message into N blocks.
- `mkdocs/docs/otlp/index.md` — **in-place edit** to the route-intro paragraph at lines 402-405,
  which still claims records "hand each one to the same split/write path" unmodified; add a
  one-line pointer to the new `### How CloudWatch namespaces surface` subsection.

## Testing Strategy

- **Unit (`rust/otel-ingestion/tests/cloudwatch_metrics_tests.rs`, no DB):**
  - `is_cloudwatch_metric_stream_resource`: true for `aws.exporter.arn`-only resource; false when
    `service.name` or `service.namespace` is also present (regression guard against rewriting a
    resource that isn't actually this producer); false when `aws.exporter.arn` is present alongside
    a real `host.name`/`host.id`/`process.pid`/`service.instance.id` (regression guard for the
    fingerprint's `is_degenerate_resource` conjunct — a non-degenerate resource that happens to
    carry `aws.exporter.arn` must not be rewritten; `is_degenerate_resource` itself is unchanged);
    **true** when `service.name`/`service.namespace` are present but hold empty or
    whitespace-only `StringValue`s (pins the `attr_norm`-over-`is_none()` design decision — a
    present-but-empty value must not be treated as "has a service name"); **false** when
    `aws.exporter.arn` is present but empty/whitespace-only (pins the same emptiness gate on the
    marker attribute itself — an empty ARN is not a real marker and must fall through untouched
    rather than match a still-degenerate resource).
  - `metric_namespace`: extracts `Namespace` from a `Summary` metric's first data point — this is
    the arm real CloudWatch traffic exercises, since CloudWatch Metric Streams'
    `opentelemetry1.0` output encodes every data point as an OTLP `Summary`
    (`mkdocs/docs/otlp/index.md:407`) — plus one `Sum`/`Gauge` case for the generic match arms;
    returns `None` when the metric has no data points, the attribute is absent, or its value is
    empty/whitespace-only.
  - `rewrite_cloudwatch_metric_streams` on a `ResourceMetrics` with `Summary` metrics (the shape
    real CloudWatch traffic uses) from two namespaces (`AWS/RDS`, `AWS/ECS`) → two output
    `ResourceMetrics`, correct `service.name`/
    `service.instance.id` on each, metrics partitioned correctly (no metric duplicated or
    dropped), a metric with no `Namespace` attribute lands in the fallback bucket with
    `service.name = "AWS/Unknown"` and `service.instance.id = <arn>` → `exe = "AWS/Unknown"`
    (asserts `exe` is never empty on this route); on a matching resource whose input
    `service.namespace` is present but whitespace-only, the rewritten resource's
    `service.namespace` is cleared and `ProcessFromResource::build` on it yields `exe` equal to
    exactly the namespace string (e.g. `"AWS/RDS"`, not `"  /AWS/RDS"`) — pins the
    `service.namespace`-clearing fix above.
  - A non-CloudWatch `ResourceMetrics` (has `service.name` already, or no `aws.exporter.arn`) →
    passed through byte-for-byte unchanged.
  - The same namespace (e.g. `AWS/RDS`) split across two distinct original `ScopeMetrics` in one
    `ResourceMetrics` → a single output `ResourceMetrics` for that namespace containing both
    scopes' metrics (not two separate `ResourceMetrics`/blocks) — pins the cross-`ScopeMetrics`
    `BTreeMap` merge the Design section specifies; correspondingly, `split_metrics` on the
    rewritten request yields exactly one `PreparedBlock` for that namespace.
  - Full pipeline: rewritten request → `split_metrics` → one `PreparedBlock` per namespace,
    distinct `process_id`s; `ProcessFromResource::build` on each → `exe` equals the expected
    namespace string; two requests with the same `aws.exporter.arn` + same namespace but
    different data → identical `process_id` (idempotent process identity across records/retries).
    Two requests with the same namespace but *different* `aws.exporter.arn` (simulating two
    accounts/regions) → distinct `process_id`s — the specific collapse this issue reports.
    Rewriting the same input record twice → byte-identical output `ResourceMetrics` per namespace
    bucket, so `split_metrics`'s `block_id` (derived from `rm.encode_to_vec()`, `block.rs:376-377`)
    is identical across the two rewrites — pins the `rewrite_cloudwatch_metric_streams`
    determinism that the Design section's `BTreeMap` rationale depends on for the route's
    Firehose-retry dedup guarantee (`mkdocs/docs/otlp/index.md` Idempotency section).
- **Regression check:** `rust/otel-ingestion/tests/firehose_tests.rs` does exercise
  `split_metrics` directly (`use micromegas_otel_ingestion::block::split_metrics;` plus a
  `split_metrics(decoded)` call asserting `blocks.len() == 1`) — it is unaffected by this change
  not because it avoids `split_metrics`, but because (a) it never reaches
  `rewrite_cloudwatch_metric_streams`, which lives in `ingest_firehose_metrics`
  (`handler.rs:370-388`), one layer above where this test calls `split_metrics` directly, and (b)
  its fixture (`fixtures::make_metrics_request`, `fixtures.rs:134-158`) sets `service.name`/
  `host.name`/`process.pid`, so `is_cloudwatch_metric_stream_resource` would return `false` on it
  even if the rewrite were in scope. The guarantee that the rewrite is a no-op for
  non-CloudWatch-shaped resources is instead pinned by the "passed through byte-for-byte
  unchanged" unit test above.
- **E2E (`python/micromegas/tests/test_otlp_e2e.py`, against real services):** a
  CloudWatch-shaped Firehose delivery, tagged with a per-run-unique synthetic
  `aws.exporter.arn`, with two or more namespaces produces one `processes` row per
  namespace, `exe` equal to the namespace, and distinct `process_id`s — queried via
  `assert_eventually` (`otlp_helpers.py:58+`) polling `property_get(properties,
  'otel.resource.aws.exporter.arn')` matching the run's ARN until the expected per-namespace
  row count appears, then asserting on the full returned row set (not `discover_process_id`,
  which returns only a single first-match row and can't distinguish the N processes one ARN now
  maps to, and not a one-shot query, which can race the write path the same way every other
  polled e2e test in this file does) — the only test that exercises `write_blocks` →
  `register_otel_process` end to end for this route, rather than stopping at in-memory
  `PreparedBlock`/`ProcessFromResource` structs. For each discovered per-namespace `process_id`,
  also poll (`assert_eventually`, matching `_post_firehose_and_assert_measures`,
  `test_otlp_e2e.py:664-698`) that `measures` has `count(*) >= 1` for that `process_id`, so the
  Summary datapoints' materialization into `measures` — not just `processes` row creation — is
  covered under the new per-namespace attribution.
- **CI:** `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`,
  `python3 build/rust_ci.py`.
