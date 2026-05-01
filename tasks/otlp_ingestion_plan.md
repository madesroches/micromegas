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

Why a single byte and not empty: each field is decoded via `ciborium::from_reader::<Vec<UserDefinedType>>(...)` in four places (`analytics/src/metadata.rs:127-133` — two adjacent decodes for dependencies/objects, `analytics/src/lakehouse/partition_source_data.rs:188-190`, `analytics/src/lakehouse/jit_partitions.rs:329-331`, `analytics/src/lakehouse/parse_block_table_function.rs:120,131`). An empty BYTEA would fail those decodes; a CBOR-encoded empty array decodes to `Vec::new()` and every existing code path (which then iterates the empty Vec) becomes a no-op without any code change. Future per-format extensions (e.g., schema URL pin) can encode richer structures here, gated on `format`.

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
    + process_start_string                 + "\x1F"
    + trim_lower(service.namespace)        + "\x1F"
    + trim_lower(service.name)             + "\x1F"
    + trim_lower(service.instance.id)

process_id = uuid_v5(NS_OTEL_PROCESS_V1, key)
```

`process_start_string` resolves the OTel attribute rename: take `process.start_time` if present (the newer name, OTel semconv ≥ 1.27), else fall back to the deprecated `process.creation.time`, else empty string. Use the attribute's raw `string_value` **verbatim** — no parse/normalize step. Two reasons: (a) the value is opaque to the hash, so any deterministic encoding works as long as it's stable across retries from the same SDK, and (b) parsing risks mismatched output across SDKs that emit subtly different ISO 8601 forms (`...:00Z` vs `...:00.000Z`). If the SDK emits an `int_value` instead of `string_value` (some implementations encode timestamps as int nanos), stringify the int. The pid case follows the same rule.

**Namespace UUID constants** (mint once, pin in `rust/otel-ingestion/src/identity.rs`; the `_V1` suffix is part of the contract — any formula change ships a `_V2` namespace):

```rust
// Generated 2026-05-01 via uuidgen; load-bearing — DO NOT change without bumping to _V2.
pub const NS_OTEL_PROCESS_V1: Uuid = uuid!("8b7d8a3e-3f9c-4f5d-9b3a-1e7c2d4f6a01");
pub const NS_OTEL_STREAM_V1:  Uuid = uuid!("8b7d8a3e-3f9c-4f5d-9b3a-1e7c2d4f6a02");
pub const NS_OTEL_BLOCK_V1:   Uuid = uuid!("8b7d8a3e-3f9c-4f5d-9b3a-1e7c2d4f6a03");
```

(The exact byte values are arbitrary — what matters is they're stable forever once shipped. Implementer should regenerate via `uuidgen` before merging if the placeholders here haven't been finalized; once shipped, never change.)

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

**Block identity**: one block per (process_id, signal) per OTLP request per Resource. A multi-Resource request fans out to N blocks across N processes' streams. `block_id = uuid_v5(NS_OTEL_BLOCK_V1, payload_bytes)` — the re-encoded protobuf bytes of this Resource submessage, fed directly to `uuid_v5` (which already SHA-1s its input; an extra hash step would be redundant). This makes block_id deterministic from payload content alone, so a retried POST collides on `ON CONFLICT (block_id) DO NOTHING`.

### Schema mapping: OTLP → parquet rows

All mappings happen in the analytics-layer block processor, which prost-decodes the OTLP proto and emits parquet rows.

#### Logs → `log_entries`

| OTel field | parquet column |
|---|---|
| `time_unix_nano` | `time` |
| `severity_number` 1–24 | `level` Int32, mapped to the Micromegas `Level` enum (`Fatal=1, Error=2, Warn=3, Info=4, Debug=5, Trace=6`): OTel TRACE 1–4 → `6`, DEBUG 5–8 → `5`, INFO 9–12 → `4`, WARN 13–16 → `3`, ERROR 17–20 → `2`, FATAL 21–24 → `1`. Out-of-range: `severity_number = 0` (UNSPECIFIED) → `Trace=6` (default to least-severe so unspecified logs aren't filtered out by default `WHERE level <= 4` queries); `severity_number > 24` → clamp to `Fatal=1` and emit a debug log. |
| `body.string_value` | `msg` |
| `body.kvlist_value` / `array_value` | JSON-stringified into `msg` (structured-body parsing deferred) |
| `attributes.*` | `properties` JSONB |
| `instrumentation_scope.name` | `target` |
| `trace_id`, `span_id` | `properties.otel.trace_id` / `otel.span_id` (promote later if hot) |
| `severity_text` | `properties.otel.severity_text` |

#### Spans → `otel_spans`

New parquet view columns:

```
process_id          Dictionary(Int32, Utf8),
stream_id           Dictionary(Int32, Utf8),
block_id            Dictionary(Int32, Utf8),
insert_time         Timestamp(Nanosecond, +00:00),
exe                 Utf8,                        -- joined from process
username            Utf8,                        -- joined from process
computer            Utf8,                        -- joined from process
process_properties  Dictionary(Int32, Binary),   -- JSONB, joined from process
trace_id            FixedSizeBinary[16],
span_id             FixedSizeBinary[8],
parent_span_id      FixedSizeBinary[8] (nullable),
start_time          Timestamp(Nanosecond, +00:00),
end_time            Timestamp(Nanosecond, +00:00),
duration            Int64,                       -- end_time - start_time, nanoseconds
name                Dictionary(Int32, Utf8),
kind                Dictionary(Int32, Utf8),     -- "INTERNAL"|"SERVER"|"CLIENT"|"PRODUCER"|"CONSUMER"|"UNSPECIFIED"
status              Dictionary(Int32, Utf8),     -- "OK"|"ERROR"|"UNSET"
status_message      Utf8 (nullable),
properties          Dictionary(Int32, Binary),   -- JSONB; carries scope info as otel.scope.* keys (see below)
events              Binary,                      -- JSONB array: [{time, name, attributes}]
links               Binary                       -- JSONB array: [{trace_id, span_id, attributes}]
```

**No top-level `scope` column** — instrumentation scope (`name`, `version`, `schema_url`, scope attributes) lives inside `properties` under the `otel.scope.*` prefix, consistent with the "Resource and Scope attributes" table below and with the rationale in the Stream identity section (one stream per signal per process, scope is sub-stream metadata). Earlier drafts of this doc listed `scope` as a column; that's resolved — scope is JSONB-only.

`SpanKind`, status code are first-class on the row because they are load-bearing for filtering (kind, status); they're materialized as small dictionaries (cardinality ≤ 6 for kind, ≤ 3 for status). Span events and links go in plain `Binary` columns carrying JSONB bytes: predicates over them rarely push into nested-Struct parquet readers anyway, and JSONB absorbs schema evolution (e.g. OTel adding `dropped_attributes_count` to events) without bumping the parquet schema. Access at query time goes through the same `jsonb_*` UDFs already used for `properties`. We deliberately do **not** use `Dictionary(Int32, Binary)` for `events`/`links` (the encoding `properties` uses): the `properties` blob repeats across many rows in a partition (it carries process/stream-wide attributes), but per-span `events`/`links` arrays are essentially unique per row, so a dictionary would have cardinality ≈ row count and pure indirection overhead with no compression payoff.

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

#### Attribute value encoding

OTel `KeyValue.value` is an `AnyValue` oneof: `string`, `bool`, `int`, `double`, `bytes`, `array`, or `kvlist`. Micromegas `properties` is a JSONB blob `{key → value}`. Mapping:

| OTel `AnyValue` variant | JSONB representation |
|---|---|
| `string_value` | JSON string |
| `bool_value` | JSON bool |
| `int_value` (i64) | JSON number (i64; jsonb supports it natively) |
| `double_value` | JSON number (f64) |
| `bytes_value` | base64-encoded JSON string (existing `properties` consumers expect text) |
| `array_value` | JSON array of values, recursively encoded |
| `kvlist_value` | JSON object `{key → value}`, recursively encoded |

Nested arrays/kvlists are preserved as nested JSON — no flattening. Query-time access uses the existing `jsonb_*` UDFs (`jsonb_extract_path`, etc.), which traverse nesting natively. This is the same path taken for `body.kvlist_value` / `array_value` in log records (whose top-level `body` field is JSON-stringified into `msg`, but if it's structured we still serialize inner attributes the same way for consistency).

### Ingest path (per OTLP request)

1. Auth (existing axum middleware — `auth_middleware` from `rust/auth/src/axum.rs`).
2. **Validate `Content-Type`**: must be `application/x-protobuf`. Anything else → `415 Unsupported Media Type` (we deliberately do not implement the JSON variant in v1; see Trade-offs).
3. **Decompress body if `Content-Encoding: gzip`** (OTel SDKs default to gzip — Python, Go, JS, .NET all set `OTEL_EXPORTER_OTLP_COMPRESSION=gzip` by default; without this, real-world OTel traffic is rejected). Wrap the request body in a `flate2::read::GzDecoder` (or use `tower-http::decompression::RequestDecompressionLayer` on the OTLP sub-router) and feed the decompressed bytes to prost. Other compression codecs (`Content-Encoding: deflate`, `zstd`) → 415 in v1; gzip is the only codec the spec mandates SDKs offer.
4. Decode the outer `ExportRequest` proto (just to walk Resource boundaries; we don't decode further).
5. **Empty top-level request** (`resource_logs`/`resource_metrics`/`resource_spans` is empty): per OTLP spec this is valid — return 200 with an empty `Export*ServiceResponse` and do nothing else. Don't insert any rows.
6. For each `ResourceLogs` / `ResourceMetrics` / `ResourceSpans`:
   1. Derive `process_id` from resource attributes (UUIDv5; see Identity).
   2. Derive `stream_id` from `(process_id, signal)` — max 3 streams per process. Scope info travels in row properties, not in stream identity.
   3. Idempotent register process + stream via `INSERT ... ON CONFLICT DO NOTHING`. No in-memory dedup cache — we let PG be the source of truth. Redundant inserts on hot paths are cheap (one round-trip, no table change, no WAL write) and avoid every cross-pod consistency concern. Stream registration sets `format = "otlp/v1/<signal>"`.
   4. Re-encode this Resource sub-message as protobuf bytes — the block payload.
   5. Build `block_wire_format::Block` with `payload.dependencies = []`, `payload.objects = <proto bytes>`.
   6. Call `WebIngestionService::insert_block_typed(block)` (typed entry point added for the OTel path; see Phase 1 step 5).
7. **Return** `200 OK` with `Content-Type: application/x-protobuf` and an empty `Export{Logs,Metrics,Trace}ServiceResponse` proto body (the proto-encoded zero-value of the response message — for whole-batch acceptance this is just an unset `partial_success` field, which serializes to zero bytes). On error: per the HTTP status mapping below, return the appropriate 4xx/5xx code and a `google.rpc.Status` proto body (NOT `Export*ServiceResponse` — see "Response body" in Backpressure).

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

- 20 MiB **decompressed** body limit on OTLP routes (matches the OTel Collector's `confighttp.max_request_body_size` default, so anything an SDK is willing to send under the conventional Collector cap will go through here too; sub-Router mechanics in Phase 1 step 6). The 20 MiB applies to the body axum sees. With `RequestDecompressionLayer` the layer order is: `RequestBodyLimitLayer` (outer, on the wire bytes) → `RequestDecompressionLayer` (inner, expands gzip) → handler. We size the limit for compressed bytes since that's what the wire carries; gzip on text-heavy proto typically expands 5–10×, so ~100–200 MiB of decoded payload fits under a 20 MiB compressed cap. If observed payloads grow past this, bump or split the limit per-route. (Alternative: limit decompressed bytes via `tower-http`'s `Limited<R>`; deferred unless needed.)
- **Content-Type matching is media-type-aware, not string-equality**. Parse the header and accept anything whose media type is `application/x-protobuf` (so `application/x-protobuf; charset=utf-8` and `application/x-protobuf ` with trailing whitespace both pass). Use `mime::Mime` parsing or `axum::http::header::CONTENT_TYPE` extractor with a starts-with check; do NOT `header::exact_ignore_case` since SDKs are allowed to add parameters.
- HTTP status mapping:
  - DB/object-store transient failures → `503 Service Unavailable` (retryable per spec) **with a `Retry-After: 30` response header** (spec: "If the request is retryable, the server SHOULD include Retry-After header"). 30s is a conservative default; tune based on observed recovery times.
  - Parse errors (malformed protobuf, malformed gzip) → `400 Bad Request` (non-retryable).
  - Wrong `Content-Type` (not `application/x-protobuf`) or unsupported `Content-Encoding` (not gzip / not absent) → `415 Unsupported Media Type`.
  - Body exceeds 20 MiB → `413 Payload Too Large`.
  - Auth failures → `401 Unauthorized`.
  - **Full retryable code list per spec**: `429`, `502`, `503`, `504`. We only emit `503` in v1 (no rate limiting → no `429`; no upstream proxy → no `502`/`504`); when rate limiting lands as a tower layer, it returns `429` + `Retry-After`.
- **Response body** (per OTLP/HTTP spec — verified against the spec text and Vector's `vector/src/sources/opentelemetry/reply.rs`):
  - **2xx success** → `Content-Type: application/x-protobuf`, body is an `Export{Logs,Metrics,Trace}ServiceResponse` proto (empty on whole-batch acceptance — zero bytes is the valid wire form; `partial_success` populated only if we ever do per-record acceptance, deferred to v2).
  - **4xx / 5xx error** → `Content-Type: application/x-protobuf`, body is a **`google.rpc.Status` proto** (NOT `Export*ServiceResponse`). Spec quote: *"The response body for all HTTP 4xx and HTTP 5xx responses MUST be a Protobuf-encoded Status message that describes the problem."* Status fields we populate: `code` (gRPC canonical code — e.g., `INVALID_ARGUMENT=3` for parse errors, `UNAUTHENTICATED=16` for 401, `RESOURCE_EXHAUSTED=8` for 413/503), `message` (human-readable). `details` is left empty in v1.
  - **Sourcing the `Status` proto type**: define it locally in `rust/otel-ingestion/src/proto.rs` as a hand-rolled prost message — three fields, no external dep. Pulling in `tonic-types` would drag tonic into the dependency graph, contradicting the "no gRPC stack" decision. Sketch:
    ```rust
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct Status {
        #[prost(int32, tag = "1")] pub code: i32,
        #[prost(string, tag = "2")] pub message: String,
        // tag = "3" is `repeated google.protobuf.Any details` — omitted in v1.
    }
    ```
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
- **Response gzip-encoding**. Spec says the server MAY gzip-encode the response when the client sends `Accept-Encoding: gzip`. Our responses are tiny (empty `Export*ServiceResponse` proto, or a `Status` proto with a short `message`), so compression saves nothing. Add later only if a client complains.
- **Per-record `partial_success`**. v1 is whole-batch accept/reject; no path emits 200 + populated `partial_success`. The infrastructure is wired (response is already an `Export*ServiceResponse` proto), so adding it later is additive.

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
3. Add `opentelemetry-proto = "0.31"` to workspace deps with `default-features = false, features = ["gen-tonic-messages", "logs", "metrics", "trace"]`. The `gen-tonic-messages` feature gates the prost message types in `opentelemetry_proto::tonic::*` (despite the misleading name, this feature does NOT pull in tonic transport — that's `gen-tonic` which adds `tonic/channel` and a `Channel` dep on top). We deliberately stay off `gen-tonic` so the gRPC stack stays out of the dependency graph.
4. Create `rust/otel-ingestion/` library crate: `identity.rs` (resource → process_id/stream_id), `proto.rs` (re-exports + hand-rolled `Status` proto), `error.rs` (HTTP-status-aware `OtelError`), `block.rs` (split ExportRequest into per-resource blocks), `handler.rs` (top-level `ingest_logs/metrics/traces` entry points used by axum). All translation logic lives here; the server only wires axum routes.
5. Add typed entry points on `WebIngestionService` so the OTel adapter doesn't pay a CBOR encode/decode round-trip on the hot path:
   - `insert_block_typed(block: block_wire_format::Block) -> Result<...>` — takes a `Block` directly, CBOR-encodes the payload once, writes to object storage, and runs the same PG INSERT. Refactor the existing `insert_block(body: bytes::Bytes)` to be a thin wrapper that ciborium-decodes and delegates.
   - `register_otel_process(process_id, exe, username, computer, distro, cpu_brand, tsc_frequency, start_time, start_ticks, properties) -> Result<...>` — writes a row to `processes` via SQL with named columns, matching the existing `ON CONFLICT (process_id) DO NOTHING` idempotency. (Companion to `register_otel_stream` from step 2.) The `processes` table has 13 columns; the three not in the parameter list are filled in by the function: `insert_time = Utc::now()` (server-side wall clock, mirroring the existing `insert_process` at `web_ingestion_service.rs:189`), `parent_process_id = NULL` (OTel has no parent-process concept), and `realname = username` (OTel has no separate "real name" attribute, so we don't burden the caller with passing the same value twice).
   - Rationale: the OTel adapter already has typed `Block`/process structs in hand. Making it CBOR-encode just so `insert_block`/`insert_process` can immediately CBOR-decode and re-encode is pure waste, and the existing `insert_process(body: Bytes)` would force the OTel path to construct a CBOR `ProcessInfo` only to have it decoded again.
6. Add OTLP routes to `telemetry-ingestion-srv/src/main.rs`. The existing 100MB body limit is applied globally to the protected router (`main.rs:62`); to scope the new 20 MiB limit to OTLP only without weakening the 100MB on `/ingestion/insert_block`:
   - Build OTLP routes as a separate sub-Router with its own `DefaultBodyLimit::disable()` + `RequestBodyLimitLayer::new(20 * 1024 * 1024)` + `tower_http::decompression::RequestDecompressionLayer::new().gzip(true)` (the layer order matters: `RequestBodyLimitLayer` is outer so it caps wire-bytes, decompression is inner so the handler always sees plain proto). Add `tower-http`'s `decompression-gzip` feature to the workspace `tower-http` dependency entry (currently `features = ["compression-gzip", "cors", "limit", "timeout"]` — add `"decompression-gzip"`).
   - `.merge()` it into the protected app **before** the existing 100MB layer; axum applies per-Router body-limit layers to routes within that sub-Router, and the outer 100MB layer doesn't override the tighter inner one. Verify with an integration test (>20 MiB POST to OTLP → 413; >20 MiB POST to `/ingestion/insert_block` → still accepted; gzip-encoded OTLP POST decompresses correctly).
   - Each handler validates `Content-Type: application/x-protobuf` (415 on mismatch), reads the (already-decompressed) protobuf body, calls into `otel-ingestion` to split + register + write blocks via the shared `WebIngestionService`, and returns a proto-encoded `Export*ServiceResponse` body with `Content-Type: application/x-protobuf`. Routes share the existing listener and `auth_middleware`. Stream registration sets `format = "otlp/v1/<signal>"`.
7. ~~End-to-end smoke test~~ — superseded by Phase 5 (`python/micromegas/tests/test_otlp_e2e.py`).

### Phase 2: per-block format dispatch + logs block processor → log_entries
1. Plumb `format` through the block-source pipeline (see "Per-block processor dispatch" in Design for the why): `blocks_view.rs` (project `streams.format`, bump file_schema_hash), `streams_view.rs` (add `first_value("streams.format") as format` to transform_query, `first_value(format) as format` to merge_query), `partition_source_data.rs` (add `"streams.format"` to the SELECT at line 247-252; add `format: String` field to `PartitionSourceBlock`; read it via `string_column_by_name(&b, "streams.format")?` in the row-decoding loop), `jit_partitions.rs` (add `"streams.format"` to the SELECT at line 257-258 in `generate_process_jit_partitions_segment` and read it into the per-row `PartitionSourceBlock`), `replication.rs` row decoding, `parse_block_table_function.rs` (SELECT `"streams.format"` + post-read format check, see Files to Modify for details).
2. Convert `BlockPartitionSpec`: replace the single `block_processor` field with `block_processors: HashMap<&'static str, Arc<dyn BlockProcessor>>`. Per-block selection in `BlockPartitionSpec::write`. Unknown formats → `warn!` and skip.
3. Update the JIT path's `write_partition_from_blocks` (`jit_partitions.rs`) to take `block_processors: HashMap<&'static str, Arc<dyn BlockProcessor>>` instead of a single `Arc<dyn BlockProcessor>`, dispatching per source block. Update **all three** call sites — `LogView::jit_update`, `MetricsView::jit_update`, and `AsyncEventsView::jit_update` — to pass the map. (Without this, JIT views — used for per-process queries — can't dispatch OTel blocks; missing the async-events caller would also break compilation.)
4. Add `OtelLogsBlockProcessor` (`analytics/src/lakehouse/otel_logs_block_processor.rs`): prost-decodes `ResourceLogs`, walks scope/log records, emits `log_entries` rows (level collapse from `severity_number`, fold trace_id/span_id/severity_text into properties).
5. Register both processors in `LogView::make_batch_partition_spec` and `LogView::jit_update`: `"micromegas-transit"` → `LogBlockProcessor`, `"otlp/v1/logs"` → `OtelLogsBlockProcessor`.
6. ~~End-to-end test~~ — folded into Phase 5's `test_otlp_logs_e2e` (the cross-format mixing — native + OTel under one query — is a Phase 5 follow-up; the Phase 5 v1 covers OTel-only).

### Phase 3: metrics block processor → measures
1. Add `analytics/src/lakehouse/otel_metrics_block_processor.rs`. Decodes `ResourceMetrics`.
2. Materializes Sum + Gauge data points into `measures`. Adds `aggregation_temporality` and `is_monotonic` to row properties. Logs and skips Histogram/ExponentialHistogram/Summary (deferred — see Trade-offs).
3. Register `OtelMetricsBlockProcessor` for `"otlp/v1/metrics"` in `MetricsView`'s `block_processors` map alongside the existing native processor.
4. ~~End-to-end test using Claude Code's emission~~ — superseded by Phase 5's `test_otlp_metrics_e2e`. Live Claude Code metric collection is a smoke test in Phase 7 (docs / ops).

### Phase 4: spans block processor → new otel_spans view (JIT-only, per-process)

`otel_spans` is a JIT-only per-process view in v1 — mirrors the `AsyncEventsView` pattern. There is no global batch path. Users query it as `view_instance('otel_spans', '<process_id>')`. Cross-process trace traversal (querying `WHERE trace_id = X` across all services) is a documented v1 limitation; it requires the user to know which `process_id`s participate in the trace, or to UNION across multiple `view_instance` calls. Re-evaluate when production volume justifies the storage cost of a global table or trace-skeleton index.

1. Add `analytics/src/lakehouse/otel_spans_table.rs` (Arrow schema) and `otel_spans_view.rs`. The view mirrors `AsyncEventsView` for the JIT-only/per-process *structure* — `make_batch_partition_spec` returns `bail!("not supported")`; `jit_update` calls `generate_process_jit_partitions(... stream_tag = "trace")` and `write_partition_from_blocks(...)` with `block_processors = {"otlp/v1/traces": OtelSpansBlockProcessor}`. View instance id is the `process_id` (UUID string); `"global"` is rejected. **Time conversion path differs from `AsyncEventsView`**: OTel timestamps are absolute nanoseconds (`tsc_frequency = 1_000_000_000`), so `OtelSpansView::jit_update` uses the simpler `find_process` flow (like `LogView::jit_update` at log_view.rs:139-148) rather than `find_process_with_latest_timing` + `make_time_converter_from_latest_timing`. `OtelSpansBlockProcessor` does NOT take a `ConvertTicks` constructor argument — it reads `time_unix_nano` fields directly off the proto.
2. Add `otel_spans_block_processor.rs`. Decodes `ResourceSpans` and materializes one row per span. Span events and links serialize to JSONB and land in dictionary-encoded `Binary` columns, same encoding as `properties`.
3. Register the view in the view factory. `OtelSpansViewMaker` follows `AsyncEventsViewMaker`'s per-process-only pattern: `make_view` rejects `"global"` and constructs an `OtelSpansView` for any other instance id. (Not `LogViewMaker`, which permits both global and per-process — `OtelSpansView` is JIT-only per Phase 4 step 1.) Unlike `AsyncEventsViewMaker`, `OtelSpansViewMaker` is a unit-like struct (no `view_factory: Arc<ViewFactory>` field) — that field on `AsyncEventsViewMaker` exists only to feed `find_process_with_latest_timing`, which `OtelSpansView` doesn't use (see Phase 4 step 1).
4. ~~End-to-end test~~ — superseded by Phase 5's `test_otlp_traces_e2e` (a 3-span trace, parent-id assertions, kind/status checks).

### Phase 5: end-to-end integration test

A small Python harness that emits OTLP payloads to a running `telemetry-ingestion-srv` and asserts the data lands queryable via FlightSQL. Goal: catch any wiring issue between OTLP request ↔ HTTP route ↔ block writer ↔ JIT view that the unit/integration tests in `otel-ingestion/` miss.

**Coverage requirement (load-bearing): all three signals must be tested.** One test per signal — logs, metrics, and traces — exercising the full path through `WebIngestionService` → object store → `log_entries` / `measures` / `otel_spans` views. Skipping any one of them leaves a wiring class untested (per-block format dispatch + view-specific JIT pipelines diverge per signal).

**Test harness shape** — mirrors the existing `python/micromegas/tests/` pattern:
- New file `python/micromegas/tests/test_otlp_e2e.py`. Driven by `poetry run pytest`. Assumes services are already running (start via `python3 local_test_env/ai_scripts/start_services.py`) — same convention as `test_log.py`/`test_metrics.py`. The test does **not** spawn services itself; CI scripts that want full automation wrap the existing `start_services.py` + `pytest` + `stop_services.py`.

**Test app** — small inline emitter rather than a separate binary, to keep the test self-contained:
```python
# Build OTLP proto requests directly with `protobuf` (already a project dep
# via opentelemetry-proto-py — pulled in for free with `opentelemetry-sdk`).
# Avoids the OTel SDK's exporter machinery, batching, and retry logic — those
# add nondeterministic delays that pytest doesn't need.
from opentelemetry.proto.collector.logs.v1 import logs_service_pb2
# ... build an ExportLogsServiceRequest with a deterministic resource +
# 5 log records spread across 3 severity levels.
import requests
resp = requests.post(
    "http://127.0.0.1:9000/ingestion/otlp/v1/logs",
    data=req.SerializeToString(),
    headers={"Content-Type": "application/x-protobuf"},
)
assert resp.status_code == 200
assert resp.headers["content-type"] == "application/x-protobuf"
```

The reason for hand-built protos rather than the full SDK: SDK exporters batch on a background thread and we'd need to call `force_flush()` then sleep — flaky in CI. Hand-built proto + `requests.post` makes timing deterministic.

**Assertions** (one test function per signal, plus one cross-cutting):
- `test_otlp_logs_e2e`: emit 5 logs with a known `service.name` + `host.name` + `process.pid`. Compute the expected `process_id` client-side using the same UUIDv5 formula (helper in `tests/otlp_helpers.py`). Query `SELECT count(*) FROM log_entries WHERE process_id = '<uuid>'` until `>= 5` (poll with `assert_eventually` up to 30s — JIT materialization may take a beat). Then `SELECT level, msg, properties->>'otel.scope.name' FROM log_entries WHERE process_id = '<uuid>' ORDER BY time` and assert level mapping (severity 9 → 4, 17 → 2, 22 → 1) plus the scope name comes through as expected.
- `test_otlp_metrics_e2e`: emit one Sum + one Gauge under the same resource. Assert `SELECT name, unit, value, properties->>'otel.metric.kind' FROM measures WHERE process_id = '<uuid>'` returns 2 rows with the right `otel.metric.kind` values.
- `test_otlp_traces_e2e`: emit a 3-span trace (root + 2 children). Query `SELECT * FROM view_instance('otel_spans', '<uuid>') WHERE trace_id = '<bytes>'` and assert: 3 rows; one row with `parent_span_id IS NULL`; durations match what was sent; `kind` and `status` columns are populated.
- `test_otlp_idempotency_e2e`: POST the same `ExportLogsServiceRequest` twice. Assert `SELECT count(*)` doesn't double — `block_id` is content-addressed so the second insert hits `ON CONFLICT (block_id) DO NOTHING` and the partition retains 5 rows.
- `test_otlp_content_type_rejection`: POST with `Content-Type: application/json`. Assert `415` and that the body decodes as a `google.rpc.Status` with `code=3` (INVALID_ARGUMENT).

**Helper module** `python/micromegas/tests/otlp_helpers.py`:
- `compute_otel_process_id(host_name, host_id, pid, start_time, service_namespace, service_name, instance_id) -> uuid.UUID` — Python port of the Rust `process_id_from_resource` formula. Uses `\x1F` separator and `uuid.uuid5(NS_OTEL_PROCESS_V1, key.encode())`. **Test-side responsibility:** if the Rust formula ever changes (which would require bumping to `_V2`), this helper has to change in lockstep — flag this in the docstring.
- `assert_eventually(query_fn, predicate, timeout_s=30, interval_s=0.5)` — polls `query_fn()` until `predicate(result)` returns truthy or the deadline passes. JIT views can take a moment to materialize after the first query against a process_id.

**Pre-test data isolation:** every test generates a fresh `service.instance.id` (UUID4) so process_id differs per test run. Avoids cross-run interference without needing a database wipe.

**Wiring into the existing service-startup script:** add `--with-otlp-test-data` flag to `local_test_env/ai_scripts/start_services.py` that, after services come up, invokes a small bootstrap that emits one of each signal (logs/metrics/traces) so devs can run the manual smoke test without writing client code each time. Optional — the pytest harness already covers this.

**Out of scope for v1:**
- A genuine OTel SDK round-trip (BatchSpanProcessor + OTLPSpanExporter). Adds value but the deterministic-proto path is sufficient for catching wiring breaks; a follow-up PR can add an "SDK e2e" suite that uses the real SDK against the same endpoints.
- Auth coverage. The current `start_services.py` uses `--disable-auth`. A separate test variant with `MICROMEGAS_API_KEYS` set + the SDK's `OTEL_EXPORTER_OTLP_HEADERS` is a follow-up.
- gzip request encoding. The decompression layer is wired but tests would need to gzip the body manually; defer until we have a failure that motivates it.

**CI integration (deferred):** the existing CI harness doesn't spin up Postgres + the services, so this test runs locally only in v1. A follow-up adds a GitHub Actions job that wraps `start_services.py` → `pytest test_otlp_e2e.py` → `stop_services.py`.

### Phase 6: production hardening
1. Monitoring + alerts on ingest latency / queue depth. (Body size limit, partial-success policy, rate limiting all decided elsewhere — see Backpressure and "What we are NOT doing in v1".)

### Phase 7: docs + ops
1. `mkdocs/docs/operating/otlp.md` — ports, env, client config snippets (Claude Code, Goose, Python OTel SDK), auth header format, troubleshooting.
2. `mkdocs/docs/guides/coding-agents.md` — Claude Code OTel config + starter DataFusion queries (cache hit ratio, redundant tool calls, time-in-tool ratio).
3. Mention OTLP as a supported wire format in `mkdocs/docs/architecture/`.
4. Update `README.md` feature list and roadmap.
5. Add example Claude Code env to `local_test_env/`.

## Files to Modify

**New crate**:
- `rust/otel-ingestion/Cargo.toml` + `src/{lib,proto,identity,block,error,handler}.rs`
- `rust/otel-ingestion/tests/{fixtures,split_tests}.rs` (integration tests)

**New modules in `analytics/`**:
- `rust/analytics/src/lakehouse/format.rs` (format-string constants used as HashMap keys for per-block dispatch)
- `rust/analytics/src/lakehouse/otel_attrs.rs` (`AnyValue` → JSONB conversion + `severity_number` → `Level` mapping; shared by all three OTel block processors)
- `rust/analytics/src/lakehouse/otel_logs_block_processor.rs`
- `rust/analytics/src/lakehouse/otel_metrics_block_processor.rs`
- `rust/analytics/src/lakehouse/otel_spans_table.rs` (Arrow schema)
- `rust/analytics/src/lakehouse/otel_spans_view.rs` (per-process JIT view, mirrors `AsyncEventsView`)
- `rust/analytics/src/lakehouse/otel_spans_block_processor.rs`

**New file in `public/`**:
- `rust/public/src/servers/otlp.rs` (axum sub-router with the three OTLP/HTTP routes; 20 MiB body limit + gzip decompression layers; `Content-Type` validation; `google.rpc.Status` response body for 4xx/5xx)

**Modified**:
- `rust/Cargo.toml` (add `opentelemetry-proto` workspace dep; add `micromegas-otel-ingestion = { path = "otel-ingestion", ... }` alongside the other workspace path deps; add `"v5"` to the `uuid` workspace features; add `"decompression-gzip"` to `tower-http` features. The `otel-ingestion` crate is auto-discovered by the existing `members = ["*", ...]` glob — no manifest member entry needed beyond creating the directory.)
- `rust/ingestion/Cargo.toml` (add `uuid` to deps so `register_otel_stream`/`register_otel_process` can take `Uuid` parameters)
- `rust/ingestion/src/sql_migration.rs` (bump `LATEST_DATA_LAKE_SCHEMA_VERSION` to 4; add `upgrade_data_lake_schema_v4` and dispatch)
- `rust/ingestion/src/web_ingestion_service.rs` (named-column `INSERT INTO streams` with `format`; new `register_otel_stream`; new `register_otel_process`; new `insert_block_typed` typed entry point used by the OTel adapter; refactor existing `insert_block(body)` to ciborium-decode and call `insert_block_typed`; add public constants `EMPTY_TRANSIT_METADATA_CBOR` and `FORMAT_TRANSIT`)
- `rust/analytics/Cargo.toml` (add `base64`, `opentelemetry-proto`, `prost` deps for the OTel block processors)
- `rust/analytics/src/replication.rs` (named-column `INSERT INTO streams` with `format`; the streams ingest path at `replication.rs:20-71` reads source columns by name from a `FlightRecordBatchStream`, so it must add `string_column_by_name(&b, "format")?` and bind it to the new column. **Source-schema requirement**: this means the source data lake must also be on schema v4+ — replicating from a v3 source will fail at the column lookup. Document this in the replication operator docs alongside the v4 migration note. We chose hard failure over a silent `'micromegas-transit'` fallback because replication is an admin-driven coordinated operation and a silent default would mask genuine schema-version mismatches.)
- `rust/analytics/src/lakehouse/blocks_view.rs` (project `streams.format` in SQL + Arrow schema; bump `blocks_file_schema_hash()` from `vec![2]` to `vec![3]` so cached partitions built against the old schema are invalidated — same precedent as the JSONB migration that bumped it from `[1]` to `[2]`)
- `rust/analytics/src/lakehouse/streams_view.rs` (carry `format` through `transform_query`/`merge_query`)
- `rust/analytics/src/lakehouse/partition_source_data.rs` (add `format` to source query + `PartitionSourceBlock`)
- `rust/analytics/src/lakehouse/jit_partitions.rs` (carry `format` on `PartitionSourceBlock`; change `write_partition_from_blocks` to take `block_processors: HashMap<&'static str, Arc<dyn BlockProcessor>>` and dispatch per source block)
- `rust/analytics/src/lakehouse/parse_block_table_function.rs` (project `"streams.format"` and bail with a clean error for `format != "micromegas-transit"`. Decoding OTel payloads here would require routing to the OTel proto walker; defer that to v2 unless a user surfaces the need.)
- `rust/analytics/src/lakehouse/block_partition_spec.rs` (replace single `block_processor` with `block_processors: BlockProcessorMap` (a `HashMap<&'static str, Arc<dyn BlockProcessor>>` type alias); per-block dispatch with `warn!`+skip on unknown formats)
- `rust/analytics/src/lakehouse/log_view.rs`, `metrics_view.rs`, and `async_events_view.rs` (register processors in the new map; log/metrics get both native + OTel via private `*_processors()` helpers; async_events gets only the native processor)
- `rust/public/Cargo.toml` (add `dep:micromegas-otel-ingestion` and `dep:tower-http` to the `server` feature)
- `rust/public/src/servers/mod.rs` (declare the new `otlp` module)
- `rust/telemetry-ingestion-srv/src/main.rs` (`.merge(servers::otlp::otlp_router())` into the protected sub-Router BEFORE the outer 100 MiB layer so the OTLP-scoped 20 MiB cap stays in effect for OTLP routes only). The crate's `Cargo.toml` is **not** modified — it depends on `micromegas` with the `server` feature, which transitively pulls in the new deps.
- `rust/analytics/src/lakehouse/mod.rs` (declare new modules: `format`, `otel_attrs`, `otel_logs_block_processor`, `otel_metrics_block_processor`, `otel_spans_block_processor`, `otel_spans_view`, `otel_spans_table`)
- `rust/analytics/src/lakehouse/view_factory.rs` (register `otel_spans` view set in `default_view_factory` via `add_view_set`, mirroring the existing `async_events` registration; per-block processor dispatch is configured inside `LogView`/`MetricsView` themselves, not here)
- `README.md` (deferred — see Phase 7)

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

