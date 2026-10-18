# Instrumentation Overhead vs. OpenTelemetry Benchmark Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1678

## Overview
Add a criterion bench crate that measures the same logging, metric and span operations in
`micromegas-tracing` and in the OpenTelemetry Rust SDK, each in its recommended production
configuration with network export replaced by an in-process sink that still encodes the payload.
It reports calling-thread cost, scaling under 1/4/8 threads, encoding cost off the calling thread,
and bytes per event on the wire, and publishes the results with their methodology in the docs.
Today the only numbers (`rust/tracing/benches/properties.rs`) compare micromegas to itself, and
the docs' "20ns per event" claim (`mkdocs/docs/index.md:22`, `:31`) has no published method
behind it.

## Current State
- `rust/tracing/benches/properties.rs` is the precedent: one global dispatch init with a
  `BenchSink` that accepts every log and hands full blocks to a discard thread; `log`, `metric`,
  `extract` and `contention` groups; cases as `fn(usize)` closures over a persistent counter so
  high-cardinality ids stay new; the contention harness uses `iter_custom` + scoped threads + a
  `Barrier`, with disjoint id ranges per thread. Its plan is
  `tasks/completed/tracing_property_benches_plan.md`.
- Production encoding of a block is `StreamBlock::encode_bin`
  (`rust/telemetry-sink/src/stream_block.rs:25`): `extract` the dependencies, LZ4-compress
  dependencies and objects, CBOR-encode the `block_wire_format::Block`. `HttpEventSink` runs it on
  its worker thread (`rust/telemetry-sink/src/http_event_sink.rs:608`), and its `on_log` is a
  no-op (`:896`).
- Spans: `span_scope!`, `span_scope_named!`, `span_scope_named_dyn!`
  (`rust/tracing/src/macros.rs:31`, `:59`, `:91`) push into a thread-local `ThreadStream`
  (`rust/tracing/src/dispatch.rs:549`). Thread streams exist only when the dispatch was created
  with CPU tracing enabled and the thread called `init_thread_stream()` (`dispatch.rs:273`).
  Micromegas spans carry no attributes; a runtime-built name is the closest equivalent.
- `tracing`-crate events reach micromegas through `install_tracing_interop`
  (`rust/telemetry-sink/src/tracing_interop.rs:121`).
- The workspace already locks `opentelemetry` 0.32.0, `opentelemetry_sdk` 0.32.1 and
  `opentelemetry-proto` 0.32.0 (with `gen-tonic-messages`, `logs`, `metrics`, `trace`) through
  `otel-ingestion` / `analytics`. Compatible bridges: `opentelemetry-appender-tracing` 0.32.x and
  `tracing-opentelemetry` 0.33.x (both require `opentelemetry ^0.32`).
- OTel SDK 0.32 behavior that shapes the design (verified in the registry sources):
  - `BatchLogProcessor` / `BatchSpanProcessor` push each record into a bounded `sync_channel`
    (default queue 2048, batch 512; schedule delay 1 s for `BatchLogProcessor`, 5 s for `BatchSpanProcessor`) drained by a dedicated thread; a full queue
    **drops** the record and only counts it internally (`dropped_logs_count` is private).
  - Metrics aggregate in-process per attribute set; the default cardinality limit is 2000 per
    instrument, past which values go to an overflow series.
  - `opentelemetry_proto::transform::logs::tonic::group_logs_by_resource_and_scope(&LogBatch,
    &ResourceAttributesWithSchema)` and `transform::trace::tonic::group_spans_by_resource_and_scope(
    Vec<SpanData>, ..)` are public; `LogBatch::new(&[(&SdkLogRecord, &InstrumentationScope)])` is
    public but documented as "may be made private". `SdkLogRecord` and `SpanData` are `Clone`.
- CI does not build benches (`build/rust_ci.py` runs `cargo clippy --workspace` without
  `--all-targets`). `cargo deny` bans duplicate versions (`rust/deny.toml:49`). New crates are
  published only if listed in `build/release.py`.
