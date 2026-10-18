# Instrumentation Overhead vs. OpenTelemetry

This page compares what it costs a Rust application to record the same logs, metrics and spans with `micromegas-tracing` and with the OpenTelemetry Rust SDK, each in its recommended production configuration with the network export replaced by an in-process sink that still encodes the payload. All numbers come from one full criterion run (not `--quick`) of the benchmark crate, except the `metric_batch` tables, which come from a later full run of the `metric_batch` group alone on the same machine, [`rust/otel-comparison-bench`](https://github.com/madesroches/micromegas/tree/main/rust/otel-comparison-bench), on the hardware listed in [Methodology](#methodology). They are not a promise for your hardware.

**Summary.**

- **Calling-thread cost.** For logs and spans, the calling thread pays a median of 45 ns per static log call and 28 ns per span (one `span/static` operation, which records a begin and an end event) with Micromegas, against 342 ns and 152 ns with the OpenTelemetry SDK, single-threaded. Micromegas copies a fixed-size event into a thread-local buffer for spans and into a shared process-wide buffer for logs; the SDK builds a record and pushes it into a bounded queue. With the bridges for the `tracing` crate the same operations cost more: 796 ns per span through `tracing-opentelemetry`, and 102 ns against 45 ns per static log for a `tracing` event routed into Micromegas.
- **Metrics, per sample.** A sample is one series' value in one batch; a batch is one value for each of N series (a game recording its frame metrics once per frame), N = 10 and 100, and every per-sample figure is the per-batch figure divided by N. On the calling thread a sample costs a median of 24.6 ns with Micromegas (an `imetric!` call) against 67.4 ns with OpenTelemetry (a `Gauge::record` call) at 10 series, and 25.2 against 68.2 ns at 100. Exporting a sample off the calling thread costs 1402 and 1200 ns with Micromegas (`encode_bin`, per batch from a block of many batches, divided by N) against 627 and 685 ns with OpenTelemetry (one collect, OTLP conversion and protobuf encode; 1868 and 965 ns with gzip). Calling thread plus export is 1426 and 1225 ns per sample with Micromegas against 694 and 753 ns with OpenTelemetry (1935 and 1033 ns with gzip), on different threads. A sample is 8.3 and 8.1 bytes on the Micromegas wire against 90.4 and 76.5 bytes of OTLP protobuf (25.1 and 5.8 gzipped).
- **Pacing.** The producer is paced so that neither library drops a record: only the emit calls are timed, in chunks of at most 2048 operations, with an untimed wait for the library's background queue to empty between chunks. The numbers are the steady state where the exporter keeps up.
- **Encoding and bytes.** Off the calling thread, the OpenTelemetry protobuf conversion is cheaper per event than Micromegas's dependency extraction, LZ4 and CBOR encoding in this run; adding gzip to it makes it costlier than Micromegas for the static cases (`log_static_attrs`, `span_static`), about equal for `span_dyn_name` (2839 against 3104 ns), and still far cheaper for `log_dyn_high_card` (806 against 3905 ns). On the wire, Micromegas is smaller than uncompressed OTLP protobuf, and gzipped OTLP is smaller than Micromegas for log records and larger for spans.

## Results

Every cell is a median from one full run, except the `metric_batch` tables (see the introduction). A dash marks a combination that does not exist: Micromegas spans carry no attributes, and `micromegas_interop` (`tracing` crate macros captured by Micromegas through `install_tracing_interop`) only has the cases that `tracing` can express.

### Hot path

**`log`: median ns per operation**

| case | `micromegas` | `micromegas_interop` | `otel` |
|---|---|---|---|
| `static` | 44.8 | 102.0 | 341.5 |
| `formatted` | 80.6 | 143.8 | 323.7 |
| `static_attrs` | 42.9 | — | 672.6 |
| `dyn_low_card` | 71.4 | — | 400.5 |
| `dyn_high_card` | 144.1 | 297.0 | 424.1 |

**`span`: median ns per operation**

| case | `micromegas` | `otel` | `otel_tracing` |
|---|---|---|---|
| `static` | 27.9 | 151.7 | 795.6 |
| `attrs` | — | 227.3 | 911.7 |
| `dyn_name` | 96.1 | 196.6 | 919.4 |

### Contention

The same operation on 1, 4 and 8 threads that start together and use disjoint ranges of ids, paced in chunks of 2048 operations in total (each thread emits `2048 / threads` per chunk). Micromegas logs share one process-wide stream, so threads contend on it; each thread has its own span stream. OpenTelemetry's shared point is its batch queue.

**`contention`: aggregate Mops/s**

| case | lib | 1 thread | 4 threads | 8 threads |
|---|---|---|---|---|
| `log/static` | `micromegas` | 22.9 | 8.8 | 10.2 |
| `log/static` | `otel` | 2.8 | 10.0 | 14.6 |
| `log/dyn_high_card` | `micromegas` | 6.4 | 1.7 | 1.6 |
| `log/dyn_high_card` | `otel` | 1.9 | 9.5 | 13.8 |
| `span/static` | `micromegas` | 33.7 | 119.9 | 217.4 |
| `span/static` | `otel` | 7.0 | 9.2 | 14.0 |

### Encoding off the calling thread

Both libraries do work on another thread before data leaves the process. Micromegas extracts the block's dependencies, compresses dependencies and objects with LZ4 and encodes the block as CBOR (`StreamBlock::encode_bin`, the encoding the HTTP sink runs on its worker thread). OpenTelemetry converts a batch to OTLP protobuf; `otel_gzip` adds the gzip compression an OTLP/HTTP exporter can apply. Each sample is one block or batch of 10 000 events captured from the cases named in the table, so the cost is per event.

**`encode`: ns per event, off the calling thread**

| case | `micromegas` | `otel` | `otel_gzip` |
|---|---|---|---|
| `log_static_attrs` | 774.3 | 514.5 | 918.6 |
| `log_dyn_high_card` | 3904.8 | 329.7 | 805.5 |
| `span_static` | 1229.4 | 256.5 | 2132.4 |
| `span_dyn_name` | 3104.1 | 273.9 | 2839.2 |

**Bytes per event**

| case | `micromegas` raw | `micromegas` wire (LZ4 + CBOR) | `otel` protobuf | `otel_gzip` |
|---|---|---|---|---|
| `log_static_attrs` | 25.0 | 5.4 | 117.0 | 3.7 |
| `log_dyn_high_card` | 95.9 | 27.1 | 100.9 | 4.9 |
| `span_static` | 34.0 | 8.5 | 66.0 | 34.5 |
| `span_dyn_name` | 76.9 | 24.1 | 68.9 | 37.0 |

### Metric batches

A metric batch is one snapshot: one value for each of N series at the same moment, like a game recording its frame metrics once per frame. N is 10 or 100. Each series is a static set of attributes, prebuilt once: three fixed pairs (`x`, `y`, `z`) and a series identifier, all under one metric name. The value changes from batch to batch (a persistent counter).

An OpenTelemetry counter or gauge reduces repeated records of the same series to one point per collect, so a raw `record` call is not a kept sample. Measuring in batches, with one collect per batch, makes every recorded value a kept sample in both libraries: Micromegas stores it and OpenTelemetry exports it as one point. A sample here is one series' value in one batch, so the per-sample cost below (the per-batch cost divided by N) compares like with like.

- **Micromegas.** A batch is N `imetric!` calls, one per series, each with its prebuilt `PropertySet` (`..set`). Many batches fill one `MetricsBlock`, which the worker thread encodes with `encode_bin`.
- **OpenTelemetry.** A batch is N `Gauge<u64>::record` calls, each with the series' prebuilt `&'static [KeyValue]`, followed by one collect through a `ManualReader` (cumulative temporality, the SDK default). The collected `ResourceMetrics` is converted to an `ExportMetricsServiceRequest` with `opentelemetry-proto` and encoded with `prost`. A real exporter does this once per interval; here the interval is one batch. Each batch size has its own meter provider, so a collect holds exactly N series.

**`metric_batch`: median ns per sample on the calling thread** (the N record calls, timed alone, divided by N; the per-batch columns are the undivided medians)

| series per batch | `micromegas` | `otel` | `micromegas` per batch | `otel` per batch |
|---|---|---|---|---|
| 10 | 24.6 | 67.4 | 246.0 | 674.2 |
| 100 | 25.2 | 68.2 | 2520.3 | 6816.4 |

**`metric_batch`: median ns per sample, off the calling thread**

| series per batch | `micromegas` | `otel` | `otel_gzip` |
|---|---|---|---|
| 10 | 1401.8 | 626.9 | 1867.9 |
| 100 | 1199.9 | 684.6 | 965.2 |

For `micromegas`, one batch is `encode_bin` on one `MetricsBlock` holding 10 000 metric events, that is B = 1000 batches at 10 series and B = 100 batches at 100 series (the bench asserts the events fit one block), divided by B, then by N. For `otel` it is one collect, the OTLP conversion and the protobuf encode of one batch, with the batch's records made before the timer starts, divided by N; `otel_gzip` adds gzip.

**Bytes per sample**

| series per batch | `micromegas` raw | `micromegas` wire (LZ4 + CBOR) | `otel` protobuf | `otel_gzip` |
|---|---|---|---|---|
| 10 | 33.2 | 8.3 | 90.4 | 25.1 |
| 100 | 34.7 | 8.1 | 76.5 | 5.8 |

**`metric_batch`: median ns per sample, calling thread plus export** (the two parts run on different threads)

| series per batch | `micromegas` | `otel` | `otel_gzip` |
|---|---|---|---|
| 10 | 1426.4 | 694.4 | 1935.3 |
| 100 | 1225.1 | 752.7 | 1033.4 |

Micromegas figures are the block's size divided by B and by N; raw is the in-memory size of the events and their dependencies, wire is the `encode_bin` output. OpenTelemetry figures are one collected batch divided by N.

## What the numbers do and do not say

- **Logs and spans.** The gap in calling-thread cost is the design difference, not a tuning difference: Micromegas writes a fixed-size event into a thread-local or process-wide buffer and does the rest elsewhere, while the OpenTelemetry SDK builds an owned record (attributes included) and hands it to a bounded queue.
- **Metrics.** In this run a sample costs the calling thread about 2.7 times less with Micromegas than with OpenTelemetry (24.6 against 67.4 ns at 10 series, 25.2 against 68.2 ns at 100). Off the calling thread the order reverses: OpenTelemetry's collect, conversion and protobuf encode takes 627 ns per sample at 10 series against 1402 ns for Micromegas's share of `encode_bin` (685 against 1200 ns at 100 series), and adding gzip brings OpenTelemetry to 1868 and 965 ns. Calling thread plus export, on different threads, is 1426 and 1225 ns per sample with Micromegas against 694 and 753 ns with OpenTelemetry (1935 and 1033 ns with gzip). On the wire a Micromegas sample is smaller than OTLP protobuf (8.3 against 90.4 bytes at 10 series), while gzipped OTLP is larger than Micromegas at 10 series (25.1 against 8.3 bytes) and smaller at 100 (5.8 against 8.1). The OpenTelemetry batch here is collected once per batch; a real exporter collects once per interval, so with a 60 s interval its collect, conversion and encode cost is paid once per interval instead of once per batch, and one point per series per interval is all it keeps, which is the right choice when you want rates and distributions. Micromegas keeps every sample, with its time, so each snapshot stays queryable.
- **Contention.** Micromegas log throughput falls as threads are added in this run (`log/static` from 22.9 Mops/s on 1 thread to 8.8 on 4 and 10.2 on 8; `log/dyn_high_card` from 6.4 to 1.6), because the shared stream is a shared point. OpenTelemetry's aggregate log throughput rises with threads (`log/static` from 2.8 to 14.6 Mops/s), so it overtakes Micromegas for logs at 4 and 8 threads in this run, while Micromegas stays ahead on a single thread. Micromegas span throughput rises with threads (33.7 to 217.4 Mops/s) because each thread writes its own stream, and OpenTelemetry's span throughput rises more slowly (7.0 to 14.0). The paced producer emits only 2048 operations in total between drains, so these figures describe bursts the background threads can absorb, not sustained overload.
- **Encoding.** The encoding comparison runs on captured data of identical content. Micromegas spends more time per event in this run, most of it for the cases with dynamic content (`log_dyn_high_card`, `span_dyn_name`), where it first extracts the dependency records that the objects refer to by id. Whether that off-thread cost matters depends on whether a core is free for it.
- **Bytes.** The raw column is the in-memory size of the events and, for Micromegas, the extracted dependencies; the wire column is what is sent. Repeated attribute values compress well in both formats, and the benchmark's records are repetitive, so wire sizes for real data are likely larger.

## Methodology

**What is measured.** Calling-thread time for one operation, with criterion (`iter_custom`, only the emit calls are timed, see Pacing below): `log` and `span` cases are one macro or API call, `metric_batch` cases are one batch of N calls (see [Metric batches](#metric-batches)). Both libraries record string attribute values (`%i` or `i.to_string()`), since Micromegas property values are strings; the comparison is about the same information, not the cheapest encoding each library offers. Both filter at INFO.

**Micromegas.** `micromegas-tracing` with the `telemetry-sink` buffer defaults (10 MiB for logs and thread spans, 1 MiB for metrics) and CPU tracing enabled. The sink accepts every log, hands each full block to a worker thread that runs `StreamBlock::encode_bin` and drops the bytes (the HTTP sink without the network), and applies the HTTP sink's byte caps to that queue: 128 MiB for thread blocks and 256 MiB for log and metric blocks. `micromegas_interop` uses the `tracing` crate's macros, captured by the interop layer installed with `install_tracing_interop`.

**OpenTelemetry.** `opentelemetry` 0.32.0 and `opentelemetry_sdk` 0.32.1 with the SDK defaults and no tuning of queue sizes: `BatchLogProcessor` and `BatchSpanProcessor` (2048-record queue, 512-record batches, drop on full), `ParentBased(AlwaysOn)` sampling with random ids, and, for the metric batches, a `ManualReader` with cumulative temporality (the SDK default). Logs enter through `opentelemetry-appender-tracing` 0.32.0 and `tracing` events; spans through the tracer directly (`otel`) or through `tracing-opentelemetry` 0.33.0 (`otel_tracing`); metrics through a `Gauge<u64>` created once per batch size. The log and span exporters convert each batch to OTLP protobuf with `opentelemetry-proto` 0.32.0, as a network exporter would, and discard it; the metric batches are collected with `ManualReader::collect` (a public trait method that the SDK exposes behind its `experimental_metrics_custom_reader` feature, enabled in the bench crate so it can keep a handle to the reader) and converted and encoded the same way.

**Two processes.** The two libraries run in separate bench binaries because both want a process-global `tracing` subscriber and because OpenTelemetry's batch and reader threads would otherwise run during the Micromegas cases. The group and case ids are the same in both, so the results pair up.

**Pacing.** Both libraries run on their defaults, and the producer is paced instead of the queues being tuned. Each criterion sample is split into chunks of at most 2048 operations (the OpenTelemetry default queue size); only the emit calls of a chunk are timed. For `metric_batch` the unit of a chunk is one batch. Between chunks, outside the timed region, the bench waits until the library's background queue is empty: `force_flush` on the OpenTelemetry providers, and for Micromegas until its worker has encoded every block that reached it (the calling thread's partially filled block stays where it is). In the contention cases each thread emits `2048 / threads` operations per chunk, all threads start a chunk together, and a chunk costs its slowest thread. Neither library ever drops a record, and the bench fails if either does: after each case it compares the number of records the exporter or worker saw with the number emitted (OpenTelemetry does not expose its dropped-record counter, so the exporter's own count is the reference) and panics on any difference. This measures the steady state where the exporter keeps up, which is normal operation; it says nothing about how either library behaves when the producer outruns its exporter for long.

**Noise.** One full run on one machine. Two cases were repeated five more times: `log/static/micromegas` gave criterion estimates between 43.1 and 44.3 ns (44.8 ns in the table) and `span/static/micromegas` between 28.1 and 28.7 ns (27.9 ns in the table). The machine was a laptop under WSL2 with no control of the CPU governor and ordinary background processes. Bench threads are not pinned, so samples can land on performance or efficiency cores, a likely source of run-to-run spread, and the multi-thread cases are the most exposed to it. Treat differences of 10% as noise.

**Hardware and software.**

| | |
|---|---|
| Laptop | ASUS ROG Flow X16 (GV601VU) |
| CPU | Intel Core i9-13900H, 20 logical CPUs (hybrid: 6 performance cores with 12 threads, 8 efficiency cores) |
| Memory | 31 GiB |
| Host OS | Windows 11 (build 10.0.26300), WSL 3.0.3.0 |
| Power | Windows "Balanced" power plan, on AC power |
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
cargo bench -p micromegas-otel-comparison-bench | tee bench.log
python3 otel-comparison-bench/report.py bench.log
```

`--quick` gives a faster, less precise run. `report.py` reads criterion's estimates from `target/criterion` and the saved output, and prints the tables on this page. A later OpenTelemetry release is a re-run: bump the versions in `rust/Cargo.toml` and run the same commands.

## Semantic differences

- Micromegas metrics are samples, one per call. OpenTelemetry instruments aggregate in the process and export one point per series per interval, with a cardinality limit per instrument (2000 attribute sets by default). That is why metrics are measured per batch, one value for each of N series with one collect per batch, and reported per sample (per batch divided by N), so every recorded value is a kept value in both libraries.
- Micromegas spans have no attributes. A name built at run time (`span_scope_named_dyn!`) is the closest equivalent, and the `span/dyn_name` case compares it with an OpenTelemetry span of the same name.
- OpenTelemetry drops records when its queue is full and Micromegas drops blocks when its queue passes its byte caps. Neither reports the drop on the calling thread. The benchmark's pacing keeps both from dropping, so no number here includes that behavior.
- Attribute values are strings in both libraries in these cases. OpenTelemetry supports typed values, which would change its encoded size.
- The cases record to an in-process sink. The network, the ingestion service and storage are not part of any number on this page; see [Cost Effectiveness](saas-vendors.md) for the storage side.
