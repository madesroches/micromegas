# Native OTLP Ingestion Plan

## Overview

Add native support for the OpenTelemetry Protocol (OTLP) as a first-class wire format in Micromegas, alongside the existing CBOR/transit protocol. Any OTel-instrumented program — Claude Code, Goose, generic OTel SDKs — can point `OTEL_EXPORTER_OTLP_ENDPOINT` at the ingestion service and have spans, metrics, and logs land in the lakehouse.

The driving use case is observability for AI coding agents: Claude Code emits a rich OTel surface (token splits by cache type, tool-call spans, compaction events, hook timings). Once those land in the lakehouse, DataFusion queries can find inefficiencies (cache thrash, redundant `Read` calls, exploration without edits).

## Terminology

**Signal**: an OTel telemetry data type. The three core signals — `logs`, `metrics`, `traces` — are independent end-to-end: separate proto message types (`ResourceLogs`, `ResourceMetrics`, `ResourceSpans`), separate gRPC services (`LogsService`, `MetricsService`, `TraceService`), separate HTTP endpoints (`/v1/logs`, `/v1/metrics`, `/v1/traces`), separate SDK exporter pipelines. We use `signal ∈ {logs, metrics, traces}` throughout this doc. Profiles (newly stabilized in 2025) is out of scope for v1 but would extend cleanly via a new `format = "otlp/v1/profiles"`.

**Resource**: OTel's "who produced this data" — a `Resource` proto carrying `repeated KeyValue attributes`. One Resource = one logical producing entity (process, container, lambda invocation). Conventionally identified by `service.name`, `service.instance.id`, and host/k8s attributes. We synthesize a `process_id` from these.

**Instrumentation Scope** (often shortened to "scope"): identifies the library/module producing a record — `"opentelemetry.instrumentation.requests"`, `"@opentelemetry/instrumentation-pg"`, `"claude-code"`, etc. A single process loads many scopes. Scope is sub-process metadata; lives on per-row properties, not in identity formulas.

## Current State

The ingestion service speaks one wire format: a custom CBOR-framed binary protocol over HTTP.

**Service**: `rust/telemetry-ingestion-srv/` (axum) — three POST routes:
- `/ingestion/insert_process` → `web_ingestion_service.rs:171`
- `/ingestion/insert_stream`  → `web_ingestion_service.rs:131`
- `/ingestion/insert_block`   → `web_ingestion_service.rs:57`

**Wire format**: CBOR via `ciborium` for the envelope, with a `BlockPayload { dependencies: Vec<u8>, objects: Vec<u8> }` whose inner bytes are produced by the `transit` crate (POD memory-layout serialization with a `UserDefinedType` reflection system). See `rust/telemetry/src/block_wire_format.rs`.

**Identity model**: three-tier hierarchy.
- `Process` (`rust/tracing/src/process_info.rs:49`): UUID, exe, username, computer, distro, CPU brand, **tsc_frequency**, start_time/start_ticks, optional parent_process_id, properties.
- `Stream` (`rust/telemetry/src/stream_info.rs:12`): UUID, process_id, dependencies_metadata + objects_metadata (transit type defs), tags, properties.
- `Block`: chunk of events from one stream with `[begin_time, end_time]` and `[begin_ticks, end_ticks]` for tick-to-nanosecond calibration.

**PostgreSQL schema** (`rust/ingestion/src/sql_telemetry_db.rs`): `processes`, `streams`, `blocks` tables. Block payloads land in object storage at `blobs/{process_id}/{stream_id}/{block_id}`.

**Lakehouse views** (`rust/analytics/src/lakehouse/`):
- `log_entries`: process_id, stream_id, block_id, insert_time, exe, username, computer, time, target, level, msg, properties (JSONB), process_properties.
- `measures`: same prefix + name, unit, value (Float64), properties.
- `thread_spans`: id, parent, depth, hash, begin/end, duration, name, target, filename, line.
- `async_events`: stream_id, block_id, time, event_type, span_id, parent_span_id, depth, hash, name, filename, target, line.

All views use Arrow Dictionary encoding for low-cardinality strings.

**Auth** (`rust/auth/`): API-key bearer tokens (`MICROMEGAS_API_KEYS`) and OIDC (JWKS, token cache). Multi-provider chain. Axum middleware applied in `telemetry-ingestion-srv/src/main.rs`.

**Existing OTel code**: none. `grep -r 'opentelemetry\|otlp\|otel' rust/ python/` is empty. Workspace already has `prost = "0.14"` available; we only need to add `opentelemetry-proto`.

## Design

### Architecture: extend the existing ingestion service; store OTLP as-is, parse at the analytics layer

The micromegas data flow is symmetric on either side of object storage:

```
producer → block bytes (opaque to ingestion) → object store + PG metadata
                                                       ↓
                              block processor (decoder + row materializer)
                                                       ↓
                                           parquet lakehouse views
```

For native producers the block payload is transit/POD. For OTel producers, we store the raw OTLP protobuf payload — the same envelope (`block_wire_format::Block`), a different decoder.

This avoids translating at ingest. Ingest becomes nearly trivial: derive `process_id` from resource attributes, write the proto bytes to object storage, INSERT one row in PG. No deps queue construction, no synthetic event types, no transit serialization round-trip. The `tracing/` crate is untouched — it's a library for in-process instrumentation, not a target for foreign data models.

