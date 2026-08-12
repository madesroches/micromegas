<p align="center">
  <a href="https://micromegas.info/"><img src="branding/micromegas-primary-light.svg" alt="Micromegas Logo" width="400"/></a><br/>
  <strong>A unified observability platform for logs, metrics, and traces, built for high-volume environments.</strong>
</p>

<p align="center">
  <a href="https://crates.io/crates/micromegas"><img src="https://img.shields.io/crates/v/micromegas.svg" alt="Crates.io"></a>
  <a href="https://github.com/madesroches/micromegas/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-Apache%20v2-blue.svg" alt="Apache licensed"></a>
  <a href="https://github.com/madesroches/micromegas/actions?query=branch%3Amain"><img src="https://github.com/madesroches/micromegas/actions/workflows/rust.yml/badge.svg" alt="Build Status"></a>
</p>

<p align="center">
  <a href="https://micromegas.info/">Website</a> •
  <a href="https://micromegas.info/docs/">Documentation</a> •
  <a href="https://micromegas.info/rustdoc/micromegas/">Rust API Docs</a> •
  <a href="https://micromegas.info/docs/grafana/">Grafana Plugin</a> •
  <a href="#presentations">Presentations</a>
</p>

---

Micromegas is an observability system designed to provide unified insights into complex applications. It allows you to collect and analyze logs, metrics, and traces in a single, scalable database. Our goal is to help you spend less time reproducing bugs and more time understanding and improving your software's quality and performance.

## Objectives

*   Empower developers with comprehensive insights, eliminating time-consuming bug reproduction.
*   Quantify issue frequency and severity to allow better priority management.
*   Provide detailed traces based on high-frequency telemetry to enable a deep understanding of every issue.

## Key Features

