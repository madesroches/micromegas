# OTLP Ingestion

Micromegas accepts native OpenTelemetry Protocol (OTLP) traffic over HTTP alongside its custom transit/CBOR wire format. Any OTel-instrumented program — Claude Code, Goose, generic OTel SDKs (Python, Go, JS, .NET, Java) — can point `OTEL_EXPORTER_OTLP_ENDPOINT` at the ingestion service and have logs, metrics, and spans land in the lakehouse.

## Overview

The ingestion service exposes the following HTTP ingestion routes. The first three mirror the OpenTelemetry specification directly; the rest accept non-OTLP payloads (Kinesis Firehose deliveries) and translate them internally:

| Route | Payload | Lands in |
|---|---|---|
| `POST /ingestion/otlp/v1/logs` | `ExportLogsServiceRequest` | `log_entries` |
| `POST /ingestion/otlp/v1/metrics` | `ExportMetricsServiceRequest` | `measures` |
| `POST /ingestion/otlp/v1/traces` | `ExportTraceServiceRequest` | `otel_spans` (per-process JIT view) |
| `POST /ingestion/otlp/v1/metrics/firehose` | one-or-more length-delimited `ExportMetricsServiceRequest` messages per Firehose record | `measures` (see [CloudWatch Metric Streams](#cloudwatch-metric-streams-kinesis-firehose)) |
| `POST /ingestion/cloudwatch/v1/logs/firehose` | CloudWatch Logs subscription-filter record per Firehose record (**not OTLP-framed** — see [CloudWatch Logs](#cloudwatch-logs-kinesis-firehose)) | `log_entries` |

Routes share the existing listener (default `127.0.0.1:9000`) and authentication chain. OTLP payloads are stored as-is in object storage; decoding into parquet rows happens lazily at the analytics layer.

**Wire format:** OTLP/HTTP with `Content-Type: application/x-protobuf` or `Content-Type: application/json`. Optional `Content-Encoding: gzip` is supported. gRPC OTLP is not supported in the current release.

## Quick Start

Point an OTel SDK at the ingestion service:

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="http://127.0.0.1:9000/ingestion/otlp"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
```

The SDK appends `/v1/{logs,metrics,traces}` to the base URL per the OTLP spec, so a request lands on `http://127.0.0.1:9000/ingestion/otlp/v1/logs`. If your operator has set per-signal endpoints (`OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`), those are full URLs and need to include the `/v1/<signal>` suffix themselves.

For a production deployment with auth, see [Authentication](#authentication) below.

## Authentication

The OTLP routes share the same auth chain as the rest of the ingestion service:
DB-backed `ingestion_api_keys` (the steady-state path — mint one via
`POST /api/ingestion-api-keys` on `analytics-web-srv`, see
[API Keys](../admin/api-keys.md)), transitional env-keyring bearer tokens
(`MICROMEGAS_API_KEYS`), and OIDC.

OTel SDKs read `OTEL_EXPORTER_OTLP_HEADERS` and attach the parsed headers to every export request:

```bash
# Server side — mint a key (see admin/api-keys.md), or use the transitional
# env keyring telemetry-ingestion-srv also accepts:
export MICROMEGAS_API_KEYS='[{"name":"team-platform","key":"mmk_2f8c...base64url..."}]'

# Client side
export OTEL_EXPORTER_OTLP_ENDPOINT="https://micromegas.example.com/ingestion/otlp"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer mmk_2f8c...base64url..."
```

If different signals need different keys, use the per-signal headers variants:

```bash
export OTEL_EXPORTER_OTLP_LOGS_HEADERS="Authorization=Bearer key-for-logs"
export OTEL_EXPORTER_OTLP_TRACES_HEADERS="Authorization=Bearer key-for-traces"
```

Per-signal headers override the catch-all.

!!! warning "TLS in production"
    Bearer tokens over plaintext leak in transit. Run the listener behind an HTTPS-terminating load balancer (or terminate TLS in-process via `axum_server::tls_rustls`). Plaintext is fine for localhost development only.

!!! note "Variable expansion"
    OTel SDKs do **not** expand `${VAR}` inside `OTEL_EXPORTER_OTLP_HEADERS`. Your shell expands those at `export` time. Config-file deployments that read headers from a JSON/YAML file need pre-substituted values or a wrapper script.

## Process identity

OTLP has no "process" concept; it has a `Resource` (key/value attributes) attached to each batch. Micromegas synthesizes a stable `process_id` by hashing the OS-honest identifying tuple together with the OTel service identity:

```
process_id = uuid_v5(NS_OTEL_PROCESS_V1,
    host.id · host.name ·
    process.pid · process.creation.time ·
    service.namespace · service.name · service.instance.id · process.owner ·
    os.type · os.version · os.name · os.description · os.build_id ·
    host.arch · host.type ·
    host.image.id · host.image.name · host.image.version ·
    host.cpu.model.id · host.cpu.model.name · host.cpu.family ·
    host.cpu.vendor.id · host.cpu.stepping · host.cpu.cache.l2.size ·
    service.version ·
    telemetry.sdk.name · telemetry.sdk.language · telemetry.sdk.version ·
    process.runtime.name · process.runtime.version · process.runtime.description)
```

`·` denotes `\x1F` (ASCII unit separator). All fields pass through lower-case + trim except
`process.pid` and `process.creation.time` which are used verbatim. Missing fields are treated
as empty strings.

The formula was extended in-place under the same `NS_OTEL_PROCESS_V1` namespace UUID —
re-deriving existing `process_id`s is always acceptable, so no namespace bump is needed.
In-flight processes receive a new `process_id` on their next batch; existing rows are unaffected
and decay under the normal retention policy.

The first time a `process_id` is observed, a row is inserted into `processes` with these mappings:

| OTel attribute | Process column |
|---|---|
| `service.name` (or `service.namespace + "/" + service.name`) | `exe` |
| `host.name` | `computer` |
| `user.name` | `username` / `realname` |
| `os.description` | `distro` |
| `host.cpu.model.name` | `cpu_brand` |
| `process.creation.time` (or first event time) | `start_time` |
| Everything else | `process.properties.otel.resource.*` |

`tsc_frequency` is set to `1_000_000_000` so ticks ≡ Unix nanoseconds — OTel timestamps pass through the existing tick-to-time conversion as identity.

### Stream identity

One stream per signal per process (max 3 streams per process):

```
stream_id = uuid_v5(NS_OTEL_STREAM_V1, process_id + "\x1F" + signal)
```

Stream tags reuse the existing micromegas vocabulary:

| Signal | Stream tag | Stream format |
|---|---|---|
| logs | `"log"` | `otlp/v1/logs` |
| metrics | `"metrics"` | `otlp/v1/metrics` |
| traces | `"trace"` | `otlp/v1/traces` |

The `streams.format` column (added in data-lake schema v4) tells the analytics layer which decoder to use per block; tags carry signal/purpose. `log_entries` and `measures` materialize blocks from both native and OTel streams uniformly.

## Schema mapping

### Logs → `log_entries`

| OTel field | parquet column |
|---|---|
| `time_unix_nano` (or `observed_time_unix_nano` if zero) | `time` |
| `severity_number` 1–24 | `level` (collapsed to the Micromegas `Level` enum: TRACE 1–4 → `6`, DEBUG 5–8 → `5`, INFO 9–12 → `4`, WARN 13–16 → `3`, ERROR 17–20 → `2`, FATAL 21–24 → `1`) |
| `body.string_value` | `msg` |
| `body.kvlist_value` / `array_value` | JSON-stringified into `msg` |
| `attributes.*` | `properties` |
| `instrumentation_scope.name` | `target` |
| `trace_id`, `span_id` | `properties.otel.trace_id` / `otel.span_id` |
| `severity_text` | `properties.otel.severity_text` |

Scope identity (`name`, `version`, `schema_url`) and scope attributes land on per-row `properties` under the `otel.scope.*` prefix.

### Metrics → `measures`

Sum, Gauge, and Summary data points are materialized. Sum/Gauge land as a single row each. Each Summary data point fans out into up to four rows — one per statistic (count, sum, min, max) — distinguished by a suffix on the metric **name** rather than a `properties` tag: `<metric>_count`, `<metric>_sum`, `<metric>_min`, `<metric>_max`. Any other `quantile_values` entry (a configured percentile like p90/p99) is skipped with a debug log, same as Histogram/ExponentialHistogram, which remain skipped entirely — a histogram-aware schema is future work. A Summary point flagged `NO_RECORDED_VALUE` is dropped whole, since `count`/`sum` are non-optional proto scalars and materializing it would inject real `0.0` samples where the series has a gap; an *unflagged* `count = 0` point is a genuine observation and is still materialized.

| OTel field | parquet column |
|---|---|
| `name` (Sum/Gauge), or `name` + `_count`/`_sum`/`_min`/`_max` (Summary) | `name` |
| `unit` (`_count` rows always get `unit = ""`) | `unit` |
| `value` (int widened to f64) | `value` |
| `time_unix_nano` | `time` |
| `aggregation_temporality`, `is_monotonic`, `otel.metric.kind` (Sum/Gauge only — Summary rows add no derived `otel.metric.*` extras, though per-point attributes still populate `properties` same as Sum/Gauge) | `properties` |

CloudWatch Metric Streams' `opentelemetry1.0` output prefixes every metric name with `amazonaws.com/<Namespace>/`, so a `CPUUtilization` metric on `AWS/EC2` arrives as `amazonaws.com/AWS/EC2/CPUUtilization` — the examples below use the real, prefixed names.

**Selecting one CloudWatch statistic:**

```sql
SELECT time, value
FROM measures
WHERE name = 'amazonaws.com/AWS/EC2/CPUUtilization_max'
```

**Grouping all statistics for a metric:**

```sql
SELECT time, name, value
FROM measures
WHERE name LIKE 'amazonaws.com/AWS/EC2/CPUUtilization\_%' ESCAPE '\'
```

### Traces → `otel_spans`

`otel_spans` is a **per-process JIT view** — query it as `view_instance('otel_spans', '<process_id>')`. There is no global instance in the current release; cross-process trace traversal requires UNION-ing across the participating processes.

See [Schema Reference: `otel_spans`](../query-guide/schema-reference.md#otel_spans) for the full column list.

## Attribute encoding

OTel `KeyValue.value` is an `AnyValue` oneof. JSONB encoding:

| OTel `AnyValue` variant | JSONB representation |
|---|---|
| `string_value` | JSON string |
| `bool_value` | JSON bool |
| `int_value` (i64) | JSON number |
| `double_value` | JSON number (f64) |
| `bytes_value` | base64-encoded JSON string |
| `array_value` | JSON array, recursively encoded |
| `kvlist_value` | JSON object, recursively encoded |

Nested structures are preserved. Query-time access uses the existing `jsonb_*` UDFs:

```sql
SELECT jsonb_as_string(jsonb_get(properties, 'otel.scope.name'))
FROM log_entries
WHERE process_id = '...';
```

## HTTP semantics

| Concern | Behavior |
|---|---|
| Body limit | 20 MiB compressed (matches the OTel Collector's default `confighttp.max_request_body_size`) |
| Compression | `Content-Encoding: gzip` supported; other codecs return `415` |
| Content-Type | `application/x-protobuf` or `application/json` (parameters like `; charset=utf-8` accepted); other types return `415` |
| Empty top-level request | `200 OK` with empty `Export*ServiceResponse` body, no rows written (per spec) |
| Success | `200 OK`, response `Content-Type` mirrors the request encoding; body is an empty `Export*ServiceResponse` |
| Parse error | `400 Bad Request`, body is a `google.rpc.Status` proto with `code = INVALID_ARGUMENT (3)` |
| Auth failure | `401 Unauthorized`, body is `google.rpc.Status` |
| Body too large | `413 Payload Too Large`, body is `google.rpc.Status` |
| Unsupported media type | `415 Unsupported Media Type`, body is `google.rpc.Status` |
| Backend transient failure | `503 Service Unavailable` with `Retry-After: 30` header, body is `google.rpc.Status` (retryable per spec) |

Per the OTLP spec, error responses always carry a `google.rpc.Status` proto, **not** an `Export*ServiceResponse`.

## Idempotency

Block IDs are content-addressed: `block_id = uuid_v5(NS_OTEL_BLOCK_V1, payload_bytes)`. The block's payload object is written create-only — a retried POST that hashes to the same `block_id` finds the object already present and leaves it untouched (first write wins), and the row insert still collides on `ON CONFLICT (block_id) DO NOTHING` and adds no rows. This makes the OTLP endpoints safe to retry on transient errors without double-counting or corrupting a previously stored payload. Both the object collision and the row conflict are counted (`block_object_duplicate`), so a sustained rate of duplicates is visible to operators rather than silent.

!!! warning "Content-hash dedup needs a distinguishing payload, not just a distinguishing event"
    Content-addressing dedups on the *bytes actually stored*, not on any identity the
    source system assigns the event. A producer whose transformed payload doesn't vary
    per event — e.g. an EventBridge `input_transformer` forwarding periodic events with
    no per-event identity in the projected fields and a constant/null message body — can
    produce byte-identical records for genuinely distinct events, which then collide on
    `block_id` and get discarded as duplicates: the producer gets no error and its event
    is simply dropped, indistinguishable from a harmless retry of the same event. As
    described above, this is visible to operators today via the `block_object_duplicate`
    metric and a server-side warning log — it isn't silent from that side. It was silent
    when [issue #1462](https://github.com/madesroches/micromegas/issues/1462) hit,
    though: at the time, the dedup-drop path only logged at `debug!` with no metric, so
    the loss went unnoticed until traced back after the fact — 72 distinct EventBridge
    events lost in a single measured 2-day window (the loss itself ran undetected for
    about two months). Declaring
    [`aws.event.id`](#event-identity-awseventid-awseventtime) breaks the collision by
    making every record's bytes depend on the source event's own identity. Once
    [issue #1466](https://github.com/madesroches/micromegas/issues/1466) — an open design
    proposal for dedup on a producer-declared idempotency key, rather than a content hash
    — lands, `aws.event.id` would be a concrete example of the kind of declared key it's
    asking for; today it only avoids the collision, since dedup is still purely
    content-hash based.

## Client recipes

### Claude Code

```bash
export CLAUDE_CODE_ENABLE_TELEMETRY=1
export OTEL_EXPORTER_OTLP_ENDPOINT="https://micromegas.example.com/ingestion/otlp"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
export OTEL_METRICS_EXPORTER=otlp
export OTEL_LOGS_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer mmk_2f8c...base64url..."

# Optional — distributed tracing (Claude Code beta)
export CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1
export OTEL_TRACES_EXPORTER=otlp

# Optional — multi-team rollups via resource attributes
export OTEL_RESOURCE_ATTRIBUTES="team.id=platform,deployment.environment=prod"

claude
```

After Claude runs once, verify on the server:

```sql
SELECT process_id, exe, computer,
       jsonb_as_string(jsonb_get(properties, 'otel.resource.service.instance.id')) AS instance
FROM processes
WHERE jsonb_as_string(jsonb_get(properties, 'otel.resource.service.name')) = 'claude-code'
ORDER BY start_time DESC LIMIT 5;

SELECT count(*) FROM log_entries
WHERE process_id IN (
    SELECT process_id FROM processes
    WHERE jsonb_as_string(jsonb_get(properties, 'otel.resource.service.name')) = 'claude-code'
);
```

### Python OTel SDK

```python
import os

os.environ["OTEL_EXPORTER_OTLP_ENDPOINT"] = "http://127.0.0.1:9000/ingestion/otlp"
os.environ["OTEL_EXPORTER_OTLP_PROTOCOL"] = "http/protobuf"

from opentelemetry import trace
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor

resource = Resource.create({"service.name": "my-service", "service.instance.id": "i-1"})
provider = TracerProvider(resource=resource)
provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))
trace.set_tracer_provider(provider)

tracer = trace.get_tracer(__name__)
with tracer.start_as_current_span("hello"):
    pass
```

### Go OTel SDK

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="http://127.0.0.1:9000/ingestion/otlp"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
export OTEL_SERVICE_NAME="my-service"
```

Then use `otlptracehttp.New(ctx)` (or the equivalent for logs/metrics) — it picks up the env vars.

## OTLP/JSON & EventBridge API Destinations

AWS EventBridge API Destinations send `Content-Type: application/json; charset=utf-8` by default, which is accepted by the ingestion server. Use an input transformer to produce the full `ExportLogsServiceRequest` envelope:

```json
{
  "resourceLogs": [{
    "resource": { "attributes": [{"key": "service.name", "value": {"stringValue": "<$.source>"}}] },
    "scopeLogs": [{
      "scope": {"name": "eventbridge"},
      "logRecords": [{
        "timeUnixNano": "<$.time_ns>",
        "severityNumber": 9,
        "body": {"stringValue": "<$.detail.message>"},
        "attributes": [{"key": "aws.event.id", "value": {"stringValue": "<$.id>"}}]
      }]
    }]
  }]
}
```

`timeUnixNano` must be a **quoted string** in the template (e.g. `"<$.time_ns>"`). EventBridge input transformers substitute variables as strings inside quotes, satisfying the OTLP/JSON spec requirement. No Lambda translation layer is needed.

### Event identity: `aws.event.id` / `aws.event.time`

Forward the source EventBridge event's `$.id` as a record-level attribute named
`aws.event.id`, as shown in the template above — the same `<...>` quoted-string
substitution used for `timeUnixNano`. This mirrors the `aws.log.event.id` convention
already documented for [CloudWatch Logs](#how-loggrouplogstreamowner-surface): it lets
a `log_entries` row be correlated back to the exact EventBridge event, and it's queryable
via `properties` like any other OTel attribute.

Declaring `aws.event.id` matters because when an `input_transformer` can't produce a
per-event identity in the payload body itself, distinct events can end up with
byte-identical stored records and collide on the content-hash `block_id` — see the note
in [Idempotency](#idempotency) above.

When an `input_transformer` can't produce nanosecond time for the event's native
timestamp shape, three levels of fallback apply, in order:

1. `timeUnixNano` — from `$.time_ns`, if the producer's template sets it (as shown above).
2. `observedTimeUnixNano` — if the producer explicitly sets it; the template shown above
   does not.
3. The block's `begin_time`, if both of the above are absent/zero. When every record in
   the `ResourceLogs` lacks a timestamp, `begin_time` is the block's ingestion arrival
   time (`Utc::now()` at block-split time) — the same arrival-time fallback the
   [Webhook ingestion](#webhook-ingestion) section documents for a different producer
   path, where each request always produces exactly one record. If sibling records in
   the same resource do carry timestamps, `begin_time` is instead the earliest of those,
   so a timestamp-less record inherits its earliest sibling's event time rather than
   arrival time.

See also [Schema mapping](#schema-mapping) for the two-level
`time_unix_nano`/`observed_time_unix_nano` → `time` column rule that step 1 → 2 above
maps onto before the block-level fallback in step 3 kicks in.

Optionally, also forward `$.time` verbatim as a companion `aws.event.time` string
attribute (EventBridge's `$.time` is second-resolution ISO-8601). This is useful for two
distinct reasons. First, it preserves the source event's original occurrence-time string
verbatim as provenance — a record of what the producer actually sent — even when
`timeUnixNano` is also set and used for `log_entries.time`. Second, and separately: if
the template sets neither `timeUnixNano` nor `observedTimeUnixNano`, the record falls
through to step 3 above and its stored `time` becomes the block's `begin_time` — the
ingestion arrival time only if every record in that resource is timestamp-less, and
otherwise a sibling record's event time — either way unrelated to when this particular
event actually occurred; in that case `aws.event.time` is the only remaining record of
the event's real occurrence time.

## Webhook ingestion

`POST /ingestion/webhook` accepts a raw webhook delivery from any header-capable
producer (GitLab, GitHub, a generic SaaS) with **no per-source configuration on the
server**. It synthesizes an OTLP `Resource` from three request headers and stores the
request body as a single log record's body, reusing the OTLP logs identity/block/write
path end-to-end — the same auth, body-limit, and idempotency rules described above
apply unchanged. Since `log_entries.msg` is `Utf8`-typed, a valid-UTF8 body (the common
case: JSON payloads from GitLab/GitHub/etc.) is stored verbatim. There is no header to
describe an alternate codec, so a non-UTF8 body is stored via lossy UTF-8 conversion
(invalid byte sequences become `U+FFFD`) rather than rejected or stored as opaque
binary.

| Header | Maps to | Result |
|---|---|---|
| `X-Micromegas-Service-Name` | resource `service.name` | `processes.exe` / `log_entries.exe` |
| `X-Micromegas-Service-Namespace` | resource `service.namespace` | folded into `exe` as `namespace/name`, and into `process_id` |
| `X-Micromegas-Target` | instrumentation scope name | `log_entries.target` |

All three headers are optional — a missing header behaves like an OTLP resource
that omits the attribute. The body is never parsed or validated server-side; an
empty body returns `400 Bad Request` (nothing to store). `Content-Type` is not
negotiated — send whatever the producer sends (typically `application/json`).

Because no per-record timestamp is known, the record's `time_unix_nano` /
`observed_time_unix_nano` both stay 0 in the stored payload, and `log_entries.time` for
such a record falls back to the block's `begin_time` — the server's ingestion wall-clock
time at the moment the resource was split into a block, not a value carried inside the
record itself. Retried deliveries dedup via the same content-addressed `block_id` scheme
described in [Idempotency](#idempotency), with a webhook-specific wrinkle:

- **`block_id` is hashed from the *full* incoming header set, not just the 3 recognized
  ones.** Only `X-Micromegas-Service-Name`/`-Service-Namespace`/`-Target` become resource
  attrs, but a producer-specific header this endpoint doesn't otherwise interpret (a
  GitLab delivery UUID, a GitHub event-type header, a signature) still changes `block_id`
  if it differs — otherwise two unrelated deliveries with byte-identical bodies but
  different unrecognized headers would collide and dedup as if they were retries of each
  other. The flip side: a genuine retry that picks up a new value for some header along
  the way (e.g. a proxy stamping a fresh `Date` or request-id on each hop) is no longer
  deduped, since that header now participates in the hash too.

Leaving both timestamps at 0 in the stored record (rather than backfilling a timestamp
before storing, as an earlier version of this endpoint did) keeps the stored payload byte-
identical across retries, which is what lets `block_id` — hashed from those same bytes —
double as a create-only write key: the arrival-time fallback lives entirely in the block's
`begin_time`/`end_time`, never in the record.

### GitLab example

Configure a GitLab group or project webhook to point at the endpoint, with the three
custom headers set once in the webhook configuration:

```
URL:     https://micromegas.example.com/ingestion/webhook
Headers: X-Micromegas-Service-Name: gitlab
         X-Micromegas-Service-Namespace: my-group
         X-Micromegas-Target: gitlab.push
         Authorization: Bearer mmk_2f8c...base64url...
```

Every push/merge-request/pipeline event GitLab sends lands as one `log_entries` row
with `target = 'gitlab.push'`, `exe = 'my-group/gitlab'`, and `msg` equal to the raw
JSON payload GitLab sent.

### Querying the stored body

The body is opaque JSON text in `msg`; parse it at query time with the `jsonb_*` UDFs
(`jsonb_parse`, `jsonb_get`, `jsonb_as_i64`, `jsonb_array_length`,
`jsonb_path_query_first` for nested/dotted access — there is no dotted-path variant of
`jsonb_get`):

```sql
SELECT
  jsonb_as_string(jsonb_get(jsonb_parse(msg), 'object_kind')) AS kind,
  jsonb_as_i64(jsonb_path_query_first(jsonb_parse(msg), '$.object_attributes.iid')) AS iid,
  jsonb_array_length(jsonb_get(jsonb_parse(msg), 'commits')) AS nb_commits
FROM log_entries
WHERE target = 'gitlab.push'
ORDER BY time DESC
LIMIT 10;
```

## CloudWatch Metric Streams (Kinesis Firehose)

`POST /ingestion/otlp/v1/metrics/firehose` speaks the **Amazon Kinesis Data Firehose HTTP
Endpoint Delivery** protocol, so a CloudWatch Metric Stream can push metrics straight into
micromegas: **Metric Stream → Firehose → micromegas**, with no Lambda, no Kinesis Data
Stream, and no collector process in between. Firehose is just a dumb managed pipe: it
wraps each record in a small JSON envelope and expects a fixed ack shape back.

This works because a Metric Stream configured with **OpenTelemetry 1.0.0** output format
delivers each record as one-or-more length-delimited OTLP `ExportMetricsServiceRequest`
protobuf messages (each prefixed with a varint byte length, back to back) — the same
message type the native `/ingestion/otlp/v1/metrics` route already decodes. The Firehose
route unwraps the envelope (gzip-aware, base64 records), decodes every length-delimited
message in a record, rewrites the CloudWatch-specific resource shape (see
[How CloudWatch namespaces surface](#how-cloudwatch-namespaces-surface) below), then hands
each one to the same split/write path; records land in `measures`, same as native OTLP
metrics.

`opentelemetry1.0` output encodes every CloudWatch data point as an OTLP `Summary`, so each
scrape of a metric lands as **4 rows under 4 distinct names** (`<metric>_count`, `_sum`,
`_min`, `_max`) rather than 1 row under the base name — see
[Metrics → `measures`](#metrics-measures) above. Any additional percentile statistics
configured via `statistics_configuration` (p90, p99, ...) are not materialized.

### Requirement: OpenTelemetry 1.0.0 output format

The Metric Stream **must** be configured with `OutputFormat: opentelemetry1.0` (or the
equivalent console option). Other output formats (JSON, Parquet) are not OTLP and are not
supported by this endpoint.

### AWS delivery-stream setup

Configure a Kinesis Firehose delivery stream with an **HTTP endpoint destination**:

- **HTTP endpoint URL**: `https://micromegas.example.com/ingestion/otlp/v1/metrics/firehose`
- **Access key**: any live `ingestion_api_keys` key (see [API Keys](../admin/api-keys.md)), or, transitionally, a value from `MICROMEGAS_API_KEYS` — sent by
  Firehose as `X-Amz-Firehose-Access-Key` on every request (Firehose cannot send
  `Authorization: Bearer`, so this route authenticates via that header instead, reusing
  the same keyring check as every other ingestion route).
- **Content encoding**: gzip (recommended — reduces wire bytes; the route decompresses
  transparently, same as the other OTLP routes).
- **Buffering hints**: tune buffer size/interval for your metric volume; every buffered
  batch arrives as one HTTP POST carrying one-or-more JSON records, and each JSON record's
  data may itself pack multiple length-delimited OTLP messages.
- **S3 backup**: configure "backup all records" or "backup failed data only" — Firehose
  retries non-200 responses and eventually spills to the configured S3 bucket, so no data
  is silently lost even during an extended micromegas outage.

Then point a CloudWatch Metric Stream at the delivery stream, with output format set to
OpenTelemetry 1.0.0.

### How CloudWatch namespaces surface

A CloudWatch Metric Stream's `Resource` carries no `service.*`/`host.*`/`process.*` identity
at all — only `cloud.account.id`, `cloud.provider`, `cloud.region`, and `aws.exporter.arn`.
Left as-is, every stream from every AWS account/region would hash to the same fully
degenerate `process_id` (see [Process identity](#process-identity) above) and `exe` would be
empty. Before handing a decoded message to the shared split/write path, this route detects
that exact fingerprint and rewrites it:

- Every metric's **first** data point carries a `Namespace` attribute (`AWS/RDS`, `AWS/ECS`,
  `ECS/ContainerInsights`, `AWS/S3`, …) — CloudWatch encodes this directly in `Metric.name`
  too (`amazonaws.com/<Namespace>/<MetricName>`), so the value is stable across all of a
  metric's data points. The rewrite reads it straight from the datapoint attribute rather
  than parsing `Metric.name`, since the namespace itself contains a `/`, making prefix-parsing
  ambiguous.
- The metrics in one delivered message are partitioned by namespace into one synthetic
  `Resource` per namespace: `service.name` = the namespace string, so `exe` (see the
  resource→`processes` mapping table under [Process identity](#process-identity)) equals the
  namespace exactly — `AWS/RDS`, `AWS/ECS`, and so on each resolve to their own `process_id`.
- `service.instance.id` is set to the record's `aws.exporter.arn`, folding the exporting
  stream's identity into the hash so distinct AWS accounts/regions never collapse onto the
  same `process_id` even when they share a namespace.
- A metric with no usable `Namespace` attribute (missing, or empty/whitespace-only) falls
  back to `service.name = "AWS/Unknown"` rather than being dropped — `exe` is never left
  empty on this route, since these rows are still fully queryable via `measures`.

One delivered message can therefore fan out into several blocks — one per namespace bucket —
where it used to produce exactly one; see [Idempotency](#idempotency_1) below for what that
means for retry dedup.

### Ack contract

Success is `200 OK` with `Content-Type: application/json` and body:

```json
{"requestId": "<echoed from X-Amz-Firehose-Request-Id>", "timestamp": 1700000000000}
```

Any non-200 status triggers a Firehose retry, and body:

```json
{"requestId": "<echoed>", "timestamp": 1700000000000, "errorMessage": "..."}
```

`requestId` always echoes the `X-Amz-Firehose-Request-Id` header — this is required by the
Firehose HTTP Endpoint Delivery contract.

!!! warning "TLS in production"
    Same as the Bearer OTLP routes: `X-Amz-Firehose-Access-Key` over plaintext leaks in
    transit. Terminate TLS in front of the listener for any production delivery stream.

### Idempotency

Same content-addressed `block_id` scheme as the rest of OTLP ingestion (see
[Idempotency](#idempotency)): a Firehose retry of a previously-succeeded batch
re-computes identical `block_id`s and dedups on write. On a partial batch failure,
Firehose retries the whole batch — already-written **messages** dedup, not just
already-written records: each length-delimited message within a record is decoded and
written as soon as it's read, so a malformed message partway through a record still
leaves every message before it in that record ingested, while that message and the rest
of the record (not yet reached) are retried along with the whole batch. CloudWatch
Metric Streams stamp distinct timestamps per scrape, so genuinely distinct data never
collides.

Dedup granularity is per-**namespace-block**, not per-message: the [namespace
partitioning](#how-cloudwatch-namespaces-surface) above turns one decoded message into one
block per CloudWatch namespace, each with its own content-addressed `block_id` derived from
that namespace's own (post-rewrite) bytes. A retry reproduces the same partitioning
deterministically, so every namespace's block still dedups independently — a retried message
doesn't need every one of its namespace blocks to have failed identically, just each one that
did.

## CloudWatch Logs (Kinesis Firehose)

`POST /ingestion/cloudwatch/v1/logs/firehose` speaks the same **Amazon Kinesis Data
Firehose HTTP Endpoint Delivery** protocol as the metrics route above, but for
**CloudWatch Logs subscription filters**: **CloudWatch Logs → subscription filter →
Firehose → micromegas**, with no Lambda, no Kinesis Data Stream, and no collector process
in between.

Unlike the metrics route, this one is **not OTLP-framed on the wire** — CloudWatch Logs
subscription-filter delivery has exactly one proprietary record format, gzip-compressed
regardless of the delivery stream's own `Content-Encoding` setting. Once decoded,
micromegas synthesizes an OTLP `ExportLogsServiceRequest` internally (one `Resource`, one
`LogRecord` per `logEvent`) and feeds it through the same logs split/write path as native
OTLP logs — so `log_entries` sees these rows exactly like any other log producer.

### Payload format

Each Firehose record's `data`, after base64-decode, is gzip-compressed. Decompressed, it
is CloudWatch's subscription-filter JSON:

```json
{
  "messageType": "DATA_MESSAGE",
  "owner": "123456789012",
  "logGroup": "/ecs/my-service",
  "logStream": "ecs/my-service/abcd1234",
  "subscriptionFilters": ["my-filter"],
  "logEvents": [
    { "id": "...", "timestamp": 1510109208016, "message": "raw log line" }
  ]
}
```

`CONTROL_MESSAGE` records — which CloudWatch sends periodically to verify
reachability — are recognized and dropped silently (not an error, no row written, no
process registered).

### AWS delivery-stream setup

Configure a Kinesis Firehose delivery stream with an **HTTP endpoint destination**,
subscribed from a CloudWatch Logs log group via a subscription filter:

- **HTTP endpoint URL**: `https://micromegas.example.com/ingestion/cloudwatch/v1/logs/firehose`
- **Access key**: any live `ingestion_api_keys` key (see [API Keys](../admin/api-keys.md)), or, transitionally, a value from `MICROMEGAS_API_KEYS` — sent by
  Firehose as `X-Amz-Firehose-Access-Key` on every request, same as the metrics route.
- **Content encoding**: CloudWatch always gzips each record's payload at the source; this
  is independent of (and unaffected by) any additional `Content-Encoding: gzip` Firehose
  itself may apply to the whole HTTP body — both layers are handled transparently.
- **S3 backup**: same recommendation as the metrics route — Firehose retries non-200
  responses and eventually spills to S3, so no data is silently lost during an extended
  micromegas outage.

### How `logGroup`/`logStream`/`owner` surface

- `service.name` = `logGroup`, `service.instance.id` = `logStream` — feeds the same
  `process_id_from_resource` identity formula every other OTLP/OTel producer uses, so
  distinct log streams (distinct ECS tasks, Lambda instances, RDS instances) resolve to
  distinct `process_id`s with no CloudWatch-specific identity logic.
- `logGroup`, `logStream`, and `owner` (AWS account id) are all set as resource
  attributes (`aws.log.group.name`, `aws.log.stream.name`, `cloud.account.id`), so they
  surface per-row via `process_properties.otel.resource.*` — the same discovery path as
  any other OTel resource attribute (see [Process identity](#process-identity) above).
- The per-event CloudWatch `id` is attached as a record-level attribute
  (`aws.log.event.id`), queryable via `properties`, letting you correlate a `log_entries`
  row back to the exact CloudWatch event.

!!! note "Cross-account collisions"
    `cloud.account.id` (`owner`) is **not** part of the `process_id` identity hash, so two
    different AWS accounts with the same `logGroup`+`logStream` names collapse onto the
    same `process_id`. Rows remain unambiguous (`owner` is still queryable per-row), only
    the process grouping is coarser than ideal across accounts — most relevant for RDS
    Postgres logs, where stream names are user-chosen DB-instance identifiers that can
    repeat across environments.

### Ack contract

Same as [CloudWatch Metric Streams](#ack-contract): `200 OK` with
`{"requestId": "<echoed>", "timestamp": ...}` on success; any non-200 status (with
`errorMessage`) triggers a Firehose retry.

### Idempotency

Same content-addressed `block_id` scheme as the rest of OTLP ingestion (see
[Idempotency](#idempotency)): a Firehose retry of a previously-succeeded batch dedups on
write. CloudWatch Logs events carry real per-event timestamps (no backfill), so genuinely
distinct log lines never collide.

!!! warning "TLS in production"
    Same as the metrics Firehose route: `X-Amz-Firehose-Access-Key` over plaintext leaks
    in transit. Terminate TLS in front of the listener for any production delivery stream.

## Limitations

- **OTLP/HTTP only.** gRPC OTLP is not implemented; SDKs that default to gRPC need `OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf`.
- **OTLP/JSON: string-encoded 64-bit fields required.** The OTLP/JSON spec mandates `"timeUnixNano"` and similar 64-bit integer fields as quoted strings (e.g. `"1700000000000000000"`). Bare JSON numbers are rejected. Conformant OTel SDKs and EventBridge input transformers produce the string form automatically.
- **No mTLS / client certs.** Only bearer-token and OIDC auth.
- **Histograms not yet materialized.** Sum, Gauge, and Summary (count/sum/min/max only) land in `measures`; Histogram and ExponentialHistogram are skipped with a debug log. Configured percentile statistics beyond min/max (e.g. p90/p99 from a CloudWatch `statistics_configuration`) are also skipped with a debug log.
- **`otel_spans` is JIT-only and per-process.** Cross-process trace queries (`WHERE trace_id = X` across all services) need to UNION across each participating process.
- **No per-tenant rate limiting.** Add at the load balancer if needed.

## Troubleshooting

**`415 Unsupported Media Type`** — the SDK is sending an unsupported `Content-Type` or omitting it entirely. Accepted types are `application/x-protobuf` and `application/json`. Other compression codecs (`deflate`, `zstd`) also return 415; only gzip is accepted.

**`401 Unauthorized`** — verify the bearer token matches a live `ingestion_api_keys` row or an entry in `MICROMEGAS_API_KEYS` on the server. Check that the SDK is actually attaching the header (`OTEL_EXPORTER_OTLP_HEADERS` is processed at export time, not at SDK init — typos are silently ignored).

**`413 Payload Too Large`** — the compressed body exceeds 20 MiB. Lower the SDK's batch size (`OTEL_BSP_MAX_EXPORT_BATCH_SIZE`, `OTEL_BLRP_MAX_EXPORT_BATCH_SIZE`) or split into more frequent exports.

**Process collapses across runs** — the formula expects `service.instance.id` to vary per OS process. If your SDK omits it (some FaaS configurations), every invocation hashes to the same `process_id`. Set it explicitly via `OTEL_RESOURCE_ATTRIBUTES=service.instance.id=$(uuidgen)` or have the SDK generate one.

**`process_id` looks identical across very different services** — `host.id`, `host.name`, `process.pid`, and `service.instance.id` all came back empty. Check the resource detector configuration on the SDK side; the server logs a degenerate-resource warning when this happens.

**Logs without an explicit severity appear with `level = 4` (Info)** — `severity_number = 0` (UNSPECIFIED) maps to Info so unspecified records pass the default `WHERE level <= 4` filter (lower number = more severe in micromegas; `level <= 4` keeps Info-and-more-severe). Set `severity_number` explicitly on the SDK side if you want a different mapping.

**Trace queries return nothing** — `otel_spans` is a JIT view and only materializes when queried with a specific `process_id`. Use `view_instance('otel_spans', '<process_id>')`, not `FROM otel_spans`. Find the right `process_id` via the `processes` view first.

**`log_entries`/`measures`/`otel_spans` is empty but ingestion returned `200 OK`** — this can mean either the materialization pipeline hasn't caught up yet, or the payload itself doesn't contain what you expect. Tell them apart with [`parse_block`](../query-guide/functions-reference.md#parse_blockblock_id), which decodes a block's raw OTLP payload independently of any view:

1. Find the block: `SELECT block_id, "streams.format" FROM blocks WHERE process_id = '<process_id>' AND "streams.format" LIKE 'otlp/%'`.
2. Run `SELECT type_name, jsonb_format_json(value) FROM parse_block('<block_id>')` on it.
3. If the records are there with the fields you expect, the problem is on the materialization side (JIT/daemon lag — see the note on the daemon's per-second/per-minute tasks below). If they're missing or malformed, the problem is upstream, in what the SDK sent.

`parse_block` is subject to the same query range as any other query: a block outside `--begin`/`--all` now errors (naming the queried range) rather than returning empty rows, so pass `--all` or widen `--begin` if step 1 or 2 comes back empty for a block you can see in `blocks` with a wider range. `blocks` itself is materialized by the maintenance daemon's every-second task (each tick covers `[t-2s, t-1s)`), so a block posted moments ago may need a short retry before it's queryable at all.

## References

- [OTLP/HTTP specification](https://opentelemetry.io/docs/specs/otlp/)
- [OpenTelemetry proto definitions](https://github.com/open-telemetry/opentelemetry-proto)
- [Claude Code monitoring guide](https://code.claude.com/docs/en/monitoring-usage)
- [Schema Reference: `otel_spans`](../query-guide/schema-reference.md#otel_spans)
- [Authentication](../admin/authentication.md)