Since OTLP is now HTTP-only, we add the three OTLP routes to the existing `telemetry-ingestion-srv` rather than creating a new binary. Shared auth middleware, shared `WebIngestionService` instance, one deployment unit. A new `rust/otel-ingestion/` library hosts the proto decode + identity synthesis + block-builder logic; the server crate just wires axum routes to it.

OTel-specific decode lives in `analytics/`, where the parquet schema is the natural translation target anyway. New block processors (one per signal) prost-decode the payload via `opentelemetry-proto` and emit rows.

```
       ┌─────────────────────────────────────────────────┐
       │  telemetry-ingestion-srv (EXISTING, EXTENDED)   │
       │   ─ existing routes:                            │
       │       POST /ingestion/{insert_process,           │
       │             insert_stream, insert_block}        │
       │   ─ NEW routes (shared auth + ingestion lib):   │
       │       POST /ingestion/otlp/v1/{logs,metrics,    │
       │              traces}                             │
       │     (same listener, /ingestion/ prefix)         │
       │   ─ derive process_id from resource attrs       │
       │   ─ write raw OTLP proto to object store        │
       │   ─ INSERT block + stream + process metadata    │
       └─────────────────┬───────────────────────────────┘
                         │ block_wire_format::Block
                         │   payload.objects = OTLP proto bytes
                         │   stream.tags   = ["log" | "metrics" | "trace"]
                         │   stream.format = "otlp/v1/<signal>"
                         ▼
       ┌─────────────────────────────────────────────────┐
       │  ingestion::WebIngestionService (EXISTING,      │
       │   used as a library, not over HTTP)             │
       │   ─ object store + PG writes                    │
       └─────────────────┬───────────────────────────────┘
                         │
                         ▼
       ┌─────────────────────────────────────────────────┐
       │  Block processors (NEW, in analytics/)          │
       │   ─ otel_logs_block_processor                   │
       │   ─ otel_metrics_block_processor                │
       │   ─ otel_spans_block_processor                  │
       │     ─ prost-decode payload                      │
       │     ─ walk Resource/Scope/data points           │
       │     ─ emit parquet rows                         │
       └─────────────────┬───────────────────────────────┘
                         ▼
       ┌─────────────────────────────────────────────────┐
       │  Lakehouse parquet views                        │
       │   ─ log_entries  (existing, +OTel rows)         │
       │   ─ measures     (existing, +OTel rows)         │
       │   ─ otel_spans   (NEW — first-class trace_id)   │
       └─────────────────────────────────────────────────┘
```

**Block payload shape for OTel streams**: `BlockPayload { dependencies: [], objects: <ResourceSpans|ResourceMetrics|ResourceLogs proto bytes> }`. The existing `Vec<u8>` payload field is opaque — no struct change needed.

**Format discrimination**: a new `format TEXT NOT NULL` column on the `streams` table. `objects_metadata BYTEA` is typeless and unreliable as a format discriminator — adding `format` makes the wire-format choice explicit and schema-enforced.

```sql
ALTER TABLE streams
  ADD COLUMN format TEXT NOT NULL DEFAULT 'micromegas-transit';
```

Values:
- `micromegas-transit` — current default for native streams; backfill applies via `DEFAULT`.
- `otlp/v1/traces` — OTel traces (payload is one `ResourceSpans` proto).
- `otlp/v1/metrics` — OTel metrics (payload is one `ResourceMetrics` proto).
- `otlp/v1/logs` — OTel logs (payload is one `ResourceLogs` proto).

The `/v1` segment leaves room to evolve per-format envelope structure without colliding with prior data. Block-processor dispatch matches on `streams.format`; tags stay free for queryable/operational metadata.

The existing `objects_metadata BYTEA` field stays as-is. Its interpretation is now defined by `format`:
- For `micromegas-transit`: CBOR-encoded transit type definitions (current behavior).
- For `otlp/v1/*`: empty (or reserved for future per-format extensions like a schema URL pin).

**One block per OTLP request per resource**. An incoming `ExportTraceServiceRequest` may carry multiple `ResourceSpans` (different services). We split into one block per resource so `process_id` is unambiguous on the metadata row. Each block's payload is the protobuf encoding of *one* `ResourceSpans` (or `ResourceMetrics` / `ResourceLogs`) — small, self-contained, and re-decodable in isolation.

### Wire-level: protocol crate

Use the `opentelemetry-proto` crate **without** the `gen-tonic` feature — we only need the prost-generated message types (`ResourceLogs`, `ResourceMetrics`, `ResourceSpans`, `ExportLogsServiceRequest`, etc.), not the gRPC service stubs.

We serve OTLP/HTTP only via `POST /ingestion/otlp/v1/{logs,metrics,traces}` on the **same listener** as the existing `/ingestion/insert_*` routes — no new port, all ingestion endpoints under one path prefix. gRPC is intentionally not supported in v1 — see Trade-offs.

Per the OTLP/HTTP spec, `OTEL_EXPORTER_OTLP_ENDPOINT` is a base URL and the SDK appends `/v1/{signal}`. So clients set `OTEL_EXPORTER_OTLP_ENDPOINT=https://host.example.com/ingestion/otlp` and the SDK POSTs to `.../ingestion/otlp/v1/traces` etc. Per-signal endpoint vars (`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`) are full URLs by spec — operators using them need to write the full path.

