# Native OTLP Ingestion Plan

## Overview

Add native support for the OpenTelemetry Protocol (OTLP) as a first-class wire format in Micromegas, alongside the existing CBOR/transit protocol. Any OTel-instrumented program — Claude Code, Goose, generic OTel SDKs — can point `OTEL_EXPORTER_OTLP_ENDPOINT` at the ingestion service and have spans, metrics, and logs land in the lakehouse.

The driving use case is observability for AI coding agents: Claude Code emits a rich OTel surface (token splits by cache type, tool-call spans, compaction events, hook timings). Once those land in the lakehouse, DataFusion queries can find inefficiencies (cache thrash, redundant `Read` calls, exploration without edits).

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

**Existing OTel code**: none. `grep -r 'opentelemetry\|otlp\|otel' rust/ python/` is empty. Workspace already has compatible deps for the receiver: `tonic = "0.14"`, `prost = "0.14"`, `arrow-flight = "57.2"`.

## Design

### Architecture: store OTLP as-is, parse at the analytics layer

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

OTel-specific decode lives in `analytics/`, where the parquet schema is the natural translation target anyway. New block processors (one per signal) prost-decode the payload via `opentelemetry-proto` and emit rows.

```
       ┌─────────────────────────────────────────────────┐
       │  otlp-ingestion-srv (NEW binary)                │
       │   ─ tonic gRPC :4317  /  axum HTTP :4318        │
       │   ─ auth middleware (existing)                  │
       │   ─ derive process_id from resource attrs       │
       │   ─ write raw OTLP proto to object store        │
       │   ─ INSERT block + stream + process metadata    │
       └─────────────────┬───────────────────────────────┘
                         │ block_wire_format::Block
                         │   payload.objects = OTLP proto bytes
                         │   stream.tags = ["otel","logs"|"metrics"|"traces"]
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

Use the `opentelemetry-proto` crate with the `gen-tonic` feature. It provides generated `tonic` services for all three signals (`TraceService`, `MetricsService`, `LogsService`) and matching prost types. If version alignment with our `tonic 0.14` is awkward at the time of implementation, fall back to checking in the canonical `.proto` files and using `tonic-build` ourselves (the .proto files live in `open-telemetry/opentelemetry-proto`, MIT-equivalent license, ~10 files).

HTTP/protobuf (port 4318) is implemented with axum routes that decode the same prost types — the message shapes are identical between transports.

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

**Stream identity** (one stream per signal type per process per instrumentation scope):
```
stream_id = uuid_v5(NS_OTEL_STREAM_V1,
    process_id + "\x1F" + signal + "\x1F" + scope.name + "\x1F" + scope.version)