- No docs page covers instrumentation overhead; `mkdocs/docs/rust/` is empty and `When to Use`
  (`mkdocs/mkdocs.yml:108`) holds the comparison pages.

## Design

### Crate
New workspace member `rust/otel-comparison-bench/` (picked up by the `members = ["*"]` glob),
package `micromegas-otel-comparison-bench`, `publish = false`. Two bench targets, one per library:

```
rust/otel-comparison-bench/
├── Cargo.toml
├── src/lib.rs              shared harness: case runner, contention runner, byte report line
├── benches/micromegas.rs   micromegas-tracing + telemetry-sink encoding
├── benches/opentelemetry.rs  opentelemetry_sdk + appender-tracing + tracing-opentelemetry
└── report.py               turns target/criterion estimates into the docs' markdown tables
```

Dependencies (added to `[workspace.dependencies]` in `rust/Cargo.toml`, alphabetical):
`criterion = "0.5"` (same as `rust/tracing`), `opentelemetry = "0.32"`,
`opentelemetry_sdk = "0.32"`, `opentelemetry-appender-tracing = "0.32"`,
`tracing-opentelemetry = "0.33"`, plus the existing `flate2`, `micromegas-telemetry-sink`,
`micromegas-tracing`, `micromegas-transit`, `opentelemetry-proto`, `prost`, `tracing`,
`tracing-subscriber`. Using 0.32 keeps a single OTel version in the lockfile; the results page
records the versions so a later bump is a re-run.

### Shared harness (`src/lib.rs`)
- `pub type Case = fn(usize);`
- `pub const BATCH: usize = 2048;` — operations per chunk, across all threads; the OTel default
  batch queue size, so a chunk can never overflow it.
- `pub type Drain = fn();` — returns once the library's background queue is empty.
- `pub fn bench_cases(c, group, lib, cases: &[(&str, Case)], drain: Drain, after_case:
  Option<fn(id: &str, emitted: u64)>)` — one criterion group per category,
  `BenchmarkId::new(case, lib)`, persistent counter across warm-up and samples (as in
  `properties.rs`). Uses `iter_custom`: each sample's `iters` are split into chunks of at most
  `BATCH`; only the emits of a chunk are timed, and `drain` runs between chunks outside the timed
  region. Same ids in both binaries, so `report.py` pairs the libraries by
  `group_id`/`function_id`/`value_str`, never by directory name. `after_case` runs once per case
  after its last sample and receives the counter's final value (the case's emitted count).
- `pub fn bench_contention(c, lib, cases, per_thread_init: fn(), per_thread_exit: fn(), drain:
  Drain, after_case: Option<fn(id: &str, emitted: u64)>)` — `iter_custom` with disjoint id
  ranges, ids `BenchmarkId::new(format!("{case}/{lib}"), threads)` for 1, 4, 8 threads with
  `Throughput::Elements(threads)`. Each thread emits `BATCH / threads` operations per chunk; all
  threads and the main thread meet at barriers around each chunk, and the main thread drains
  between chunks. A chunk's time is the slowest thread's emit time, measured inside the thread so
  barrier latency is excluded. `per_thread_init` runs in each spawned thread before the first
  barrier (micromegas needs `init_thread_stream()` for spans); `per_thread_exit` runs in each
  spawned thread after its last chunk (micromegas calls `unregister_thread_stream()` so no
  dangling `*mut ThreadStream` stays in the dispatch). `after_case` runs once per case and thread
  count with the total emitted across threads.
- `pub fn print_bytes(id, nb_events, raw_bytes, wire_bytes)` — one `println!` line per encoded
  sample (criterion has no custom-metric output), parsed by `report.py`.

### Operations and cases
Both libraries record string attribute values (`%i` / `i.to_string()`), since micromegas property
values are strings; the comparison is about the same information, not the cheapest encoding each
library offers.

`log` group — micromegas native macros vs. `tracing` macros through `OpenTelemetryTracingBridge`:

