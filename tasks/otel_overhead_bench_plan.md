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
- `pub fn bench_cases(c, group, lib, cases: &[(&str, Case)], after_case: Option<fn(id: &str, emitted: u64)>)`
  — one criterion group per category, `BenchmarkId::new(case, lib)`, persistent counter across
  warm-up and samples (as in `properties.rs`). Same ids in both binaries, so `report.py` pairs the
  libraries by `group_id`/`function_id`/`value_str`, never by directory name. `after_case` runs once per case after
  its last sample and receives the counter's final value (the case's emitted count).
- `pub fn bench_contention(c, lib, cases, per_thread_init: fn(), per_thread_exit: fn(),
  after_case: Option<fn(id: &str, emitted: u64)>)` — the `properties.rs`
  `iter_custom` + `Barrier` + disjoint-range harness, ids `BenchmarkId::new(format!("{case}/{lib}"),
  threads)` for 1, 4, 8 threads with `Throughput::Elements(threads)`. `per_thread_init` runs in
  each spawned thread before the barrier (micromegas needs `init_thread_stream()` for spans);
  `per_thread_exit` runs in each spawned thread before it exits (micromegas calls
  `unregister_thread_stream()` so no dangling `*mut ThreadStream` stays in the dispatch). `after_case`
  runs once per case and thread count with the total emitted across threads.
- `pub fn print_bytes(id, nb_events, raw_bytes, wire_bytes)` — one `println!` line per encoded
  sample (criterion has no custom-metric output), parsed by `report.py`.

### Operations and cases
Both libraries record string attribute values (`%i` / `i.to_string()`), since micromegas property
values are strings; the comparison is about the same information, not the cheapest encoding each
library offers.

`log` group — micromegas native macros vs. `tracing` macros through `OpenTelemetryTracingBridge`:

| case | `micromegas` | `micromegas_tracing` (via interop) | `otel` |
|---|---|---|---|
| `static` | `info!("static")` | `tracing::info!("static")` | `tracing::info!("static")` |
| `formatted` | `info!("value {}", i)` | `tracing::info!("value {}", i)` | same |
| `static_attrs` | `info!(a = "1", b = "2", c = "3", "static")` | — | `tracing::info!(a = "1", b = "2", c = "3", "static")` |
| `dyn_low_card` | `info!(l = LABELS[i % 4], "static")` | — | `tracing::info!(l = LABELS[i % 4], "static")` |
| `dyn_high_card` | `info!(id = %i, "static")` | `tracing::info!(id = %i, "static")` | same |

`micromegas_tracing` shows the cost for an application already instrumented with `tracing`; it
runs in the micromegas binary, whose global `tracing` subscriber is the interop layer.

`metric` group — micromegas samples (`imetric!`) vs. OTel instruments created once
(`OnceLock<Counter<u64>>`, `OnceLock<Gauge<u64>>`), each case run against both `otel_counter`
(`add`) and `otel_gauge` (`record`):

| case | `micromegas` | `otel_counter` / `otel_gauge` attributes |
|---|---|---|
| `plain` | `imetric!("name", "unit", 1)` | `&[]` |
| `static_attrs` | `imetric!("name", "unit", tier = "ram", 1)` | `&[KeyValue::new("tier", "ram")]` |
| `set` | `imetric!("name", "unit", ..set, 1)` (3 pairs) | a prebuilt `&'static [KeyValue]` of the same 3 pairs |
| `dyn_low_card` | `l = LABELS[i % 4]` | `KeyValue::new("l", LABELS[i % 4])` |
| `dyn_high_card` | `id = %i` | `KeyValue::new("id", i.to_string())` — hits the 2000-series overflow after warm-up |

`span` group — micromegas thread spans vs. the native OTel tracer (`otel`) and `tracing` spans
through `tracing-opentelemetry` (`otel_tracing`). Each span is a root span (no active context):

| case | `micromegas` | `otel` | `otel_tracing` |
|---|---|---|---|
| `static` | `span_scope!("static")` | `let _s = tracer.start("static");` | `let _e = tracing::info_span!("static").entered();` |
| `attrs` | — (no span attributes) | `tracer.span_builder("static").with_attributes([KeyValue::new("id", i.to_string())]).start(&tracer)` | `info_span!("static", id = %i)` |
| `dyn_name` | `span_scope_named_dyn!(format!("span {i}"))` | `tracer.start(format!("span {i}"))` | `info_span!("static", otel.name = %format_args!("span {i}"))` |

