# Instrumentation Overhead vs. OpenTelemetry

Instrumentation must not change the shape of the thread it instruments. This page measures what `micromegas-tracing` and the OpenTelemetry Rust SDK add to the calling thread per log event, span and metric sample, so that recording telemetry does not change the timing profile of the code it observes.

Both libraries record the same logs, spans and metric samples, each with its production defaults and with the network replaced by an in-process encoder. Every figure is a median per log event, span or metric sample, measured on one laptop, so treat it as a comparison rather than a promise for your hardware. [Methodology](#methodology) explains how it was measured. A dash in a table marks a combination that does not exist: Micromegas spans carry no attributes, and `micromegas_interop` (`tracing` crate macros captured by Micromegas through `install_tracing_interop`) only has the cases that `tracing` can express.

## Calling-thread cost

This is the time the application thread spends recording one item, which is the cost that stays on in production.

**`log`: median ns per log call on the calling thread**

| case | `micromegas` | `micromegas_interop` | `otel` |
|---|---|---|---|
| `static` | 44.6 | 104.0 | 293.2 |
| `formatted` | 77.3 | 156.1 | 297.5 |
| `static_attrs` | 45.0 | — | 638.0 |
| `dyn_low_card` | 67.6 | — | 470.4 |
| `dyn_high_card` | 150.9 | 314.4 | 418.3 |

Micromegas is faster in every case; a `tracing` event routed into Micromegas costs more than the native macro but still less than OpenTelemetry.

**`span`: median ns per span on the calling thread** (one operation records a begin and an end event)

| case | `micromegas` | `otel` | `otel_tracing` |
|---|---|---|---|
| `static` | 27.7 | 146.3 | 776.5 |
| `attrs` | — | 218.8 | 902.8 |
| `dyn_name` | 87.1 | 193.7 | 876.2 |

`otel_tracing` is OpenTelemetry through the `tracing-opentelemetry` bridge, which costs several times more than the direct tracer.

**`metric`: median ns per metric sample on the calling thread** (one value of one series; the series setup is in [Metric samples](#metric-samples))

| case | `micromegas` | `otel` |
|---|---|---|
| 10 series | 24.2 | 68.5 |
| 100 series | 24.1 | 67.9 |

A sample costs the calling thread about 2.8 times less with Micromegas (`imetric!`) than with OpenTelemetry (`Gauge::record`).

## Overall cost (off the calling thread)

This work runs on a background thread and does not affect the instrumented thread. Both libraries do it before data leaves the process: Micromegas runs `StreamBlock::encode_bin` (dependency extraction, LZ4, CBOR); OpenTelemetry converts to OTLP protobuf, and `otel_gzip` adds the gzip an OTLP/HTTP exporter can apply. The production Micromegas encoder compresses with LZ4 at level 10 (LZ4 HC, see `rust/telemetry/src/compression.rs`), trading worker CPU for wire size, which dominates its encode cost.

**`encode`: median ns per event, off the calling thread**

| case | `micromegas` | `otel` | `otel_gzip` |
|---|---|---|---|
| `log_static_attrs` | 754.4 | 535.8 | 927.7 |
| `log_dyn_high_card` | 3776.7 | 319.4 | 814.3 |
| `span_static` | 1205.9 | 257.3 | 2130.4 |
| `span_dyn_name` | 2997.7 | 274.2 | 3027.9 |

OpenTelemetry encodes faster per event, most clearly for `log_dyn_high_card`. With gzip it is costlier than Micromegas for the static cases and about equal for `span_dyn_name`.

**Bytes per log event or span**

| case | `micromegas` raw | `micromegas` wire (LZ4 + CBOR) | `otel` protobuf | `otel_gzip` |
|---|---|---|---|---|
| `log_static_attrs` | 25.0 | 5.4 | 117.0 | 3.7 |
| `log_dyn_high_card` | 95.9 | 27.1 | 100.9 | 4.9 |
| `span_static` | 34.0 | 8.5 | 66.0 | 34.5 |
| `span_dyn_name` | 76.9 | 24.1 | 68.9 | 37.0 |

The Micromegas wire format is smaller than uncompressed OTLP. Gzipped OTLP is smaller than Micromegas for logs and larger for spans.

**`metric`: median ns per metric sample, off the calling thread**

| case | `micromegas` | `otel` | `otel_gzip` |
|---|---|---|---|
| 10 series | 1309.7 | 641.7 | 1921.1 |
| 100 series | 1221.6 | 692.5 | 983.5 |

OpenTelemetry wins metric export cost: its collect, conversion and encode is cheaper per sample than Micromegas's share of `encode_bin`, and gzip erases that lead only at 10 series.

**`metric`: bytes per metric sample**

| case | `micromegas` raw | `micromegas` wire (LZ4 + CBOR) | `otel` protobuf | `otel_gzip` |
|---|---|---|---|---|
| 10 series | 33.2 | 8.3 | 90.4 | 25.5 |
| 100 series | 34.7 | 8.1 | 76.5 | 5.8 |

Micromegas is smaller than uncompressed OTLP. Gzipped OTLP is larger at 10 series and smaller at 100, where OpenTelemetry wins on size.

## Reading the numbers

- **Logs and spans.** On the calling thread Micromegas is faster in every log and span case. The gap is a design difference: Micromegas writes a fixed-size event into a thread-local or process-wide buffer and does the rest elsewhere, while the OpenTelemetry SDK builds an owned record, attributes included, and hands it to a bounded queue.
- **Metrics.** A metric sample costs the calling thread about 2.8 times less with Micromegas. Every sample counted here is kept by both libraries (see [Metric samples](#metric-samples)). In real use an OpenTelemetry exporter collects once per interval and keeps one point per series per interval, so its collect, conversion and encode cost is paid once per interval. That suits rates and distributions. Micromegas keeps every sample with its time, so each snapshot stays queryable.
- **Off the calling thread.** OpenTelemetry encodes faster per event; Micromegas trades that worker time for smaller wire sizes against uncompressed OTLP.
- **Drops.** Pacing keeps both libraries from dropping data, so no number includes drop behavior or says what happens when a producer outruns its exporter for long.
- **Bytes.** The benchmark's records are repetitive and repeated attribute values compress well in both formats, so wire sizes for real data are likely larger.

## Methodology

### What is measured

Calling-thread time for one operation, with criterion (`iter_custom`, only the emit calls are timed, see Pacing): `log` and `span` cases are one macro or API call, `metric_batch` cases are one batch of N calls. Both libraries record string attribute values (`%i` or `i.to_string()`), since Micromegas property values are strings, so the comparison is about the same information, not the cheapest encoding each library offers. Both filter at INFO. Every cell is a median from a single full run (not `--quick`) of [`rust/otel-comparison-bench`](https://github.com/madesroches/micromegas/tree/main/rust/otel-comparison-bench), the metric tables included. All cases are single-threaded: heavy contention on telemetry is not a realistic workload.

### Library configurations

**Micromegas.** `micromegas-tracing` with the `telemetry-sink` buffer defaults (10 MiB for logs and thread spans, 1 MiB for metrics) and CPU tracing enabled. The sink accepts every log, hands each full block to a worker thread that runs `StreamBlock::encode_bin` and drops the bytes (the HTTP sink without the network), and applies the HTTP sink's byte caps to that queue: 128 MiB for thread blocks and 256 MiB for log and metric blocks. `micromegas_interop` uses the `tracing` crate's macros, captured by the interop layer installed with `install_tracing_interop`.

**OpenTelemetry.** `opentelemetry` 0.32.0 and `opentelemetry_sdk` 0.32.1 with the SDK defaults and no tuning of queue sizes: `BatchLogProcessor` and `BatchSpanProcessor` (2048-record queue, 512-record batches, drop on full), `ParentBased(AlwaysOn)` sampling with random ids, and, for the metric batches, a `ManualReader` with cumulative temporality (the SDK default). Logs enter through `opentelemetry-appender-tracing` 0.32.0 and `tracing` events; spans through the tracer directly (`otel`) or through `tracing-opentelemetry` 0.33.0 (`otel_tracing`); metrics through a `Gauge<u64>` created once per batch size. The log and span exporters convert each batch to OTLP protobuf with `opentelemetry-proto` 0.32.0, as a network exporter would, and discard it.

### Metric samples

A metric batch is one snapshot: one value for each of N series at the same moment, like a game recording its frame metrics once per frame. N is 10 or 100. Each series is a static set of attributes, prebuilt once: three fixed pairs (`x`, `y`, `z`) and a series identifier, all under one metric name. The value changes from batch to batch (a persistent counter). A sample is one series' value in one batch, and every per-sample figure is the per-batch figure divided by N.

An OpenTelemetry counter or gauge reduces repeated records of the same series to one point per collect, so a raw `record` call is not a kept sample. Measuring in batches, with one collect per batch, makes every recorded value a kept sample in both libraries: Micromegas stores it and OpenTelemetry exports it as one point.

- **Micromegas.** A batch is N `imetric!` calls, one per series, each with its prebuilt `PropertySet` (`..set`). Many batches fill one `MetricsBlock`, which the worker thread encodes with `encode_bin`. The off-thread figure is `encode_bin` on one `MetricsBlock` holding 10 000 metric events, that is B = 1000 batches at 10 series and B = 100 batches at 100 series (the bench asserts the events fit one block), divided by B, then by N. Bytes are the block's size divided by B and by N; raw is the in-memory size of the events and their dependencies, wire is the `encode_bin` output.
- **OpenTelemetry.** A batch is N `Gauge<u64>::record` calls, each with the series' prebuilt `&'static [KeyValue]`, followed by one `ManualReader::collect` (a public trait method that the SDK exposes behind its `experimental_metrics_custom_reader` feature, enabled in the bench crate so it can keep a handle to the reader). The collected `ResourceMetrics` is converted to an `ExportMetricsServiceRequest` with `opentelemetry-proto` and encoded with `prost`. A real exporter does this once per interval; here the interval is one batch. Each batch size has its own meter provider, so a collect holds exactly N series. The off-thread figure is one collect, conversion and encode with the batch's records made before the timer starts, divided by N; `otel_gzip` adds gzip. Bytes are one collected batch divided by N.

### Encode benchmarks

Each `encode` sample is one block or batch of 10 000 events captured from the cases named in the table, so the cost and bytes are per event. Both libraries encode data of identical content. Micromegas extracts the block's dependencies, compresses dependencies and objects with LZ4 and encodes the block as CBOR, which is why its cost is highest for the cases with dynamic content (`log_dyn_high_card`, `span_dyn_name`): the objects refer to dependency records by id. The raw bytes column is the in-memory size of the events and, for Micromegas, the extracted dependencies; the wire column is what is sent.

### Two processes

The two libraries run in separate bench binaries because both want a process-global `tracing` subscriber and because OpenTelemetry's batch and reader threads would otherwise run during the Micromegas cases. The group and case ids are the same in both, so the results pair up.

### Pacing and the drop guard

Both libraries run on their defaults, and the producer is paced instead of the queues being tuned. Each criterion sample is split into chunks of at most 2048 operations (the OpenTelemetry default queue size); only the emit calls of a chunk are timed. For `metric_batch` the unit of a chunk is one batch. Between chunks, outside the timed region, the bench waits until the library's background queue is empty: `force_flush` on the OpenTelemetry providers, and for Micromegas until its worker has encoded every block that reached it (the calling thread's partially filled block stays where it is).

Neither library ever drops a record, and the bench fails if either does: after each case it compares the number of records the exporter or worker saw with the number emitted (OpenTelemetry does not expose its dropped-record counter, so the exporter's own count is the reference) and panics on any difference. The numbers are therefore the steady state where the exporter keeps up, which is normal operation.

### Noise

Every number on this page comes from one full run on one machine, so no run-to-run spread is reported. The machine was a laptop under WSL2 with no control of the CPU governor and ordinary background processes. Bench threads are not pinned, so samples can land on performance or efficiency cores, a likely source of run-to-run spread. Treat differences of 10% as noise.

### Hardware and software

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

### Reproduce

From the `rust/` directory of the repository:

```bash
cargo bench -p micromegas-otel-comparison-bench | tee bench.log
python3 otel-comparison-bench/report.py bench.log
```

`--quick` gives a faster, less precise run. `report.py` reads criterion's estimates from `target/criterion` and the saved output, and prints the tables on this page. A later OpenTelemetry release is a re-run: bump the versions in `rust/Cargo.toml` and run the same commands.

### Semantic differences

- Micromegas metrics are samples, one per call. OpenTelemetry instruments aggregate in the process and export one point per series per interval, with a cardinality limit per instrument (2000 attribute sets by default). That is why metrics are measured per batch and reported per sample.
- Micromegas spans have no attributes. A name built at run time (`span_scope_named_dyn!`) is the closest equivalent, and the `span/dyn_name` case compares it with an OpenTelemetry span of the same name.
- OpenTelemetry drops records when its queue is full and Micromegas drops blocks when its queue passes its byte caps. Neither reports the drop on the calling thread.
- Attribute values are strings in both libraries in these cases. OpenTelemetry supports typed values, which would change its encoded size.
- The cases record to an in-process sink. The network, the ingestion service and storage are not part of any number on this page; see [Cost Effectiveness](saas-vendors.md) for the storage side.