| case | `micromegas` | `micromegas_interop` | `otel` |
|---|---|---|---|
| `static` | `info!("static")` | `tracing::info!("static")` | `tracing::info!("static")` |
| `formatted` | `info!("value {}", i)` | `tracing::info!("value {}", i)` | same |
| `static_attrs` | `info!(a = "1", b = "2", c = "3", "static")` | — | `tracing::info!(a = "1", b = "2", c = "3", "static")` |
| `dyn_low_card` | `info!(l = LABELS[i % 4], "static")` | — | `tracing::info!(l = LABELS[i % 4], "static")` |
| `dyn_high_card` | `info!(id = %i, "static")` | `tracing::info!(id = %i, "static")` | same |

`micromegas_interop` shows the cost for an application already instrumented with `tracing`; it
runs in the micromegas binary, whose global `tracing` subscriber is the interop layer.

`metric_batch` group — a batch is one snapshot: one value for each of N static series on one
metric name (like a game recording its frame metrics once per frame), N = 10 and 100. Each series
is a prebuilt static set: three fixed pairs plus a series identifier. The value is the persistent
counter. Both libraries keep one value per series per batch, so a sample (one series' value in one
batch) is kept by both; per-sample is derived as per batch / N. A raw `record` is not a kept
sample in OTel, which aggregates in-process, hence the per-batch measurement with one collect per batch.
- Calling thread (`record/10`, `record/100`, via `bench_cases`, so paced and drop-guarded):
  micromegas does N `imetric!("name", "unit", ..set_k, value)` with prebuilt `PropertySet`s; OTel
  does N `Gauge<u64>::record(value, attrs_k)` with prebuilt `&'static [KeyValue]`.
- Export off the calling thread (`export/10`, `export/100`, ids `otel`, `otel_gzip`,
  `micromegas`): OTel does one `ManualReader::collect` into a reused `ResourceMetrics`, converts
  it with `ExportMetricsServiceRequest::from(&ResourceMetrics)` and `prost` encodes it (gzip for
  `otel_gzip`), with the batch's records made in untimed setup; cumulative temporality. Each batch
  size has its own meter provider so a collect holds exactly N series. Micromegas captures one
  `MetricsBlock` of 10 000 metric events, so B = 10 000 / N batches (1000 and 100), asserts a
  single block, times `encode_bin` and reports per batch through `Throughput::Elements(B)`.
- Bytes per batch: `print_bytes("metric_batch/N/lib")`; micromegas raw and `encode_bin` lengths
  over B, OTel protobuf and gzip lengths of one collected batch.
- `ManualReader` is public, but `MetricReader::collect` is reachable only with the SDK's
  `experimental_metrics_custom_reader` feature, enabled on the bench crate's dev-dependency; a
  small `MetricReader` wrapper over an `Arc<ManualReader>` keeps a handle after the provider takes
  ownership.

`span` group — micromegas thread spans vs. the native OTel tracer (`otel`) and `tracing` spans
through `tracing-opentelemetry` (`otel_tracing`). Each span is a root span (no active context):

| case | `micromegas` | `otel` | `otel_tracing` |
|---|---|---|---|
| `static` | `span_scope!("static")` | `let _s = tracer.start("static");` | `let _e = tracing::info_span!("static").entered();` |
| `attrs` | — (no span attributes) | `tracer.span_builder("static").with_attributes([KeyValue::new("id", i.to_string())]).start(&tracer)` | `info_span!("static", id = %i)` |
| `dyn_name` | `span_scope_named_dyn!(format!("span {i}"))` | `tracer.start(format!("span {i}"))` | `info_span!("static", otel.name = %format_args!("span {i}"))` |

`contention` group — `log/static`, `log/dyn_high_card`, `span/static` for each library at 1, 4, 8
threads. Micromegas logs share one process-wide stream; OTel's batch channel is its shared point.

