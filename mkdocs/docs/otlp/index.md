# OTLP Ingestion

Micromegas accepts native OpenTelemetry Protocol (OTLP) traffic over HTTP alongside its custom transit/CBOR wire format. Any OTel-instrumented program — Claude Code, Goose, generic OTel SDKs (Python, Go, JS, .NET, Java) — can point `OTEL_EXPORTER_OTLP_ENDPOINT` at the ingestion service and have logs, metrics, and spans land in the lakehouse.

## Overview

The ingestion service exposes three OTLP/HTTP routes that mirror the OpenTelemetry specification:

| Route | OTLP message | Lands in |
|---|---|---|
| `POST /ingestion/otlp/v1/logs` | `ExportLogsServiceRequest` | `log_entries` |
| `POST /ingestion/otlp/v1/metrics` | `ExportMetricsServiceRequest` | `measures` |
| `POST /ingestion/otlp/v1/traces` | `ExportTraceServiceRequest` | `otel_spans` (per-process JIT view) |

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

The OTLP routes share the same auth chain as the rest of the ingestion service: API-key bearer tokens (configured via `MICROMEGAS_API_KEYS`) and OIDC.

OTel SDKs read `OTEL_EXPORTER_OTLP_HEADERS` and attach the parsed headers to every export request:

```bash
# Server side — same JSON keyring telemetry-ingestion-srv already uses
export MICROMEGAS_API_KEYS='[{"name":"team-platform","key":"mm_abc123def..."}]'

# Client side
export OTEL_EXPORTER_OTLP_ENDPOINT="https://micromegas.example.com/ingestion/otlp"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer mm_abc123def..."
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
    host.id  + "\x1F" + host.name + "\x1F" +
    process.pid + "\x1F" + process.creation.time + "\x1F" +
    service.namespace + "\x1F" + service.name + "\x1F" +
    service.instance.id)
```

Missing fields are treated as empty strings. The formula is stable across batches as long as the SDK's Resource is immutable for the lifetime of the process — which is what the OTel spec requires.

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

Sum and Gauge data points are materialized directly. Histogram, ExponentialHistogram, and Summary are skipped with a debug log in the current release — they will land in a follow-up that defines a histogram-aware schema.

| OTel field | parquet column |
|---|---|
| `name` | `name` |
| `unit` | `unit` |
| `value` (int widened to f64) | `value` |
| `time_unix_nano` | `time` |
| `aggregation_temporality`, `is_monotonic`, `otel.metric.kind` | `properties` |

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

Block IDs are content-addressed: `block_id = uuid_v5(NS_OTEL_BLOCK_V1, payload_bytes)`. Retried POSTs collide on `ON CONFLICT (block_id) DO NOTHING` and add no rows. This makes the OTLP endpoints safe to retry on transient errors without double-counting.

## Client recipes

### Claude Code

```bash
export CLAUDE_CODE_ENABLE_TELEMETRY=1
export OTEL_EXPORTER_OTLP_ENDPOINT="https://micromegas.example.com/ingestion/otlp"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
export OTEL_METRICS_EXPORTER=otlp
export OTEL_LOGS_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer mm_abc123def..."

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
        "body": {"stringValue": "<$.detail.message>"}
      }]
    }]
  }]
}
```

`timeUnixNano` must be a **quoted string** in the template (e.g. `"<$.time_ns>"`). EventBridge input transformers substitute variables as strings inside quotes, satisfying the OTLP/JSON spec requirement. No Lambda translation layer is needed.

## Limitations

- **OTLP/HTTP only.** gRPC OTLP is not implemented; SDKs that default to gRPC need `OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf`.
- **OTLP/JSON: string-encoded 64-bit fields required.** The OTLP/JSON spec mandates `"timeUnixNano"` and similar 64-bit integer fields as quoted strings (e.g. `"1700000000000000000"`). Bare JSON numbers are rejected. Conformant OTel SDKs and EventBridge input transformers produce the string form automatically.
- **No mTLS / client certs.** Only bearer-token and OIDC auth.
- **Histograms not yet materialized.** Sum and Gauge land in `measures`; Histogram, ExponentialHistogram, and Summary are skipped with a debug log.
- **`otel_spans` is JIT-only and per-process.** Cross-process trace queries (`WHERE trace_id = X` across all services) need to UNION across each participating process.
- **`parse_block` does not decode OTel payloads.** It returns a clean error on `format != "micromegas-transit"`.
- **No per-tenant rate limiting.** Add at the load balancer if needed.

## Troubleshooting

**`415 Unsupported Media Type`** — the SDK is sending an unsupported `Content-Type` or omitting it entirely. Accepted types are `application/x-protobuf` and `application/json`. Other compression codecs (`deflate`, `zstd`) also return 415; only gzip is accepted.

**`401 Unauthorized`** — verify the bearer token matches an entry in `MICROMEGAS_API_KEYS` on the server. Check that the SDK is actually attaching the header (`OTEL_EXPORTER_OTLP_HEADERS` is processed at export time, not at SDK init — typos are silently ignored).

**`413 Payload Too Large`** — the compressed body exceeds 20 MiB. Lower the SDK's batch size (`OTEL_BSP_MAX_EXPORT_BATCH_SIZE`, `OTEL_BLRP_MAX_EXPORT_BATCH_SIZE`) or split into more frequent exports.

**Process collapses across runs** — the formula expects `service.instance.id` to vary per OS process. If your SDK omits it (some FaaS configurations), every invocation hashes to the same `process_id`. Set it explicitly via `OTEL_RESOURCE_ATTRIBUTES=service.instance.id=$(uuidgen)` or have the SDK generate one.

**`process_id` looks identical across very different services** — `host.id`, `host.name`, `process.pid`, and `service.instance.id` all came back empty. Check the resource detector configuration on the SDK side; the server logs a degenerate-resource warning when this happens.

**Logs without an explicit severity appear with `level = 4` (Info)** — `severity_number = 0` (UNSPECIFIED) maps to Info so unspecified records pass the default `WHERE level <= 4` filter (lower number = more severe in micromegas; `level <= 4` keeps Info-and-more-severe). Set `severity_number` explicitly on the SDK side if you want a different mapping.

**Trace queries return nothing** — `otel_spans` is a JIT view and only materializes when queried with a specific `process_id`. Use `view_instance('otel_spans', '<process_id>')`, not `FROM otel_spans`. Find the right `process_id` via the `processes` view first.

## References

- [OTLP/HTTP specification](https://opentelemetry.io/docs/specs/otlp/)
- [OpenTelemetry proto definitions](https://github.com/open-telemetry/opentelemetry-proto)
- [Claude Code monitoring guide](https://code.claude.com/docs/en/monitoring-usage)
- [Schema Reference: `otel_spans`](../query-guide/schema-reference.md#otel_spans)
- [Authentication](../admin/authentication.md)