```
where `signal ∈ {logs, metrics, traces}`. Stream tags: `["otel", signal]`. Stream properties: scope attributes.

**Block identity**: one block per (process_id, stream_id) per OTLP request. `block_id = uuid_v5(NS_OTEL_BLOCK_V1, hash(payload bytes))` for idempotent retry.

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
| Scope attributes | `Stream` properties |
| DataPoint / Span / LogRecord attributes | per-event `properties` |

### Ingest path (per OTLP request)

1. Auth (tonic interceptor / axum middleware — existing chain).
2. Decode the outer `ExportRequest` proto (just to walk Resource boundaries; we don't decode further).
3. For each `ResourceLogs` / `ResourceMetrics` / `ResourceSpans`:
   1. Derive `process_id` from resource attributes (UUIDv5; see Identity).
   2. Derive `stream_id` from `(process_id, signal, scope.name, scope.version)`.
   3. Idempotent register process + stream (in-memory dedup cache + `ON CONFLICT DO NOTHING` PG inserts). Stream registration sets `format = "otlp/v1/<signal>"`.
   4. Re-encode this Resource sub-message as protobuf bytes — the block payload.
   5. Build `block_wire_format::Block` with `payload.dependencies = []`, `payload.objects = <proto bytes>`.
   6. Call existing `WebIngestionService::insert_block`.
4. Return OTLP success (or RESOURCE_EXHAUSTED on PG/object-store failure, INVALID_ARGUMENT on parse error).

There's no event-by-event translation at ingest. The hot path serializes one protobuf submessage and writes it.

### Tick calibration

Native blocks carry `tsc_frequency` and `(begin_ticks, end_ticks)` so timestamps can be rebuilt at query time. OTel timestamps are already absolute nanoseconds. We set `tsc_frequency = 1_000_000_000` and `begin_ticks = begin_time_unix_nano` (and same for end), so existing tick→time math passes through cleanly.

### Backpressure and gRPC semantics

OTLP gRPC uses unary RPCs (`Export`) — one request, one response. The response carries:
- `partial_success`: `rejected_*_count` and `error_message` for partial accepts.

Implementation:
- Tonic server with default concurrency limits; tower middleware for rate limiting using the existing patterns from `flight-sql-srv`.
- On any DB or object-store error, return gRPC status `RESOURCE_EXHAUSTED` (retryable per OTLP spec) for transient failures (PG connection errors, S3 5xx) and `INVALID_ARGUMENT` (non-retryable) for parse errors.
- No partial-success accounting in v1: either the whole batch is accepted or the whole batch is rejected. OTel SDKs handle this fine.
- Body size limit: 10 MB compressed (matches OTel collector default).

### Auth

OTLP authenticates via headers, configured client-side as `OTEL_EXPORTER_OTLP_HEADERS=Authorization=Bearer ...`.

Mapping:
- gRPC: tonic interceptor reads `authorization` from request metadata, calls into the existing `auth::AuthValidator`.
- HTTP: axum middleware (the same one `telemetry-ingestion-srv` uses) — no change needed to the auth crate.

The existing API-key + OIDC chain works unchanged. We add one wrinkle: an **operator-defined map from API-key name to default `service.namespace`**, so that a key issued to a team automatically tags their data. Configured via a new env var `MICROMEGAS_OTLP_NAMESPACE_MAP` (JSON `{"key-name": "namespace"}`); applied as a fallback when the OTel client doesn't set `service.namespace`.

### Configuration

New env vars for the new binary:
- `MICROMEGAS_OTLP_GRPC_LISTEN_ADDR` (default `0.0.0.0:4317`)
- `MICROMEGAS_OTLP_HTTP_LISTEN_ADDR` (default `0.0.0.0:4318`)
- `MICROMEGAS_OTLP_MAX_RECV_BYTES` (default `10_000_000`)
- `MICROMEGAS_OTLP_NAMESPACE_MAP` (optional, see above)

Existing env vars reused: `MICROMEGAS_SQL_CONNECTION_STRING`, `MICROMEGAS_OBJECT_STORE_URI`, `MICROMEGAS_API_KEYS`, `MICROMEGAS_OIDC_CONFIG`.

## Implementation Steps

### Phase 1: schema + ingest service (gRPC + HTTP, raw payload to lake)
1. Add migration: `ALTER TABLE streams ADD COLUMN format TEXT NOT NULL DEFAULT 'micromegas-transit';` in a new step in `rust/ingestion/src/sql_migration.rs`. Update `create_streams_table` in `sql_telemetry_db.rs` to include the column for fresh installs.
2. Update `ingestion::WebIngestionService::insert_stream` to accept and persist `format` (default `'micromegas-transit'` if caller doesn't specify, preserving backwards compatibility).
3. Add `opentelemetry-proto = "0.31"` (workspace dep) — confirmed compatible with our `tonic 0.14.5`, `prost 0.14.3`.
4. Create `rust/otel-ingestion/` library crate: `identity.rs` (resource → process_id/stream_id), `proto.rs` (re-exports), `error.rs`, `block.rs` (split ExportRequest into per-resource blocks).
5. Create `rust/otlp-ingestion-srv/` binary: tonic gRPC :4317, axum HTTP :4318 (`/v1/traces|metrics|logs`). Both transports share the same handler that calls `otel-ingestion` to split + register + write blocks via `WebIngestionService`. Stream registration sets `format = "otlp/v1/<signal>"`.
6. Wire existing auth as tonic interceptor + axum middleware.
7. End-to-end smoke test: Python OTel SDK pointed at the service; verify rows land in PG `processes`/`streams`/`blocks` (with `format = "otlp/v1/..."` on streams) and bytes land in object store.

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
1. Backpressure: tonic concurrency limits + tower rate limiter (per-subject if straightforward).
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

**New crates**:
- `rust/otel-ingestion/Cargo.toml` + `src/{lib,proto,identity,block,error}.rs`
- `rust/otlp-ingestion-srv/Cargo.toml` + `src/{main,grpc,http}.rs`

**New modules in `analytics/`**:
- `rust/analytics/src/lakehouse/otel_logs_block_processor.rs`
- `rust/analytics/src/lakehouse/otel_metrics_block_processor.rs`
- `rust/analytics/src/lakehouse/otel_spans_table.rs` (Arrow schema)
- `rust/analytics/src/lakehouse/otel_spans_view.rs` (view + partitioning)
- `rust/analytics/src/lakehouse/otel_spans_block_processor.rs`
- `rust/analytics/src/lakehouse/otel_metrics_histograms_*.rs` (Phase 6)

**Modified**:
- `rust/Cargo.toml` (add `opentelemetry-proto = "0.31"`; new workspace members)
- `rust/ingestion/src/sql_migration.rs` (ADD COLUMN streams.format)
- `rust/ingestion/src/sql_telemetry_db.rs` (include `format` in fresh `CREATE TABLE streams`)
- `rust/ingestion/src/web_ingestion_service.rs` (persist `format` on stream insert)
- `rust/analytics/src/lakehouse/mod.rs` (register OTel views + processor dispatch by `streams.format`)
- `local_test_env/ai_scripts/start_services.py` (start otlp-ingestion-srv)
- `README.md`

**`rust/tracing/` is not touched.** OTel is an analytics-layer concern.

## Trade-offs

**Store OTLP as-is vs translate at ingest**: chose store-as-is. Two earlier drafts of this plan (a) invented a parallel CBOR record format and (b) added OTel interop event types to the `tracing/` crate. Both bend the architecture around OTel. The clean version is symmetric with native: native blocks store opaque transit bytes parsed at the analytics layer; OTel blocks store opaque OTLP bytes parsed at the analytics layer. Same envelope, different decoder.

The wins of as-is storage:
- Ingest path is auth + write — no translation, no event-type dispatch, no synthesized POD records.
- `tracing/` crate stays focused on in-process instrumentation.
- Lossless: every OTel attribute, exemplar, link preserved verbatim. New parquet column can be derived from raw payloads later.
- OTel evolution is decoupled. New OTel field → only the materialization changes.

The cost: two parsers (transit and OTLP proto) — but that's true of any design here; the question is just where they live.

**Separate binary vs extending telemetry-ingestion-srv**: chose separate. tonic gRPC + the conventional 4317/4318 ports + OTel-specific auth-header semantics don't mix cleanly into the existing axum app. The new binary calls `WebIngestionService` as a library, so the underlying PG/object-store path is shared.

**One block per Resource (not per ExportRequest)**: chose per-Resource. An `ExportRequest` may carry multiple resources (different services); splitting at Resource boundaries means each block has an unambiguous `process_id` and is independently re-decodable. Block_id derived from a hash of the bytes for idempotency.

**OTel spans get their own view, not async_events**: chose new `otel_spans` view. The existing `async_events` view is derived from thread-event blocks where parent is inferred from begin/end ordering on a thread; OTel spans carry explicit `parent_span_id` and have no thread-of-origin concept. Forcing OTel into the `async_events` processor would either lie about parent inference or require a pseudo-thread per trace. Native and OTel span data live in sibling views; cross-source queries can `UNION` if needed (open question #5).

**`trace_id` and `span_id` as first-class columns**: chose first-class on `otel_spans` (the load-bearing trace-analytics columns). On `log_entries` they go into JSONB until profiling shows they're hot.

**Histograms in v1 vs deferred**: deferred. Claude Code doesn't emit histograms; bundling the new view+processor would block the driving use case.

**Span events and links as columns vs JSONB**: first-class `List<Struct{...}>` columns on `otel_spans`. Exemplars on metric data points → deferred (land in row properties when surfaced).

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
- Start otlp-ingestion-srv. Run a Python OTel SDK script emitting ~100 spans/logs/metrics. Query `log_entries`, `async_events`, `measures` via FlightSQL; assert counts match.
- Run Claude Code with `CLAUDE_CODE_ENABLE_TELEMETRY=1` pointed at the local server; assert at least the documented metrics (`claude_code.token.usage`, `claude_code.session.count`) land.

**Performance smoke**:
- Throughput target: 10k spans/sec on a single ingest pod (matches the existing CBOR pathway). The translation cost is dominated by JSON-encoding properties; if it's too slow, fall back to a tighter binary attribute encoding.

## Open Questions

1. **`opentelemetry-proto` version pinning**: confirm a published version compatible with `tonic 0.14` + `prost 0.14`, otherwise vendor the .proto files. Resolve at the start of Phase 1.
2. **`trace_id` representation in `otel_spans`**: `FixedSizeBinary[16]` (compact, exact lookups by bytes), `Utf8` hex string (human-friendly in `WHERE`), or both via a generated column. Affects query ergonomics and storage size.
3. **Stream lifecycle**: OTel processes run for days; the stream's `objects_metadata` (format descriptor) is stored once at registration. Is stream rotation needed, or does time-based lakehouse partitioning handle it?
4. **Per-tenant rate limiting**: the auth context exposes `subject` but no rate-limit hook. Add per-subject limits at the tower layer in v1, or defer until we hit a problem?
5. **`otel_spans` vs unified `spans` view**: want a single `spans` view (with a `source` discriminator) that unifies native async-spans and OTel spans for cross-source trace queries? Cost: schema-design complexity — the two sources have non-overlapping identity (pointer-id vs span_id) and partially overlapping columns. Could ship as a DataFusion view (UNION) without changing storage.
6. **Block payload re-encoding**: when we split an `ExportRequest` into per-Resource blocks, we re-encode each `Resource*` submessage. Alternative: store the entire `ExportRequest` once and have the block reference an offset/length within it — saves re-encoding cost but makes blocks non-self-contained. Likely not worth the complexity, but worth noting.
7. **OpenTelemetry Collector sample config**: ship one that fans out to Micromegas + a file exporter for production safety? Natural follow-up, not core.
8. **Compaction interaction**: OTLP payloads tend to be small per-call. Does the existing lakehouse compaction strategy handle the influx well, or do we need a hint to coalesce more aggressively for OTel streams?

## References

- OTLP spec: https://opentelemetry.io/docs/specs/otlp/
- OTel proto repo: https://github.com/open-telemetry/opentelemetry-proto
- Claude Code monitoring: https://code.claude.com/docs/en/monitoring-usage
- Anthropic monitoring guide: https://github.com/anthropics/claude-code-monitoring-guide
- `opentelemetry-proto` crate: https://crates.io/crates/opentelemetry-proto