`encode` group — off-thread cost per block, on captured data, with
`Throughput::Elements(nb_events)`:
- micromegas: capture one `LogBlock` of 10 000 `log/static_attrs` and `log/dyn_high_card`
  events and one `ThreadBlock` of 10 000 `span/static` and `span/dyn_name` scopes (capture flag
  on the sink, single-block assertion, as in `properties.rs`); bench `encode_bin(&process_info)`.
  `print_bytes` reports `len_bytes()` of objects + extracted dependencies (raw) and the
  `encode_bin` output length (wire).
- OTel: the capturing exporter clones the same 10 000 records (`(SdkLogRecord,
  InstrumentationScope)` / `SpanData`). The 2048-slot batch queue drops on full, so capture calls
  `force_flush()` at least every 2048 emits and asserts exactly 10 000 records were captured
  before benching; bench `group_logs_by_resource_and_scope` /
  `group_spans_by_resource_and_scope` + `prost::Message::encode_to_vec` of the
  `Export*ServiceRequest` as `otel`, and the same followed by gzip (`flate2`, the OTLP exporters'
  compression option) as `otel_gzip`. Span conversion takes `Vec<SpanData>` by value, so it uses
  `iter_batched` with the clone in setup. `print_bytes` reports the protobuf length (raw) and the
  gzip length (wire).
- Metrics are not in this group; they are measured per batch and reported per sample in the `metric_batch` group.