`contention` group — `log/static`, `log/dyn_high_card`, `metric/static_attrs`, `span/static`
for each library at 1, 4, 8 threads. Micromegas logs and metrics share one process-wide stream
per kind; OTel's batch channel and metric aggregation map are its shared points.

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
- Metrics are not in this group: OTel exports one point per series per interval, so bytes per
  event has no OTel counterpart. The page states micromegas metric bytes/event on its own,
  printed from a captured `MetricsBlock` of 5 000 `metric/static_attrs` samples.

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
- `SdkMeterProvider::builder().with_reader(PeriodicReader::builder(NoopMetricExporter).build())`
  (default 60 s interval, cumulative temporality, default cardinality limit).
- All processor/reader settings are SDK defaults.
- `NoopLogExporter` / `NoopSpanExporter` convert each batch to OTLP protobuf (the same code as
  the `encode` group) so the background thread does the work a real exporter does, add the record
  count to an `AtomicU64`, and, when a capture flag is set, clone the records into a `Mutex<Vec>`.
  `NoopMetricExporter` returns `Ok` and does nothing.

### Drop accounting
A tight bench loop can outrun OTel's background thread; a full queue makes `emit` / `end`
return early. Dropped records are data the exporter never sees, so the drop rate is reported next to each number. After each OTel log and span case the bench
calls `force_flush()` from the `after_case` callback, compares the exporter's count with the
case's emitted count, and prints `<id>: dropped N of M (P%)` for `report.py`. The exporter's
`AtomicU64` is cumulative, so it is snapshotted before each case and the per-case delta is used.
This covers the `contention` log and span cases at each thread count too, which share the same
2048 queue. The micromegas binary prints the same line format for its capped worker queue, counting the
events (`nb_objects()`) in each dropped block so N and M are event counts. The results page shows the drop rate next to each log and span number.

### Report script (`report.py`)
Stdlib-only Python. Reads `target/criterion/{log,metric,span,contention,encode}/**/new/estimates.json`
(median point estimate), taking group, case, lib and thread count from the `group_id`,
`function_id` and `value_str` fields of the sibling `new/benchmark.json` rather than from the
directory path (criterion replaces `/` with `_`, and case and lib names contain `_`, so paths
cannot be split). Only ids whose lib is one of this crate's libs (`micromegas`,
`micromegas_tracing`, `otel`, `otel_counter`, `otel_gauge`, `otel_tracing`, `otel_gzip`) are
kept, since `rust/tracing/benches/properties.rs` writes the same group names into the same
`rust/target/criterion`. It also reads the bench stdout saved to a file (`print_bytes` and drop lines) and
prints the markdown tables for the docs page: hot path (ns/op per case × library), contention
(aggregate Mops/s per case × library × threads), encode (ns/event, raw and wire bytes/event),
drop rates. Usage: `cargo bench -p micromegas-otel-comparison-bench | tee bench.log` then
`python3 otel-comparison-bench/report.py bench.log`.

## Implementation Steps
1. Add the new dependencies to `[workspace.dependencies]` in `rust/Cargo.toml`.
2. Create `rust/otel-comparison-bench/Cargo.toml` (`publish = false`, two `[[bench]]` targets with
   `harness = false`) and `src/lib.rs` with the shared harness.
3. Write `benches/micromegas.rs`: sink, setup, `log` / `metric` / `span` / `contention` /
   `encode` groups.
4. Write `benches/opentelemetry.rs`: exporters, providers, global subscriber, the same groups,
   drop accounting.
5. Write `report.py`.
6. From `rust/`: `cargo clippy -p micromegas-otel-comparison-bench --all-targets -- -D warnings`,
   `cargo fmt`, `cargo machete`, `cargo deny check bans` (add `skip` entries in `deny.toml` only
   for duplicates the new bridges bring in, with a comment naming the crate).
7. Run the benches on a quiet machine (performance governor, nothing else running), generate the
   tables, and write the docs page (below) with hardware, OS, `rustc` version and crate versions.
8. Reconcile the "20ns per event" claim in `mkdocs/docs/index.md`, `README.md`,
   `mkdocs/docs/architecture/index.md` and `rust/tracing/src/lib.rs`, `rust/public/Cargo.toml`,
   `rust/tracing/proc-macros/src/lib.rs`, `welcome/src/components/Differentiators.tsx` and
   `.github/copilot-instructions.md` with the measured `log/static/micromegas` number, and link it to the new page from the docs.
9. Add a `CHANGELOG.md` entry under Unreleased.