**Unit** (status as of implementation):
- ID derivation stability (`identity` module) — **done** (7 tests).
- Per-resource block split shape (`block` module) — **done** (3 unit + 7 integration).
- Translation of representative `ResourceLogs` / `ResourceSpans` / `ResourceMetrics` payloads to row batches — covered end-to-end by Phase 5 rather than at the Rust unit level (would need a fixture object store + DB pool to exercise `BlockProcessor::process`; cheaper to pay it once at the e2e tier).
- Sentinel dispatch in each block processor — implicit: an unknown format produces a `warn!` + zero rows (no panic). Explicit dispatch test against a mixed transit + OTLP block stream is deferred to a later "regression suite" PR.

**Integration** (in `tests/` of `otel-ingestion`) — **done** for the splitter; deferred for the in-process axum round-trip:
- Construct test payloads by building the prost-generated message types directly (`ResourceLogs`, `ResourceSpans`, `ResourceMetrics`, `KeyValue`, etc.) — `opentelemetry-proto` alone is sufficient. Pulling in the full `opentelemetry-sdk` and `opentelemetry-otlp` exporter just to materialize bytes adds tokio/tower/reqwest transitively for no test-coverage gain. `tests/fixtures.rs` provides helpers like `make_logs_request(service, host, pid, records)` to keep tests readable without an SDK round-trip.
- Round-trip: build `ExportLogsServiceRequest` proto → serialize with prost → POST to the route via `axum::Router` in-process (no socket) → query the Arrow record batches the block processor would emit. **Deferred to Phase 5**: the in-process variant needs a mock or in-memory `WebIngestionService` (PG + object store) to exercise the write side, which is roughly the same plumbing as the live-server path Phase 5 already covers. We chose to pay the integration cost once at Phase 5 rather than build two harnesses.

