# Instrumentation Overhead vs. OpenTelemetry

This page compares what it costs a Rust application to record the same logs, metrics and spans with `micromegas-tracing` and with the OpenTelemetry Rust SDK, each in its recommended production configuration with the network export replaced by an in-process sink that still encodes the payload. All numbers come from one full criterion run (not `--quick`) of the benchmark crate, except the `metric` table, which comes from a later full run of the `metric` group alone on the same machine, [`rust/otel-comparison-bench`](https://github.com/madesroches/micromegas/tree/main/rust/otel-comparison-bench), on the hardware listed in [Methodology](#methodology). They are not a promise for your hardware.

**Summary.**

- **Calling-thread cost.** For logs and spans, the calling thread pays a median of 45 ns per static log call and 28 ns per span (one `span/static` operation, which records a begin and an end event) with Micromegas, against 342 ns and 152 ns with the OpenTelemetry SDK, single-threaded. Micromegas copies a fixed-size event into a thread-local buffer for spans and into a shared process-wide buffer for logs; the SDK builds a record and pushes it into a bounded queue. With the bridges for the `tracing` crate the same operations cost more: 796 ns per span through `tracing-opentelemetry`, and 102 ns against 45 ns per static log for a `tracing` event routed into Micromegas.
- **Metrics are a different trade.** OpenTelemetry aggregates in the process, so a plain counter increment costs 7 ns, less than Micromegas's 24 ns, which records every sample. With attributes the two are within about a factor of two in either direction (the `set` case is 2.2 times in Micromegas's favor), and the difference between them is that Micromegas keeps every sample while OpenTelemetry keeps one aggregated point per series per export interval: at its defaults, 99.997% to 100% of the samples recorded in the metric cases never reach the exporter as an individual point.
- **Pacing.** The producer is paced so that neither library drops a record: only the emit calls are timed, in chunks of at most 2048 operations, with an untimed wait for the library's background queue to empty between chunks. The numbers are the steady state where the exporter keeps up.
- **Encoding and bytes.** Off the calling thread, the OpenTelemetry protobuf conversion is cheaper per event than Micromegas's dependency extraction, LZ4 and CBOR encoding in this run; adding gzip to it makes it costlier than Micromegas for the static cases (`log_static_attrs`, `span_static`), about equal for `span_dyn_name` (2839 against 3104 ns), and still far cheaper for `log_dyn_high_card` (806 against 3905 ns). On the wire, Micromegas is smaller than uncompressed OTLP protobuf, and gzipped OTLP is smaller than Micromegas for log records and larger for spans.

## Results

Every cell is a median from one full run, except the `metric` table (see the introduction). A dash marks a combination that does not exist: Micromegas spans carry no attributes, and `micromegas_interop` (`tracing` crate macros captured by Micromegas through `install_tracing_interop`) only has the cases that `tracing` can express.

### Hot path

**`log`: median ns per operation**

| case | `micromegas` | `micromegas_interop` | `otel` |
|---|---|---|---|
| `static` | 44.8 | 102.0 | 341.5 |
| `formatted` | 80.6 | 143.8 | 323.7 |
| `static_attrs` | 42.9 | — | 672.6 |
| `dyn_low_card` | 71.4 | — | 400.5 |
| `dyn_high_card` | 144.1 | 297.0 | 424.1 |

**`metric`: median ns per operation**

| case | `micromegas` | `otel_counter` | `otel_gauge` | `otel_counter` dropped samples % | `otel_gauge` dropped samples % |
|---|---|---|---|---|---|
| `plain` | 24.4 | 7.0 | 3.9 | 100.000% | 100.000% |
| `static_attrs` | 25.0 | 38.8 | 32.9 | 100.000% | 100.000% |
| `set` | 25.4 | 56.3 | 54.7 | 100.000% | 100.000% |
| `dyn_low_card` | 53.0 | 35.0 | 31.2 | 100.000% | 100.000% |
| `dyn_high_card` | 120.9 | 142.5 | 136.3 | 99.997% | 99.997% |

