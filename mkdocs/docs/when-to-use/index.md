# When to Use Micromegas

Micromegas is an open-source ([Apache-2.0](https://github.com/madesroches/micromegas/blob/main/LICENSE)), self-hosted observability stack that collects logs, metrics and traces from native and client processes, stores raw payloads in object storage with [PostgreSQL metadata][arch], and is queried with SQL over Apache Arrow FlightSQL.

**TL;DR.** Choose Micromegas for low instrumentation overhead and low cost, and for the high-frequency, full-resolution telemetry that efficiency makes affordable. Choose a peer when you want the established, widely adopted default, or when you do not want to operate the stack yourself. The [summary at the end](#summary-which-one-fits) maps workloads to tools.

*Last reviewed: October 2026*

Every claim on this page links to its source: the project's own docs or repository for the peers, and the Micromegas docs or source code for Micromegas. If something is wrong or out of date, please [open an issue](https://github.com/madesroches/micromegas/issues).

## Micromegas in brief

- **Instrumentation.** Native SDKs for Rust (the `micromegas-tracing` macros such as `span_scope!`, in [`macros.rs`](https://github.com/madesroches/micromegas/blob/main/rust/tracing/src/macros.rs), and `#[span_fn]`, in [`lib.rs`](https://github.com/madesroches/micromegas/blob/main/rust/tracing/proc-macros/src/lib.rs)) and an [Unreal Engine plugin](../unreal/index.md) record spans, logs and metrics. A [C ABI](../native/index.md) records logs and metrics and makes Micromegas easy to embed in any app or language that can load a shared library ([`lib.rs`](https://github.com/madesroches/micromegas/blob/main/rust/capi/src/lib.rs)); the [Blender add-on](../blender/index.md) is an example, calling it from Blender's embedded Python through `ctypes`. The `tracing`-crate interop captures existing `tracing` events as logs ([`tracing_interop.rs`](https://github.com/madesroches/micromegas/blob/main/rust/telemetry-sink/src/tracing_interop.rs)). Events are recorded in-process on the calling thread, the telemetry sink batches and ships them off the hot path, and the intent is instrumentation that stays on in production.
- **Ingestion.** Ingestion is HTTP: the native transit format and [OTLP/HTTP][otlp-wire] (protobuf or JSON, gzip) for logs, metrics and traces ([ingestion service][arch]). Queries are served over FlightSQL ([FlightSQL server](../admin/flight-sql.md)).
- **Analytics.** Raw payloads stay in object storage (S3, GCS or local) and metadata lives in PostgreSQL ([architecture][arch]). A lakehouse materializes views to Parquet and queries them with Apache DataFusion ([lakehouse architecture](../architecture/index.md#lakehouse-architecture), [query guide](../query-guide/index.md)). Logs and metrics are materialized continuously into global views by the maintenance daemon; spans and per-process views are materialized only when queried ([JIT ETL][jit], [on-demand processing][cost-ondemand]), so their processing cost follows what is queried.
- **Presentation.** The [analytics web app](../web-app/index.md) runs notebooks. Its server fetches query results over FlightSQL and streams them to the browser as Arrow IPC record batches ([`stream_query.rs`](https://github.com/madesroches/micromegas/blob/main/rust/analytics-web-srv/src/stream_query.rs), [`arrow-stream.ts`](https://github.com/madesroches/micromegas/blob/main/analytics-web-app/src/lib/arrow-stream.ts)). The browser keeps them in Arrow format in memory, where DataFusion, compiled to WASM, queries them locally, so later cells can query earlier cells' results without going back to the server ([execution model][exec]). A [Grafana data source plugin](../grafana/index.md) covers dashboards, and Grafana is also the path for alerting. A process's spans can be exported as a Perfetto trace ([`perfetto_trace_chunks`][perfetto]).
- **Access control.** Every row is stamped server-side with an audience taken from the ingestion credential, so a producer cannot forge it ([audience stamping](../admin/authorization.md#audience-stamping)). Read grants are separate and editable, and re-sharing applies immediately to already-ingested data, with no restamping ([authorization][authz]). All of it is in the Apache-2.0 build, with no paid tier.

**Limits**, stated plainly:

- PostgreSQL is required for metadata ([architecture][arch]).
- OTLP is HTTP-only; there is no OTLP/gRPC ([OTLP limitations][otlp-limits]).
- There is no built-in alert engine; alerting goes through Grafana ([Grafana plugin](../grafana/index.md)).
- Spans come from the Rust and Unreal SDKs; the C ABI records logs and metrics ([native SDK](../native/index.md), [Unreal plugin](../unreal/index.md)).
- There is no RUM or session replay.
- The community is smaller than the peers' communities.
- You operate it yourself: PostgreSQL, object storage, and the services (or the [single-process monolith](../admin/monolith.md)).

## At a glance

Each project's section below gives the sources for its row.

**Deployment and licensing**

| Project | License (OSS edition) | Storage | Also requires |
|---|---|---|---|
| [**Micromegas**](#micromegas-in-brief) | Apache-2.0, no paid tier | Raw payloads in object storage; Parquet views | PostgreSQL |
| [Parseable](#micromegas-vs-parseable) | AGPL-3.0; paid PromQL, HA | Parquet on object storage | Nothing |
| [OpenObserve](#micromegas-vs-openobserve) | AGPL-3.0; paid SSO, advanced RBAC | Parquet on object storage | Nothing on one node; PostgreSQL and NATS for HA |
| [GreptimeDB](#micromegas-vs-greptimedb) | Apache-2.0; paid alerting, RBAC | Parquet on object storage | Nothing standalone; etcd, PostgreSQL or MySQL when distributed |
| [SigNoz](#micromegas-vs-signoz) | MIT with proprietary `ee/`; AGPL collector | ClickHouse | ClickHouse Keeper or ZooKeeper; SQLite |
| [ClickHouse / ClickStack](#micromegas-vs-clickhouse-and-clickstack) | Apache-2.0; HyperDX MIT | Local disk, S3 as tiered disk | Keeper for replication; MongoDB for ClickStack |
| [Grafana LGTM](#micromegas-vs-grafana-lgtm-loki-tempo-mimir-pyroscope) | AGPL-3.0; paid GEL, GET, GEM | Object storage | Kafka for Mimir's Helm default and Tempo microservices |
| [InfluxDB 3 Core](#micromegas-vs-influxdb-3-core) | MIT or Apache-2.0; paid HA | Parquet on object storage | Nothing |
| [VictoriaMetrics](#micromegas-vs-victoriametrics-victorialogs-and-victoriatraces) | Apache-2.0; paid downsampling, mTLS | Local disk | Nothing |
| [Quickwit](#micromegas-vs-quickwit) | Apache-2.0 | Index splits on object storage | PostgreSQL when distributed |
| [Prometheus](#micromegas-vs-prometheus) (Thanos) | Apache-2.0 | Local TSDB; Thanos adds object storage | Alertmanager for alerts |

**Capabilities**

| Project | Signals | Query | Own in-process SDKs | UI / alerting |
|---|---|---|---|---|
| [**Micromegas**](#micromegas-in-brief) | Logs, metrics, traces | SQL | Rust, Unreal Engine, C ABI | Notebooks; alerts via Grafana |
| [Parseable](#micromegas-vs-parseable) | Logs, metrics, traces, events | SQL; PromQL paid | None (OTel); small Go SDK | Built-in; threshold alerts |
| [OpenObserve](#micromegas-vs-openobserve) | Logs, metrics, traces, RUM, session replay, profiles | SQL, PromQL, full-text | RUM SDKs; OTel for backends | Built-in; alerts |
| [GreptimeDB](#micromegas-vs-greptimedb) | Metrics, logs, traces | SQL, PromQL | Ingest clients only | Dashboard; alerting paid |
| [SigNoz](#micromegas-vs-signoz) | Logs, metrics, traces, exceptions | Query builder, PromQL, ClickHouse SQL | None (OTel) | Built-in APM; alerts |
| [ClickHouse / ClickStack](#micromegas-vs-clickhouse-and-clickstack) | Logs, metrics, traces | ClickHouse SQL; HyperDX search | None (OTel) | HyperDX; alerts |
| [Grafana LGTM](#micromegas-vs-grafana-lgtm-loki-tempo-mimir-pyroscope) | Logs, traces, metrics, profiles | LogQL, TraceQL, PromQL | None (OTel); Faro, Beyla, Pyroscope | Grafana; Grafana Alerting |
| [InfluxDB 3 Core](#micromegas-vs-influxdb-3-core) | Metrics, events; logs and traces via Telegraf | SQL, InfluxQL | Write clients only | Explorer; plugin alerts |
| [VictoriaMetrics](#micromegas-vs-victoriametrics-victorialogs-and-victoriatraces) | Metrics, logs, traces | MetricsQL, LogsQL | Go `metrics` package | vmui; vmalert |
| [Quickwit](#micromegas-vs-quickwit) | Logs, traces | Elasticsearch-compatible API | None (OTel) | Basic UI; Grafana, Jaeger |
| [Prometheus](#micromegas-vs-prometheus) (Thanos) | Metrics | PromQL | Metric client libraries | Expression browser; Alertmanager |

## Micromegas vs. Parseable

**Parseable** is a Rust "unified observability platform on a data lake architecture" for logs, metrics, traces and events ([repo](https://github.com/parseablehq/parseable)). It is AGPL-3.0; paid Cloud and Enterprise tiers gate PromQL, the HA cluster, APM, anomaly detection and AI features, while SQL, dashboards, threshold alerts, OIDC/SSO and RBAC are in the open-source edition ([pricing](https://www.parseable.com/pricing)).

**Choose Parseable when** you want one binary with no metadata database, with all signals stored as Parquet on object storage and queried with SQL, and broad ingestion compatibility: its own HTTP JSON API, OTLP over HTTP, Kafka, Fluent Bit, Vector, Logstash and Filebeat ([integrations](https://www.parseable.com/docs/integrations), [architecture](https://www.parseable.com/docs/architecture)).

**How Micromegas differs.** Micromegas and Parseable both run SQL on DataFusion over Parquet ([Parseable `Cargo.toml`](https://github.com/parseablehq/parseable/blob/main/Cargo.toml)), so that axis is shared. Parseable relies on OTel for instrumentation, with a small Go SDK; Micromegas ships [in-process SDKs](../native/index.md) for Rust and Unreal Engine. The ingestion paths differ as well. Parseable parses each request into JSON values, infers and merges a schema over every record and converts the batch to Arrow, staged on local disk and turned into Parquet each minute ([`json.rs`](https://github.com/parseablehq/parseable/blob/d3cc4110bbdb4d1cc32a5cd91d2cbbde957d8e45/src/event/format/json.rs#L64-L186), [architecture](https://www.parseable.com/docs/architecture)). Its open-source edition accepts OTLP as JSON only, so OTel exporters and collectors must be set to send JSON ([`ingest_utils.rs`](https://github.com/parseablehq/parseable/blob/d3cc4110bbdb4d1cc32a5cd91d2cbbde957d8e45/src/handlers/http/modal/utils/ingest_utils.rs#L158-L164), [OTLP logs](https://www.parseable.com/docs/OpenTelemetry/logs)). Micromegas's SDKs send batched binary blocks, and its ingestion service stores each block as received, with one object-storage write and one PostgreSQL row and no per-event parsing ([`web_ingestion_service.rs`](https://github.com/madesroches/micromegas/blob/main/rust/ingestion/src/web_ingestion_service.rs)). Events are parsed afterwards: logs and metrics continuously, spans and per-process views [only when queried][jit]. Micromegas stores every emission as its own row. In the open-source edition Parseable's distributed mode allows many ingest nodes but only one query node ([OSS Helm](https://www.parseable.com/docs/self-hosted/installation/distributed/k8s-helm-oss)). Micromegas needs PostgreSQL for metadata; Parseable does not.

## Micromegas vs. OpenObserve

**OpenObserve** is a Rust backend with a Vue UI covering logs, metrics, traces, RUM, session replay, profiles and LLM observability ([repo](https://github.com/openobserve/openobserve)). It is AGPL-3.0 in the open-source edition (it moved from Apache); the Enterprise edition is under a commercial license, free up to 50 GB/day ([license](https://openobserve.ai/docs/enterprise-setup/license-and-pricing/)) and gates SSO, advanced RBAC, audit logs, federation and AI features ([features](https://openobserve.ai/docs/enterprise-setup/enterprise-features/)). It has an LLM observability feature set for agent traces.

**Choose OpenObserve when** you need the broadest signal coverage (RUM and session replay included), a rich built-in UI with dashboards, pipelines, alerts and incidents, full-text search via Tantivy, or an easy migration off ELK through its Elasticsearch-compatible `_bulk` API ([ingestion](https://openobserve.ai/docs/user-guide/ingestion/), [metrics](https://openobserve.ai/docs/features/metrics/)).

**How Micromegas differs.** Both run SQL on DataFusion over Parquet on object storage ([OpenObserve `Cargo.toml`](https://github.com/openobserve/openobserve/blob/main/Cargo.toml)). OpenObserve uses OTel SDKs for backend code, plus its own RUM SDKs ([repo](https://github.com/openobserve/openobserve)); Micromegas has in-process [Rust and Unreal SDKs](../native/index.md). OpenObserve writes Parquet at ingestion, whereas Micromegas processes spans and per-process views only [when queried][jit]. OpenObserve keeps metadata in SQLite on one node, and PostgreSQL plus NATS in HA mode ([architecture](https://openobserve.ai/docs/architecture/)). Micromegas also offers one SQL surface rather than SQL plus PromQL, and notebooks running the same engine in the browser ([WASM][exec]). OpenObserve's Enterprise edition gates advanced RBAC ([features](https://openobserve.ai/docs/enterprise-setup/enterprise-features/)), whereas Micromegas's [per-row access control][authz] is in the open-source build. Where OpenObserve offers profiling, Micromegas's difference is spans named by the data being processed rather than sampled call stacks ([code vs. data](#complementary-tools)).

## Micromegas vs. GreptimeDB

**GreptimeDB** is a Rust "observability database": one columnar engine for metrics, logs and traces, with SQL joins across signals ([repo](https://github.com/GreptimeTeam/greptimedb)). The core is Apache-2.0 under an open-core model; Enterprise gates Triggers (alerting), LDAP, RBAC, audit logs and automatic rebalancing ([enterprise](https://docs.greptime.com/enterprise/overview/), [triggers](https://docs.greptime.com/reference/sql/trigger-syntax/)).

**Choose GreptimeDB when** you want a long-term replacement for Prometheus storage, want to migrate one signal at a time (it ingests OTLP, Prometheus remote write, Loki push, Elasticsearch `_bulk`, InfluxDB line protocol and the MySQL and PostgreSQL wire protocols), or have high-cardinality metrics. Standalone mode needs no dependencies.

**How Micromegas differs.** Both use SQL on DataFusion over Parquet in object storage ([config](https://docs.greptime.com/user-guide/deployments-administration/configuration/)). GreptimeDB ships ingester client libraries only, with no instrumentation SDKs; Micromegas has [in-process SDKs](../native/index.md). GreptimeDB writes Parquet at ingestion; Micromegas processes spans and per-process views [only when queried][jit]. Micromegas also offers notebooks running the same engine in the browser ([WASM][exec]). GreptimeDB's Enterprise edition gates RBAC ([enterprise](https://docs.greptime.com/enterprise/overview/)), whereas Micromegas's [per-row access control][authz] is in the open-source build. Distributed GreptimeDB needs etcd, PostgreSQL or MySQL for metasrv, plus optional Kafka for the WAL; Micromegas needs PostgreSQL.

## Micromegas vs. SigNoz

**SigNoz** is a Go and React OTel-native APM covering logs, metrics, traces, exceptions and LLM observability ([repo](https://github.com/SigNoz/signoz)). The code is MIT outside `ee/` and `cmd/enterprise/`, which are proprietary, and its collector is AGPL-3.0 ([LICENSE](https://github.com/SigNoz/signoz/blob/main/LICENSE), [collector](https://github.com/SigNoz/signoz-otel-collector)). Cloud and Enterprise gate anomaly detection, SAML, fine-grained RBAC and audit logs ([pricing](https://signoz.io/pricing/)).

**Choose SigNoz when** you want the best out-of-the-box APM experience for OTel-instrumented services, with built-in APM views, traces, logs, dashboards and alerts and no Grafana needed ([architecture](https://signoz.io/docs/architecture/)). It also has LLM observability features for agent workloads.

**How Micromegas differs.** SigNoz relies on OTel SDKs only; Micromegas adds [in-process SDKs](../native/index.md) for Rust and Unreal Engine. SigNoz stores data in ClickHouse, with ClickHouse Keeper or ZooKeeper and SQLite for dashboards, alerts and users ([moldings](https://github.com/SigNoz/foundry/blob/main/docs/concepts/moldings.md)); Micromegas stores Parquet on object storage with PostgreSQL metadata. Micromegas keeps raw payloads in object storage and processes spans and per-process views [only when queried][jit]. Every emission is stored as its own row. SigNoz offers a query builder, PromQL and ClickHouse SQL ([architecture](https://signoz.io/docs/architecture/)); Micromegas has one SQL surface, plus notebooks running the same engine in the browser ([WASM][exec]). SigNoz Cloud and Enterprise gate fine-grained RBAC ([pricing](https://signoz.io/pricing/)), whereas Micromegas's [per-row access control][authz] is in the open-source build.

## Micromegas vs. ClickHouse and ClickStack

**ClickHouse** is a C++ columnar OLAP database; its own docs say it "isn't an out-of-the-box solution for Observability" but is a highly efficient storage engine ([intro](https://clickhouse.com/docs/use-cases/observability/introduction)). **ClickStack** bundles ClickHouse, the HyperDX UI and an OTel collector distribution ([overview](https://clickhouse.com/docs/use-cases/observability/clickstack/overview)). ClickHouse is Apache-2.0 and HyperDX is MIT ([ClickHouse](https://github.com/ClickHouse/ClickHouse), [HyperDX](https://github.com/hyperdxio/hyperdx)).

**Choose ClickHouse when** you need raw query speed and compression at very large scale and are prepared to build your own pipeline, with the benefit of a mature ecosystem; ClickStack's HyperDX adds built-in search, traces, dashboards and alerts. ClickStack accepts OTLP over HTTP and gRPC through its collector.

**How Micromegas differs.** ClickHouse stores MergeTree data on local disk, with S3 as a tiered disk ([S3](https://clickhouse.com/docs/integrations/s3)); replication needs ClickHouse Keeper or ZooKeeper, and self-hosted ClickStack also needs MongoDB for dashboards, saved searches and alerts ([deployment](https://clickhouse.com/docs/use-cases/observability/clickstack/deployment/hyperdx-only)). Micromegas keeps raw payloads in object storage with PostgreSQL metadata, and [processes spans and per-process views only when queried][jit]. ClickStack's instrumentation is OTel-based; Micromegas adds [in-process SDKs](../native/index.md). Micromegas stores every emission as its own row, with one SQL surface and notebooks running the same engine in the browser ([WASM][exec]). ClickHouse's open-source build has [row policies](https://clickhouse.com/docs/sql-reference/statements/create/row-policy) for row filtering; the Micromegas difference is that each row's audience is stamped server-side from the ingestion credential rather than set by the producer ([audience stamping](../admin/authorization.md#audience-stamping)).

## Micromegas vs. Grafana LGTM (Loki, Tempo, Mimir, Pyroscope)

**Grafana LGTM** is one backend per signal viewed in Grafana: Loki indexes labels, not log contents; Tempo stores traces; Mimir is long-term Prometheus storage; Pyroscope adds continuous profiling. It is AGPL-3.0, with Apache-2.0 exceptions in `LICENSING.md`; paid GEL, GET and GEM add tenant management, token auth and cross-tenant query ([GEL](https://grafana.com/docs/enterprise-logs/latest/), [GET](https://grafana.com/docs/enterprise-traces/latest/), [GEM](https://grafana.com/docs/enterprise-metrics/latest/)).

**Choose Grafana LGTM when** you want the de-facto self-hosted standard: object-storage backends ([Loki storage](https://grafana.com/docs/loki/latest/configure/storage/)), purpose-built query languages including PromQL ([LogQL](https://grafana.com/docs/loki/latest/query/), [TraceQL](https://grafana.com/docs/tempo/latest/traceql/)), a mature UI and alerting (Grafana Alerting, plus the Mimir [ruler](https://grafana.com/docs/mimir/latest/references/architecture/components/ruler/) and [Alertmanager](https://grafana.com/docs/mimir/latest/references/architecture/components/alertmanager/)), and an OTel-first approach via [Alloy](https://github.com/grafana/alloy). Each backend runs as one binary with `-target=all` or as microservices ([Loki](https://grafana.com/docs/loki/latest/get-started/deployment-modes/), [Mimir](https://grafana.com/docs/mimir/latest/references/architecture/deployment-modes/)).

**How Micromegas differs.** Micromegas is one system where LGTM is several. LGTM runs one backend per signal, each deployed, scaled and configured with its own storage, and each queried in its own language (LogQL, TraceQL, PromQL), with no SQL. Grafana ties the signals together by configuring links between data sources: trace to logs maps span attributes to Loki labels and generates a LogQL query, and the way back relies on applications writing trace IDs into their log lines ([trace to logs](https://grafana.com/docs/grafana/latest/datasources/tempo/configure-tempo-data-source/configure-trace-to-logs/)). Micromegas has one ingestion service, one store and one SQL engine for logs, metrics and traces ([architecture][arch]). Every signal is keyed by the same process and stream identifiers, so one query can join logs, metrics and spans ([schema reference](../query-guide/schema-reference.md#view-relationships)), and one access-control model covers all of them ([authorization][authz]).

LGTM relies on upstream OTel SDKs, Faro, Beyla and Pyroscope profiling SDKs ([otel docs](https://grafana.com/docs/opentelemetry/)); Micromegas has [in-process SDKs](../native/index.md) for Rust and Unreal Engine. The `mimir-distributed` Helm chart enables Kafka-based ingest storage by default, which "requires a production-grade Apache Kafka cluster" ([ingest storage](https://grafana.com/docs/mimir/latest/set-up/jsonnet/configure-ingest-storage/)), and Tempo's microservices mode requires a Kafka-compatible system ([modes](https://grafana.com/docs/tempo/latest/set-up-for-tracing/setup-tempo/plan/deployment-modes/)); Micromegas needs PostgreSQL and object storage. Micromegas stores every emission as its own row and offers notebooks running the same engine in the browser ([WASM][exec]). Access control in LGTM is a paid add-on per backend: Enterprise Logs adds label-based access control and Enterprise Metrics adds fine-grained access control ([GEL](https://grafana.com/docs/enterprise-logs/latest/), [GEM](https://grafana.com/docs/enterprise-metrics/latest/)); Micromegas's [per-row access control][authz] is in the open-source build. Against Pyroscope's sampled call stacks, Micromegas spans can be named by the data being processed ([code vs. data](#complementary-tools)). Micromegas also ships a [Grafana data source plugin](../grafana/index.md), so the two can be combined.

## Micromegas vs. InfluxDB 3 Core

**InfluxDB 3 Core** is a time-series database built for recent data, with last-value and distinct-value caches and an embedded Python processing engine ([docs](https://docs.influxdata.com/influxdb3/core/)). It handles metrics and events first; logs and traces arrive only via Telegraf conversion. It is MIT or Apache-2.0 ([repo](https://github.com/influxdata/influxdb)); commercial Enterprise adds HA, read replicas, multi-node, long-range historical queries and historical compaction ([product](https://www.influxdata.com/products/influxdb-core/)). Core queries cover about 72 hours by default (`query-file-limit`), raisable at a memory and speed cost ([query](https://docs.influxdata.com/influxdb3/core/get-started/query/), [config](https://docs.influxdata.com/influxdb3/core/reference/config-options/)).

**Choose InfluxDB 3 Core when** you want a permissive license, the same Arrow, DataFusion, Parquet and Flight SQL stack, all metadata in object storage ([setup](https://docs.influxdata.com/influxdb3/core/get-started/setup/), [backup](https://docs.influxdata.com/influxdb3/core/admin/backup-restore/)), very fast recent-data queries, and an embedded Python engine. It queries with SQL and InfluxQL over HTTP, Arrow Flight and Flight SQL ([query](https://docs.influxdata.com/influxdb3/core/get-started/query/)).

**How Micromegas differs.** InfluxDB 3 Core and Micromegas both run DataFusion over Parquet. InfluxDB 3 ingests line protocol only, with OTLP converted by Telegraf ([write](https://docs.influxdata.com/influxdb3/core/write-data/), [Telegraf OTel](https://docs.influxdata.com/telegraf/v1/input-plugins/opentelemetry/)), and its [client libraries](https://docs.influxdata.com/influxdb3/core/reference/client-libraries/v3/) are for writing and querying, not instrumentation; Micromegas accepts [OTLP/HTTP][otlp-wire] natively and has [in-process SDKs](../native/index.md). Core is a single node with retention fixed per database at creation ([retention](https://docs.influxdata.com/influxdb3/core/reference/internals/data-retention/)); Micromegas runs independently scaled services over object storage with PostgreSQL metadata. Micromegas processes spans and per-process views into Parquet [only when queried][jit], and also offers notebooks running the same engine in the browser ([WASM][exec]).

## Micromegas vs. VictoriaMetrics, VictoriaLogs and VictoriaTraces

**VictoriaMetrics** is a family of three Go databases from one vendor: VictoriaMetrics (Prometheus long-term storage), VictoriaLogs, and VictoriaTraces, which is built on VictoriaLogs and stores spans as structured logs ([VictoriaTraces docs](https://docs.victoriametrics.com/victoriatraces/)). All three are Apache-2.0, cluster versions included ([cluster](https://docs.victoriametrics.com/victoriametrics/cluster-victoriametrics/)); Enterprise gates downsampling, multiple retentions, backup automation, mTLS and anomaly detection ([enterprise](https://docs.victoriametrics.com/victoriametrics/enterprise/)). VictoriaTraces is pre-1.0 and warns that its APIs "may not be backward compatible" ([repo](https://github.com/VictoriaMetrics/VictoriaTraces)).

**Choose VictoriaMetrics when** you want drop-in compatibility with Prometheus, Loki, Elasticsearch and Jaeger clients, operational simplicity ("a single small executable without external dependencies"), an open-source cluster mode, and low resource use. It ingests Prometheus remote write and scraping, Influx line protocol, OTLP over HTTP and more ([VictoriaMetrics](https://docs.victoriametrics.com/victoriametrics/integrations/opentelemetry/), [VictoriaLogs](https://docs.victoriametrics.com/victorialogs/data-ingestion/)), and vmalert evaluates MetricsQL and LogsQL rules ([vmalert](https://docs.victoriametrics.com/victoriametrics/vmalert/)).

**How Micromegas differs.** VictoriaMetrics stores data on local disk and uses object storage for vmbackup snapshots only ([vmbackup](https://docs.victoriametrics.com/victoriametrics/vmbackup/)); it does not use Parquet. Micromegas keeps raw payloads in object storage and processes spans and per-process views into Parquet [only when queried][jit]. VictoriaMetrics queries with MetricsQL ("backwards-compatible with PromQL", [metricsql](https://docs.victoriametrics.com/victoriametrics/metricsql/)) and LogsQL, with no SQL ([faq](https://docs.victoriametrics.com/victorialogs/faq/)); Micromegas has one SQL surface and notebooks running the same engine in the browser ([WASM][exec]). VictoriaMetrics relies on OTel and Prometheus clients, with one first-party Go [metrics](https://github.com/VictoriaMetrics/metrics) package; Micromegas has [in-process SDKs](../native/index.md) for Rust and Unreal Engine. Micromegas stores every emission as its own row. VictoriaMetrics needs no PostgreSQL; Micromegas does.

## Micromegas vs. Quickwit

**Quickwit** is a Rust search engine built on Tantivy for logs and traces, with compute separated from storage and search running directly on object storage ([overview](https://quickwit.io/docs/overview/introduction)). It is Apache-2.0 (relicensed from AGPL when Datadog acquired the team in January 2025); the founders said they would focus on "building a new product with Datadog", and there is no standalone commercial offering or paid support ([announcement](https://quickwit.io/blog/quickwit-joins-datadog)). It is still being released: v0.9.1 shipped on 2026-09-23 ([releases](https://github.com/quickwit-oss/quickwit/releases)).

**Choose Quickwit when** you want fast full-text log search on cheap object storage, a drop-in for Elasticsearch-compatible tooling, or Jaeger-native trace storage. It ingests OTLP for logs and traces, Jaeger, an Elasticsearch-compatible API, Kafka and SQS ([v0.9.0 notes](https://github.com/quickwit-oss/quickwit/releases/tag/v0.9.0)).

**How Micromegas differs.** Quickwit and Micromegas both use object storage and can keep metadata in PostgreSQL ([metastore](https://quickwit.io/docs/configuration/metastore-config)); that axis is shared. Quickwit indexes logs and traces and does not provide metrics aggregations; Micromegas stores logs, metrics and traces as Parquet. Quickwit exposes an Elasticsearch-compatible query API with no SQL; Micromegas queries with SQL. Quickwit relies on OTel SDKs; Micromegas has [in-process SDKs](../native/index.md). Micromegas offers notebooks running the same engine in the browser ([WASM][exec]), and a [Grafana plugin](../grafana/index.md).

## Micromegas vs. Prometheus

**Prometheus** is "an open-source systems monitoring and alerting toolkit" ([overview](https://prometheus.io/docs/introduction/overview/)). It is metrics only: each sample is a float64 or native histogram with a millisecond timestamp ([data model](https://prometheus.io/docs/concepts/data_model/)). It is Apache-2.0, written in Go, and CNCF graduated; Thanos is Apache-2.0 and CNCF incubating ([Prometheus](https://github.com/prometheus/prometheus), [Thanos](https://github.com/thanos-io/thanos)).

**Choose Prometheus when** you want the de-facto standard for service metrics: PromQL, simple single-binary operation, service discovery, mature alerting through Alertmanager ([alerting](https://prometheus.io/docs/alerting/latest/overview/)), and the largest exporter ecosystem, with metric client libraries for Go, Java/Scala, Node.js, Python, Ruby and Rust ([client libraries](https://prometheus.io/docs/instrumenting/clientlibs/)). Thanos and remote storage add long-term, global views.

**How Micromegas differs.** Micromegas is a Prometheus alternative for sub-second, high-frequency metrics, with three differences, each set against Prometheus's own docs.

*Frequency.* Prometheus records one sample per series per scrape, and `scrape_interval` defaults to `1m` ([config](https://prometheus.io/docs/prometheus/latest/configuration/configuration/)); gauges are "snapshots of state" ([instrumentation](https://prometheus.io/docs/practices/instrumentation/)), and "If you need 100% accuracy, such as for per-request billing, Prometheus is not a good choice, as the collected data will likely not be detailed and complete enough" ([overview](https://prometheus.io/docs/introduction/overview/)). Micromegas stores every metric emission as its own row with a nanosecond timestamp ([`measures`][schema-measures]). Its built-in system monitor samples host-wide CPU usage and used and free memory every 200 ms in each process using the Rust telemetry sink or the C ABI, plus the process's own memory every 5 s ([`system_monitor.rs`](https://github.com/madesroches/micromegas/blob/main/rust/telemetry-sink/src/system_monitor.rs)).

*Dimensionality.* In Prometheus "every unique combination of key-value label pairs represents a new time series… Do not use labels to store dimensions with high cardinality" ([naming](https://prometheus.io/docs/practices/naming/)); "Each labelset is an additional time series that has RAM, CPU, disk, and network costs", and over 100 values it suggests "moving the analysis away from monitoring and to a general-purpose processing system" ([instrumentation](https://prometheus.io/docs/practices/instrumentation/)). Labels are therefore low-cardinality and chosen at instrumentation time. In Micromegas, process, executable, computer and user are columns on each row alongside properties, and SQL can group or filter by any of them at query time ([`measures`][schema-measures]). Micromegas has its own producer-side cardinality contract: metric names, log targets and property sets are interned in process memory, so they must stay bounded, and free-form values belong in the log message body ([native SDK](../native/index.md), [Blender add-on](../blender/index.md#cardinality)).

*Scale.* "Prometheus's local storage is limited to a single node's scalability and durability" and "is not clustered or replicated" ([storage](https://prometheus.io/docs/prometheus/latest/storage/)); high availability means running "identical Prometheus servers on two or more separate machines" ([FAQ](https://prometheus.io/docs/introduction/faq/)). Thanos adds object storage for blocks, a global query view, deduplication and downsampling ([Thanos](https://github.com/thanos-io/thanos)). Micromegas runs independently scaled services over object storage ([architecture][arch]).

## Complementary tools

Tracy, Unreal Insights and Perfetto give a deep view of one session; they are not head-to-head alternatives to a fleet-wide store.

- **Tracy** is BSD-3-Clause, with a "hybrid frame and sampling profiler" ([repo](https://github.com/wolfpld/tracy)).
- **Unreal Insights** ships with Unreal Engine under the Epic EULA (source-available, not open source) ([docs](https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-insights-in-unreal-engine)).
- **Perfetto** is Apache-2.0, runs SQL over single trace files through trace_processor, and offers [call-stack sampling](https://perfetto.dev/docs/getting-started/cpu-profiling) ([repo](https://github.com/google/perfetto)).

Micromegas keeps the history of many processes in a single store and makes it queryable. It also exports a process's spans as a Perfetto trace that opens in the Perfetto UI ([`perfetto_trace_chunks`][perfetto]).

**Code vs. data.** A sampling profiler sees the call stack, so it shows which code is hot. Instrumentation can also record which data that code was processing: spans named by the asset or script (an `FName` in Unreal, a statically allocated string in Rust; see the [Unreal instrumentation API](../unreal/instrumentation-api.md) and [`macros.rs`](https://github.com/madesroches/micromegas/blob/main/rust/tracing/src/macros.rs)), and context such as the current level, asset or route attached as an interned property set that every log and metric event references for the cost of one pointer ([Default Context API][unreal-ctx], [`property_set.rs`](https://github.com/madesroches/micromegas/blob/main/rust/tracing/src/property_set.rs)). In a game it is rarely the animation code that is slow; it is a particular animation. The same holds for any interpreter or resolver (script VMs, query engines, template renderers, rule engines, asset loaders, routers, dependency resolvers): the stack is the same for every input, and the cost depends on the input. This is the point against sampling profilers such as Tracy's sampler and Perfetto's call-stack sampling, and continuous profilers like Pyroscope. Against Unreal Insights' instrumented scopes, the difference is cost and retention: data-named spans cheap enough to leave on everywhere, kept across the fleet.

## Also considered

- **Elasticsearch / OpenSearch**: Elasticsearch is AGPL, SSPL or ELv2 licensed ([repo](https://github.com/elastic/elasticsearch)) and OpenSearch is Apache-2.0 ([repo](https://github.com/opensearch-project/OpenSearch)); Quickwit and OpenObserve, above, cover the Elasticsearch-compatible use case.
- **Uptrace**: AGPL-3.0, built on ClickHouse with PostgreSQL metadata ([repo](https://github.com/uptrace/uptrace)); its latest release is a beta (v2.1.0-beta.8).
- **Jaeger (and Zipkin)**: Apache-2.0, traces only, with storage delegated to other backends ([Jaeger](https://github.com/jaegertracing/jaeger)).
- **Apache SkyWalking**: Apache-2.0, agent-centric APM with BanyanDB storage and GraphQL, PromQL, LogQL and TraceQL APIs ([storage docs](https://skywalking.apache.org/docs/main/next/en/setup/backend/backend-storage/)).
- **Apache Doris / StarRocks**: Apache-2.0 SQL warehouses, the same build-your-own category as plain ClickHouse ([Doris](https://github.com/apache/doris), [StarRocks](https://github.com/StarRocks/starrocks)).
- **Sentry self-hosted**: FSL-1.1-Apache-2.0, which is not OSI open source ([repo](https://github.com/getsentry/self-hosted)).

## Commercial SaaS

SaaS vendors bill on volume (hosts, GB ingested, spans), while Micromegas runs on your own object storage, so the comparison is a cost model rather than a feature list. The [vs. SaaS Vendors](../cost-effectiveness.md) pages ([methodology](../cost-comparisons/index.md), [Datadog](../cost-comparisons/datadog.md), [Dynatrace](../cost-comparisons/dynatrace.md), [Elastic](../cost-comparisons/elastic.md), [Grafana Cloud](../cost-comparisons/grafana.md), [New Relic](../cost-comparisons/newrelic.md), [Splunk](../cost-comparisons/splunk.md)) work through the numbers.

## Summary: which one fits

These projects overlap more than they compete, and many teams run two of them.

| If you need... | Look at |
|---|---|
| The standard self-hosted stack, PromQL, and the Grafana ecosystem | Grafana LGTM |
| Prometheus-compatible metrics and logs with few moving parts and no external dependencies | VictoriaMetrics / VictoriaLogs |
| A ready-made APM UI for OTel-instrumented services, alerting included | SigNoz or ClickStack |
| All signals as Parquet on object storage, one binary, no metadata database | Parseable |
| The widest signal coverage (RUM, session replay), or a migration off ELK | OpenObserve |
| One SQL database for metrics, logs and traces, replacing Prometheus long-term storage | GreptimeDB |
| Raw query speed at very large scale, if you build your own pipeline | ClickHouse |
| Recent-data time-series queries on Arrow/Parquet, with SQL and InfluxQL | InfluxDB 3 Core |
| Elasticsearch-compatible log and trace search directly on object storage | Quickwit |
| Scrape-based service monitoring and alerting, with the largest exporter ecosystem | Prometheus (Thanos for long-term, global view) |
| A deep look at one session on one machine | Tracy, Unreal Insights, Perfetto (alongside any of the above) |
| You're on a SaaS vendor and cost at high volume is the problem | see the [vs. SaaS Vendors](../cost-effectiveness.md) cost pages |

**Choose Micromegas when** efficiency matters, meaning instrumentation overhead and cost:

- you instrument native code and want detailed spans (Rust crates, Unreal plugin), logs and metrics (also from any app through the C ABI, as the [Blender add-on](../blender/index.md) does) left on in production;
- you want very high-frequency, high-resolution telemetry, or full-resolution traces without sampling: Rust CPU traces record every span unsampled in production, enabled with `MICROMEGAS_ENABLE_CPU_TRACING=true` ([`lib.rs`](https://github.com/madesroches/micromegas/blob/main/rust/telemetry-sink/src/lib.rs)), and `telemetry.spans.all` does the same in Unreal, whose default keeps blocks around frame spikes ([console variables][unreal-cvars], [`SamplingController.h`](https://github.com/madesroches/micromegas/blob/main/unreal/MicromegasTelemetrySink/Private/SamplingController.h));
- your cost depends on the data more than the code (assets, URLs, scripts, queries going through an interpreter or resolver) and you need to know which input was slow, not just which function;
- your telemetry comes from many processes that aren't classic services: desktop or mobile clients, edge devices, batch jobs, CI runners, game clients and servers;
- you need high event volume and long retention at a predictable cost, stored as Parquet on your own object storage, with spans processed only when queried ([on-demand processing][cost-ondemand]); one production deployment on AWS runs at about $1,100/month for 449 billion events over 90 days ([cost breakdown][cost]);
- you want one SQL surface across logs, metrics and traces, including in notebooks, instead of one query language per signal;
- you need [per-row access control][authz] so each team sees the telemetry meant for it, with privacy guarantees.

**Look elsewhere if**:

- you want the established, widely adopted default, with the largest community and integration ecosystem (Grafana LGTM, Prometheus, SigNoz);
- you don't want to operate the stack (a SaaS vendor, or a peer's hosted offering);
- you can't run PostgreSQL, need a built-in alert engine, or have PromQL dashboards and alert rules you want to keep.

## FAQ

### What is an open-source, self-hosted alternative to Datadog that I can query with SQL?

It depends on the workload. For OTel-instrumented services with a ready-made APM UI, look at SigNoz or ClickStack. For native code and client fleets where efficiency matters, Micromegas is Apache-2.0, self-hosted and queried with SQL over FlightSQL ([query guide](../query-guide/index.md)).

### How do I reduce observability costs at high event volume?

Micromegas stores raw data in your own object storage and processes spans only when they are queried; one production deployment runs at about $1,100/month for 449 billion events over 90 days ([cost breakdown][cost]). VictoriaMetrics is a strong choice for metrics, with low resource use and no external dependencies ([VictoriaMetrics](https://github.com/VictoriaMetrics)).

### How do I record full-resolution traces in production without sampling?

In Rust, set `MICROMEGAS_ENABLE_CPU_TRACING=true` and every CPU span is recorded unsampled ([`lib.rs`](https://github.com/madesroches/micromegas/blob/main/rust/telemetry-sink/src/lib.rs)). In Unreal Engine, set `telemetry.spans.all 1` ([console variables][unreal-cvars]); by default Unreal keeps blocks around frame spikes.

### How do I collect telemetry from Unreal Engine games in production?

Micromegas has an [Unreal Engine plugin](../unreal/index.md) that records spans, logs and metrics and ships them to your own object storage. Unreal Insights remains the right tool for a deep look at a single session.

### How do I collect telemetry from desktop apps or game clients across many users?

Micromegas keeps one row per emission with process, computer and user columns, so SQL can group by any of them across the fleet ([`measures`][schema-measures]). Use the Rust or Unreal SDKs, the [C ABI](../native/index.md) for logs and metrics, or [OTLP/HTTP][otlp-wire].

### How do I find which asset, script or query made my code slow, not just which function?

Name spans by the data being processed (an `FName` in Unreal, a statically allocated string in Rust) and attach context as a property set, then query the spans in SQL. A sampling profiler shows the hot function; data-named spans show the input behind it ([code vs. data](#complementary-tools)).

### What is a Prometheus alternative for sub-second, high-frequency metrics?

Prometheus's `scrape_interval` defaults to `1m` ([config](https://prometheus.io/docs/prometheus/latest/configuration/configuration/)). Micromegas stores every metric emission as its own row ([`measures`][schema-measures]), and GreptimeDB is a good fit for high-cardinality metrics and Prometheus long-term storage.

### How do I give each team access to the right telemetry, with privacy guarantees, in a self-hosted observability store?

Micromegas stamps every row with an audience taken from the ingestion credential, and data stamped for an audience is visible only to principals granted that audience, so each team sees the data meant for it. All of this is in the Apache-2.0 build ([authorization][authz]). Grafana Enterprise Logs offers label-based access control, but it is a paid tier ([GEL](https://grafana.com/docs/enterprise-logs/latest/)).

### How do I trace Rust applications in production with low overhead?

Use the `micromegas-tracing` macros such as `span_scope!` and `#[span_fn]`, which record spans in-process on the calling thread; existing `tracing` events are captured as logs ([`tracing_interop.rs`](https://github.com/madesroches/micromegas/blob/main/rust/telemetry-sink/src/tracing_interop.rs)).

[arch]: ../architecture/index.md#core-components
[jit]: ../architecture/index.md#2-dual-processing-strategies
[otlp-wire]: ../otlp/index.md#overview
[otlp-limits]: ../otlp/index.md#limitations
[schema-measures]: ../query-guide/schema-reference.md#measures
[cost]: ../cost-effectiveness.md#scale-perspective
[cost-ondemand]: ../cost-effectiveness.md#on-demand-processing-tail-sampling
[unreal-cvars]: ../unreal/installation.md#runtime-console-commands-and-cvars
[unreal-ctx]: ../unreal/instrumentation-api.md#default-context-api
[perfetto]: ../query-guide/functions-reference.md#perfetto_trace_chunksprocess_id-span_types-start_time-end_time
[authz]: ../admin/authorization.md#query-time-audience-filtering
[exec]: ../web-app/notebooks/execution.md
