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

Since we're scoping v1 to OTLP/HTTP only (see Trade-offs for the gRPC/JSON discussion), we add the three OTLP routes to the existing `telemetry-ingestion-srv` rather than creating a new binary. Shared auth middleware, shared `WebIngestionService` instance, one deployment unit. A new `rust/otel-ingestion/` library hosts the proto decode + identity synthesis + block-builder logic; the server crate just wires axum routes to it.

OTel-specific decode lives in `analytics/`, where the parquet schema is the natural translation target anyway. New block processors (one per signal) prost-decode the payload via `opentelemetry-proto` and emit rows.

```
       ┌─────────────────────────────────────────────────┐
       │  telemetry-ingestion-srv (EXISTING, EXTENDED)   │
       │   ─ existing routes:                            │
       │       POST /ingestion/{insert_process,           │
       │             insert_stream, insert_block}        │
       │   ─ NEW routes (shared auth + ingestion lib):   │
       │       POST /ingestion/otlp/v1/logs              │
       │            ↳ ExportLogsServiceRequest proto     │
       │       POST /ingestion/otlp/v1/metrics           │
       │            ↳ ExportMetricsServiceRequest proto  │
       │       POST /ingestion/otlp/v1/traces            │
       │            ↳ ExportTraceServiceRequest proto    │
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
       │   ─ otel_spans   (NEW — JIT-only per-process,   │
       │                  first-class trace_id)          │
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

The existing `dependencies_metadata BYTEA` and `objects_metadata BYTEA` fields stay as-is. Their interpretation is now defined by `format`:
- For `micromegas-transit`: CBOR-encoded `Vec<UserDefinedType>` (current behavior).
- For `otlp/v1/*`: **both** fields contain a CBOR-encoded **empty** `Vec<UserDefinedType>` — the single byte `0x80`, NOT a zero-length BYTEA.

Why a single byte and not empty: each field is decoded via `ciborium::from_reader::<Vec<UserDefinedType>>(...)` in four places (`analytics/src/metadata.rs:127-131`, `analytics/src/lakehouse/partition_source_data.rs:188-190`, `analytics/src/lakehouse/jit_partitions.rs:329-331`, `analytics/src/lakehouse/parse_block_table_function.rs:120,131`). An empty BYTEA would fail those decodes; a CBOR-encoded empty array decodes to `Vec::new()` and every existing code path (which then iterates the empty Vec) becomes a no-op without any code change. Future per-format extensions (e.g., schema URL pin) can encode richer structures here, gated on `format`.

**One block per OTLP request per resource**. An incoming `ExportTraceServiceRequest` may carry multiple `ResourceSpans` (different services). We split into one block per resource so `process_id` is unambiguous on the metadata row. Each block's payload is the protobuf encoding of *one* `ResourceSpans` (or `ResourceMetrics` / `ResourceLogs`) — small, self-contained, and re-decodable in isolation.

### Wire-level: protocol crate

Use `opentelemetry-proto` **without** the `gen-tonic` feature — we only need the prost-generated message types (`ResourceLogs`, `ResourceMetrics`, `ResourceSpans`, `ExportLogsServiceRequest`, etc.), not the gRPC service stubs.

Per the OTLP/HTTP spec, `OTEL_EXPORTER_OTLP_ENDPOINT` is a base URL and the SDK appends `/v1/{signal}`, so clients set `OTEL_EXPORTER_OTLP_ENDPOINT=https://host.example.com/ingestion/otlp` and the SDK POSTs to `.../ingestion/otlp/v1/{logs,metrics,traces}`. Per-signal endpoint vars (`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`) are full URLs — operators using them write the full path.

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
- `\x1F` separates concatenated string fields to prevent tuple-boundary collisions (e.g. `("abc", "")` vs `("ab", "c")`). OTel allows any UTF-8 in attribute values, but a real-world resource attribute containing `\x1F` is negligible in practice; we accept the residual collision risk rather than escaping.
- If all of `host.id`, `host.name`, `process.pid`, `service.instance.id` are empty, log a degenerate-resource warning so we notice — produces a single collapsed `process_id` per `(service.namespace, service.name)`.

The first time a `process_id` is seen, INSERT into `processes` (idempotent — existing `ON CONFLICT DO NOTHING`) with:
- `exe` ← `service.name` (or `service.namespace + "/" + service.name` if namespace present)
- `username` ← `user.name` if present, else empty
- `realname` ← same as `username` (OTel has no analogue for a separate "real name"; reuse `user.name`)
- `computer` ← `host.name` if present, else empty
- `distro` ← `os.description` if present
- `cpu_brand` ← `host.cpu.model.name` if present, else empty
- `tsc_frequency` ← `1_000_000_000` (OTel timestamps are nanoseconds; ticks = ns)
- `start_time` ← `process.creation.time` if present, else first observed event time
- `start_ticks` ← `start_time` converted to nanoseconds since the Unix epoch (consistent with `tsc_frequency = 1_000_000_000`, so `time = start_time + (ticks - start_ticks) / tsc_frequency` collapses to identity for OTel data)
- `parent_process_id` ← always `NULL` for OTel-derived processes (OTel has no parent-process concept; reserved for native producers)
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

**Per-block processor dispatch (architectural change).** The current architecture binds a single `BlockProcessor` to a view: `LogView::make_batch_partition_spec` constructs `BlockPartitionSpec { block_processor: Arc::new(LogBlockProcessor {}) }` (`analytics/src/lakehouse/log_view.rs:117,178`), and `fetch_partition_source_data` filters streams by tag only (`WHERE array_has("streams.tags", '{tag}')` in `analytics/src/lakehouse/partition_source_data.rs:254`). A naive add-only approach would route OTLP-encoded blocks (tagged `"log"`) into `LogBlockProcessor`, which decodes them as transit/POD payloads and fails.

Resolution: introduce per-block format dispatch by changing two pieces:

1. **Source data carries `format`.** Extend `fetch_partition_source_data` to `SELECT "streams.format"` alongside the existing columns and propagate it into `PartitionSourceBlock` (alongside `block`, `stream`, `process`). Update `blocks_view`, `streams_view`, and `partition_source_data` row decoding to surface the new column. This is non-trivial because `streams_view` (an `SqlBatchView`) materializes its own parquet, so its schema needs the new column too.

2. **`BlockPartitionSpec` dispatches by format.** Replace the single `block_processor: Arc<dyn BlockProcessor>` field with a `block_processors: HashMap<&'static str, Arc<dyn BlockProcessor>>` keyed by format string. The spec selects the processor per source block. `LogView` registers `{"micromegas-transit": LogBlockProcessor, "otlp/v1/logs": OtelLogsBlockProcessor}`; `MetricsView` does the analogous thing. `OtelSpansView` does **not** participate in batch dispatch — it is a JIT-only per-process view in v1 (see Phase 4).

3. **JIT path takes the same map.** Per-process JIT views (used by `LogView::jit_update`, `MetricsView::jit_update`, `AsyncEventsView::jit_update`, and the new `OtelSpansView::jit_update`) call `write_partition_from_blocks(... block_processor: Arc<dyn BlockProcessor>)` directly (`jit_partitions.rs`). Change that signature to `block_processors: HashMap<&'static str, Arc<dyn BlockProcessor>>` and dispatch per source block the same way `BlockPartitionSpec::write` does. All four call sites must be updated; `AsyncEventsView` only registers `{"micromegas-transit": AsyncEventsBlockProcessor}` since OTel spans go to the new `otel_spans` view, not async_events. `OtelSpansView` registers only `{"otlp/v1/traces": OtelSpansBlockProcessor}` (no native source for OTel spans). Without this, OTel JIT views can't dispatch by format — only the global batch path would work.

Trade-off considered: an alternative is to keep dispatch view-bound and split into sibling views (`otel_log_entries`, `otel_measures`). That keeps the block-processing hot path unchanged but breaks the "native + OTel queryable from one view" promise and forces user queries to `UNION`. We chose dispatch-by-format because logs/metrics genuinely *are* the same tabular shape and forcing two views is a leaky abstraction.

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
| `severity_number` 1–24 | `level` Int32, mapped to the Micromegas `Level` enum (`Fatal=1, Error=2, Warn=3, Info=4, Debug=5, Trace=6`): OTel TRACE 1–4 → `6`, DEBUG 5–8 → `5`, INFO 9–12 → `4`, WARN 13–16 → `3`, ERROR 17–20 → `2`, FATAL 21–24 → `1` |
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
trace_id (FixedSizeBinary[16]),
span_id  (FixedSizeBinary[8]),
parent_span_id (FixedSizeBinary[8], nullable),
start_time, end_time, duration,
name, scope, kind, status, status_message,
properties (JSONB),
events (JSONB — array of {time, name, attributes}),
links  (JSONB — array of {trace_id, span_id, attributes})
```

`SpanKind`, status code are first-class on the row because they are load-bearing for filtering (kind, status). Span events and links go in plain `Binary` columns carrying JSONB bytes: predicates over them rarely push into nested-Struct parquet readers anyway, and JSONB absorbs schema evolution (e.g. OTel adding `dropped_attributes_count` to events) without bumping the parquet schema. Access at query time goes through the same `jsonb_*` UDFs already used for `properties`. We deliberately do **not** use `Dictionary(Int32, Binary)` here (the encoding `properties` uses): the `properties` blob repeats across many rows in a partition (it carries process/stream-wide attributes), but per-span `events`/`links` arrays are essentially unique per row, so a dictionary would have cardinality ≈ row count and pure indirection overhead with no compression payoff.

#### Metrics

- **Sum / Gauge** → `measures` rows directly. `name`, `unit`, `value` (int widened to f64), `time`. `aggregation_temporality` and `is_monotonic` go into row properties.
- **Histogram, ExponentialHistogram** → deferred (out of scope for v1). The block processor logs and skips them.
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
   6. Call `WebIngestionService::insert_block_typed(block)` (typed entry point added for the OTel path; see Phase 1 step 5).
4. Return OTLP success (or RESOURCE_EXHAUSTED on PG/object-store failure, INVALID_ARGUMENT on parse error).

There's no event-by-event translation at ingest. The hot path serializes one protobuf submessage and writes it.

### Tick calibration and block time bounds

Native blocks carry `tsc_frequency` and `(begin_ticks, end_ticks)` so timestamps can be rebuilt at query time. OTel timestamps are already absolute nanoseconds. We set `tsc_frequency = 1_000_000_000` so ticks ≡ Unix nanoseconds, and the existing tick→time math passes through cleanly.

`block_wire_format::Block` has four time fields: `begin_time`/`end_time` (RFC3339 strings, persisted as `TIMESTAMPTZ` on the `blocks` row) and `begin_ticks`/`end_ticks` (`i64`). With OTel's identity calibration, `*_ticks` and `*_time` carry the same instant in two encodings — `begin_ticks = begin_time_unix_nano`, `begin_time = DateTime::<Utc>::from_timestamp_nanos(begin_ticks).to_rfc3339()`; same for `end`. The `blocks.begin_time`/`end_time` columns drive partition-time filtering, so they need to actually bracket the records in the payload.

The block-builder walks the `ResourceX` submessage once before re-encoding it and computes min/max nanos across all records:

| Signal | Per-record timestamp fields |
|---|---|
| `ResourceLogs` | `LogRecord.time_unix_nano`; fall back to `observed_time_unix_nano` when it's 0 (per OTLP spec, `time_unix_nano` is optional and SDKs are required to set the observed timestamp) |
| `ResourceSpans` | min over `Span.start_time_unix_nano`, max over `Span.end_time_unix_nano` |
| `ResourceMetrics` | min/max over `DataPoint.time_unix_nano` across every Sum/Gauge data point (Histogram/ExponentialHistogram/Summary points are skipped by the v1 processor but their timestamps still count toward bounds so the block insert-time predicate matches what's actually in the payload) |

Edge cases:
- **Empty Resource submessage** (no records survive the walk): skip — don't write a block. Bounds are undefined.
- **All-zero timestamps** (broken instrumentation; e.g. logs missing both `time_unix_nano` and `observed_time_unix_nano`): fall back to ingest wall clock for both bounds and emit a warning. Otherwise the block lands at `1970-01-01` and breaks partition pruning.

### Backpressure and HTTP semantics

OTLP/HTTP is one POST per export call. The proto response body carries `partial_success { rejected_*_count, error_message }` for batch-level reporting; in v1 we use whole-batch accept/reject (SDKs handle that fine) and don't fill in partial counts.

- 20 MiB body limit on OTLP routes (matches the OTel Collector's `confighttp.max_request_body_size` default, so anything an SDK is willing to send under the conventional Collector cap will go through here too; sub-Router mechanics in Phase 1 step 6).
- HTTP status mapping:
  - DB/object-store transient failures → `503 Service Unavailable` (retryable per OTLP/HTTP spec).
  - Parse errors / unknown content-type → `400 Bad Request` (non-retryable).
  - Auth failures → `401 Unauthorized`.
- No rate limiting in v1.

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

#### What we are NOT doing in v1

- **mTLS / client certs**. The existing auth crate doesn't support it; the axum middleware would need a separate code path. Skip until someone asks.
- **Per-tenant rate limiting**. Out of scope for v1. The auth context exposes `subject` (API-key name) which is enough to add it later as a tower layer; we'll do it when we have a real noisy-neighbor problem.
- **Per-route auth requirements**. Every OTLP route requires auth; no public health endpoint on the OTel listener (health goes on a separate unauthenticated port).

### Configuration

OTLP routes share the existing `--listen-endpoint-http` listener (default `127.0.0.1:8081`) and the existing env vars (`MICROMEGAS_SQL_CONNECTION_STRING`, `MICROMEGAS_OBJECT_STORE_URI`, `MICROMEGAS_API_KEYS`, `MICROMEGAS_OIDC_CONFIG`). No new env vars in v1.

The OTLP body limit is a compile-time constant (20 MiB, matching the OTel Collector's `confighttp.max_request_body_size` default), separate from the 100MB limit on `/ingestion/insert_block` — see Phase 1 step 6 for the layering. Promoting it to a runtime knob is straightforward later if an operator needs it.

## Implementation Steps

### Phase 1: schema + OTLP routes on telemetry-ingestion-srv
1. Schema migration in `rust/ingestion/src/sql_migration.rs`:
   - Bump `LATEST_DATA_LAKE_SCHEMA_VERSION` from `3` to `4`.
   - Add `upgrade_data_lake_schema_v4(tr)` running `ALTER TABLE streams ADD COLUMN format TEXT NOT NULL DEFAULT 'micromegas-transit';` and `UPDATE migration SET version=4;`.
   - Wire it into `execute_migration` with an `if 3 == current_version { ... }` block, mirroring the v2/v3 dispatch pattern.
   - Do **not** alter `create_streams_table` in `sql_telemetry_db.rs`. The codebase convention (e.g. `insert_time` on `blocks` was added in v2 without touching `create_blocks_table`) is that `create_tables()` always creates the v1 schema and migrations bring it forward. Fresh installs go `create_tables` (v1) → v2 → v3 → v4; touching `create_streams_table` would make the v4 `ALTER TABLE ... ADD COLUMN format` fail with "column already exists".
2. Add `format` to the streams write path. Two callers write to the streams table — both need updating because PostgreSQL's positional `INSERT INTO streams VALUES($1,...)` breaks when a column is added without specifying names:
   - `ingestion/src/web_ingestion_service.rs:146` — switch to a named-column `INSERT INTO streams (stream_id, process_id, ...) VALUES (...)` and bind a hard-coded `'micromegas-transit'` for `format` (the CBOR `StreamInfo` wire struct is unchanged — native producers don't know about format).
   - `analytics/src/replication.rs:54` — same change (also reads/writes the streams table).
   - The OTel ingest path does NOT use `WebIngestionService::insert_stream(body: bytes::Bytes)` (that decodes CBOR `StreamInfo`); instead, the new `otel-ingestion` crate calls a new typed method `WebIngestionService::register_otel_stream(stream_id, process_id, tags, properties, format) -> Result<...>` that writes directly via SQL with the named-column INSERT. The method hard-codes both `dependencies_metadata` and `objects_metadata` to the single byte `0x80` (CBOR-encoded empty `Vec<UserDefinedType>`) so the four ciborium decode sites continue to work uniformly across formats. This keeps native producers' wire protocol untouched.
3. Add `opentelemetry-proto = "0.31"` to workspace deps **without** the `gen-tonic` feature (we only need the prost message types).
4. Create `rust/otel-ingestion/` library crate: `identity.rs` (resource → process_id/stream_id), `proto.rs` (re-exports), `error.rs`, `block.rs` (split ExportRequest into per-resource blocks). All translation logic lives here; the server only wires axum routes.
5. Add typed entry points on `WebIngestionService` so the OTel adapter doesn't pay a CBOR encode/decode round-trip on the hot path:
   - `insert_block_typed(block: block_wire_format::Block) -> Result<...>` — takes a `Block` directly, CBOR-encodes the payload once, writes to object storage, and runs the same PG INSERT. Refactor the existing `insert_block(body: bytes::Bytes)` to be a thin wrapper that ciborium-decodes and delegates.
   - `register_otel_process(process_id, exe, username, computer, distro, cpu_brand, tsc_frequency, start_time, start_ticks, properties) -> Result<...>` — writes a row to `processes` via SQL with named columns, matching the existing `ON CONFLICT (process_id) DO NOTHING` idempotency. (Companion to `register_otel_stream` from step 2.) The `processes` table has 13 columns; the three not in the parameter list are filled in by the function: `insert_time = Utc::now()` (server-side wall clock, mirroring the existing `insert_process` at `web_ingestion_service.rs:189`), `parent_process_id = NULL` (OTel has no parent-process concept), and `realname = username` (OTel has no separate "real name" attribute, so we don't burden the caller with passing the same value twice).
   - Rationale: the OTel adapter already has typed `Block`/process structs in hand. Making it CBOR-encode just so `insert_block`/`insert_process` can immediately CBOR-decode and re-encode is pure waste, and the existing `insert_process(body: Bytes)` would force the OTel path to construct a CBOR `ProcessInfo` only to have it decoded again.
6. Add OTLP routes to `telemetry-ingestion-srv/src/main.rs`. The existing 100MB body limit is applied globally to the protected router (`main.rs:62`); to scope the new 20 MiB limit to OTLP only without weakening the 100MB on `/ingestion/insert_block`:
   - Build OTLP routes as a separate sub-Router with its own `DefaultBodyLimit::disable()` + `RequestBodyLimitLayer::new(20 * 1024 * 1024)`.
   - `.merge()` it into the protected app **before** the existing 100MB layer; axum applies per-Router body-limit layers to routes within that sub-Router, and the outer 100MB layer doesn't override the tighter inner one. Verify with an integration test (>20 MiB POST to OTLP → 413; >20 MiB POST to `/ingestion/insert_block` → still accepted).
   - Routes share the existing listener and `auth_middleware`. Each handler reads the protobuf body, calls into `otel-ingestion` to split + register + write blocks via the shared `WebIngestionService`. Stream registration sets `format = "otlp/v1/<signal>"`.
7. End-to-end smoke test: Python OTel SDK with `OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf` pointed at the existing ingestion endpoint; verify rows land in PG `processes`/`streams`/`blocks` (with `format = "otlp/v1/..."` on streams) and bytes land in object store.

### Phase 2: per-block format dispatch + logs block processor → log_entries
1. Plumb `format` through the block-source pipeline (see "Per-block processor dispatch" in Design for the why): `blocks_view.rs` (project `streams.format`, bump file_schema_hash), `streams_view.rs` (add `first_value("streams.format") as format` to transform_query, `first_value(format) as format` to merge_query), `partition_source_data.rs` (add `"streams.format"` to the SELECT at line 247-252; add `format: String` field to `PartitionSourceBlock`; read it via `string_column_by_name(&b, "streams.format")?` in the row-decoding loop), `jit_partitions.rs` (add `"streams.format"` to the SELECT at line 257-258 in `generate_process_jit_partitions_segment` and read it into the per-row `PartitionSourceBlock`), `replication.rs` row decoding, `parse_block_table_function.rs` (SELECT `"streams.format"` + post-read format check, see Files to Modify for details).
2. Convert `BlockPartitionSpec`: replace the single `block_processor` field with `block_processors: HashMap<&'static str, Arc<dyn BlockProcessor>>`. Per-block selection in `BlockPartitionSpec::write`. Unknown formats → `warn!` and skip.
3. Update the JIT path's `write_partition_from_blocks` (`jit_partitions.rs`) to take `block_processors: HashMap<&'static str, Arc<dyn BlockProcessor>>` instead of a single `Arc<dyn BlockProcessor>`, dispatching per source block. Update **all three** call sites — `LogView::jit_update`, `MetricsView::jit_update`, and `AsyncEventsView::jit_update` — to pass the map. (Without this, JIT views — used for per-process queries — can't dispatch OTel blocks; missing the async-events caller would also break compilation.)
4. Add `OtelLogsBlockProcessor` (`analytics/src/lakehouse/otel_logs_block_processor.rs`): prost-decodes `ResourceLogs`, walks scope/log records, emits `log_entries` rows (level collapse from `severity_number`, fold trace_id/span_id/severity_text into properties).
5. Register both processors in `LogView::make_batch_partition_spec` and `LogView::jit_update`: `"micromegas-transit"` → `LogBlockProcessor`, `"otlp/v1/logs"` → `OtelLogsBlockProcessor`.
6. End-to-end test: emit OTel logs from Python AND native logs from a Rust producer in the same test; verify both populate `log_entries` cleanly.

### Phase 3: metrics block processor → measures
1. Add `analytics/src/lakehouse/otel_metrics_block_processor.rs`. Decodes `ResourceMetrics`.
2. Materializes Sum + Gauge data points into `measures`. Adds `aggregation_temporality` and `is_monotonic` to row properties. Logs and skips Histogram/ExponentialHistogram/Summary (deferred — see Trade-offs).
3. Register `OtelMetricsBlockProcessor` for `"otlp/v1/metrics"` in `MetricsView`'s `block_processors` map alongside the existing native processor.
4. End-to-end test using Claude Code's emission: `claude_code.token.usage`, `claude_code.cost.usage`, `claude_code.session.count`.

### Phase 4: spans block processor → new otel_spans view (JIT-only, per-process)

`otel_spans` is a JIT-only per-process view in v1 — mirrors the `AsyncEventsView` pattern. There is no global batch path. Users query it as `view_instance('otel_spans', '<process_id>')`. Cross-process trace traversal (querying `WHERE trace_id = X` across all services) is a documented v1 limitation; it requires the user to know which `process_id`s participate in the trace, or to UNION across multiple `view_instance` calls. Re-evaluate when production volume justifies the storage cost of a global table or trace-skeleton index.

1. Add `analytics/src/lakehouse/otel_spans_table.rs` (Arrow schema) and `otel_spans_view.rs`. The view mirrors `AsyncEventsView` for the JIT-only/per-process *structure* — `make_batch_partition_spec` returns `bail!("not supported")`; `jit_update` calls `generate_process_jit_partitions(... stream_tag = "trace")` and `write_partition_from_blocks(...)` with `block_processors = {"otlp/v1/traces": OtelSpansBlockProcessor}`. View instance id is the `process_id` (UUID string); `"global"` is rejected. **Time conversion path differs from `AsyncEventsView`**: OTel timestamps are absolute nanoseconds (`tsc_frequency = 1_000_000_000`), so `OtelSpansView::jit_update` uses the simpler `find_process` flow (like `LogView::jit_update` at log_view.rs:139-148) rather than `find_process_with_latest_timing` + `make_time_converter_from_latest_timing`. `OtelSpansBlockProcessor` does NOT take a `ConvertTicks` constructor argument — it reads `time_unix_nano` fields directly off the proto.
2. Add `otel_spans_block_processor.rs`. Decodes `ResourceSpans` and materializes one row per span. Span events and links serialize to JSONB and land in dictionary-encoded `Binary` columns, same encoding as `properties`.
3. Register the view in the view factory. `OtelSpansViewMaker` follows `AsyncEventsViewMaker`'s per-process-only pattern: `make_view` rejects `"global"` and constructs an `OtelSpansView` for any other instance id. (Not `LogViewMaker`, which permits both global and per-process — `OtelSpansView` is JIT-only per Phase 4 step 1.) Unlike `AsyncEventsViewMaker`, `OtelSpansViewMaker` is a unit-like struct (no `view_factory: Arc<ViewFactory>` field) — that field on `AsyncEventsViewMaker` exists only to feed `find_process_with_latest_timing`, which `OtelSpansView` doesn't use (see Phase 4 step 1).
4. End-to-end test: emit a multi-span trace from one process; verify trace traversal via `SELECT * FROM view_instance('otel_spans', '<process_id>') WHERE trace_id = ...`.

### Phase 5: production hardening
1. Monitoring + alerts on ingest latency / queue depth. (Body size limit, partial-success policy, rate limiting all decided elsewhere — see Backpressure and "What we are NOT doing in v1".)

### Phase 6: docs + ops
1. `mkdocs/docs/operating/otlp.md` — ports, env, client config snippets (Claude Code, Goose, Python OTel SDK), auth header format, troubleshooting.
2. `mkdocs/docs/guides/coding-agents.md` — Claude Code OTel config + starter DataFusion queries (cache hit ratio, redundant tool calls, time-in-tool ratio).
3. Mention OTLP as a supported wire format in `mkdocs/docs/architecture/`.
4. Update `README.md` feature list and roadmap.
5. Add example Claude Code env to `local_test_env/`.

## Files to Modify

**New crate**:
- `rust/otel-ingestion/Cargo.toml` + `src/{lib,proto,identity,block,error}.rs`

**New modules in `analytics/`**:
- `rust/analytics/src/lakehouse/otel_logs_block_processor.rs`
- `rust/analytics/src/lakehouse/otel_metrics_block_processor.rs`
- `rust/analytics/src/lakehouse/otel_spans_table.rs` (Arrow schema)
- `rust/analytics/src/lakehouse/otel_spans_view.rs` (per-process JIT view, mirrors `AsyncEventsView`)
- `rust/analytics/src/lakehouse/otel_spans_block_processor.rs`

**Modified**:
- `rust/Cargo.toml` (add `opentelemetry-proto = "0.31"`; new `otel-ingestion` member)
- `rust/ingestion/src/sql_migration.rs` (bump `LATEST_DATA_LAKE_SCHEMA_VERSION` to 4; add `upgrade_data_lake_schema_v4` and dispatch)
- `rust/ingestion/src/web_ingestion_service.rs` (named-column `INSERT INTO streams` with `format`; new `register_otel_stream`; new `register_otel_process`; new `insert_block_typed` typed entry point used by the OTel adapter; refactor existing `insert_block(body)` to ciborium-decode and call `insert_block_typed`)
- `rust/analytics/src/replication.rs` (named-column `INSERT INTO streams` with `format`; the streams ingest path at `replication.rs:20-71` reads source columns by name from a `FlightRecordBatchStream`, so it must add `string_column_by_name(&b, "format")?` and bind it to the new column. **Source-schema requirement**: this means the source data lake must also be on schema v4+ — replicating from a v3 source will fail at the column lookup. Document this in the replication operator docs alongside the v4 migration note. We chose hard failure over a silent `'micromegas-transit'` fallback because replication is an admin-driven coordinated operation and a silent default would mask genuine schema-version mismatches.)
- `rust/analytics/src/lakehouse/blocks_view.rs` (project `streams.format` in SQL + Arrow schema; bump `blocks_file_schema_hash()` from `vec![2]` to `vec![3]` so cached partitions built against the old schema are invalidated — same precedent as the JSONB migration that bumped it from `[1]` to `[2]`)
- `rust/analytics/src/lakehouse/streams_view.rs` (carry `format` through `transform_query`/`merge_query`)
- `rust/analytics/src/lakehouse/partition_source_data.rs` (add `format` to source query + `PartitionSourceBlock`)
- `rust/analytics/src/lakehouse/jit_partitions.rs` (carry `format` on `PartitionSourceBlock`; change `write_partition_from_blocks` to take `block_processors: HashMap<&'static str, Arc<dyn BlockProcessor>>` and dispatch per source block)
- `rust/analytics/src/lakehouse/parse_block_table_function.rs` (this SQL table function calls `parse_block(...)` which decompresses `payload.dependencies/objects` as transit bytes — fails on OTel proto payloads. Extend the SELECT at line 86-91 to project `"streams.format"` alongside the existing columns, read it from the resulting RecordBatch, and return a clean `unsupported format` error for `format != "micromegas-transit"`. Decoding OTel payloads here would require routing to the OTel proto walker; defer that to v2 unless a user surfaces the need.)
- `rust/analytics/src/lakehouse/block_partition_spec.rs` (replace single `block_processor` with `block_processors: HashMap<&'static str, Arc<dyn BlockProcessor>>`; per-block dispatch)
- `rust/analytics/src/lakehouse/log_view.rs`, `metrics_view.rs`, and `async_events_view.rs` (register processors in the new map; log/metrics get both native + OTel, async_events gets only the native processor)
- `rust/telemetry-ingestion-srv/Cargo.toml` (add `otel-ingestion`, `opentelemetry-proto`, `prost`)
- `rust/telemetry-ingestion-srv/src/main.rs` (add OTLP routes via a separate sub-Router with its own 20 MiB `RequestBodyLimitLayer`, merged into the protected app)
- `rust/public/src/servers/` (new module for OTLP route registration alongside `ingestion.rs`)
- `rust/analytics/src/lakehouse/mod.rs` (declare new modules: `otel_logs_block_processor`, `otel_metrics_block_processor`, `otel_spans_block_processor`, `otel_spans_view`, `otel_spans_table`)
- `rust/analytics/src/lakehouse/view_factory.rs` (register `otel_spans` view set in `default_view_factory` via `add_view_set`, mirroring the existing `async_events` registration; per-block processor dispatch is configured inside `LogView`/`MetricsView` themselves, not here)
- `README.md`

**No new binary** — OTLP routes live in `telemetry-ingestion-srv`. `rust/tracing/` is also not touched. OTel is an analytics-layer concern; the ingest side is just a thin protobuf-to-block-bytes adapter.

## Trade-offs

**Store OTLP as-is vs translate at ingest** (see Architecture for the full picture): chose store-as-is. Two earlier drafts (a) invented a parallel CBOR record format and (b) added OTel interop event types to the `tracing/` crate. Both bend the architecture around OTel. As-is keeps ingest at auth+write, leaves `tracing/` focused on in-process instrumentation, and is lossless — every attribute, exemplar, link is preserved for later column derivation. The cost: two parsers (transit and OTLP proto), but that's unavoidable in any design.

**HTTP/protobuf only, no gRPC, no JSON**: OTLP/HTTP allows protobuf or JSON encoding per spec, but every SDK we care about supports the protobuf wire form (some default to gRPC — Go, older Java auto-instrumentation — but accept the protobuf-over-HTTP fallback when `OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf` is set). We deliberately scope v1 to `http/protobuf` and skip JSON. HTTP-only means standard HTTP load balancers (ALB, GCLB, nginx, HAProxy) just work — no gRPC/HTTP-2 LB compatibility matrix — and no `tonic` dep. Critically, no transport-stack mismatch with axum, which is why the OTLP routes ride in the existing `telemetry-ingestion-srv` rather than a separate binary. Adding tonic later for v2 gRPC, or a JSON decoder, are contained changes.

**One block per Resource (not per ExportRequest)**: chose per-Resource. An `ExportRequest` may carry multiple resources (different services); splitting at Resource boundaries means each block has an unambiguous `process_id` and is independently re-decodable. Block_id derived from a hash of the bytes for idempotency.

**OTel spans get their own view, not async_events**: chose new `otel_spans` view. The existing `async_events` view is derived from thread-event blocks where parent is inferred from begin/end ordering on a thread; OTel spans carry explicit `parent_span_id` and have no thread-of-origin concept. Forcing OTel into the `async_events` processor would either lie about parent inference or require a pseudo-thread per trace. Native and OTel span data live in sibling views; cross-source queries can `UNION` if needed (open question #5).

**`otel_spans` is JIT-only, per-process in v1**: mirrors `async_events` / `thread_spans`. Spans are the highest-volume signal once auto-instrumentation is on, and a global materialized table would dominate storage cost for a feature whose driving v1 use case (Claude Code) is low volume. Cost: cross-service trace-by-id queries (`WHERE trace_id = X`) need to be issued per process — the user supplies the `process_id` to `view_instance('otel_spans', ...)`. We accept that for v1; if a real cross-process trace-traversal use case shows up, a follow-up adds either a global skeleton index (`time, trace_id, span_id, parent_span_id, process_id, name`) or a full global table.

**`trace_id` and `span_id` as first-class columns**: chose first-class on `otel_spans` (the load-bearing trace-analytics columns), stored as `FixedSizeBinary[16]` and `FixedSizeBinary[8]` — the lengths are fixed by the W3C Trace Context standard, so we save the variable-`Binary` offsets overhead. Half the size of hex `Utf8`, exact-match lookups are byte comparisons; a small `hex(...)` UDF or query-time `encode(trace_id, 'hex')` covers human-readable display. On `log_entries` they go into JSONB until profiling shows they're hot.

**Span events and links as `List<Struct>` vs JSONB**: chose JSONB (matches the existing `properties`-column pattern across the lakehouse). DataFusion filter pushdown into nested-Struct parquet columns is unreliable, so predicates over event/link fields would scan whole row groups and filter in memory anyway; JSONB makes that cost honest and routes through the same `jsonb_*` UDFs already in use. JSONB also tolerates OTel schema evolution (e.g. new `dropped_*` counters) without parquet schema bumps. The argument for `List<Struct>` would be cheap exact-shape access ("the 3rd event's name"), but real trace queries are predicate ("any event where X") or render-the-whole-list — both well-served by JSONB. Exemplars on metric data points → deferred (land in row properties when surfaced).

**Histograms deferred**: Claude Code doesn't emit histograms; the new view + bucket schema is a non-trivial design exercise and would block the driving use case. Sum/Gauge cover the v1 demand.

**OTel path is allowed to be less efficient than native**: explicit non-goal to match the per-event overhead of `micromegas-tracing`. Producers that want minimum overhead use the native SDK; OTel is for compatibility and ecosystem reach. This frees us from optimizations like proto envelope sharing across resources, zero-copy splitting, etc. — re-encoding per Resource is fine.

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
2. ~~**`trace_id` representation in `otel_spans`**~~ — decided: `FixedSizeBinary[16]` for `trace_id`, `FixedSizeBinary[8]` for `span_id` / `parent_span_id`. Lengths are fixed by W3C Trace Context, so the offsets array of variable `Binary` would be pure overhead. A `hex(...)` UDF (or query-time `encode`) handles human-readable display.
3. **Stream lifecycle**: OTel processes run for days; the stream's `objects_metadata` (format descriptor) is stored once at registration. Is stream rotation needed, or does time-based lakehouse partitioning handle it?
4. ~~**Per-tenant rate limiting**~~ — decided: out of scope for v1. Add when there's a real noisy-neighbor problem.
5. **`otel_spans` vs unified `spans` view**: want a single `spans` view (with a `source` discriminator) that unifies native async-spans and OTel spans for cross-source trace queries? Cost: schema-design complexity — the two sources have non-overlapping identity (pointer-id vs span_id) and partially overlapping columns. Could ship as a DataFusion view (UNION) without changing storage. Tied to the v1 JIT-only decision: a unified view would also be per-process in v1 unless we add a cross-process trace index.
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
# row).
export OTEL_RESOURCE_ATTRIBUTES="team.id=platform,cost_center=eng-123,deployment.environment=prod"

# ── Launch ───────────────────────────────────────────────────────────────────
claude
```

### Server-side counterpart

```bash
# Same JSON keyring as telemetry-ingestion-srv already uses
export MICROMEGAS_API_KEYS='[{"name":"team-platform","key":"mm_abc123def456..."}]'

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