Dropped samples % is the share of recorded samples that never reach the exporter as an individual point, because a counter sums them and a gauge keeps only the last value per series per export. It is measured, not computed: each case has its own instrument, and the bench counts the data points the exporter receives under that instrument's name across every export, including a forced flush after the case. Micromegas has no such column; it keeps every sample. The low-cardinality cases export 1 to 4 points for 128 to 156 million samples; `dyn_high_card` exports 2001 points (2000 series plus the overflow series) for about 62 million.

**`span`: median ns per operation**

| case | `micromegas` | `otel` | `otel_tracing` |
|---|---|---|---|
| `static` | 27.9 | 151.7 | 795.6 |
| `attrs` | — | 227.3 | 911.7 |
| `dyn_name` | 96.1 | 196.6 | 919.4 |

### Contention

The same operation on 1, 4 and 8 threads that start together and use disjoint ranges of ids, paced in chunks of 2048 operations in total (each thread emits `2048 / threads` per chunk). Micromegas logs and metrics share one process-wide stream per kind, so threads contend on it; each thread has its own span stream. OpenTelemetry's shared points are its batch queue and its metric aggregation map.

**`contention`: aggregate Mops/s**

| case | lib | 1 thread | 4 threads | 8 threads |
|---|---|---|---|---|
| `log/static` | `micromegas` | 22.9 | 8.8 | 10.2 |
| `log/static` | `otel` | 2.8 | 10.0 | 14.6 |
| `log/dyn_high_card` | `micromegas` | 6.4 | 1.7 | 1.6 |
| `log/dyn_high_card` | `otel` | 1.9 | 9.5 | 13.8 |
| `metric/static_attrs` | `micromegas` | 39.4 | 18.1 | 25.0 |
| `metric/static_attrs` | `otel_counter` | 26.1 | 18.6 | 29.5 |
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

**Micromegas metric bytes per event**

| case | raw | wire (LZ4 + CBOR) |
|---|---|---|
| `metric_static_attrs` | 33.1 | 5.3 |

Metrics are not in the encoding comparison: OpenTelemetry exports one aggregated point per series per interval, so bytes per event has no counterpart there. Micromegas's own figure is above.

## What the numbers do and do not say

- **Logs and spans.** The gap in calling-thread cost is the design difference, not a tuning difference: Micromegas writes a fixed-size event into a thread-local or process-wide buffer and does the rest elsewhere, while the OpenTelemetry SDK builds an owned record (attributes included) and hands it to a bounded queue.
- **Metrics.** OpenTelemetry's aggregation makes the hot path cheap and makes the output small, and it is the right choice when you want rates and distributions per series. The cost is that individual samples are gone: in the metric cases here, 99.997% to 100% of the recorded samples never reach the exporter as an individual point. It limits each instrument to 2000 attribute sets by default, after which values go to an overflow series; the `dyn_high_card` metric cases exceed it. Micromegas records each sample with its time and properties, so high-cardinality ids remain queryable, at a calling-thread cost that does not depend on cardinality.
- **Contention.** Micromegas log and metric throughput falls as threads are added in this run (`log/static` from 22.9 Mops/s on 1 thread to 8.8 on 4 and 10.2 on 8; `log/dyn_high_card` from 6.4 to 1.6), because the shared stream is a shared point. OpenTelemetry's aggregate log throughput rises with threads (`log/static` from 2.8 to 14.6 Mops/s), so it overtakes Micromegas for logs at 4 and 8 threads in this run, while Micromegas stays ahead on a single thread; the counter is about even with Micromegas at 4 and 8 threads (18.6 against 18.1, 29.5 against 25.0 Mops/s). Micromegas span throughput rises with threads (33.7 to 217.4 Mops/s) because each thread writes its own stream, and OpenTelemetry's span throughput rises more slowly (7.0 to 14.0). The paced producer emits only 2048 operations in total between drains, so these figures describe bursts the background threads can absorb, not sustained overload.
- **Encoding.** The encoding comparison runs on captured data of identical content. Micromegas spends more time per event in this run, most of it for the cases with dynamic content (`log_dyn_high_card`, `span_dyn_name`), where it first extracts the dependency records that the objects refer to by id. Whether that off-thread cost matters depends on whether a core is free for it.
- **Bytes.** The raw column is the in-memory size of the events and, for Micromegas, the extracted dependencies; the wire column is what is sent. Repeated attribute values compress well in both formats, and the benchmark's records are repetitive, so wire sizes for real data are likely larger.

