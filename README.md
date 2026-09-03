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
*   **🛡️ Audience-Based Access Control:** Host private data on a shared deployment. Incoming telemetry is tagged server-side with the audience of the credential that sent it, and queries only return the data the user has been granted access to.

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

### v0.30.0 (September 2026)
* **Audience-based access control, end to end** — every materialized-view query plan carries an audience predicate, every id-addressed function is guarded, and ingestion stamps the audience from the authenticated credential instead of trusting the client
* **Grants and groups now live in Postgres** — a database-backed grant store, local groups with a reserved `admins` group, a Groups admin page, and a `micromegas-groups` CLI
* **Deterministic merged order** — every non-ordering-declaring view merges through one reader per source file group, restoring row-group pruning on time-filtered queries
* **Admin-managed query deny list** — replica-cached rules evaluated as DataFusion expressions before a query is planned
* **Ops and ergonomics** — a new `micromegas-redis-exporter` service, jemalloc tuning across all eight binaries, complete JSONB UDF coverage, and per-row bar charts for `Histogram` columns in the web app
* **Operator action required** — the `MICROMEGAS_ADMINS*` environment variables are now refused at startup, the IdP `groups` claim is no longer read, and the six global-instance views need partition regeneration

### v0.29.0 (August 2026)
* **API keys moved into the database** — only a SHA-256 hash is stored, with a full mint/revoke audit trail and HTTP routes hosted on `analytics-web-srv`
* **Query failures are diagnosable** — distinct gRPC status codes instead of a blanket `Internal`, plus peak memory, spill, client attribution, and originating notebook cell in the audit log
* **Smaller, more correct blocks** — CBOR payload encoding cuts stored size 40-45%, `parse_block` decodes OTLP as well as native blocks, and JIT span partitions cut in event-time order, fixing call trees fragmented by late arrivals
* **Python client** — AWS-CLI-style named connection profiles, `--version` on every console script, and the deprecated module-wrapper escape hatch removed
* **Web app** — Pie Chart cell, `Utf8View`/`BinaryView` decode fix, and an execution-state favicon

### v0.28.0 (August 2026)
* **Mutating lakehouse functions are admin-gated** — all five now check the caller's admin status before touching partitions
* **CloudWatch straight into the lakehouse** — Metric Streams and Logs arrive through Kinesis Firehose with no Lambda or collector in between
* **One-pass ordered merges** for `SqlBatchView`s and `blocks_view`, collapsing sorted partitions instead of buffering a full sort
* **Screens folders** — organize saved screens in a sidebar tree, with a folder picker in the Save dialog
* **More self-observability** — per-query FlightSQL audit log, a `pg_stat_*` collector, RSS/jemalloc gauges on every service, and per-view failure isolation during materialization
* Object-cache hardening (circuit breaker, race fix, shutdown drain); Python 3.11 minimum; DataFusion 54.1; Rust 1.97.1

### v0.27.0 (July 2026)
* **Tiered object read cache** — a range-aware S3 read cache service (`micromegas-object-cache-srv`, RAM + disk) and an in-process L1, with single-flight coalescing, bounded prefetch, streamed range responses, and write-time warming
* **The `partition_metadata` table is gone** — Parquet metadata is read straight from the footer through the cache, removing TOAST and write-path overhead
* **Blender add-on** — built on a new `micromegas-capi` C-ABI crate, capturing actions, performance metrics, crashes, and exceptions
* **Tougher clients** — resilient sink transport (priority queues, retry tuning, in-flight gating) and block parsing hardened against malformed payloads
* Supply-chain gates (`cargo audit`, `cargo deny`) on every CI build; `telemetry-admin` renamed to `telemetry-maintenance-srv`; DataFusion 54.0

Every item of every release, including breaking-change notes and dependency bumps, is in [CHANGELOG.md](./CHANGELOG.md).

## Contributing

We welcome contributions from the community! If you're interested in helping improve Micromegas, please see our [Contribution Guidelines](https://micromegas.info/docs/contributing/) for more details on how to get involved.

Whether it's bug reports, feature requests, or code contributions, your input is valuable.

## License

Micromegas is licensed under the [Apache License, Version 2.0](./LICENSE).