## Files to Modify
- `rust/Cargo.toml`
- `rust/Cargo.lock`
- `rust/deny.toml` (only if `cargo deny` flags new duplicates)
- `rust/otel-comparison-bench/Cargo.toml` (new)
- `rust/otel-comparison-bench/src/lib.rs` (new)
- `rust/otel-comparison-bench/benches/micromegas.rs` (new)
- `rust/otel-comparison-bench/benches/opentelemetry.rs` (new)
- `rust/otel-comparison-bench/report.py` (new)
- `mkdocs/docs/when-to-use/instrumentation-overhead.md` (new)
- `mkdocs/mkdocs.yml`
- `mkdocs/docs/when-to-use/index.md`
- `mkdocs/docs/index.md`
- `README.md`
- `mkdocs/docs/architecture/index.md`
- `rust/tracing/src/lib.rs`
- `rust/public/Cargo.toml`
- `rust/tracing/proc-macros/src/lib.rs`
- `welcome/src/components/Differentiators.tsx`
- `.github/copilot-instructions.md`
- `CHANGELOG.md`

## Trade-offs
- **New crate vs. a bench in `rust/tracing`.** OTel, the `tracing` bridges and `telemetry-sink`
  (for `encode_bin`) would become dev-dependencies of the instrumentation crate every user
  compiles; a separate unpublished crate keeps them out.
- **Two bench binaries vs. one.** Both stacks want a process-global `tracing` subscriber, and
  OTel's batch and reader threads would run during micromegas cases. Separate processes give each
  library the machine to itself; shared ids keep the results side by side.
- **SDK defaults vs. tuned OTel queues.** Raising the queue size only delays the point where a
  producer faster than the exporter drops records; reporting the drop rate at defaults is more
  informative than picking a size that hides it on one machine.
- **Encoding in the no-op sinks vs. pure discard.** Discarding would leave a background core idle
  that production keeps busy (LZ4 + CBOR for micromegas, protobuf for OTel); encoding keeps the
  hot-path numbers representative of a real deployment's CPU contention without network noise.
- **`tracing` front end for OTel logs vs. the Logs API.** `Logger::emit` is meant for appenders;
  measuring it would describe no real application. The `micromegas_tracing` column gives the
  like-for-like `tracing` comparison.
- **Calling `LogBatch::new` despite its "may be made private" note.** It is the only way to
  re-run the OTLP conversion on captured records; versions are pinned, and a future removal shows
  up as a compile error in the bench only.

## Documentation
- New `mkdocs/docs/when-to-use/instrumentation-overhead.md` ("Instrumentation Overhead vs.
  OpenTelemetry"), added to the `When to Use` nav in `mkdocs/mkdocs.yml`: summary, hot-path,
  contention, encode and bytes tables, then methodology — versions, configuration of each
  library, hardware/OS/`rustc`, the semantic differences (per-sample metrics vs. in-process
  aggregation and its cardinality limit, no micromegas span attributes, OTel drops at full
  queues, string attribute values), and the exact commands to reproduce.
- `mkdocs/docs/when-to-use/index.md`: link the page from the "Micromegas in brief"
  instrumentation bullet.
- `mkdocs/docs/index.md`, `README.md`, `mkdocs/docs/architecture/index.md`,
  `rust/tracing/src/lib.rs`: link the per-event overhead claim to the page (docs) and correct the
  number if the measurement disagrees. `rust/public/Cargo.toml`,
  `rust/tracing/proc-macros/src/lib.rs`, `welcome/src/components/Differentiators.tsx` and
  `.github/copilot-instructions.md`: correct the number only.

## Testing Strategy
The deliverable is a benchmark and a report generator; the bench code has no product logic to
unit-test, and its guards are inline: the single-block assertion before each `encode` sample, and
the drop accounting that makes a starved OTel exporter visible instead of silently fast.
`report.py`'s parsing is exercised by generating the docs tables from a real run (Implementation
Step 7); a malformed table is obvious on the page. Bench compilation is checked by Step 6's
clippy run, since CI does not build benches.

## Manual Verification
Bench output is read by a human, so these are not automated.
1. `cd rust && cargo bench -p micromegas-otel-comparison-bench -- --quick | tee /tmp/bench.log` —
   every group runs in both binaries; `log/static/micromegas` reports tens of ns (logs enabled);
   `span/static/micromegas` is non-zero and its `encode` sample reports a non-empty block (thread
   streams enabled); each OTel log/span case prints a drop line.
2. `python3 otel-comparison-bench/report.py /tmp/bench.log` — prints every table with no empty
   cells except the `—` combinations listed in the case tables.
3. `cd mkdocs && mkdocs serve` — the new page renders and is reachable from `When to Use`.

## Decisions
- `mkdocs/docs/unreal/index.md:14` is Unreal-specific and is left as is.
- Blog posts and `doc/` presentations are historical records and are left unchanged.
- OTel runs on SDK defaults only, with no tuned-OTel row; its drop rate is reported instead.