### Identity: synthesizing process and stream

OTel has no "process" object; it has a `Resource` (key/value attributes) attached to each batch. We derive a stable `process_id` by hashing the OS-honest identifying tuple together with the OTel service identity:

```
key = trim_lower(host.id)                  + "\x1F"
    + trim_lower(host.name)                + "\x1F"
    + str(process.pid)                     + "\x1F"
    + str(process.creation.time as ns)     + "\x1F"
    + trim_lower(service.namespace)        + "\x1F"
    + trim_lower(service.name)             + "\x1F"
    + trim_lower(service.instance.id)

process_id = uuid_v5(NS_OTEL_PROCESS_V1, key)
```

Rules:
- Missing fields → empty string. Stability across batches relies on OTel's Resource-immutability contract.
- Field order is the contract; changing it requires a new namespace UUID (`_V2`).
- `\x1F` (unit separator) cannot legitimately appear inside attribute values per OTel spec.
- If all of `host.id`, `host.name`, `process.pid`, `service.instance.id` are empty, log a degenerate-resource warning so we notice — produces a single collapsed `process_id` per `(service.namespace, service.name)`.

The first time a `process_id` is seen, INSERT into `processes` (idempotent — existing `ON CONFLICT DO NOTHING`) with:
- `exe` ← `service.name` (or `service.namespace + "/" + service.name` if namespace present)
- `username` ← `user.name` if present, else empty
- `computer` ← `host.name` if present, else empty
- `distro` ← `os.description` if present
- `cpu_brand` ← `host.cpu.model.name` if present, else empty
- `tsc_frequency` ← `1_000_000_000` (OTel timestamps are nanoseconds; ticks = ns)
- `start_time` / `start_ticks` ← `process.creation.time` if present, else first observed event time
- `properties` ← every other resource attribute, prefixed `otel.resource.*`. Includes `service.instance.id`, `process.pid`, `process.creation.time`, etc. (queryable, not in identity formula contract beyond the hash).

**Claude Code mapping**: each `claude` OS invocation = one `process_id`. `--continue` starts a new OS process (new pid, new creation.time, new SDK-generated `service.instance.id`) → new `process_id`. Cross-invocation correlation via `session.id` (an event attribute, not a resource attribute) is a query-time concern.

**Known limitation**: in FaaS environments where the OS process is reused across cold invocations, the OTel Lambda layer sets `service.instance.id = invocation_id` to disambiguate. Our formula respects that. If a poorly-instrumented FaaS app omits `service.instance.id`, multiple invocations collapse — documented limitation, not a v1 blocker.

**Stream identity** (one stream per signal per process — max 3 streams per process):
```
stream_id = uuid_v5(NS_OTEL_STREAM_V1,
    process_id + "\x1F" + signal)
```
where `signal ∈ {logs, metrics, traces}`.

Stream tags reuse the existing micromegas vocabulary so views naturally load native + OTel streams uniformly:

| Signal | Stream tag | Stream format |
|---|---|---|
| logs | `"log"` (existing — native producers also use this) | `otlp/v1/logs` |
| metrics | `"metrics"` (existing) | `otlp/v1/metrics` |
| traces | `"trace"` (new — native async spans live in `"cpu"` streams alongside other thread events) | `otlp/v1/traces` |

**Tags and format are orthogonal axes**: tags say *what* the stream contains (signal/purpose); format says *how* the bytes are encoded (wire-format). The `log_entries` view loads any stream tagged `"log"` regardless of format; per-block dispatch reads `format` to pick the right block processor. Same for `measures`. The new `otel_spans` view loads streams tagged `"trace"`.

Scope is intentionally **not** in stream identity. A single process loads many instrumentation libraries (HTTP framework, DB driver, app code, OTel SDK internals, ...), all sharing one process and emitting into the per-signal stream. Putting scope in the formula would multiply stream count by the number of loaded libraries (often 5–20) for no structural gain.

**Scope as row-level metadata.** Scope info lives on per-row JSONB properties at materialization time:
- `otel.scope.name`
- `otel.scope.version`
- `otel.scope.schema_url`
- scope attributes (each prefixed `otel.scope.attr.<key>`)

Queries that filter by library work via these properties:
```sql
WHERE properties->>'otel.scope.name' = 'opentelemetry.instrumentation.requests'
```

**Block identity**: one block per (process_id, signal) per OTLP request per Resource. A multi-Resource request fans out to N blocks across N processes' streams. `block_id = uuid_v5(NS_OTEL_BLOCK_V1, hash(payload bytes))` for idempotent retry.

### Schema mapping: OTLP → parquet rows

All mappings happen in the analytics-layer block processor, which prost-decodes the OTLP proto and emits parquet rows.

#### Logs → `log_entries`

| OTel field | parquet column |
|---|---|
| `time_unix_nano` | `time` |
| `severity_number` 1–24 | collapsed `level` (1–4=Trace, 5–8=Debug, 9–12=Info, 13–16=Warn, 17–20=Error, 21–24=Fatal) |
| `body.string_value` | `msg` |
| `body.kvlist_value` / `array_value` | JSON-stringified into `msg` (structured-body parsing deferred) |
| `attributes.*` | `properties` JSONB |
| `instrumentation_scope.name` | `target` |
| `trace_id`, `span_id` | `properties.otel.trace_id` / `otel.span_id` (promote later if hot) |
| `severity_text` | `properties.otel.severity_text` |

