# Open-Source Peer Comparison Page Plan

Issue: [#1636](https://github.com/madesroches/micromegas/issues/1636)

## Overview

Add one handwritten docs page comparing Micromegas with the open-source observability tools it is
most often weighed against. The existing `cost-comparisons/` section only covers SaaS vendors
(Datadog, Dynatrace, Elastic, Grafana Cloud, New Relic, Splunk); nothing addresses the self-hosted
peers that search results and LLM answers recommend for "open source Datadog alternative,
self-hosted, SQL". The page must be neutral: for each peer, say plainly where it is the better
choice. A wrong claim about another project is worse than no page, so every peer statement is
linked to that project's own current docs or repo.

## Current State

- **Docs site**: MkDocs Material, config in `mkdocs/mkdocs.yml`, sources in `mkdocs/docs/`. The nav
  is explicit (`mkdocs.yml` `nav:`), with tabs enabled, so every top-level entry becomes a tab.
- **Sitemap**: MkDocs generates `/docs/sitemap.xml` from every built page automatically.
  `welcome/public/robots.txt` already advertises it, and `welcome/public/sitemap.xml` lists only
  non-MkDocs pages. A new page therefore lands in the sitemap without any manual edit.
- **llms.txt**: `welcome/public/llms.txt` is a hand-maintained index, grouped by section, with a
  `## Cost` section that links the SaaS comparisons.
- **CI check**: `.github/workflows/publish-docs.yml` runs on PRs touching `mkdocs/**` or
  `welcome/**`. It builds the staged site and runs `build/check_docs_site.py public_docs`, which
  checks several things:
  - sitemap `<loc>`s resolve
  - canonical tags are present
  - every on-site `llms.txt` link resolves to a built file (`check_llms_txt`)

  That last check covers the issue's "link check passing in CI" requirement as-is. No checker
  change is needed.
- **Precedent**: `cost-comparisons/*.md` open with a "Last reviewed" or "verified" date and a
  disclaimer, use one summary table, then qualitative sections. `tasks/completed/rework_cost_section_plan.md`
  embedded its dated research in the plan itself; this plan does the same below.

### Micromegas facts the page may state (verified in-repo)

| Claim | Source |
|---|---|
| Apache-2.0 | `LICENSE`, `README.md` |
| Raw payloads in object storage (S3, GCS, local); **metadata in PostgreSQL** | `mkdocs/docs/architecture/index.md` (Storage) |
| Lakehouse materializes views to Parquet; SQL via Apache DataFusion | `architecture/index.md`, `query-guide/` |
| **FlightSQL is the query protocol**, not ingestion. Ingestion is HTTP (native transit/CBOR format and OTLP) | `admin/flight-sql.md`, `admin/ingestion.md` |
| **Accepts** OTLP/HTTP (protobuf or JSON, gzip) for logs, metrics, traces. **No OTLP/gRPC** | `otlp/index.md` (Wire format; limitations at line ~693) |
| Native SDKs: Rust (`tracing` / `telemetry` crates), Unreal Engine plugin, C ABI | `unreal/`, `native/`, `rust/` |
| Notebooks run DataFusion in the browser via WASM | `web-app/notebooks/execution.md` |
| Grafana data source plugin; alerting goes **through Grafana**. No built-in alert engine | `grafana/`, `llms.txt` |
| Per-row audience access control on ingested data | `admin/authorization.md`, blog 2026-09-03 |
| Single-process `micromegas-monolith` or split services | `admin/monolith.md` |

**Instrumentation cost**: the page does **not** quote the ~20 ns figure, since it depends on too many
variables (see Decisions). Describe the design instead:
- events are recorded in-process on the calling thread;
- the telemetry sink batches and ships them off the hot path;
- sampling decisions are made per batch, not per event;
- the intent is instrumentation that stays on in production.

Compare this with OpenTelemetry **SDKs**, which are the in-process counterpart, never with
collectors.

## Design

### Page location and nav

- New file: `mkdocs/docs/comparisons/open-source.md`, served at
  `https://micromegas.info/docs/comparisons/open-source/`.
- New top-level nav section, placed after `Getting Started` because comparing tools is something
  readers do while evaluating:
  ```yaml
  - Comparisons:
    - Open-Source Peers: comparisons/open-source.md
  ```
- The `comparisons/` directory has room for the planned AI-agent observability page (see
  Decisions). That page gets added beside this one, and this page doesn't change. The SaaS cost
  pages stay where they are, because moving them would change URLs that are already linked in
  `llms.txt` and indexed.

### Peer set

| Tier | Projects | Treatment |
|---|---|---|
| Full section | Parseable, OpenObserve, GreptimeDB, SigNoz, ClickHouse (incl. ClickStack/HyperDX), Grafana LGTM (Loki/Tempo/Mimir), InfluxDB 3 Core, VictoriaMetrics/Logs/Traces | Glance-table row plus a section |
| Complementary tools | Tracy, Unreal Insights, Perfetto | Short section: session-local profilers vs. a fleet-wide, historical store. Not head-to-head |
| Also considered | Elasticsearch/OpenSearch, Quickwit, Uptrace, Jaeger, Apache Doris/StarRocks, Sentry self-hosted | One line each with the reason it isn't a full entry |

The five peers named in the issue are kept. Three are added from the additional-peer research:
- **Grafana LGTM**: it is the default self-hosted answer and the stack most readers already run.
- **InfluxDB 3 Core**: it has the closest architecture to Micromegas (Rust, Arrow, DataFusion, Parquet, Flight, object storage).
- **VictoriaMetrics family**: it is widely recommended and now covers all three signals.

Excluded, and not listed on the page:
- archived or discontinued projects: SigLens, HoraeDB, Highlight.io;
- different categories: Netdata, Zabbix, Coroot, DeepFlow, OneUptime;
- niche or stale projects: gigapipe, CnosDB, Optick, MicroProfile;
- Superluminal, which is not open source.

### Page structure

1. **Intro**:
   - scope: open-source and self-hosted only, with a link to the SaaS cost comparisons;
   - a `*Last reviewed: October 2026*` line;
   - one sentence saying every peer claim links to that project's docs, and inviting corrections via GitHub issues.
2. **Micromegas in brief**: four short bullets, one per stage (instrumentation, ingestion,
   analytics, presentation), followed by a **Limits** list stated plainly:
   - PostgreSQL is required for metadata;
   - OTLP is HTTP-only (no gRPC);
   - no built-in alert engine (alerting goes through Grafana);
   - no PromQL/LogQL;
   - native SDKs only for Rust, Unreal and C;
   - no RUM or session replay;
   - smaller community than the peers.
3. **At a glance** table. The columns below are the ones the research could fill with a source for
   every cell:

   | Project | License (OSS edition) | Signals | Storage | Required services beyond the binary | Query languages | Own in-process SDKs | Built-in UI / alerting |
   |---|---|---|---|---|---|---|---|

   License cells must name gated editions where they exist, e.g. "AGPL-3.0; paid tiers gate PromQL, HA".
4. **One section per full peer**, each with the same three headings, which keeps entries comparable and stops
   them drifting into a sales sheet:
   - *What it is*: one or two sentences, plus license and editions.
   - *Choose it when*: the peer's real strengths, credited without hedging.
   - *How Micromegas differs*: only the axes where the difference is real for that peer. The
     candidate axes are:
     - in-process native SDKs vs. relying on OTel SDKs;
     - Parquet on object storage plus PostgreSQL metadata vs. that peer's storage and dependencies;
     - one SQL surface (DataFusion) vs. that peer's query languages;
     - notebooks running the same engine in the browser (WASM).

     Where a peer shares an axis (e.g. Parseable, OpenObserve, GreptimeDB and InfluxDB 3 also run
     DataFusion over Parquet), the section says so and leaves that axis out of the differences.
   - OpenObserve, Parseable and SigNoz each get one sentence on their LLM/agent observability features.
5. **Complementary tools**: Tracy, Unreal Insights and Perfetto give a deep view of one session.
   Micromegas keeps the history of many processes in a single store and makes it queryable. Teams
   commonly use both.
6. **Also considered**: one line each.

### Peer research (October 2026)

All facts below were fetched on 2026-10-02 from the linked source. The implementer re-checks each
cell against the link before publishing (step 1), because these projects release weekly. Items
marked **unverified** must be checked or left off the page.

#### Parseable — [repo](https://github.com/parseablehq/parseable)
- **What it is**: Rust "unified observability platform on a data lake architecture" for logs,
  metrics, traces and events.
- **License and editions**: AGPL-3.0. Paid Cloud/Enterprise tiers gate PromQL, the HA cluster, APM,
  anomaly detection and AI features. SQL, dashboards, threshold alerts, OIDC/SSO and RBAC are in OSS
  ([pricing](https://www.parseable.com/pricing)).
- **Ingestion**: OTLP over HTTP and gRPC; Prometheus remote write; Fluent Bit, Vector, Logstash,
  Filebeat; Kafka ([integrations](https://www.parseable.com/docs/integrations)).
- **Storage**: Arrow staged on local disk, then converted to Parquet on S3, GCS, Azure Blob or the
  local filesystem.
- **Metadata**: no external database; metadata lives in the object store
  ([architecture](https://www.parseable.com/docs/architecture)).
- **Query**: SQL on DataFusion (`Cargo.toml`). PromQL is paid-only.
- **UI**: built-in UI with dashboards, alerts and RBAC.
- **Deployment**: single binary, standalone or distributed with ingest, query and search roles.
- **SDKs**: relies on OTel; there is a small Go SDK.
- **Unverified**: an Elasticsearch `_bulk` endpoint; the exact OSS limits on distributed mode.
- **Credit it for**: smallest footprint of the set (one binary, no metadata DB); broad ingestion compatibility.

#### OpenObserve — [repo](https://github.com/openobserve/openobserve)
- **What it is**: Rust backend with a Vue UI, covering logs, metrics, traces, RUM, session replay,
  profiles and LLM observability.
- **License and editions**: AGPL-3.0 OSS (it moved from Apache). The Enterprise edition is under a
  commercial license and free up to 50 GB/day
  ([license](https://openobserve.ai/docs/enterprise-setup/license-and-pricing/)). It gates SSO,
  advanced RBAC, audit logs, federation and AI features
  ([features](https://openobserve.ai/docs/enterprise-setup/enterprise-features/)).
- **Ingestion**: OTLP; JSON, `_multi` and Elasticsearch-compatible `_bulk` APIs; syslog; the
  Collector, Vector, Fluent Bit and Filebeat; Prometheus and Telegraf
  ([ingestion](https://openobserve.ai/docs/ingestion/)).
- **Storage**: Parquet on S3, GCS, Azure Blob or MinIO, or local disk on a single node.
- **Metadata**: SQLite on a single node; PostgreSQL plus NATS in HA mode
  ([architecture](https://openobserve.ai/docs/architecture/)).
- **Query**: SQL on DataFusion (`Cargo.toml`), PromQL, and full-text search via Tantivy.
- **UI**: rich built-in UI with dashboards, pipelines, alerts and incidents.
- **SDKs**: OTel SDKs for backend code; its own RUM SDKs for browser, Android, iOS and React Native.
- **Unverified**: native Kafka ingestion; whether PromQL is free in OSS.
- **Credit it for**: broadest signal coverage; easy migration from ELK via `_bulk`; full-text indexing.

#### GreptimeDB — [repo](https://github.com/GreptimeTeam/greptimedb)
- **What it is**: Rust "observability database"; one columnar engine for metrics, logs and traces,
  with SQL joins across signals.
- **License and editions**: Apache-2.0 core, open-core model. Enterprise gates Triggers (alerting),
  LDAP, RBAC, audit logs and automatic rebalancing
  ([enterprise](https://docs.greptime.com/enterprise/overview/)). There is also GreptimeCloud.
- **Ingestion**: OTLP, Prometheus remote write, Loki push, Elasticsearch `_bulk`, InfluxDB line
  protocol, and the MySQL and PostgreSQL wire protocols.
- **Storage**: Parquet on S3, GCS or Azure Blob, or local file storage
  ([config](https://docs.greptime.com/user-guide/deployments-administration/configuration/)).
- **Metadata**: standalone needs no dependencies. Distributed mode needs etcd, PostgreSQL or MySQL
  for metasrv, plus optional Kafka for the WAL.
- **Query**: SQL on DataFusion and PromQL (with documented gaps). No LogQL.
- **UI**: built-in dashboard and a Grafana plugin. The OSS edition has no built-in alerting.
- **SDKs**: ingester client libraries only; no instrumentation SDKs.
- **Credit it for**: Prometheus long-term storage replacement; migrating one signal at a time; high-cardinality metrics.

#### SigNoz — [repo](https://github.com/SigNoz/signoz)
- **What it is**: Go and React OTel-native APM covering logs, metrics, traces, exceptions and LLM observability.
- **License**:
  - MIT outside `ee/` and `cmd/enterprise/`, which are under a proprietary license
    ([LICENSE](https://github.com/SigNoz/signoz/blob/main/LICENSE));
  - the collector is AGPL-3.0 ([repo](https://github.com/SigNoz/signoz-otel-collector));
  - the license column must say this precisely.
- **Editions**: Cloud and Enterprise gate anomaly detection, SAML, fine-grained RBAC and audit
  logs ([pricing](https://signoz.io/pricing/)).
- **Ingestion**: through the SigNoz OTel Collector: OTLP, Jaeger, Zipkin, Kafka, Prometheus, and
  more.
- **Storage and dependencies**: ClickHouse plus ClickHouse Keeper or ZooKeeper, with SQLite
  (PostgreSQL in `ee/`) for dashboards, alerts and users
  ([Foundry moldings](https://github.com/SigNoz/foundry/blob/main/docs/concepts/moldings.md)).
- **Query**: query builder, PromQL, ClickHouse SQL.
- **UI**: built-in APM views, traces, logs, dashboards and alerts (Ruler plus Alertmanager)
  ([architecture](https://signoz.io/docs/architecture/)).
- **SDKs**: OTel SDKs only.
- **Unverified**: object-storage tiering in SigNoz deployments; first-party docs for Prometheus remote write.
- **Credit it for**: the best out-of-box APM experience for an OTel-instrumented service, with no Grafana needed.

#### ClickHouse / ClickStack — [ClickHouse](https://github.com/ClickHouse/ClickHouse), [HyperDX](https://github.com/hyperdxio/hyperdx)
- **What it is**: C++ columnar OLAP database. Its own docs say it "isn't an out-of-the-box solution
  for Observability" but is a highly efficient storage engine
  ([intro](https://clickhouse.com/docs/use-cases/observability/introduction)).
- **ClickStack**: ClickHouse plus the HyperDX UI plus an OTel collector distribution
  ([overview](https://clickhouse.com/docs/use-cases/observability/clickstack/overview)).
- **License**: ClickHouse is Apache-2.0; HyperDX is MIT. ClickHouse Cloud is a managed service.
- **Ingestion**: ClickStack accepts OTLP over HTTP and gRPC through its collector. Other paths are
  the contrib `clickhouseexporter`
  ([README](https://github.com/open-telemetry/opentelemetry-collector-contrib/tree/main/exporter/clickhouseexporter))
  and the Kafka engine.
- **Storage**:
  - MergeTree on local disk, with S3 as a tiered disk
    ([S3](https://clickhouse.com/docs/integrations/s3));
  - SharedMergeTree (object-storage native) is only documented for Cloud;
  - reads and writes Parquet.
- **Coordination and dependencies**: replication needs ClickHouse Keeper or ZooKeeper. Self-hosted
  ClickStack also needs **MongoDB** for dashboards, alerts and users
  ([architecture](https://clickhouse.com/docs/use-cases/observability/clickstack/architecture)).
- **Query**: ClickHouse SQL; ClickStack adds Lucene-style search and a SQL WHERE mode. Metrics and
  PromQL support are described as less mature.
- **UI**: HyperDX provides search, traces, dashboards, alerts and session replay.
- **SDKs**: OTel-based SDKs.
- **Credit it for**: raw query speed and compression at very large scale, plus ecosystem maturity (about 50k stars).

#### Grafana LGTM — [Loki](https://github.com/grafana/loki), [Tempo](https://github.com/grafana/tempo), [Mimir](https://github.com/grafana/mimir)
- **Verified so far**:
  - license AGPL-3.0;
  - active (Loki v3.7.8 2026-09-17, Tempo v3.1.0 2026-09-29, Mimir 3.2.1 2026-09-10);
  - three stores with LogQL, TraceQL and PromQL respectively.
- **To verify in step 1**:
  - object-storage backends per component;
  - Tempo's Parquet block format;
  - the required dependencies;
  - Pyroscope's role;
  - alerting via Grafana and Mimir/Loki rulers.
- **Credit it for**: the de-facto self-hosted standard, the Grafana ecosystem, and PromQL compatibility.

#### InfluxDB 3 Core — [repo](https://github.com/influxdata/influxdb)
- **Verified so far**:
  - MIT/Apache-2.0;
  - active (v3.11.4, 2026-09-08);
  - Rust on Arrow, DataFusion, Parquet and Flight with object storage
    ([GA post](https://www.influxdata.com/blog/influxdb-3-oss-ga));
  - time-series and metrics focused.
- **To verify in step 1**:
  - which features are Core vs. Enterprise (retention, compaction, HA);
  - query languages (SQL, InfluxQL);
  - ingestion paths (line protocol; OTLP?);
  - whether a metadata catalog is stored in object storage.

#### VictoriaMetrics / VictoriaLogs / VictoriaTraces — [org](https://github.com/VictoriaMetrics)
- **Verified so far**:
  - Apache-2.0;
  - active (VM v1.153.0, VictoriaLogs v1.53.0, VictoriaTraces v0.12.0, all Sept–Oct 2026);
  - Go;
  - query languages MetricsQL and LogsQL; no SQL.
- **To verify in step 1**:
  - storage on local disk vs. object storage;
  - which features are cluster vs. enterprise;
  - ingestion protocols (Prometheus, OTLP, Loki, ES);
  - VictoriaTraces' maturity (pre-1.0).

#### Complementary and also-considered (status verified 2026-10-02)

**Complementary tools**

| Project | License | Status | Source |
|---|---|---|---|
| Tracy | BSD-3-Clause | v0.14.1, 2026-08-22 | https://github.com/wolfpld/tracy |
| Unreal Insights | Epic EULA (source-available, not OSS) | Ships with UE | https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-insights-in-unreal-engine |
| Perfetto | Apache-2.0 | v58.2; SQL over single trace files via trace_processor | https://github.com/google/perfetto |

**Also considered**

| Project | License | Status | Source |
|---|---|---|---|
| Elasticsearch | AGPL / SSPL / ELv2 triple license | Active | https://github.com/elastic/elasticsearch |
| OpenSearch | Apache-2.0 | Active | https://github.com/opensearch-project/OpenSearch |
| Quickwit | Apache-2.0 | Acquired by Datadog Jan 2025; still releasing (v0.9.1, 2026-09-23) | https://www.datadoghq.com/blog/datadog-acquires-quickwit/ |
| Uptrace | AGPL-3.0 | ClickHouse + PostgreSQL; v2.1.0-beta.8 | https://github.com/uptrace/uptrace |
| Jaeger | Apache-2.0 | Traces only | https://github.com/jaegertracing/jaeger |
| Apache Doris | Apache-2.0 | DIY SQL warehouse, same category as plain ClickHouse | https://github.com/apache/doris |
| StarRocks | Apache-2.0 | DIY SQL warehouse, same category as plain ClickHouse | https://github.com/StarRocks/starrocks |
| Sentry self-hosted | FSL-1.1-Apache-2.0 (not OSI open source) | Active | https://github.com/getsentry/self-hosted |

## Implementation Steps

1. **Re-verify and complete the research.** For every peer, open each linked source and confirm each
   cell in the research above. Then finish the "to verify" items for LGTM, InfluxDB 3 and the
   Victoria family. Anything that stays unconfirmed is dropped from the page; it is not hedged.
2. **Write `mkdocs/docs/comparisons/open-source.md`** following the page structure above:
   - inline-link every peer claim to its source;
   - avoid superlatives about Micromegas and the 20 ns figure;
   - use the Micromegas phrasing from "Micromegas facts the page may state".
3. **Nav**: add the `Comparisons` section to `mkdocs/mkdocs.yml` after `Getting Started`.
4. **Cross-links**:
   - one line at the top of `mkdocs/docs/cost-comparisons/index.md` pointing open-source readers to the new page;
   - one line in the matching section of `mkdocs/docs/index.md`, if it has a comparison/why section.
5. **llms.txt**: add a `## Comparisons` section to `welcome/public/llms.txt` above `## Cost`, with
   `[Open-source peers](https://micromegas.info/docs/comparisons/open-source/)` and a one-line
   description naming the peers, since LLM retrieval matches on those names.
6. **Build and check locally** (see Testing Strategy).

## Files to Modify

- `mkdocs/docs/comparisons/open-source.md` (new)
- `mkdocs/mkdocs.yml` (nav)
- `welcome/public/llms.txt`
- `mkdocs/docs/cost-comparisons/index.md` (one cross-link line)
- `mkdocs/docs/index.md` (optional cross-link)
- `CHANGELOG.md` (docs entry under Unreleased, if docs pages are logged there)

## Trade-offs

- **One page vs. one page per peer** (like `cost-comparisons/`): the issue asks for one page. One
  page keeps the glance table meaningful and puts all the review-date maintenance in a single
  place. Per-peer pages would rank better for "X vs Micromegas" searches but would multiply the
  pages that go stale. If search data later justifies them, they can split off.
- **New `comparisons/` section vs. nesting under Cost Effectiveness**: this page is about
  architecture and fit, not cost, and its peers are free software, so a cost framing would be
  misleading. A separate section also has room for the agent-observability page.
- **Also-considered one-liners vs. silence**: a short "also considered" list answers "why isn't X
  here?" cheaply. Projects that are archived or in a different category are left off entirely,
  rather than listed only to dismiss them.

## Decisions

- Don't quote the ~20 ns instrumentation figure; describe the design instead (user call: it depends on too many variables).
- LLM agent observability gets its own page under `comparisons/` in a separate issue, against its own peers (Langfuse, Arize Phoenix, Opik, etc.). This page carries only a one-sentence agent-features note on OpenObserve, Parseable and SigNoz.
- Peer set extended beyond the issue with Grafana LGTM, InfluxDB 3 Core and the VictoriaMetrics family as full entries.
- No change to `build/check_docs_site.py`: its existing `llms.txt` check already enforces the issue's CI requirement.
- No manual sitemap edit: MkDocs adds the page to `/docs/sitemap.xml` automatically.

## Documentation

This change is documentation. Besides the new page: the nav, `llms.txt`, and one cross-link line
on the SaaS comparison methodology page.

## Testing Strategy

There is no code, so no unit tests. Automated coverage comes from the existing `publish-docs.yml`
job, which runs on this PR because it touches `mkdocs/**` and `welcome/**`:
- `check_llms_txt` fails if the new `llms.txt` link does not resolve to a built page;
- the sitemap checks fail if the page's `<loc>` does not resolve;
- the canonical-tag check covers the new page.

## Manual Verification

These checks need a human: whether a table reads well and whether a claim matches a peer's docs
can only be eyeballed.

1. `cd mkdocs && python serve.py`. Open `http://127.0.0.1:8000/docs/comparisons/open-source/`
   (or the URL `serve.py` prints). The page should render with the glance table readable at
   laptop width and the new `Comparisons` tab visible in the nav.
2. Build the staged tree the way CI does, then run the checker:
   ```
   mkdocs build --config-file mkdocs/mkdocs.yml --site-dir $PWD/public_docs/docs
   cp welcome/public/{llms.txt,robots.txt,sitemap.xml} public_docs/ && echo micromegas.info > public_docs/CNAME
   python3 build/check_docs_site.py public_docs
   ```
   Expected: `OK`, and `grep comparisons/open-source public_docs/docs/sitemap.xml` returns one line.
   The root `index.html` comes from the welcome build. If it is missing locally, the canonical check
   for it is simply not run.
3. Click every peer source link on the rendered page. Each one should load and support the
   statement next to it.

## Open Questions

- Is the peer set right: the issue's five plus LGTM, InfluxDB 3 Core and VictoriaMetrics as full
  entries, Tracy, Unreal Insights and Perfetto as complementary, and the rest as one-liners? Should
  Quickwit be promoted to a full entry?
- Is a top-level `Comparisons` nav tab OK, or should the page sit under an existing tab
  (e.g. `Operations`) to keep the tab bar short?
- Should the separate AI-agent observability issue be filed now?