**End-to-end** (via `local_test_env`) — see Phase 5 for the concrete plan:
- Start `telemetry-ingestion-srv` via `start_services.py`. Drive `pytest python/micromegas/tests/test_otlp_e2e.py`, which POSTs hand-built OTLP proto requests via `requests.post` (no full OTel SDK — avoids batching/retry nondeterminism) covering all three signals + idempotency + content-type rejection. Query `log_entries`, `otel_spans`, `measures` via FlightSQL; assert counts and column shape match.
- Smoke (manual, Phase 7): run Claude Code with `CLAUDE_CODE_ENABLE_TELEMETRY=1` pointed at the local server; assert at least the documented metrics (`claude_code.token.usage`, `claude_code.session.count`) land. This validates the full SDK round-trip; pytest e2e is the primary regression net.

**Performance smoke**:
- Throughput target: 10k spans/sec on a single ingest pod (matches the existing CBOR pathway). The translation cost is dominated by JSON-encoding properties; if it's too slow, fall back to a tighter binary attribute encoding.

## Open Questions

1. ~~**`opentelemetry-proto` version pinning**~~ — decided: locked at `0.31` (with `gen-tonic-messages` feature, NOT `gen-tonic`) in the workspace `Cargo.toml`. Confirmed compatible with `prost 0.14.x` during implementation.
2. ~~**`trace_id` representation in `otel_spans`**~~ — decided: `FixedSizeBinary[16]` for `trace_id`, `FixedSizeBinary[8]` for `span_id` / `parent_span_id`. Lengths are fixed by W3C Trace Context, so the offsets array of variable `Binary` would be pure overhead. A `hex(...)` UDF (or query-time `encode`) handles human-readable display.
3. **Stream lifecycle**: OTel processes run for days; the stream's `objects_metadata` (format descriptor) is stored once at registration. Is stream rotation needed, or does time-based lakehouse partitioning handle it?
4. ~~**Per-tenant rate limiting**~~ — decided: out of scope for v1. Add when there's a real noisy-neighbor problem.
5. **`otel_spans` vs unified `spans` view**: want a single `spans` view (with a `source` discriminator) that unifies native async-spans and OTel spans for cross-source trace queries? Cost: schema-design complexity — the two sources have non-overlapping identity (pointer-id vs span_id) and partially overlapping columns. Could ship as a DataFusion view (UNION) without changing storage. Tied to the v1 JIT-only decision: a unified view would also be per-process in v1 unless we add a cross-process trace index.
6. ~~**Block payload re-encoding**~~ — decided: accept the cost. Splitting an `ExportRequest` into per-Resource blocks re-encodes each submessage; an offset/length scheme into a shared envelope would save the re-encoding but make blocks non-self-contained. OTel ingestion is allowed to be less efficient than the native micromegas-tracing path; producers that want minimum overhead use the native SDK.
7. **OpenTelemetry Collector sample config**: ship one that fans out to Micromegas + a file exporter for production safety? Natural follow-up, not core.
8. ~~**Compaction interaction**~~ — decided: nothing to do. The existing lakehouse compaction handles small-block influxes the same way it does for native streams.
9. ~~**Reference-implementation cross-check**~~ — done. Diffed the plan against the OTLP/HTTP spec text and Vector's `vector/src/sources/opentelemetry/{http,reply}.rs`. Findings incorporated into the plan:
    - **Fixed**: error-response body is `google.rpc.Status` proto, not `Export*ServiceResponse` — corrected in "Response body" bullet of Backpressure, with sourcing strategy (hand-roll a 3-field prost message in `otel-ingestion/src/proto.rs` rather than pull in `tonic-types`).
    - **Fixed**: added `Retry-After: 30` header on 503 responses (spec SHOULD).
    - **Fixed**: Content-Type matching is media-type-aware (parse, then check); accepts `application/x-protobuf; charset=...`. Not exact-string-equality.
    - **Fixed**: full retryable-code list (`429`, `502`, `503`, `504`) called out; we only emit `503` in v1.
    - **Deferred**: response gzip-encoding (spec MAY; tiny response bodies, no win) — added to "What we are NOT doing in v1".
    - **Confirmed correct**: 200 + empty `Export*ServiceResponse` on success; empty top-level request → 200; gzip-only request decompression; `415` for wrong Content-Type.

