# Instrumentation Overhead vs. OpenTelemetry

This page compares what it costs a Rust application to record the same logs, metrics and spans with `micromegas-tracing` and with the OpenTelemetry Rust SDK, each in its recommended production configuration with the network export replaced by an in-process sink that still encodes the payload. All numbers come from one run of the benchmark crate, [`rust/otel-comparison-bench`](https://github.com/madesroches/micromegas/tree/main/rust/otel-comparison-bench), on the hardware listed in [Methodology](#methodology). They are not a promise for your hardware, and the run used criterion's `--quick` mode, which takes fewer samples than a full run.

**Summary.**

- **Calling-thread cost.** For logs and spans, the calling thread pays a median of 49 ns per static log call and 30 ns per span (one `span/static` operation, which records a begin and an end event) with Micromegas, against 297 ns and 156 ns with the OpenTelemetry SDK, in a single-threaded `--quick` run. Micromegas copies the event into a thread-local buffer; the SDK builds a record and pushes it into a bounded queue. With the bridges for the `tracing` crate the same operations cost more: 942 ns per span through `tracing-opentelemetry`, and 114 ns against 49 ns per static log for a `tracing` event routed into Micromegas. The OpenTelemetry figures come from runs that drop a large share of records, which the [Results](#results) tables show.
- **Metrics are a different trade.** OpenTelemetry aggregates in the process, so a plain counter increment costs 7 ns, less than Micromegas's 27 ns, which records every sample. With attributes the two are within about a factor of two in either direction (the `set` case is 2.2 times), and the difference between them is that Micromegas keeps every sample while OpenTelemetry keeps one aggregated point per series per export interval.
- **Drops.** At the SDK's default queue sizes, a producer faster than the exporter thread makes OpenTelemetry drop records. In this benchmark's tight loops that is 21% to 65% of log records and 33% to 60% of spans, and the drop rate is printed next to each number. Micromegas also drops when its encoding worker falls behind its 128 MiB (thread blocks) or 256 MiB (log and metric blocks) queue caps. It dropped nothing in the single-threaded cases of the run below, but most spans in the 4- and 8-thread contention cases; an earlier run of the same code also dropped in two single-threaded cases, because the number of iterations criterion picks changes how long the loop outruns the worker.
- **Encoding and bytes.** Off the calling thread, the OpenTelemetry protobuf conversion is cheaper per event than Micromegas's dependency extraction, LZ4 and CBOR encoding in this run; adding gzip to it makes it costlier than Micromegas for the static cases (`log_static_attrs`, `span_static`), about equal for `span_dyn_name` (3067 against 3296 ns), and still far cheaper for `log_dyn_high_card` (888 against 4227 ns). On the wire, Micromegas is smaller than uncompressed OTLP protobuf, and gzipped OTLP is smaller than Micromegas for log records and larger for spans.

## Results

Every cell is a median from one `--quick` run. A dash marks a combination that does not exist: Micromegas spans carry no attributes, and `micromegas_tracing` (the `tracing` crate's macros routed into Micromegas through its interop layer) only has the cases that `tracing` can express. The percentage in parentheses is the share of emitted records or spans that never reached the exporter or worker; a number with a high drop rate measures a producer that was partly discarding its work, so read it with that in mind.

### Hot path

**`log`: median ns per operation (dropped %)**

| case | `micromegas` | `micromegas_tracing` | `otel` |
|---|---|---|---|
| `static` | 48.9 (0.0%) | 113.6 (0.0%) | 297.0 (21.1%) |
| `formatted` | 83.7 (0.0%) | 142.9 (0.0%) | 258.4 (26.1%) |
| `static_attrs` | 46.7 (0.0%) | — | 781.3 (65.0%) |
| `dyn_low_card` | 71.4 (0.0%) | — | 261.0 (38.2%) |
| `dyn_high_card` | 190.3 (0.0%) | 401.2 (0.0%) | 321.7 (24.1%) |

**`metric`: median ns per operation (dropped %)**

| case | `micromegas` | `otel_counter` | `otel_gauge` |
|---|---|---|---|
| `plain` | 27.3 (0.0%) | 7.4 | 4.1 |
| `static_attrs` | 25.9 (0.0%) | 36.9 | 34.0 |
| `set` | 25.9 (0.0%) | 57.3 | 57.7 |
| `dyn_low_card` | 58.1 (0.0%) | 35.1 | 32.1 |
| `dyn_high_card` | 167.6 (0.0%) | 148.9 | 148.4 |

**`span`: median ns per operation (dropped %)**

| case | `micromegas` | `otel` | `otel_tracing` |
|---|---|---|---|
| `static` | 30.0 (0.0%) | 156.0 (48.1%) | 941.7 (33.9%) |
| `attrs` | — | 201.8 (59.7%) | 1081.5 (39.3%) |
| `dyn_name` | 96.8 (0.0%) | 204.0 (42.3%) | 1126.2 (33.4%) |

### Contention

The same operation on 1, 4 and 8 threads that start together and use disjoint ranges of ids. Micromegas logs and metrics share one process-wide stream per kind, so threads contend on it; each thread has its own span stream. OpenTelemetry's shared points are its batch queue and its metric aggregation map. Rows with a high drop rate are not comparable with rows without one: throughput that includes discarded records is not sustained delivery.

**`contention`: aggregate Mops/s (dropped %)**

| case | lib | 1 thread | 4 threads | 8 threads |
|---|---|---|---|---|
| `log/static` | `micromegas` | 20.5 (0.0%) | 7.4 (0.0%) | 5.7 (0.0%) |
| `log/static` | `otel` | 3.8 (31.2%) | 13.6 (80.7%) | 19.7 (88.3%) |
| `log/dyn_high_card` | `micromegas` | 4.9 (0.0%) | 1.5 (0.0%) | 1.4 (0.0%) |
| `log/dyn_high_card` | `otel` | 2.9 (30.4%) | 11.5 (84.7%) | 17.9 (92.1%) |
| `metric/static_attrs` | `micromegas` | 31.0 (0.0%) | 11.0 (0.0%) | 8.3 (0.0%) |
| `metric/static_attrs` | `otel_counter` | 26.6 | 11.2 | 11.9 |
| `span/static` | `micromegas` | 27.9 (0.0%) | 122.7 (87.5%) | 166.2 (93.5%) |
| `span/static` | `otel` | 6.6 (51.9%) | 8.1 (65.4%) | 11.1 (75.4%) |

### Encoding off the calling thread

Both libraries do work on another thread before data leaves the process. Micromegas extracts the block's dependencies, compresses dependencies and objects with LZ4 and encodes the block as CBOR (`StreamBlock::encode_bin`, the encoding the HTTP sink runs on its worker thread). OpenTelemetry converts a batch to OTLP protobuf; `otel_gzip` adds the gzip compression an OTLP/HTTP exporter can apply. Each sample is one block or batch of 10 000 events captured from the cases named in the table, so the cost is per event.

**`encode`: ns per event, off the calling thread**

| case | `micromegas` | `otel` | `otel_gzip` |
|---|---|---|---|
| `log_static_attrs` | 821.9 | 540.0 | 1049.5 |
| `log_dyn_high_card` | 4227.2 | 348.6 | 887.5 |
| `span_static` | 1331.0 | 277.6 | 2270.9 |
| `span_dyn_name` | 3296.4 | 288.4 | 3066.8 |

**Bytes per event**

| case | `micromegas` raw | `micromegas` wire (LZ4 + CBOR) | `otel` protobuf | `otel_gzip` |
|---|---|---|---|---|
| `log_static_attrs` | 25.0 | 5.4 | 117.0 | 3.7 |
| `log_dyn_high_card` | 95.9 | 27.1 | 100.9 | 5.0 |
| `span_static` | 34.0 | 8.5 | 66.0 | 34.6 |
| `span_dyn_name` | 76.9 | 24.1 | 68.9 | 37.1 |

**Micromegas metric bytes per event**

| case | raw | wire (LZ4 + CBOR) |
|---|---|---|
| `metric_static_attrs` | 33.1 | 5.3 |

Metrics are not in the encoding comparison: OpenTelemetry exports one aggregated point per series per interval, so bytes per event has no counterpart there. Micromegas's own figure is above.

## What the numbers do and do not say

- **Logs and spans.** The gap in calling-thread cost is the design difference, not a tuning difference: Micromegas writes a fixed-size event into a thread-local or process-wide buffer and does the rest elsewhere, while the OpenTelemetry SDK builds an owned record (attributes included) and hands it to a bounded queue. The OpenTelemetry cost also depends on the queue: when it is full the record is dropped, and a dropped record skips part of the work, so the OpenTelemetry numbers with high drop rates may understate what delivering every record would cost on the calling thread.
- **Metrics.** OpenTelemetry's aggregation makes the hot path cheap and makes the output small, and it is the right choice when you want rates and distributions per series. It limits each instrument to 2000 attribute sets by default, after which values go to an overflow series; the `dyn_high_card` metric cases exceed it. Micromegas records each sample with its time and properties, so high-cardinality ids remain queryable, at a calling-thread cost that does not depend on cardinality.
- **Contention.** Micromegas log and metric throughput falls as threads are added in this run (`log/static` from 20.5 to 5.7 Mops/s between 1 and 8 threads), because the shared stream is a shared point. OpenTelemetry's aggregate log and span throughput rises with threads, but only because most records are dropped at its queue (80% to 92% of logs); the counter's throughput falls from 26.6 to 11.9 Mops/s. Micromegas span throughput rises with threads because each thread writes its own stream, and its high drop rate at 4 and 8 threads means that the encoding worker, a single thread in this benchmark, could not drain thread blocks as fast as they were produced. In a real process the producer rarely runs flat out; the benchmark does, on purpose.
- **Encoding.** The encoding comparison runs on captured data of identical content. Micromegas spends more time per event in this run, most of it for the cases with dynamic content (`log_dyn_high_card`, `span_dyn_name`), where it first extracts the dependency records that the objects refer to by id. Whether that off-thread cost matters depends on whether a core is free for it.
- **Bytes.** The raw column is the in-memory size of the events and, for Micromegas, the extracted dependencies; the wire column is what is sent. Repeated attribute values compress well in both formats, and the benchmark's records are repetitive, so wire sizes for real data are likely larger.

## Methodology

**What is measured.** Calling-thread time for one operation, with criterion: `log` and `span` cases are one macro or API call, `metric` cases are one sample. Both libraries record string attribute values (`%i` or `i.to_string()`), since Micromegas property values are strings; the comparison is about the same information, not the cheapest encoding each library offers. Both filter at INFO.

**Micromegas.** `micromegas-tracing` with the `telemetry-sink` buffer defaults (10 MiB for logs and thread spans, 1 MiB for metrics) and CPU tracing enabled. The sink accepts every log, hands each full block to a worker thread that runs `StreamBlock::encode_bin` and drops the bytes (the HTTP sink without the network), and applies the HTTP sink's byte caps to that queue: 128 MiB for thread blocks and 256 MiB for log and metric blocks. Dropped blocks are counted in events. `micromegas_tracing` events go through `install_tracing_interop`.

**OpenTelemetry.** `opentelemetry` 0.32.0 and `opentelemetry_sdk` 0.32.1 with the SDK defaults and no tuning: `BatchLogProcessor` and `BatchSpanProcessor` (2048-record queue, 512-record batches, drop on full), `ParentBased(AlwaysOn)` sampling with random ids, and a `PeriodicReader` with its default 60 s interval, cumulative temporality and the default cardinality limit. Logs enter through `opentelemetry-appender-tracing` 0.32.0 and `tracing` events; spans through the tracer directly (`otel`) or through `tracing-opentelemetry` 0.33.0 (`otel_tracing`); metrics through a `Counter<u64>` (`otel_counter`) and a `Gauge<u64>` (`otel_gauge`) created once. The log and span exporters convert each batch to OTLP protobuf with `opentelemetry-proto` 0.32.0, as a network exporter would, and discard it; the metric exporter does nothing, because the first export is a minute away. Tuned queues are deliberately not measured: a larger queue only delays the point where a producer faster than the exporter drops records, and the drop rate at defaults is the more informative figure.

**Two processes.** The two libraries run in separate bench binaries because both want a process-global `tracing` subscriber and because OpenTelemetry's batch and reader threads would otherwise run during the Micromegas cases. The group and case ids are the same in both, so the results pair up.

**Drop accounting.** After each log and span case, the bench flushes the library's queue, compares the number of records the exporter or worker saw with the number emitted (including criterion's warm-up iterations), and prints the difference. OpenTelemetry does not expose its dropped-record counter, so the exporter's own count is the reference.

**Noise.** One run on one machine, `--quick`. Two cases were repeated. `log/static/micromegas`: six runs gave medians between 46.4 and 51.3 ns. `span/static/micromegas`: six later runs gave medians of 37.0, 37.2, 37.1, 35.8, 35.9 and 35.3 ns, higher than the 30.0 ns in the table, so the per-span cost in this run is 30 to 37 ns. The machine was a laptop under WSL2 with no control of the CPU governor and ordinary background processes, so treat differences of 10% as noise.

**Hardware and software.**

| | |
|---|---|
| CPU | Intel Core i9-13900H, 20 logical CPUs |
| Memory | 31 GiB |
| OS | Linux 6.18.54.1-microsoft-standard-WSL2 x86_64, under a Microsoft hypervisor |
| `rustc` | 1.99.0 (b940084d7 2026-09-28), `bench` profile (optimized) |
| `micromegas-tracing` | 0.32.0 |
| `opentelemetry` / `opentelemetry_sdk` | 0.32.0 / 0.32.1 |
| `opentelemetry-appender-tracing` | 0.32.0 |
| `tracing-opentelemetry` | 0.33.0 |
| `opentelemetry-proto` | 0.32.0 |
| Date | October 2026 |

**Reproduce.** From the `rust/` directory of the repository:

```bash
cargo bench -p micromegas-otel-comparison-bench -- --quick | tee bench.log
python3 otel-comparison-bench/report.py bench.log
```

Drop `--quick` for a longer, more precise run. `report.py` reads criterion's estimates from `target/criterion` and the saved output, and prints the tables on this page. A later OpenTelemetry release is a re-run: bump the versions in `rust/Cargo.toml` and run the same commands.

## Semantic differences

- Micromegas metrics are samples, one per call. OpenTelemetry instruments aggregate in the process and export one point per series per interval, with a cardinality limit per instrument.
- Micromegas spans have no attributes. A name built at run time (`span_scope_named_dyn!`) is the closest equivalent, and the `span/dyn_name` case compares it with an OpenTelemetry span of the same name.
- OpenTelemetry drops records when its queue is full and Micromegas drops blocks when its queue passes its byte caps. Neither reports the drop on the calling thread.
- Attribute values are strings in both libraries in these cases. OpenTelemetry supports typed values, which would change its encoded size.
- The cases record to an in-process sink. The network, the ingestion service and storage are not part of any number on this page; see [Cost Effectiveness](saas-vendors.md) for the storage side.