### micromegas binary setup
- `BenchSink` like `properties.rs`, except non-captured blocks go to a worker thread that calls
  `encode_bin` and drops the bytes — the HTTP sink minus the network — and `on_startup` stores
  the `Arc<ProcessInfo>` for that worker and the `encode` group. The worker queue has the same
  default byte caps as `HttpSinkConfig` (`DEFAULT_MAX_QUEUE_BYTES`, the 128 MiB soft cap for thread (Traces) blocks, and
  `DEFAULT_HARD_QUEUE_BYTES`, the 256 MiB hard cap for log/metric blocks, both checked against the
  queue's total bytes) and a drop counter, as in `HttpEventSink`'s `SharedQueue::try_push`.
- `TracingSystemGuard::new(10 MiB, 1 MiB, 10 MiB, sink, HashMap::new(), true)` (CPU tracing on,
  the `telemetry-sink` buffer defaults), `set_max_level(LevelFilter::Info)`,
  `init_thread_stream()` on the main thread, `install_tracing_interop(None)` (it copies the
  current max level, so both libraries filter at INFO).

### opentelemetry binary setup
- One `Resource` (`service.name = "bench"`).
- `SdkLoggerProvider::builder().with_batch_exporter(NoopLogExporter)`;
  `tracing_subscriber::registry().with(OpenTelemetryTracingBridge::new(&logger_provider))
  .with(tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer("bench")))`, set as the
  global default, with an `INFO` level filter on both layers matching micromegas' level check.
- `SdkTracerProvider::builder().with_batch_exporter(NoopSpanExporter)` (default `ParentBased(AlwaysOn)`
  sampler, random ids).
- One `SdkMeterProvider` per batch size with a `ManualReader` (cumulative temporality, the SDK
  default), so the bench triggers each collect itself.
- All processor/reader settings are SDK defaults.
- `NoopLogExporter` / `NoopSpanExporter` convert each batch to OTLP protobuf (the same code as
  the `encode` group) so the background thread does the work a real exporter does, add the record
  count to an `AtomicU64`, and, when a capture flag is set, clone the records into a `Mutex<Vec>`.


### Pacing and drop guard
A tight bench loop outruns the background threads of both libraries: OTel's 2048-slot batch queue
and Micromegas's byte-capped worker queue both drop under it, and the results would describe
dropped data rather than normal operation. Instead of tuning queues, the producer is paced: chunks
of at most `BATCH` (2048) operations are timed, and an untimed `drain` between chunks waits for the
background side to empty.
- OTel `drain`: `force_flush()` on the logger and tracer providers.
- Micromegas `drain`: waits until the bench sink's worker queue is empty and the worker is idle
  (a pending-block counter and a condvar). It does not flush the calling thread's partially
  filled block; blocks reaching the worker as they fill is the normal hot-path behavior.

Drops stay accounted as a guard. After each case, `after_case` panics with the id and the counts
if anything was dropped: for OTel, the exporter's cumulative `AtomicU64` is snapshotted per case
and compared with the emitted count (the SDK's own dropped counter is private); for Micromegas,
the events in each dropped block (`nb_objects()`) are counted. This covers the `contention` cases
at each thread count too. The `encode` capture asserts the exact captured count.

Metric batches go through the same pacing (a chunk is at most 2048 batches) and guard on the
micromegas side. OTel metrics have no queue: recording aggregates in place and the collect is
part of the measured export, so nothing is dropped and nothing is counted.

### Report script (`report.py`)
Stdlib-only Python. Reads `target/criterion/{log,span,contention,encode,metric_batch}/**/new/estimates.json`
(median point estimate), taking group, case, lib and thread count from the `group_id`,
`function_id` and `value_str` fields of the sibling `new/benchmark.json` rather than from the
directory path (criterion replaces `/` with `_`, and case and lib names contain `_`, so paths
cannot be split). Only ids whose lib is one of this crate's libs (`micromegas`,
`micromegas_interop`, `otel`, `otel_tracing`, `otel_gzip`) are
kept, since `rust/tracing/benches/properties.rs` writes the same group names into the same
`rust/target/criterion`. It also reads the bench stdout saved to a file (`print_bytes` lines) and
prints the markdown tables for the docs page: hot path (ns/op per case × library for `log` and `span`), contention
(aggregate Mops/s per case × library × threads), encode (ns/event, raw and wire bytes/event) and
`metric_batch` (per sample, derived as per batch / N: ns on the calling thread with per-batch
columns, ns off the calling thread, bytes, and calling thread plus export; the Micromegas export and
bytes are divided by B first).
Usage: `cargo bench -p micromegas-otel-comparison-bench | tee bench.log` then
`python3 otel-comparison-bench/report.py bench.log`.

## Implementation Steps
1. Add the new dependencies to `[workspace.dependencies]` in `rust/Cargo.toml`.
2. Create `rust/otel-comparison-bench/Cargo.toml` (`publish = false`, two `[[bench]]` targets with
   `harness = false`) and `src/lib.rs` with the shared harness.
3. Write `benches/micromegas.rs`: sink, setup, `log` / `metric_batch` / `span` / `contention` /
   `encode` groups.
4. Write `benches/opentelemetry.rs`: exporters, providers, global subscriber, the same groups,
   pacing, drain and drop guard.
5. Write `report.py`.
6. From `rust/`: `cargo clippy -p micromegas-otel-comparison-bench --all-targets -- -D warnings`,
   `cargo fmt`, `cargo machete`, `cargo deny check bans` (add `skip` entries in `deny.toml` only
   for duplicates the new bridges bring in, with a comment naming the crate).
7. Run the benches on a quiet machine (performance governor, nothing else running), generate the
   tables, and write the docs page (below) with hardware, OS, `rustc` version and crate versions.
8. Add a `CHANGELOG.md` entry under Unreleased.

## Files to Modify
- `rust/Cargo.toml`
- `rust/Cargo.lock`
- `rust/deny.toml` (only if `cargo deny` flags new duplicates)
- `rust/otel-comparison-bench/Cargo.toml` (new; enables `opentelemetry_sdk`'s `experimental_metrics_custom_reader`)
- `rust/otel-comparison-bench/src/lib.rs` (new)
- `rust/otel-comparison-bench/benches/micromegas.rs` (new)
- `rust/otel-comparison-bench/benches/opentelemetry.rs` (new)
- `rust/otel-comparison-bench/report.py` (new)
- `mkdocs/docs/when-to-use/instrumentation-overhead.md` (new)
- `mkdocs/mkdocs.yml`
- `mkdocs/docs/when-to-use/index.md`
- `CHANGELOG.md`

## Trade-offs
- **New crate vs. a bench in `rust/tracing`.** OTel, the `tracing` bridges and `telemetry-sink`
  (for `encode_bin`) would become dev-dependencies of the instrumentation crate every user
  compiles; a separate unpublished crate keeps them out.
- **Two bench binaries vs. one.** Both stacks want a process-global `tracing` subscriber, and
  OTel's batch and reader threads would run during micromegas cases. Separate processes give each
  library the machine to itself; shared ids keep the results side by side.
- **Pacing vs. tuned queues.** Both libraries keep their defaults. Raising a queue size only
  delays the point where a producer faster than the exporter drops records, and a benchmark that
  reports dropped data says little about normal operation, where drops are rare. Pacing the
  producer in chunks no larger than the smallest queue, with an untimed drain between chunks,
  measures the steady state where the exporter keeps up. The cost is that sustained overload is
  not measured.
- **Encoding in the no-op sinks vs. pure discard.** Discarding would leave a background core idle
  that production keeps busy (LZ4 + CBOR for micromegas, protobuf for OTel); encoding keeps the
  hot-path numbers representative of a real deployment's CPU contention without network noise.
- **`tracing` front end for OTel logs vs. the Logs API.** `Logger::emit` is meant for appenders;
  measuring it would describe no real application. The `micromegas_interop` column gives the
  like-for-like `tracing` comparison.
- **Calling `LogBatch::new` despite its "may be made private" note.** It is the only way to
  re-run the OTLP conversion on captured records; versions are pinned, and a future removal shows
  up as a compile error in the bench only.

## Documentation
- New `mkdocs/docs/when-to-use/instrumentation-overhead.md` ("Instrumentation Overhead vs.
  OpenTelemetry"), added to the `When to Use` nav in `mkdocs/mkdocs.yml`: summary, hot-path,
  contention, encode and bytes tables, then methodology — versions, configuration of each
  library, hardware/OS/`rustc`, the semantic differences (per-sample metrics vs. in-process
  aggregation and its cardinality limit, measured per batch, reported per sample, no micromegas span attributes, OTel drops at full
  queues (not exercised by the paced bench), string attribute values), and the exact commands to reproduce.
- `mkdocs/docs/when-to-use/index.md`: link the page from the "Micromegas in brief"
  instrumentation bullet.

## Testing Strategy
The deliverable is a benchmark and a report generator; the bench code has no product logic to
unit-test, and its guards are inline: the single-block assertion before each `encode` sample, and
the drop guard that fails the bench if either library loses a record instead of silently measuring a partly discarding producer.
`report.py`'s parsing is exercised by generating the docs tables from a real run (Implementation
Step 7); a malformed table is obvious on the page. Bench compilation is checked by Step 6's
clippy run, since CI does not build benches.

## Manual Verification
Bench output is read by a human, so these are not automated.
1. `cd rust && cargo bench -p micromegas-otel-comparison-bench | tee /tmp/bench.log` —
   every group runs in both binaries; `log/static/micromegas` reports tens of ns (logs enabled);
   `span/static/micromegas` is non-zero and its `encode` sample reports a non-empty block (thread
   streams enabled); the bench panics with the id and counts if either library drops anything;
   the OTel `metric_batch` export asserts that a collect holds exactly N points.
2. `python3 otel-comparison-bench/report.py /tmp/bench.log` — prints every table (the three
   `metric_batch` tables included) with no empty cells except the `—` combinations listed in the case tables.
3. `cd mkdocs && mkdocs serve` — the new page renders and is reachable from `When to Use`.

## Decisions
- `mkdocs/docs/unreal/index.md:14` is Unreal-specific and is left as is.
- Blog posts and `doc/` presentations are historical records and are left unchanged.
- Both libraries run on defaults, paced in chunks of at most 2048 operations with an untimed drain between chunks, so no record is dropped; a drop fails the bench.
- The project-wide "20 ns per event" figure (CPU tracing, one span begin or end event) stays as a simple headline; only the new page carries precise, per-operation numbers.
- Metrics are measured per batch (one value for each of N static series, N = 10 and 100, OTel collecting once per batch) and reported per sample (per batch / N), so every recorded value is a kept value in both libraries.