## Implementation Notes

(Filled in as the plan is built.)

### Phase 1 (schema, otel-ingestion, OTLP routes)

**Done:**
- `rust/ingestion/src/sql_migration.rs` — bumped `LATEST_DATA_LAKE_SCHEMA_VERSION` to 4 and added `upgrade_data_lake_schema_v4`.
- `rust/ingestion/src/web_ingestion_service.rs` — switched `insert_stream` to a named-column INSERT with `format`. Added `register_otel_stream`, `register_otel_process`, `insert_block_typed`. Exported `EMPTY_TRANSIT_METADATA_CBOR` (the single byte `0x80` — CBOR-encoded empty `Vec<UserDefinedType>`) and `FORMAT_TRANSIT`.
  - Subtle: `register_otel_process` issues `INSERT INTO processes (...) VALUES ($1,...,$3,$4,...)` and binds `username` twice to fill `username` and `realname`. Earlier draft used `$3,$3` which sqlx rejects as "ambiguous" in some configurations — bind separately.
- `rust/analytics/src/replication.rs` — switched to a named-column INSERT and reads the `format` column. Hard fails if the source data lake doesn't have v4 schema (matches the plan's "no silent fallback" decision).
- `rust/otel-ingestion/` — new crate. Modules:
  - `identity.rs` — `process_id_from_resource`, `stream_id_from_process_signal`, `block_id_from_payload`. Namespace UUIDs were generated 2026-05-01 via `uuidgen`. 10 unit tests cover stability across attribute reordering, host case-folding, pid difference, missing-attribute fallback, signal-discrimination, content-addressing, degenerate detection.
  - `block.rs` — `split_logs/metrics/traces` produce `PreparedBlock` per Resource. Walks each Resource for min/max timestamps; falls back to wall clock only if every record has zero timestamps.
  - `proto.rs` — re-exports prost message types from `opentelemetry-proto` (with `gen-tonic-messages` feature, NOT `gen-tonic` — the latter pulls in tonic transport which we don't need). Hand-rolled 3-field `Status` message for OTLP/HTTP error responses.
  - `error.rs` — `OtelError` with HTTP status mapping + retryable flag. `Signal` implements `Display` so `thiserror`'s `#[error]` template can use it.
  - `handler.rs` — top-level `ingest_logs`/`metrics`/`traces` that decode the proto, register process/stream, and write blocks. Empty top-level requests short-circuit to a 200 with empty response per spec.
- `rust/Cargo.toml` — added `opentelemetry-proto = "0.31"` (with `gen-tonic-messages,logs,metrics,trace` features), enabled `v5` on the `uuid` workspace dep, added `decompression-gzip` to `tower-http` features, registered `micromegas-otel-ingestion` workspace member.
- `rust/public/src/servers/otlp.rs` — three POST handlers, `Content-Type` validated via media-type prefix (so `application/x-protobuf; charset=utf-8` works). 4xx/5xx responses carry a `google.rpc.Status` proto body; 503 responses include `Retry-After: 30`. Sub-router applies `RequestBodyLimitLayer(20 MiB)` + `RequestDecompressionLayer.gzip(true)` before merging into the protected app, scoping the OTLP limit independently of the parent's 100 MiB cap.
- `rust/telemetry-ingestion-srv/src/main.rs` — `.merge(otlp_router())` BEFORE the outer 100 MiB layer so per-route layers stay scoped.
- `rust/public/Cargo.toml` — added `dep:micromegas-otel-ingestion` and `dep:tower-http` to the `server` feature.

**Deviations from plan:**
- The plan said "OtelError variants `Database`/`Storage` map to 503". They do — but the gRPC code embedded in the `Status` proto body is `UNAVAILABLE=14` (not `RESOURCE_EXHAUSTED=8` as the plan suggested). UNAVAILABLE is the canonical gRPC mapping for transient backend failures.
- Plan said `WebIngestionService::insert_block(body: bytes::Bytes)` should be a "thin wrapper" delegating to `insert_block_typed`. Implementation: `insert_block` ciborium-decodes once and forwards the typed `Block` to `insert_block_typed`, which does all the actual work. Same effect, slightly different function-call structure.

### Phase 2 (per-block dispatch + OTel logs processor)

**Done:**
- `partition_source_data.rs` — `PartitionSourceBlock` carries a new `format: String` field.
- `blocks_view.rs`, `streams_view.rs`, `partition_source_data.rs`, `jit_partitions.rs` — project `streams.format` end-to-end. Bumped `blocks_file_schema_hash` to `vec![3]`.
- `block_partition_spec.rs` — replaced the single `block_processor` field with `block_processors: HashMap<&'static str, Arc<dyn BlockProcessor>>`. The `BlockPartitionSpec::write` loop looks up the processor by `src_block.format` per block; missing format → `warn!` + skip rather than error.
- `jit_partitions.rs` — `write_partition_from_blocks` takes the same map. All four call sites updated (`LogView`, `MetricsView`, `AsyncEventsView`, `OtelSpansView`).
- `parse_block_table_function.rs` — projects `streams.format` and bails with a clear message on non-transit formats. Decoding OTel payloads here would require routing to the OTel proto walker — deferred to v2.
- `format.rs` (new) — central `FORMAT_TRANSIT` / `FORMAT_OTLP_LOGS` / `FORMAT_OTLP_METRICS` / `FORMAT_OTLP_TRACES` constants used as HashMap keys.
- `otel_attrs.rs` (new) — `AnyValue` → JSONB conversion (recursive for arrays/kvlists, base64 for bytes), `severity_number` → `Level` mapping table.
- `otel_logs_block_processor.rs` (new) — prost-decodes `ResourceLogs`, walks each scope and record, emits one `log_entries` row per record. Folds trace/span ids, severity_text, scope info into JSONB properties under `otel.*` prefixes. Uses observed_time fallback when `time_unix_nano == 0`. Records with no timestamp at all are dropped with a warning.
- `log_view.rs` / `metrics_view.rs` — register both native and OTel processors via small private `log_processors()` / `metrics_processors()` helpers that build the HashMap on each call (cheap — two entries with `Arc::clone`-ed processors).

**Deviations from plan:**
- Plan said tests should "feed transit blocks and OTLP blocks to a sentinel-dispatch test". That test is deferred to Phase 9 (the integration tests bundle) — the unit-level test that an unknown format prints `warn!` and skips is implicit (no panic; the partition just emits zero rows for unrecognized blocks).

### Phase 3 (OTel metrics processor)

**Done:**
- `otel_metrics_block_processor.rs` — Sum + Gauge data points → `measures` rows. Histogram / ExponentialHistogram / Summary skipped with per-kind `debug!` logs that include the metric name, unit, and point count. `aggregation_temporality`, `is_monotonic`, and `otel.metric.kind` ("sum"/"gauge") fold into per-row properties so queries can filter by metric kind without adding columns.
- Number-data-point handling supports both `as_double` and `as_int` value variants.
- Registered for `"otlp/v1/metrics"` in `MetricsView` via `metrics_processors()`.

### Phase 4 (otel_spans view)

**Done:**
- `otel_spans_table.rs` — Arrow schema. `trace_id` is `FixedSizeBinary[16]`, `span_id` / `parent_span_id` are `FixedSizeBinary[8]` (latter nullable for root spans). `events` and `links` are plain `Binary` columns carrying JSONB arrays.
- `otel_spans_block_processor.rs` — emits one row per span. Skips spans with `start_time == 0`, `end_time == 0`, or wrong-length trace_id/span_id with `debug!`.
- `otel_spans_view.rs` — JIT-only per-process view, mirrors `AsyncEventsView` structure but uses the simpler `find_process` flow (OTel's `tsc_frequency = 1ns` makes `make_time_converter_from_latest_timing` unnecessary). Rejects `"global"`. Registers only `"otlp/v1/traces"` in its block-processor map (no native source).
- `view_factory.rs` — registered `OtelSpansViewMaker` (unit struct, no `view_factory: Arc<ViewFactory>` field — different from `AsyncEventsViewMaker` because we don't use `find_process_with_latest_timing`).

### Style cleanup
- Comments throughout switched from "CBOR streams" to "transit streams" / "native streams". CBOR is only the envelope — the actual stream wire format is transit. (Per user feedback during implementation.)

### Tests added (17 total)
- `rust/otel-ingestion/src/identity.rs` (7 unit tests) — process_id stability across attribute reordering, host case-folding, pid difference, missing-attribute fallback (`process.start_time` ↔ `process.creation.time`), signal-discrimination (logs ≠ metrics ≠ traces), content-addressing of block_id, degenerate-resource detection.
- `rust/otel-ingestion/src/block.rs` (3 unit tests) — split_logs produces one block per resource, skips resources with empty scope_logs, block_id changes when payload changes.
- `rust/otel-ingestion/tests/split_tests.rs` (7 integration tests) — log bounds derivation from min/max time_unix_nano, idempotent block_id with content-addressed payload, mixed-kind metrics emit one block per resource, traces payload round-trips through prost decode, empty top-level request yields no blocks, distinct resources split into distinct processes, format constants align with signal keys.
- `rust/otel-ingestion/tests/fixtures.rs` — helper module that builds `ExportLogsServiceRequest` / `ExportMetricsServiceRequest` / `ExportTraceServiceRequest` from prost-generated message types directly. Per the plan's testing strategy, this avoids pulling in `opentelemetry-sdk` + `opentelemetry-otlp` (which would drag tokio/tower/reqwest in transitively).

### CI status (post-implementation)
- `cargo fmt --check`: passes.
- `cargo clippy --workspace -- -D warnings`: passes.
- `cargo machete`: no unused dependencies.
- `cargo test`: all tests pass (otel-ingestion: 17, analytics + everything else: ~150+).
- `python3 build/rust_ci.py native`: green.

### Phase 5 / 6 / 7 (e2e tests + hardening + docs) — NOT DONE
- **Phase 5 (e2e integration test)** — `python/micromegas/tests/test_otlp_e2e.py` + `otlp_helpers.py`. Hand-built proto requests via `requests.post` against a running stack from `start_services.py`; one test per signal + idempotency + wrong-content-type. See Phase 5 in Implementation Steps for the full design.
- **Phase 6 (production hardening)** — monitoring + alerts (deferred — needs production traffic to size correctly).
- **Phase 7 (docs)** — `mkdocs/docs/operating/otlp.md`, `mkdocs/docs/guides/coding-agents.md`, `README.md` feature-list update, `local_test_env/` example Claude Code env.

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