*   **🚀 Unified Observability:** Store and query logs, metrics, and traces together to get a complete picture of your application's behavior.
*   **🔌 OpenTelemetry Compatible:** Native OTLP/HTTP ingestion alongside the high-performance native protocol — point any OTel SDK at the ingestion service and have logs, metrics, and traces land in the lakehouse.
*   **⚡ Low-Overhead Instrumentation:** Client-side instrumentation adds minimal overhead, averaging just **20 ns per event** in the calling thread.
*   **🌊 High-Frequency Data Collection:** Built to handle up to **100,000 events per second** from a single instrumented process.
*   **☁️ Scalable & Cloud-Native:** The backend is designed to scale horizontally, capable of ingesting data from millions of concurrent processes using object storage (S3) and PostgreSQL.
*   **💰 Cost-Efficient by Design:** Keep costs low with tail sampling and on-demand ETL. Raw data is stored cheaply and only processed when you need to query it.
*   **🔍 Powerful SQL Interface:** Query your data using a powerful and familiar SQL interface, powered by [Apache DataFusion](https://datafusion.apache.org/) and accessible via [Apache Arrow FlightSQL](https://arrow.apache.org/blog/2022/02/16/introducing-arrow-flight-sql/).
*   **📓 Interactive Notebooks:** A built-in web app for exploring data through composable notebook cells — queries, charts, flame graphs, maps, and logs — over the same SQL engine.
*   **🔐 Enterprise Authentication:** Secure your data with OIDC authentication supporting both human users (browser-based login) and service accounts (OAuth 2.0 client credentials).

## How It Works

Micromegas consists of several key components:

1.  **Instrumentation Libraries:** Lightweight libraries that send telemetry from your applications — native SDKs for Rust and Unreal Engine, a C ABI (`micromegas-capi`) for C/C++ and other FFI-capable languages, and a Blender add-on. See [Optimism](https://github.com/madesroches/optimism) for an example Bevy project using Micromegas.
2.  **Ingestion Service (`telemetry-ingestion-srv`):** A scalable service that receives telemetry (native transit/CBOR and OTLP/HTTP) and writes it to blob storage.
3.  **Analytics Service (`flight-sql-srv`):** A DataFusion-powered service that exposes a FlightSQL endpoint for running queries against your data.
4.  **Analytics Web App (`analytics-web-srv`):** A browser UI for exploring data through interactive notebooks.
5.  **Maintenance Daemon (`telemetry-maintenance-srv`):** Runs the on-demand and continuous ETL that materializes raw blocks into Parquet views, so data is only processed when it's worth querying.
6.  **PostgreSQL Database:** Stores metadata about processes, streams, and data blocks, keeping the object storage indexable and fast to query.
7.  **Object Storage (S3/GCS):** Stores all raw telemetry payloads and materialized query results in Parquet format.

These roles can run as independent, horizontally-scalable services or bundled into a single `micromegas-monolith` process for local development and single-machine deployments. An optional shared read cache (`micromegas-object-cache-srv`) can front the object store to cut egress cost and read latency across services.

## Cost-Effectiveness

Unlike traditional observability platforms with opaque and often escalating costs, Micromegas offers a transparent and **orders of magnitude more efficient** solution. With Micromegas, you can afford to record billions of events without relying heavily on sampling, gaining a complete and accurate picture of your systems. By leveraging your own cloud infrastructure, Micromegas drastically reduces your observability spend, especially at scale.

Discover how Micromegas achieves this unparalleled cost efficiency and compare it with traditional solutions in our detailed [Cost Effectiveness](https://micromegas.info/docs/cost-effectiveness/) document.

## Presentations

Learn more about Micromegas through our technical presentations:

- **[An Introduction to Micromegas](https://micromegas.info/intro-micromegas/)** (April 2026) - 30-minute overview: the unified pipeline, low-overhead instrumentation, and SQL on Arrow
- **[Interactive Notebooks for Observability](https://micromegas.info/notebooks/)** (February 2026) - Composable notebook cells with an in-browser query engine
- **[Unified Observability for Games](https://micromegas.info/unified-observability-for-games/)** (January 2026) - Why a unified architecture is easier to use and more powerful
- **[High-Frequency Observability: Cost-Efficient Telemetry at Scale](https://micromegas.info/high-frequency-observability/)** (October 2025) - How to record more data for less money with tail sampling and lakehouse architecture
- **[Design Presentation](https://micromegas.info/doc/design-presentation/design.html)** (February 2025) - Architecture and design principles

## Getting Started

Run Micromegas locally with Docker in a couple of commands — see the [Getting Started](https://micromegas.info/docs/getting-started/) guide.

Building from source or contributing code? See the [Build Guide](https://micromegas.info/docs/development/build/).

## Recent Releases

### v0.29.0 (August 2026)
* DB-backed API key store: `ingestion_api_keys`/`analytics_api_keys` tables holding only a SHA-256 hash plus a full mint/revoke audit trail, with mint/list/revoke/import HTTP routes hosted entirely on `analytics-web-srv`
* FlightSQL query audit hardening: failures now classify into distinct gRPC status codes instead of always `Internal`, and the audit log gains per-query peak memory/spill attribution, client attribution headers, and originating notebook/cell
* `parse_block(block_id)` now decodes OTLP blocks (logs/metrics/traces), not just `micromegas-transit` ones
* `thread_spans`/`net_spans` JIT partitions now cut in event-time order instead of registration order, fixing fragmented call trees for out-of-order block arrival
* Ingestion: block payload object writes are now create-only, closing an OTLP-redelivery regression; `BlockPayload` dependencies/objects encode as CBOR byte strings, cutting stored payload size ~40-45% for new blocks
* Python client: removed the deprecated `MICROMEGAS_PYTHON_MODULE_WRAPPER` escape hatch; AWS-CLI-style named connection profiles; `--version` on all console scripts
* Web app: Pie Chart notebook cell; Arrow `Utf8View`/`BinaryView` decode fix; chart axis fixes; tab favicon execution-state indicator
* `undici`, `cryptography`, `event-listener`, `js-yaml`, `nanoid` security bumps; `analytics-web-app` migrated ESLint 8→10 and Tailwind 3→4

### v0.28.0 (August 2026)
* Audience-based Access Control: the five mutating lakehouse SQL functions (`retire_partitions`, `materialize_partitions`, `regenerate_partitions`, `retire_partition_by_file`, `retire_partition_by_metadata`) are now gated on the caller's admin status via a new gRPC header, matched against `MICROMEGAS_ADMINS`/`MICROMEGAS_ANALYTICS_ADMINS`
* CloudWatch Firehose ingestion: new endpoints let CloudWatch Metric Streams and CloudWatch Logs reach micromegas via Kinesis Data Firehose HTTP Endpoint Delivery with no Lambda or collector in between, partitioned per CloudWatch namespace
* Order-preserving k-way merge for `SqlBatchView`s and `blocks_view` via a new per-file scan mode, collapsing sorted partitions in one pass instead of buffering a full sort
* Python client: raised the minimum supported Python version to 3.11, enabling native RFC 3339 `Z`-suffixed timestamp parsing across the FlightSQL client and `micromegas-query` CLI
* Screens folders: folder organization for saved screens (list/create/rename/move/delete), sidebar tree, and a Save-dialog folder picker
* Observability: per-query FlightSQL audit log, a `pg_stat_*` self-observability collector on the maintenance daemon, process RSS/jemalloc gauges on every service, and per-view failure isolation in the materialization pass
* Object-cache hardening: client-side circuit breaker, a two-step read replacing a foyer race, disk→RAM promotion metrics, and graceful-shutdown drain fixes
* DataFusion 54.1; Rust toolchain 1.97.1; Grafana plugin SDK 12.4.6; `analytics-web-app` migrated from Jest to Vitest; assorted Dependabot fixes

### v0.27.0 (July 2026)
* Tiered object read cache: new `micromegas-object-cache` engine powering a standalone range-aware S3 read cache service (`micromegas-object-cache-srv`, Foyer RAM+disk) and an in-process L1 cache; single-flight coalescing, priority budgeting, memory-bounded prefetch, NDJSON-streamed `/prefetch`, streamed range responses, write-time cache warming, and extensive performance telemetry
* Removed the Postgres `partition_metadata` table (schema v6): partition Parquet metadata is now read solely from the Parquet footer via the object-cache-backed reader, eliminating TOAST and write-path overhead
* Blender observability add-on: new `micromegas-capi` C-ABI crate (`cdylib`/`staticlib`) and a Blender 4.2+ Python extension (action capture, performance metrics, crash harvester, exception capture), with `capi-release` and `blender-extension` CI workflows
* Hardening: resilient Rust telemetry sink transport (priority queues, retry tuning, in-flight gating) and transit block parsing hardened against malformed payloads
* Supply-chain CI gates: `cargo audit` and `cargo deny` run on every Rust CI build
* `telemetry-admin` maintenance daemon renamed to `telemetry-maintenance-srv`
* DataFusion 54.0; internal proc-macros migrated `syn` 1→2; `opentelemetry-proto` 0.32 (GHSA-w9wp-h8wv-79jx); Dependabot fixes

### v0.26.0 (June 2026)
* `micromegas-monolith`: single-process deployment running all roles (`ingestion`, `analytics`, `web`, `admin`) in one binary — simplifies self-hosted and single-machine deployments
* Image streams: instrumented apps can send screenshots as telemetry via `send_image()`; queryable via the `images` SQL table; Unreal `telemetry.screenshot` console command with `telemetry.images.enable` CVar
* `#[micromegas_main]` extended with optional arguments (`ctrlc_handling`, `local_sink_enabled`, `local_sink_max_level`, `install_log_capture`, `system_metrics`, `telemetry_url`, `api_key`) for inline `TelemetryGuardBuilder` configuration
* Resilient Unreal telemetry sink: `FHttpRetrySystem` with exponential backoff, four priority queues (Metadata/Logs/Metrics/Traces), idle-aware spike sampling, `TimeSinceLastInput` metric
* ARM64 cross-compilation support in all production Dockerfiles; `build_docker_images.py --arm64` flag
* Deep `/ready` readiness probe for all services (PostgreSQL pool + blob storage verification, 503 on unhealthy)
* Graceful SIGTERM shutdown for all services with configurable drain period
* jemalloc global allocator for all production service binaries
* Image notebook cell: carousel viewer for images stored in the `images` view
* Chart threshold indicators: reference lines, per-row colors, series color assignment
* Map cell: orthographic camera mode, camera-relative keyboard controls, hover tooltip preview for markers
* Swimlane cell: optional color and label columns
* Log cell: resizable columns, one-click copy icon
* OTLP/JSON content-type support (`application/json`) on all three OTLP/HTTP routes
* `make_histogram` accepts runtime scalar bounds (CTEs, subqueries, CROSS JOIN columns)
* `format_value(value, unit)` template function for adaptive unit formatting in detail templates
* Batched expiry pipeline: bounded memory and transaction sizes for partition retirement, block deletion, and temporary-file cleanup; `DELETE…RETURNING` for atomic operation
* DataFusion 53.1; react-router 6.30.4 (CVE), esbuild 0.28.1, dompurify 3.4.11, undici ≥6.27.0 security updates

For the full history, see [CHANGELOG.md](./CHANGELOG.md).

## Contributing

We welcome contributions from the community! If you're interested in helping improve Micromegas, please see our [Contribution Guidelines](https://micromegas.info/docs/contributing/) for more details on how to get involved.

Whether it's bug reports, feature requests, or code contributions, your input is valuable.

## License

Micromegas is licensed under the [Apache License, Version 2.0](./LICENSE).