## Methodology

**What is measured.** Calling-thread time for one operation, with criterion (`iter_custom`, only the emit calls are timed, see Pacing below): `log` and `span` cases are one macro or API call, `metric` cases are one sample. Both libraries record string attribute values (`%i` or `i.to_string()`), since Micromegas property values are strings; the comparison is about the same information, not the cheapest encoding each library offers. Both filter at INFO.

**Micromegas.** `micromegas-tracing` with the `telemetry-sink` buffer defaults (10 MiB for logs and thread spans, 1 MiB for metrics) and CPU tracing enabled. The sink accepts every log, hands each full block to a worker thread that runs `StreamBlock::encode_bin` and drops the bytes (the HTTP sink without the network), and applies the HTTP sink's byte caps to that queue: 128 MiB for thread blocks and 256 MiB for log and metric blocks. `micromegas_interop` uses the `tracing` crate's macros, captured by the interop layer installed with `install_tracing_interop`.

**OpenTelemetry.** `opentelemetry` 0.32.0 and `opentelemetry_sdk` 0.32.1 with the SDK defaults and no tuning of queue sizes: `BatchLogProcessor` and `BatchSpanProcessor` (2048-record queue, 512-record batches, drop on full), `ParentBased(AlwaysOn)` sampling with random ids, and a `PeriodicReader` with its default 60 s interval, cumulative temporality and the default cardinality limit. Logs enter through `opentelemetry-appender-tracing` 0.32.0 and `tracing` events; spans through the tracer directly (`otel`) or through `tracing-opentelemetry` 0.33.0 (`otel_tracing`); metrics through a `Counter<u64>` (`otel_counter`) and a `Gauge<u64>` (`otel_gauge`) created once per case. The log and span exporters convert each batch to OTLP protobuf with `opentelemetry-proto` 0.32.0, as a network exporter would, and discard it; the metric exporter only counts the data points it receives per instrument name, because the first periodic export is a minute away and the bench forces one after each case.

**Two processes.** The two libraries run in separate bench binaries because both want a process-global `tracing` subscriber and because OpenTelemetry's batch and reader threads would otherwise run during the Micromegas cases. The group and case ids are the same in both, so the results pair up.

**Pacing.** Both libraries run on their defaults, and the producer is paced instead of the queues being tuned. Each criterion sample is split into chunks of at most 2048 operations (the OpenTelemetry default queue size); only the emit calls of a chunk are timed. Between chunks, outside the timed region, the bench waits until the library's background queue is empty: `force_flush` on the OpenTelemetry providers, and for Micromegas until its worker has encoded every block that reached it (the calling thread's partially filled block stays where it is). In the contention cases each thread emits `2048 / threads` operations per chunk, all threads start a chunk together, and a chunk costs its slowest thread. Neither library ever drops a record, and the bench fails if either does: after each case it compares the number of records the exporter or worker saw with the number emitted (OpenTelemetry does not expose its dropped-record counter, so the exporter's own count is the reference) and panics on any difference. This measures the steady state where the exporter keeps up, which is normal operation; it says nothing about how either library behaves when the producer outruns its exporter for long.

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

- Micromegas metrics are samples, one per call. OpenTelemetry instruments aggregate in the process and export one point per series per interval, with a cardinality limit per instrument.
- Micromegas spans have no attributes. A name built at run time (`span_scope_named_dyn!`) is the closest equivalent, and the `span/dyn_name` case compares it with an OpenTelemetry span of the same name.
- OpenTelemetry drops records when its queue is full and Micromegas drops blocks when its queue passes its byte caps. Neither reports the drop on the calling thread. The benchmark's pacing keeps both from dropping, so no number here includes that behavior.
- Attribute values are strings in both libraries in these cases. OpenTelemetry supports typed values, which would change its encoded size.
- The cases record to an in-process sink. The network, the ingestion service and storage are not part of any number on this page; see [Cost Effectiveness](saas-vendors.md) for the storage side.