#### Spans → `otel_spans`

New parquet view columns:

```
process_id, stream_id, block_id, insert_time,
exe, username, computer, process_properties,   -- joined from process
trace_id (FixedSizeBinary[16] or hex Utf8),
span_id  (FixedSizeBinary[8]  or UInt64),
parent_span_id,
start_time, end_time, duration,
name, scope, kind, status, status_message,
properties (JSONB),
events (List<Struct{time, name, attributes}>),  -- span events
links  (List<Struct{trace_id, span_id, attributes}>)
```

`SpanKind`, status code, and span events/links are first-class on the row (not stuffed in JSONB) because they are the load-bearing fields for trace analysis.

#### Metrics

- **Sum / Gauge** → `measures` rows directly. `name`, `unit`, `value` (int widened to f64), `time`. `aggregation_temporality` and `is_monotonic` go into row properties.
- **Histogram, ExponentialHistogram** → new view `otel_metrics_histograms` (Phase 6). Bucket arrays as native columns.
- **Summary** (deprecated in OTel) → drop with a debug log.
- **Exemplars** → deferred (land in row properties when we surface them).

#### Resource and Scope attributes

| OTel level | Lands on |
|---|---|
| Resource attributes | `Process` — `service.name` → `exe`, `host.name` → `computer`, `user.name` → `username`, everything else → `process.properties.otel.resource.*` |
| Scope identity (`name`, `version`, `schema_url`) and scope attributes | per-row `properties.otel.scope.*` (not on the stream — see Stream identity) |
| DataPoint / Span / LogRecord attributes | per-row `properties` |

### Ingest path (per OTLP request)

1. Auth (existing axum middleware — `auth_middleware` from `rust/auth/src/axum.rs`).
2. Decode the outer `ExportRequest` proto (just to walk Resource boundaries; we don't decode further).
3. For each `ResourceLogs` / `ResourceMetrics` / `ResourceSpans`:
   1. Derive `process_id` from resource attributes (UUIDv5; see Identity).
   2. Derive `stream_id` from `(process_id, signal)` — max 3 streams per process. Scope info travels in row properties, not in stream identity.
   3. Idempotent register process + stream (in-memory dedup cache + `ON CONFLICT DO NOTHING` PG inserts). Stream registration sets `format = "otlp/v1/<signal>"`.
   4. Re-encode this Resource sub-message as protobuf bytes — the block payload.
   5. Build `block_wire_format::Block` with `payload.dependencies = []`, `payload.objects = <proto bytes>`.
   6. Call existing `WebIngestionService::insert_block`.
4. Return OTLP success (or RESOURCE_EXHAUSTED on PG/object-store failure, INVALID_ARGUMENT on parse error).

There's no event-by-event translation at ingest. The hot path serializes one protobuf submessage and writes it.

### Tick calibration

Native blocks carry `tsc_frequency` and `(begin_ticks, end_ticks)` so timestamps can be rebuilt at query time. OTel timestamps are already absolute nanoseconds. We set `tsc_frequency = 1_000_000_000` and `begin_ticks = begin_time_unix_nano` (and same for end), so existing tick→time math passes through cleanly.

### Backpressure and HTTP semantics

OTLP/HTTP uses one POST per export call (one request, one response). The proto response body carries `partial_success { rejected_*_count, error_message }` for batch-level reporting.

Implementation:
- Axum routes with `RequestBodyLimitLayer` (10 MB default — matches the OTel Collector default).
- No rate limiting in v1 — out of scope (single global axum listener, no per-tenant accounting).
- HTTP status mapping:
  - DB/object-store transient failures → `503 Service Unavailable` (retryable per OTLP/HTTP spec).
  - Parse errors / unknown content-type → `400 Bad Request` (non-retryable).
  - Auth failures → `401 Unauthorized`.
- No partial-success accounting in v1: whole-batch accept/reject. SDKs handle this fine.
- Standard HTTP load balancers (ALB, GCLB, nginx, HAProxy) work without special config — all traffic is HTTP/1.1 or HTTP/2 POST with `application/x-protobuf` body.

### Auth

The existing API-key + OIDC chain works unchanged. OTLP authenticates via standard HTTP headers, which axum sees natively — no new auth code, no changes to the `auth` crate.

#### Wire mechanism

OTel SDKs read `OTEL_EXPORTER_OTLP_HEADERS` and propagate the parsed headers on every export request. Format: `key=value` pairs, comma-separated.

```bash
# Server side — same keyring as telemetry-ingestion-srv
export MICROMEGAS_API_KEYS='[{"name":"team-platform","key":"mm_abc123def..."}]'

# Client side — Claude Code, Goose, any OTel SDK
export OTEL_EXPORTER_OTLP_ENDPOINT="https://micromegas.example.com/ingestion/otlp"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer mm_abc123def..."
export CLAUDE_CODE_ENABLE_TELEMETRY=1
claude
```

The OTel SDK attaches the `Authorization` header to every POST. Our axum middleware (`auth::axum::auth_middleware`) parses the bearer token, looks it up in the `MICROMEGAS_API_KEYS` keyring via `ApiKeyAuthProvider`, and inserts the resulting `AuthContext` into request extensions. Same code path the existing `telemetry-ingestion-srv` uses.

#### Per-signal headers (advanced)

If an operator wants different keys for different signals (rare), OTel supports:
```bash
export OTEL_EXPORTER_OTLP_LOGS_HEADERS="Authorization=Bearer key-for-logs"
export OTEL_EXPORTER_OTLP_TRACES_HEADERS="Authorization=Bearer key-for-traces"
```
Per-signal headers override the catch-all. We don't need to do anything special — each POST carries whatever the SDK attached.

#### Three caveats worth documenting

1. **TLS is mandatory in production.** Bearer tokens over plaintext = leaked tokens. The HTTP listener should sit behind an HTTPS-terminating load balancer (or run TLS directly via `axum_server::tls_rustls`). Local dev to localhost is fine plaintext.
2. **No `${VAR}` expansion in OTel SDKs.** The SDK reads `OTEL_EXPORTER_OTLP_HEADERS` literally; the shell expands `${VAR}` at `export` time. Config-file deployments (where headers come from a JSON/YAML file) need pre-substituted values or wrapper scripts.
3. **Comma in a token value would break parsing.** OTLP header parsing splits on `,`. Bearer tokens that are UUIDs/JWTs/base64 don't contain commas; if anyone ever introduces a custom token format that does, it must be URL-encoded.

#### Multi-tenancy hook

The `AuthContext.subject` field is set to the matched API-key `name` (e.g. `"team-platform"`). We use this two ways:

- **Audit / observability**: every block insert log line and the existing `subject` extension are tagged with which key authored the data.
- **Optional namespace defaulting**: an env var `MICROMEGAS_OTLP_NAMESPACE_MAP` (JSON `{"key-name":"service.namespace"}`) lets operators force a `service.namespace` per key when the OTel client doesn't set it. So a key issued to `team-platform` automatically tags its data with `service.namespace=platform` regardless of how the SDK is configured. Cheap multi-tenancy on top of existing auth.

#### What we are NOT doing in v1

- **mTLS / client certs**. The existing auth crate doesn't support it; the axum middleware would need a separate code path. Skip until someone asks.
- **OAuth/OIDC for OTel clients**. The existing OIDC provider is for human/SSO flows, not machine credentials. Coding agents and services use API keys. If we ever need machine OIDC (workload identity, IAM-roles-anywhere), it's a separate workstream.
- **Per-tenant rate limiting**. Out of scope for v1. The auth context exposes `subject` (API-key name) which is enough to add it later as a tower layer; we'll do it when we have a real noisy-neighbor problem.
- **Per-route auth requirements**. Every OTLP route requires auth; no public health endpoint on the OTel listener (health goes on a separate unauthenticated port).

### Configuration

OTLP routes share the existing `telemetry-ingestion-srv` listen address (configured via the existing `--listen-endpoint-http` flag, default `127.0.0.1:8081`). No new listener-related env var.

New env vars on `telemetry-ingestion-srv`:
- `MICROMEGAS_OTLP_MAX_RECV_BYTES` (default `10_000_000`) — applied as a per-route body limit on the OTLP routes; the existing 100MB limit on `/ingestion/insert_block` is unchanged.
- `MICROMEGAS_OTLP_NAMESPACE_MAP` (optional, JSON `{"key-name":"namespace"}`) — per-API-key default `service.namespace`.

Existing env vars reused: `MICROMEGAS_SQL_CONNECTION_STRING`, `MICROMEGAS_OBJECT_STORE_URI`, `MICROMEGAS_API_KEYS`, `MICROMEGAS_OIDC_CONFIG`.

## Implementation Steps

### Phase 1: schema + OTLP routes on telemetry-ingestion-srv
1. Add migration: `ALTER TABLE streams ADD COLUMN format TEXT NOT NULL DEFAULT 'micromegas-transit';` in a new step in `rust/ingestion/src/sql_migration.rs`. Update `create_streams_table` in `sql_telemetry_db.rs` to include the column for fresh installs.
2. Update `ingestion::WebIngestionService::insert_stream` to accept and persist `format` (default `'micromegas-transit'` if caller doesn't specify, preserving backwards compatibility).
3. Add `opentelemetry-proto = "0.31"` to workspace deps **without** the `gen-tonic` feature (we only need the prost message types).
4. Create `rust/otel-ingestion/` library crate: `identity.rs` (resource → process_id/stream_id), `proto.rs` (re-exports), `error.rs`, `block.rs` (split ExportRequest into per-resource blocks). All translation logic lives here; the server only wires axum routes.
5. Add OTLP routes to `rust/telemetry-ingestion-srv/src/main.rs`: extend the protected `Router` with `POST /ingestion/otlp/v1/{logs,metrics,traces}`, applying a per-route 10MB `RequestBodyLimitLayer` (the existing 100MB limit on `/ingestion/insert_block` stays untouched). Routes share the existing axum listener and the existing `auth_middleware`. Each route reads the protobuf body, calls into `otel-ingestion` to split + register + write blocks via the shared `WebIngestionService`. Stream registration sets `format = "otlp/v1/<signal>"`.
6. End-to-end smoke test: Python OTel SDK with `OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf` pointed at the existing ingestion endpoint; verify rows land in PG `processes`/`streams`/`blocks` (with `format = "otlp/v1/..."` on streams) and bytes land in object store.

### Phase 2: logs block processor → log_entries
1. Add `analytics/src/lakehouse/otel_logs_block_processor.rs`. Uses `prost` to decode `ResourceLogs` from the block payload bytes.
2. Walks scope/log records and emits `log_entries` rows (level collapse from `severity_number`, fold trace_id/span_id/severity_text into properties).
3. Register the processor for streams matching `format = "otlp/v1/logs"`.
4. End-to-end test: emit OTel logs from Python; query `log_entries`.

### Phase 3: metrics block processor → measures
1. Add `analytics/src/lakehouse/otel_metrics_block_processor.rs`. Decodes `ResourceMetrics`.
2. Materializes Sum + Gauge data points into `measures`. Adds `aggregation_temporality` and `is_monotonic` to row properties. Emits `warn!` for Histogram/ExponentialHistogram/Summary (handled in Phase 6).
3. End-to-end test using Claude Code's emission: `claude_code.token.usage`, `claude_code.cost.usage`, `claude_code.session.count`.

### Phase 4: spans block processor → new otel_spans view
1. Add `analytics/src/lakehouse/otel_spans_table.rs` (Arrow schema) and `otel_spans_view.rs` (view definition + time-based partitioning).
2. Add `otel_spans_block_processor.rs`. Decodes `ResourceSpans` and materializes one row per span. Span events and links become `List<Struct{...}>` columns.
3. Register the view in the view factory.
4. End-to-end test: emit a multi-span trace; verify trace traversal via `SELECT * FROM otel_spans WHERE trace_id = ...`.

### Phase 5: production hardening
1. Backpressure: monitoring + alerts on ingest latency / queue depth. Rate limiting is explicitly out of scope for v1.
2. Body size limit (10 MB compressed default).
3. Partial-success: not implemented in v1 (whole-batch accept/reject).
4. Optional `MICROMEGAS_OTLP_NAMESPACE_MAP` to default `service.namespace` per API key.

### Phase 6: histograms (separate PR)
1. Add `analytics/src/lakehouse/otel_metrics_histograms_table.rs` (count, sum, bounds, bucket_counts; exp variant adds scale, zero_count, positive/negative offsets+buckets).
2. Add `otel_metrics_histograms_view.rs` and `otel_metrics_histograms_block_processor.rs` (same payload bytes — different decode path that picks the histogram variants out of the proto).

### Phase 7: docs + ops
1. `mkdocs/docs/operating/otlp.md` — ports, env, client config snippets (Claude Code, Goose, Python OTel SDK), auth header format.
2. `mkdocs/docs/guides/coding-agents.md` — Claude Code OTel config + starter DataFusion queries (cache hit ratio, redundant tool calls, time-in-tool ratio).
3. Update `README.md` roadmap.
4. Add example Claude Code env to `local_test_env/`.

## Files to Modify

**New crate**:
- `rust/otel-ingestion/Cargo.toml` + `src/{lib,proto,identity,block,error}.rs`

**New modules in `analytics/`**:
- `rust/analytics/src/lakehouse/otel_logs_block_processor.rs`
- `rust/analytics/src/lakehouse/otel_metrics_block_processor.rs`
- `rust/analytics/src/lakehouse/otel_spans_table.rs` (Arrow schema)
- `rust/analytics/src/lakehouse/otel_spans_view.rs` (view + partitioning)
- `rust/analytics/src/lakehouse/otel_spans_block_processor.rs`
- `rust/analytics/src/lakehouse/otel_metrics_histograms_*.rs` (Phase 6)

**Modified**:
- `rust/Cargo.toml` (add `opentelemetry-proto = "0.31"`; new `otel-ingestion` member)
- `rust/ingestion/src/sql_migration.rs` (ADD COLUMN streams.format)
- `rust/ingestion/src/sql_telemetry_db.rs` (include `format` in fresh `CREATE TABLE streams`)
- `rust/ingestion/src/web_ingestion_service.rs` (persist `format` on stream insert)
- `rust/telemetry-ingestion-srv/Cargo.toml` (add `otel-ingestion`, `opentelemetry-proto`, `prost`)
- `rust/telemetry-ingestion-srv/src/main.rs` (add OTLP routes to the existing axum router; per-route body limit)
- `rust/public/src/servers/` (new module for OTLP route registration alongside `ingestion.rs`)
- `rust/analytics/src/lakehouse/mod.rs` (register OTel views + processor dispatch by `streams.format`)
- `README.md`

**No new binary** — OTLP routes live in `telemetry-ingestion-srv`. `rust/tracing/` is also not touched. OTel is an analytics-layer concern; the ingest side is just a thin protobuf-to-block-bytes adapter.

## Trade-offs

**Store OTLP as-is vs translate at ingest**: chose store-as-is. Two earlier drafts of this plan (a) invented a parallel CBOR record format and (b) added OTel interop event types to the `tracing/` crate. Both bend the architecture around OTel. The clean version is symmetric with native: native blocks store opaque transit bytes parsed at the analytics layer; OTel blocks store opaque OTLP bytes parsed at the analytics layer. Same envelope, different decoder.

The wins of as-is storage:
- Ingest path is auth + write — no translation, no event-type dispatch, no synthesized POD records.
- `tracing/` crate stays focused on in-process instrumentation.
- Lossless: every OTel attribute, exemplar, link preserved verbatim. New parquet column can be derived from raw payloads later.
- OTel evolution is decoupled. New OTel field → only the materialization changes.

The cost: two parsers (transit and OTLP proto) — but that's true of any design here; the question is just where they live.

**Extend telemetry-ingestion-srv vs separate binary**: chose extend. An earlier draft proposed a separate `otlp-ingestion-srv` binary on the assumption we needed tonic for gRPC alongside axum. Once we decided OTLP/HTTP is sufficient (see "HTTP-only" below), there's no transport-stack mismatch and no reason to fork the binary. One process, shared auth, shared `WebIngestionService`, simpler ops. The new `otel-ingestion` library still exists as the home for proto decode + identity + block builder logic, so the wire-format complexity stays out of the server crate.

**HTTP/protobuf only, no gRPC**: the OTLP spec mandates HTTP/protobuf support in every compliant SDK. Some SDKs default to gRPC (Go, older Java auto-instrumentation) but accept the HTTP fallback when `OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf` is set. Going HTTP-only means standard HTTP load balancers (ALB, GCLB, nginx, HAProxy) just work — no gRPC/HTTP-2 LB compatibility matrix — and no `tonic` dep in the OTel ingest path. If a v2 user genuinely needs gRPC, adding tonic to the same binary later is a contained change.

**One block per Resource (not per ExportRequest)**: chose per-Resource. An `ExportRequest` may carry multiple resources (different services); splitting at Resource boundaries means each block has an unambiguous `process_id` and is independently re-decodable. Block_id derived from a hash of the bytes for idempotency.

**OTel spans get their own view, not async_events**: chose new `otel_spans` view. The existing `async_events` view is derived from thread-event blocks where parent is inferred from begin/end ordering on a thread; OTel spans carry explicit `parent_span_id` and have no thread-of-origin concept. Forcing OTel into the `async_events` processor would either lie about parent inference or require a pseudo-thread per trace. Native and OTel span data live in sibling views; cross-source queries can `UNION` if needed (open question #5).

**`trace_id` and `span_id` as first-class columns**: chose first-class on `otel_spans` (the load-bearing trace-analytics columns). On `log_entries` they go into JSONB until profiling shows they're hot.

**Histograms in v1 vs deferred**: deferred. Claude Code doesn't emit histograms; bundling the new view+processor would block the driving use case.

**Span events and links as columns vs JSONB**: first-class `List<Struct{...}>` columns on `otel_spans`. Exemplars on metric data points → deferred (land in row properties when surfaced).

**OTel path is allowed to be less efficient than native**: explicit non-goal to match the per-event overhead of `micromegas-tracing`. Producers that want minimum overhead use the native SDK; OTel is for compatibility and ecosystem reach. This frees us from optimizations like proto envelope sharing across resources, zero-copy splitting, etc. — re-encoding per Resource is fine.

## Documentation

- New: `mkdocs/docs/operating/otlp.md` — ports, env, client snippets, troubleshooting.
- Update `mkdocs/docs/architecture/` (or equivalent) to mention OTLP as a supported wire format.
- Update `README.md` feature list and roadmap.
- Add a `mkdocs/docs/guides/coding-agents.md` walkthrough showing the Claude Code OTel config + a starter set of DataFusion queries (cache hit ratio, redundant tool calls, time-in-tool ratio) — leverages the driving use case to demonstrate value.

## Testing Strategy

**Unit**:
- ID derivation stability (`identity` module).
- Translation of representative `ResourceLogs` / `ResourceSpans` / `ResourceMetrics` payloads — assert produced row records match expected.
- Sentinel dispatch in each block processor — fed both legacy transit blocks and v1 OTel blocks, expect both to land correctly.

**Integration** (in `tests/` of `otel-ingestion`):
- Use `opentelemetry_sdk` in test code to construct realistic protobuf payloads; round-trip through translate → block_builder → block processor → in-memory Arrow recordbatch; assert the final shape.

**End-to-end** (via `local_test_env`):
- Start `telemetry-ingestion-srv` with the OTLP routes enabled. Run a Python OTel SDK script (with `OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf`) emitting ~100 spans/logs/metrics. Query `log_entries`, `otel_spans`, `measures` via FlightSQL; assert counts match.
- Run Claude Code with `CLAUDE_CODE_ENABLE_TELEMETRY=1` pointed at the local server; assert at least the documented metrics (`claude_code.token.usage`, `claude_code.session.count`) land.

**Performance smoke**:
- Throughput target: 10k spans/sec on a single ingest pod (matches the existing CBOR pathway). The translation cost is dominated by JSON-encoding properties; if it's too slow, fall back to a tighter binary attribute encoding.

## Open Questions

1. **`opentelemetry-proto` version pinning**: 0.31 confirmed compatible with `prost 0.14.3` (verified during research). Lock at first use.
2. **`trace_id` representation in `otel_spans`**: `FixedSizeBinary[16]` (compact, exact lookups by bytes), `Utf8` hex string (human-friendly in `WHERE`), or both via a generated column. Affects query ergonomics and storage size.
3. **Stream lifecycle**: OTel processes run for days; the stream's `objects_metadata` (format descriptor) is stored once at registration. Is stream rotation needed, or does time-based lakehouse partitioning handle it?
4. ~~**Per-tenant rate limiting**~~ — decided: out of scope for v1. Add when there's a real noisy-neighbor problem.
5. **`otel_spans` vs unified `spans` view**: want a single `spans` view (with a `source` discriminator) that unifies native async-spans and OTel spans for cross-source trace queries? Cost: schema-design complexity — the two sources have non-overlapping identity (pointer-id vs span_id) and partially overlapping columns. Could ship as a DataFusion view (UNION) without changing storage.
6. ~~**Block payload re-encoding**~~ — decided: accept the cost. Splitting an `ExportRequest` into per-Resource blocks re-encodes each submessage; an offset/length scheme into a shared envelope would save the re-encoding but make blocks non-self-contained. OTel ingestion is allowed to be less efficient than the native micromegas-tracing path; producers that want minimum overhead use the native SDK.
7. **OpenTelemetry Collector sample config**: ship one that fans out to Micromegas + a file exporter for production safety? Natural follow-up, not core.
8. ~~**Compaction interaction**~~ — decided: nothing to do. The existing lakehouse compaction handles small-block influxes the same way it does for native streams.

## References

- OTLP spec: https://opentelemetry.io/docs/specs/otlp/
- OTel proto repo: https://github.com/open-telemetry/opentelemetry-proto
- Claude Code monitoring: https://code.claude.com/docs/en/monitoring-usage
- Anthropic monitoring guide: https://github.com/anthropics/claude-code-monitoring-guide
- `opentelemetry-proto` crate: https://crates.io/crates/opentelemetry-proto

## Appendix: Claude Code OTel client configuration

Typical env vars an operator sets to make Claude Code emit OTel data into a Micromegas server. Set these in the shell that launches `claude` (e.g., `~/.bashrc`, `~/.zshrc`, or a wrapper script).

```bash
# ── Required: enable Claude Code's OTel emission ─────────────────────────────
export CLAUDE_CODE_ENABLE_TELEMETRY=1

# ── Required: where to send the data ─────────────────────────────────────────
# Base URL only — the SDK appends /v1/{signal} per OTLP spec, so this points
# at /ingestion/otlp and the SDK POSTs to /ingestion/otlp/v1/{logs,metrics,traces}.
export OTEL_EXPORTER_OTLP_ENDPOINT="https://micromegas.example.com/ingestion/otlp"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"   # gRPC not supported in v1

# ── Required: which signals to export ────────────────────────────────────────
export OTEL_METRICS_EXPORTER=otlp
export OTEL_LOGS_EXPORTER=otlp

# ── Required: auth ───────────────────────────────────────────────────────────
# The bearer token comes from the MICROMEGAS_API_KEYS keyring on the server.
# OTel does not expand ${VAR}; the shell does at export time.
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer mm_abc123def456..."

# ── Optional: distributed tracing (Claude Code beta) ─────────────────────────
# Required for the interaction → llm_request → tool span hierarchy and for
# OTEL_LOG_TOOL_CONTENT below.
export CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1
export OTEL_TRACES_EXPORTER=otlp

# ── Optional: tool-level visibility (privacy-sensitive) ──────────────────────
# Tool name + args/result summaries on every tool span/event.
export OTEL_LOG_TOOL_DETAILS=1
# Full tool content (file paths, bash output, edit diffs). Spans only;
# requires CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1.
export OTEL_LOG_TOOL_CONTENT=1

# ── Optional: full Messages API request/response bodies ──────────────────────
# Recommended sink is a file (not the OTLP exporter) due to size.
export OTEL_LOG_RAW_API_BODIES="file:/var/log/claude-code-bodies"

# ── Optional: tag the data for multi-team rollups ────────────────────────────
# Lands on the Process row's properties (and via process_properties on every
# row). The trailing service.namespace=... can be omitted if the server's
# MICROMEGAS_OTLP_NAMESPACE_MAP defaults it from the API-key name.
export OTEL_RESOURCE_ATTRIBUTES="team.id=platform,cost_center=eng-123,deployment.environment=prod"

# ── Launch ───────────────────────────────────────────────────────────────────
claude
```

### Server-side counterpart

```bash
# Same JSON keyring as telemetry-ingestion-srv already uses
export MICROMEGAS_API_KEYS='[{"name":"team-platform","key":"mm_abc123def456..."}]'

# Optional: default service.namespace per API key when the client omits it
export MICROMEGAS_OTLP_NAMESPACE_MAP='{"team-platform":"platform"}'

# OTLP-specific body limit (existing /ingestion/insert_block stays at 100MB)
export MICROMEGAS_OTLP_MAX_RECV_BYTES=10000000

# Existing ingestion env vars unchanged
export MICROMEGAS_SQL_CONNECTION_STRING="postgres://..."
export MICROMEGAS_OBJECT_STORE_URI="s3://..."
```

### Verifying it works

After Claude runs once with the above env, on the server:

```sql
SELECT process_id, exe, computer, properties->>'service.instance.id'
FROM processes
WHERE properties->>'otel.resource.service.name' = 'claude-code'
ORDER BY start_time DESC LIMIT 5;

SELECT count(*) FROM log_entries
WHERE process_id IN (SELECT process_id FROM processes
                      WHERE properties->>'otel.resource.service.name' = 'claude-code');
```
