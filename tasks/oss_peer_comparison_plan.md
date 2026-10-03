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

**Goal**: give LLMs (and the search results they draw on) content that lets them recommend
Micromegas for the workloads it fits. Neutrality serves that goal: a sourced page that credits its
peers gets quoted as a reference, while a sales sheet gets discounted. See "Writing for LLM
retrieval" below.

## Current State

- **Docs site**: MkDocs Material, config in `mkdocs/mkdocs.yml`, sources in `mkdocs/docs/`. The nav
  is explicit (`mkdocs.yml` `nav:`), with tabs enabled, so every top-level entry becomes a tab.
  There are seven tabs today: Home, Blog, Getting Started, Query Guide, Analytics Web App,
  Integrations, Operations. The SaaS cost pages sit under **Operations → Cost Effectiveness**.
- **Sitemap**: MkDocs generates `/docs/sitemap.xml` from every built page automatically.
  `welcome/public/robots.txt` already advertises it, and `welcome/public/sitemap.xml` lists only
  non-MkDocs pages.
- **llms.txt**: `welcome/public/llms.txt` is a hand-maintained index, grouped by section, with a
  `## Cost` section that links the SaaS comparisons.
- **CI check**: `.github/workflows/publish-docs.yml` runs on PRs touching `mkdocs/**` or
  `welcome/**`. It builds the staged site and runs `build/check_docs_site.py public_docs`, which
  checks several things:
  - sitemap `<loc>`s resolve
  - canonical tags are present
  - every on-site `llms.txt` link resolves to a built file (`check_llms_txt`)
- **Precedent**: `cost-comparisons/*.md` open with a "Last reviewed" or "verified" date and a
  disclaimer, use one summary table, then qualitative sections. `tasks/completed/rework_cost_section_plan.md`
  embedded its dated research in the plan itself; this plan does the same below.

### Micromegas facts the page may state (verified in-repo)

| Claim | Source |
|---|---|
| Apache-2.0 | `LICENSE`, `README.md` |
| Raw payloads in object storage (S3, GCS, local); **metadata in PostgreSQL** | `mkdocs/docs/architecture/index.md` (Storage) |
| Lakehouse materializes views to Parquet; SQL via Apache DataFusion | `architecture/index.md`, `query-guide/` |
| **FlightSQL is the query protocol**, not ingestion. Ingestion is HTTP (native transit/CBOR format and OTLP) | `architecture/index.md` (Ingestion Service), `admin/flight-sql.md`, `otlp/index.md` |
| **Accepts** OTLP/HTTP (protobuf or JSON, gzip) for logs, metrics, traces. **No OTLP/gRPC** | `otlp/index.md` (Wire format; limitations at line ~693) |
| Native SDKs: Rust (`micromegas-tracing` macros such as `span_scope!` / `#[span_fn]`) and Unreal Engine plugin for spans, logs and metrics; the C ABI covers logs and metrics only. The `tracing`-crate interop forwards `tracing` **events** as logs, not spans | `unreal/`, `rust/`, `rust/capi/src/lib.rs`, `rust/telemetry-sink/src/tracing_interop.rs`, `mkdocs/docs/native/index.md` |
| Full-resolution spans: Rust CPU (thread) spans are recorded at full resolution, unsampled, and run in production; `MICROMEGAS_ENABLE_CPU_TRACING=true` turns them on (the default is off, a conservative setting; user call). The Unreal plugin samples by default (blocks kept around frame spikes); `telemetry.spans.all 1` records every span | `rust/telemetry-sink/src/lib.rs` (`MICROMEGAS_ENABLE_CPU_TRACING`, default off), `unreal/instrumentation-api.md` (Console Commands) |
| Raw data stays in object storage. Spans and per-process views are materialized only when queried (JIT ETL), so their processing cost follows what is queried; logs and metrics are materialized continuously into global views by the maintenance daemon | `architecture/index.md` (JIT ETL), `cost-effectiveness.md` (On-Demand Processing) |
| A production deployment on AWS: ~$1,100/month total, 449 billion events over 90 days, 8.5 TB in S3 | `cost-effectiveness.md` (Scale Perspective, cost breakdown) |
| Span names can come from runtime data as long as the string is statically allocated: an `FName` (asset, UObject) in Unreal, a `&'static str` (e.g. interned) in Rust | `rust/tracing/src/macros.rs` (`span_scope_named!`, `instrument_named!`), `unreal/instrumentation-api.md` (`MICROMEGAS_SPAN_NAME`, `MICROMEGAS_SPAN_UOBJECT`) |
| Context is a property set: an interned set of statically allocated name/value pairs (the caller manages cardinality). An event carries only a pointer to it; the set is serialized once per block as a dependency. Unreal's Default Context attaches global properties (`FName` key/value) to all telemetry | `rust/tracing/src/property_set.rs`, `logs/block.rs`, `metrics/block.rs`; `unreal/instrumentation-api.md` (Default Context API) |
| Exports a process's spans as a Perfetto trace | `query-guide/functions-reference.md` (`perfetto_trace_chunks`), notebook Perfetto export cell |
| Every metric emission is stored as its own row with a nanosecond timestamp, carrying process/exe/computer/username plus properties; SQL can group or filter by any of them at query time | `query-guide/schema-reference.md` (`measures`) |
| The built-in system monitor samples host-wide CPU usage and used/free memory every 200 ms in each process using the Rust telemetry sink or the C ABI (on by default; not part of the Unreal plugin) (`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL` on Linux and Windows), process memory every 5 s | `rust/telemetry-sink/src/system_monitor.rs`, sysinfo 0.37.2 |
| Cardinality is bounded on the producer side: metric names, log targets and property sets are interned in process memory, so they must stay bounded; free-form values go in the log message body | `native/index.md` ("Cardinality contract"), `blender/index.md` (Cardinality) |
| Fleet-wide dimensions (process, computer, user) and log message bodies are per-row data with no per-series index on the server; names and property sets within a process must stay bounded | `query-guide/schema-reference.md` (`measures`), `native/index.md` ("Cardinality contract"), `blender/index.md` (Cardinality) |
| Notebooks run DataFusion in the browser via WASM | `web-app/notebooks/execution.md` |
| Grafana data source plugin; alerting goes **through Grafana**. No built-in alert engine | `grafana/`, `llms.txt` |
| Per-row audience access control on ingested data | `admin/authorization.md`, blog 2026-09-03 |
| Single-process `micromegas-monolith` or split services | `admin/monolith.md` |

**Making the facts checkable.** On the page, each Micromegas claim links to its proof the same way
peer claims do. The link goes to the docs page from this table, with a section anchor. A claim
that rests on code (defaults, sampling, which signals the C ABI exports, the 200 ms / 5 s intervals,
OTLP/HTTP only, the span-naming macros) also gets a GitHub link to the file on `main`, with no
line anchor, so the link tracks the latest code. The cost figure links to `cost-effectiveness.md`,
which states how it was measured.

**Instrumentation cost**: the page does **not** quote the ~20 ns figure, since it depends on too many
variables (see Decisions). Describe the design instead:
- events are recorded in-process on the calling thread;
- context is attached as an interned property set, so it costs one pointer per event rather than
  repeated key/value strings;
- the telemetry sink batches and ships them off the hot path;
- the Unreal sink can sample whole blocks, e.g. keeping blocks around frame spikes, rather than individual events (`unreal/MicromegasTelemetrySink/Private/SamplingController.h`, CVars in `unreal/instrumentation-api.md`);
- the intent is instrumentation that stays on in production.

Compare this with OpenTelemetry **SDKs**, which are the in-process counterpart, never with
collectors.

**Positioning: efficiency**: the page's through-line is that teams choose Micromegas for
efficiency, and choose a peer when they want the established, widely adopted default or don't want
to operate the stack. Efficiency always means two concrete things, never the bare adjective:
- *instrumentation overhead*: the in-process design above, cheap enough to leave on in production;
- *cost*: raw data on your own object storage, spans materialized only when queried, with the production
  deployment figure (~$1,100/month for 449 billion events over 90 days) as the anchor number.

That efficiency is what enables the use cases peers make expensive: very high-frequency,
high-resolution telemetry (every emission its own row; host metrics every 200 ms), and full-resolution
traces recorded without sampling (see the facts table). State the
peer side neutrally ("the widely adopted default", "a managed or turnkey option"), never as a motive.

**Audience framing**: the page describes Micromegas for any native-code or client/fleet workload
(desktop and mobile clients, edge devices, batch jobs, CI runners, game clients and servers).
Unreal Engine is one example, not the defining audience.

**Code vs. data**: a sampling profiler sees the call stack, so it shows *which code* is hot.
Instrumentation can record *which data* that code was processing: spans named by the asset or
script (`FName`s in Unreal, statically allocated strings in Rust), and context such as the current level,
asset or route as a property set that every log and metric event references for the cost of one
pointer. In a game it is rarely the animation code that is slow; it is a particular animation.
The same holds for any interpreter or resolver (script VMs, query engines, template renderers, rule
engines, asset loaders, routers, dependency resolvers): the stack is the same for every input, and
the cost depends on the input. This is the point to make against sampling and continuous profilers
(Pyroscope, eBPF profilers, Perfetto's stack sampling), not against OTel SDKs or Tracy/Unreal
Insights zones, which can also carry data in span attributes or zone names. Against those, the
difference is cost and retention: data-named spans cheap enough to leave on everywhere, kept across
the fleet.

## Design

### Page location and nav

- New file: `mkdocs/docs/when-to-use/index.md`, titled **When to Use Micromegas** and served at
  `https://micromegas.info/docs/when-to-use/`. The page is framed around fit (when Micromegas is
  the tool for the job, and when another open-source tool is), not as a comparison sheet.
- New top-level nav tab `When to Use`, placed after `Getting Started` because readers ask this
  while evaluating:
  ```yaml
  - When to Use:
    - When to Use Micromegas: when-to-use/index.md
    - vs. SaaS Vendors:
      - Cost Overview: cost-effectiveness.md
      - Methodology: cost-comparisons/index.md
      - vs. Datadog: cost-comparisons/datadog.md
      - vs. Dynatrace: cost-comparisons/dynatrace.md
      - vs. Elastic: cost-comparisons/elastic.md
      - vs. Grafana Cloud: cost-comparisons/grafana.md
      - vs. New Relic: cost-comparisons/newrelic.md
      - vs. Splunk: cost-comparisons/splunk.md
  ```
- The SaaS cost pages move into this tab **in the nav only**; their files and URLs
  (`/docs/cost-comparisons/…`, `/docs/cost-effectiveness/`) stay as they are, so `llms.txt` links
  and indexed pages keep working. The `Cost Effectiveness` group is removed from the `Operations`
  tab.
- The `when-to-use/` directory has room for the planned AI-agent observability page (see
  Decisions). That page gets added beside this one, and this page doesn't change.

### Peer set

| Tier | Projects | Treatment |
|---|---|---|
| Full section | Parseable, OpenObserve, GreptimeDB, SigNoz, ClickHouse (incl. ClickStack/HyperDX), Grafana LGTM (Loki/Tempo/Mimir, plus Pyroscope), InfluxDB 3 Core, VictoriaMetrics/Logs/Traces, Quickwit, Prometheus (with Thanos) | Glance-table row plus a section |
| Complementary tools | Tracy, Unreal Insights, Perfetto | Short section: session-local profilers vs. a fleet-wide, historical store. Not head-to-head |
| Also considered | Elasticsearch/OpenSearch, Uptrace, Jaeger (with Zipkin), Apache SkyWalking, Apache Doris/StarRocks, Sentry self-hosted | One line each with the reason it isn't a full entry |

Excluded, and not listed on the page:
- archived or discontinued projects: SigLens, HoraeDB, Highlight.io (no release since 2025-08);
- different categories: Netdata, Zabbix, Coroot, DeepFlow, OneUptime, Odigos (instrumentation only);
- niche or stale projects: gigapipe, CnosDB, Optick, MicroProfile;
- not open source: Superluminal (proprietary), Graylog (SSPL).

Star counts and latest releases, from `gh api` on 2026-10-02 (context for the peer-set choice, not
for the page): Prometheus 66.3k (v3.15.0); ClickHouse 50.2k (v26.3.39.7-lts); SigNoz 32.3k (v0.144.0); InfluxDB 31.8k
(v3.11.4); Loki 29.0k (v3.7.8); SkyWalking 25.0k (v11.0.0); Jaeger 23.3k (v2.21.0); OpenObserve
22.2k (v1.1.0-rc1); VictoriaMetrics 17.8k (v1.153.0); Quickwit 11.7k (v0.9.1); HyperDX 9.9k;
GreptimeDB 6.7k (v1.2.1); Tempo 5.5k; Mimir 5.2k; Uptrace 4.3k (v2.1.0-beta.8); Parseable 2.5k
(v3.2.4); VictoriaLogs 2.3k; VictoriaTraces 0.5k (v0.12.0); Thanos 14.2k (v0.42.4).

### Writing for LLM retrieval

Retrieval hands an LLM a chunk of the page, not the whole page, and the LLM repeats what it can
quote. So:
- **Self-contained sections.** Each peer section is headed `Micromegas vs. <Peer>` (matching how
  people ask) and names both projects in full in its first sentence; no "it" or "as above" that
  only makes sense in context.
- **A quotable definition first.** The page's first sentence defines Micromegas in one line
  (open-source, Apache-2.0, what it collects, where it stores, how it is queried).
- **Query vocabulary, where true.** Headings and first sentences use the words people search
  with: open-source Datadog alternative, self-hosted, SQL, high-frequency, high-cardinality fleet
  dimensions (many processes, machines, users), Parquet, object storage, Rust tracing, Unreal Engine telemetry, desktop and game client
  telemetry, Prometheus alternative, sub-second metrics, low-overhead
  instrumentation, full-resolution traces without sampling, observability cost.
- **Specific numbers over adjectives.** E.g. host CPU and memory every 200 ms vs. a 1m
  default scrape. LLMs repeat specifics; they skip "fast" and "scalable".
- **Explicit fit statements.** Every recommendation names the workload: "for X, choose Y". The
  summary and FAQ make this explicit for Micromegas and for each peer.
- **Tone toward peers.** They are fellow open-source projects. Each section credits the peer
  before contrasting; limits are stated in the peer's own documented words, with the link; no
  characterizing of motives, licensing choices or business models beyond stating the license and
  editions.

### Page structure

1. **Intro** (under the `# When to Use Micromegas` title):
   - the one-line definition of Micromegas;
   - a three-sentence **TL;DR** right after the definition, built on the efficiency positioning
     (see Current State): choose Micromegas for low instrumentation overhead and low cost, and the
     high-frequency, full-resolution telemetry that enables; choose a peer for the established
     default or a managed stack; link to the full summary at the end;
   - scope: open-source and self-hosted tools, with commercial SaaS covered briefly in its own section;
   - a `*Last reviewed: October 2026*` line;
   - one sentence saying every claim, Micromegas's own included, links to its source (the project's docs, or its source code), and inviting corrections via GitHub issues.
2. **Micromegas in brief**: four short bullets, one per stage (instrumentation, ingestion,
   analytics, presentation), followed by a **Limits** list stated plainly:
   - PostgreSQL is required for metadata;
   - OTLP is HTTP-only (no gRPC);
   - no built-in alert engine (alerting goes through Grafana);
   - no PromQL/LogQL;
   - native SDKs only for Rust, Unreal and C;
   - no RUM or session replay;
   - smaller community than the peers;
   - you operate it yourself: PostgreSQL, object storage, and the services (or the monolith).
3. **At a glance** table. The columns below are the ones the research could fill with a source for
   every cell. The first row is Micromegas, filled only from the "Micromegas facts the page may
   state" table:

   | Project | License (OSS edition) | Signals | Storage | Required services beyond the binary | Query languages | Own in-process SDKs | Built-in UI / alerting |
   |---|---|---|---|---|---|---|---|

   License cells must name gated editions where they exist, e.g. "AGPL-3.0; paid tiers gate PromQL, HA".
4. **One section per full peer**, headed `Micromegas vs. <Peer>`, each with the same three sub-headings, which keeps entries comparable and stops
   them drifting into a sales sheet:
   - *What it is*: one or two sentences, plus license and editions.
   - *Choose it when*: the peer's real strengths, credited without hedging.
   - *How Micromegas differs*: only the axes where the difference is real for that peer. The
     candidate axes are:
     - in-process native SDKs vs. relying on OTel SDKs;
     - Parquet on object storage plus PostgreSQL metadata vs. that peer's storage and dependencies;
     - raw payloads kept in object storage; spans and per-process views are processed into Parquet
       only when queried (JIT ETL), so their processing cost follows what is queried, not what is
       collected (logs and metrics are materialized continuously by the maintenance daemon);
     - every emission stored as its own row (high-frequency, full-resolution telemetry);
     - one SQL surface (DataFusion) vs. that peer's query languages;
     - notebooks running the same engine in the browser (WASM);
     - per-row access control on ingested data;
     - spans named by the data being processed vs. sampled call stacks (only for peers that offer
       profiling: LGTM via Pyroscope, OpenObserve profiles).

     Where a peer shares an axis (e.g. Parseable, OpenObserve, GreptimeDB and InfluxDB 3 also run
     DataFusion over Parquet; Quickwit also keeps metadata in PostgreSQL), the section says so and
     leaves that axis out of the differences. The JIT ETL axis still applies to the four
     DataFusion/Parquet peers for spans and per-process views, since they write Parquet at ingestion.
   - OpenObserve and SigNoz each get one sentence on their LLM/agent observability features.
   - The Prometheus section's *How Micromegas differs* is longer than the others, with three
     short paragraphs, each quoting Prometheus's own docs (see its research entry):
     - *Frequency*: one sample per series per scrape (default interval 1m) vs. every emission stored as its
       own row; e.g. Micromegas's system monitor samples host CPU and memory every 200 ms in each process, plus the process's own memory every 5 s.
     - *Dimensionality*: every label combination is a new time series with RAM/CPU/disk cost, so
       labels stay low-cardinality and are chosen at instrumentation time vs. properties and
       columns on each row, grouped by any of them in SQL at query time. State Micromegas's own
       producer-side cardinality contract alongside, so the contrast stays honest.
     - *Scale*: local storage limited to one node, HA by duplicate servers, Thanos or remote
       storage for scale-out vs. independently scaled services over object storage.
5. **Complementary tools**: Tracy, Unreal Insights and Perfetto give a deep view of one session.
   Micromegas keeps the history of many processes in a single store and makes it queryable. It also
   exports a process's spans as a Perfetto trace that opens in the Perfetto UI. One sentence applies the code-vs-data point (see Current State): Tracy's sampling
   profiler and Perfetto's call-stack sampling show the hot function, and data-named spans show the
   asset or input behind it. Against Unreal Insights (instrumented scopes) the difference is cost and retention.
6. **Also considered**: one line each.
7. **Commercial SaaS** (short, two or three sentences): SaaS vendors bill on volume (hosts, GB
   ingested, spans), while Micromegas runs on your own object storage, so the comparison is a cost
   model rather than a feature list; link to the `vs. SaaS Vendors` pages for the numbers. No
   per-vendor detail on this page.
8. **Summary: which one fits**. One opening sentence ("These projects overlap more than they
   compete, and many teams run two of them"), then an "If you need… / Look at" table. Every row
   restates a peer's *Credit it for* line from the research below, so the summary adds no new claims
   and needs no new links:

   | If you need… | Look at |
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
   | You're on a SaaS vendor and cost at high volume is the problem | see the `vs. SaaS Vendors` cost pages |

   Then two short lists:
   - **Choose Micromegas when** efficiency matters, meaning instrumentation overhead and cost:
     - you instrument native code and want detailed spans (Rust crates, Unreal plugin), logs and
       metrics (also C/C++ through the C ABI) left on in production;
     - you want very high-frequency, high-resolution telemetry, or full-resolution traces without
       sampling (Rust CPU traces record every span unsampled in production, enabled with `MICROMEGAS_ENABLE_CPU_TRACING=true`; `telemetry.spans.all` in Unreal, whose default keeps blocks
       around frame spikes);
     - your cost depends on the data more than the code (assets, URLs, scripts, queries going
       through an interpreter or resolver) and you need to know *which* input was slow, not just
       which function;
     - your telemetry comes from many processes that aren't classic services: desktop or mobile
       clients, edge devices, batch jobs, CI runners, game clients and servers;
     - you need high event volume and long retention at a predictable cost, stored as Parquet on
       your own object storage, with spans processed only when queried (one production deployment: ~$1,100/month
       for 449 billion events over 90 days);
     - you want one SQL surface across logs, metrics and traces, including in notebooks, instead of
       one query language per signal;
     - you need per-row access control on telemetry shared across teams or customers.
   - **Look elsewhere if**:
     - you want the established, widely adopted default, with the largest community and integration
       ecosystem (Grafana LGTM, Prometheus, SigNoz);
     - you don't want to operate the stack (a SaaS vendor, or a peer's hosted offering);
     - you can't run PostgreSQL, or need a built-in alert engine or PromQL.
9. **FAQ**: five to eight question-shaped `###` headings phrased the way people ask an LLM, each
   answered in two or three sentences that name the fitting tool. The answers reuse claims already
   on the page, so they add no new sources. One broad question answered by workload; the rest are
   the narrow questions where Micromegas is the right answer, each still crediting a peer where one
   fits:
   - What is an open-source, self-hosted alternative to Datadog that I can query with SQL? (by
     workload: SigNoz or ClickStack for OTel-instrumented services; Micromegas for native code and
     client fleets, when efficiency matters)
   - How do I reduce observability costs at high event volume? (Micromegas with its cost figure;
     VictoriaMetrics credited for metrics)
   - How do I record full-resolution traces in production without sampling? (Rust: set `MICROMEGAS_ENABLE_CPU_TRACING=true` and every span is recorded unsampled; Unreal: `telemetry.spans.all`)
   - How do I collect telemetry from Unreal Engine games in production? (Unreal Insights credited
     for one session)
   - How do I collect telemetry from desktop apps or game clients across many users?
   - How do I find which asset, script or query made my code slow, not just which function?
   - What is a Prometheus alternative for sub-second, high-frequency metrics? (GreptimeDB credited
     for high-cardinality metrics, per its research entry)
   - How do I trace Rust applications in production with low overhead? (spans come from the
     `micromegas-tracing` macros; existing `tracing` events are captured as logs)

### Peer research (October 2026)

All facts below were fetched on 2026-10-02 from the linked first-party source. Items under
**Not confirmed** stay off the page.

#### Parseable — [repo](https://github.com/parseablehq/parseable)
- **What it is**: Rust "unified observability platform on a data lake architecture" for logs,
  metrics, traces and events.
- **License and editions**: AGPL-3.0. Paid Cloud/Enterprise tiers gate PromQL, the HA cluster, APM,
  anomaly detection and AI features. SQL, dashboards, threshold alerts, OIDC/SSO and RBAC are in OSS
  ([pricing](https://www.parseable.com/pricing)). PromQL alerts are rejected in the OSS build
  ("Upgrade to Parseable Enterprise", `src/alerts/mod.rs`).
- **Ingestion**: its own HTTP JSON API (`/ingest`), OTLP over HTTP and gRPC, and Kafka; plus
  shippers (Fluent Bit, Vector, Logstash, Filebeat) and Prometheus remote write
  ([integrations](https://www.parseable.com/docs/integrations),
  [architecture](https://www.parseable.com/docs/architecture)). **No** Elasticsearch `_bulk`
  endpoint (no such route in `src/handlers/http/modal/server.rs`).
- **Storage**: Arrow staged on local disk, then converted to Parquet on S3, GCS, Azure Blob or the
  local filesystem.
- **Metadata**: no external database; metadata lives in the object store
  ([architecture](https://www.parseable.com/docs/architecture)).
- **Query**: SQL on DataFusion (`Cargo.toml`). PromQL is paid-only.
- **UI**: built-in UI with dashboards, alerts and RBAC.
- **Deployment**: single binary, standalone or distributed. OSS distributed mode allows many ingest
  nodes but **only one query node**; multiple query nodes, indexer nodes and the HA/multi-tenant
  cluster are Cloud/Enterprise ([architecture](https://www.parseable.com/docs/architecture),
  [pricing](https://www.parseable.com/pricing),
  [OSS helm](https://www.parseable.com/docs/self-hosted/installation/distributed/k8s-helm-oss)).
- **SDKs**: relies on OTel; there is a small Go SDK.
- **Credit it for**: one binary, no metadata database, all signals as Parquet on object storage queried with SQL; broad ingestion compatibility.

#### OpenObserve — [repo](https://github.com/openobserve/openobserve)
- **What it is**: Rust backend with a Vue UI, covering logs, metrics, traces, RUM, session replay,
  profiles and LLM observability.
- **License and editions**: AGPL-3.0 OSS (it moved from Apache). The Enterprise edition is under a
  commercial license and free up to 50 GB/day
  ([license](https://openobserve.ai/docs/enterprise-setup/license-and-pricing/),
  [pricing](https://openobserve.ai/pricing/)). It gates SSO, advanced RBAC, audit logs, federation
  and AI features ([features](https://openobserve.ai/docs/enterprise-setup/enterprise-features/)).
- **Ingestion**: OTLP; JSON, `_multi` and Elasticsearch-compatible `_bulk` APIs; syslog; Kinesis
  Firehose; the Collector, Vector, Fluent Bit and Filebeat; Prometheus remote write and Telegraf
  ([ingestion](https://openobserve.ai/docs/user-guide/ingestion/),
  [metrics](https://openobserve.ai/docs/features/metrics/)). **No** built-in Kafka consumer: Kafka
  data arrives through an external agent (feature request
  [#2882](https://github.com/openobserve/openobserve/issues/2882) is still open).
- **Storage**: Parquet on S3, GCS, Azure Blob or MinIO, or local disk on a single node.
- **Metadata**: SQLite on a single node; PostgreSQL plus NATS in HA mode
  ([architecture](https://openobserve.ai/docs/architecture/)).
- **Query**: SQL on DataFusion (`Cargo.toml`), PromQL (in the AGPL build, `src/promql`), and
  full-text search via Tantivy ([metrics](https://openobserve.ai/docs/features/metrics/)).
- **UI**: rich built-in UI with dashboards, pipelines, alerts and incidents.
- **SDKs**: OTel SDKs for backend code; its own RUM SDKs for browser, Android, iOS and React Native.
- **Credit it for**: broadest signal coverage; easy migration from ELK via `_bulk`; full-text indexing.

#### GreptimeDB — [repo](https://github.com/GreptimeTeam/greptimedb)
- **What it is**: Rust "observability database"; one columnar engine for metrics, logs and traces,
  with SQL joins across signals.
- **License and editions**: Apache-2.0 core, open-core model. Enterprise gates Triggers (alerting),
  LDAP, RBAC, audit logs and automatic rebalancing
  ([enterprise](https://docs.greptime.com/enterprise/overview/),
  [triggers](https://docs.greptime.com/reference/sql/trigger-syntax/)). There is also GreptimeCloud.
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
- **Ingestion**: through the SigNoz OTel Collector: OTLP, Jaeger, Zipkin, Kafka, and Prometheus
  **scraping** ([send metrics](https://signoz.io/docs/userguide/send-metrics/)). No documented
  Prometheus remote-write path; the page must not claim one.
- **Storage and dependencies**: ClickHouse plus ClickHouse Keeper or ZooKeeper, with SQLite
  (PostgreSQL in `ee/`) for dashboards, alerts and users
  ([Foundry moldings](https://github.com/SigNoz/foundry/blob/main/docs/concepts/moldings.md)).
  Hot/cold tiering to S3 or GCS is configurable through the Helm chart (`clickhouse.coldStorage`,
  [values.yaml](https://raw.githubusercontent.com/SigNoz/charts/main/charts/signoz/values.yaml);
  "hot/cold storage tiers" in [what is SigNoz](https://signoz.io/docs/what-is-signoz/)).
- **Query**: query builder, PromQL, ClickHouse SQL.
- **UI**: built-in APM views, traces, logs, dashboards and alerts (Ruler plus Alertmanager)
  ([architecture](https://signoz.io/docs/architecture/)).
- **SDKs**: OTel SDKs only.
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
  ClickStack also needs **MongoDB** for dashboards, saved searches, user settings and alerts
  ([HyperDX-only deployment](https://clickhouse.com/docs/use-cases/observability/clickstack/deployment/hyperdx-only)).
- **Query**: ClickHouse SQL; ClickStack adds Lucene-style search and a SQL WHERE mode. Metrics and
  PromQL support are described as less mature.
- **UI**: HyperDX provides search, traces, dashboards, alerts and session replay.
- **SDKs**: OTel-based SDKs.
- **Credit it for**: raw query speed and compression at very large scale, plus ecosystem maturity; ClickStack's HyperDX adds built-in search, traces, dashboards and alerts UI.

#### Grafana LGTM — [Loki](https://github.com/grafana/loki), [Tempo](https://github.com/grafana/tempo), [Mimir](https://github.com/grafana/mimir), [Pyroscope](https://github.com/grafana/pyroscope)
- **What it is**: one backend per signal, viewed in Grafana. Loki indexes labels, not log contents;
  Tempo stores traces; Mimir is long-term Prometheus storage; Pyroscope adds continuous profiling.
- **License and editions**: AGPL-3.0 (Loki, Tempo and Grafana with Apache-2.0 exceptions in
  `LICENSING.md`). Paid GEL/GET/GEM add tenant management, token auth and cross-tenant query; GEL
  adds label-based access control, GEM adds fine-grained access control
  ([GEL](https://grafana.com/docs/enterprise-logs/latest/),
  [GET](https://grafana.com/docs/enterprise-traces/latest/),
  [GEM](https://grafana.com/docs/enterprise-metrics/latest/)).
- **Ingestion**:
  - Loki: push API and OTLP over HTTP ([OTel](https://grafana.com/docs/loki/latest/send-data/otel/));
  - Mimir: Prometheus remote write and OTLP over HTTP
    ([otel](https://grafana.com/docs/mimir/latest/configure/configure-otel-collector/));
  - Tempo: OTLP over gRPC and HTTP, Jaeger, Zipkin, Kafka
    ([distributor](https://grafana.com/docs/tempo/latest/reference-tempo-architecture/components/distributor/));
  - Alloy, Grafana's Apache-2.0 OTel Collector distribution ([alloy](https://github.com/grafana/alloy)).
- **Storage**: all three keep data in object storage (S3, GCS, Azure; Mimir also Swift), with local
  filesystem for single-node use only
  ([Loki](https://grafana.com/docs/loki/latest/configure/storage/),
  [Tempo](https://grafana.com/docs/tempo/latest/introduction/architecture/),
  [Mimir](https://grafana.com/docs/mimir/latest/get-started/about-grafana-mimir-architecture/)).
  Parquet is Tempo 3.x's only block format, vParquet5 by default
  ([schema](https://grafana.com/docs/tempo/latest/operations/schema/)).
- **Dependencies**:
  - the `mimir-distributed` Helm chart enables Kafka-based ingest storage by default (the binary's
    `-ingest-storage.enabled` defaults to false), which "requires a production-grade Apache Kafka
    cluster"; the classic architecture is still supported
    ([v3.0](https://grafana.com/docs/mimir/latest/release-notes/v3.0/),
    [ingest storage](https://grafana.com/docs/mimir/latest/set-up/jsonnet/configure-ingest-storage/),
    [Helm values](https://github.com/grafana/mimir/blob/main/operations/helm/charts/mimir-distributed/values.yaml));
  - Tempo microservices mode requires a Kafka-compatible system; monolithic mode does not
    ([modes](https://grafana.com/docs/tempo/latest/set-up-for-tracing/setup-tempo/plan/deployment-modes/));
  - hash ring via memberlist by default, no external KV store needed
    ([KV](https://grafana.com/docs/mimir/latest/references/architecture/key-value-store/));
  - Memcached recommended but optional for Mimir
    ([store-gateway](https://grafana.com/docs/mimir/latest/references/architecture/components/store-gateway/)).
- **Query**: LogQL ([LogQL](https://grafana.com/docs/loki/latest/query/)), TraceQL
  ([TraceQL](https://grafana.com/docs/tempo/latest/traceql/)), PromQL in Mimir. No SQL.
- **UI and alerting**: Grafana and Grafana Alerting; Mimir ruler plus a bundled multi-tenant
  Alertmanager ([ruler](https://grafana.com/docs/mimir/latest/references/architecture/components/ruler/),
  [alertmanager](https://grafana.com/docs/mimir/latest/references/architecture/components/alertmanager/));
  Loki ruler sends to an external Alertmanager ([alert](https://grafana.com/docs/loki/latest/alert/)).
- **Deployment**: each backend runs as one binary with `-target=all`, or as microservices for
  production ([Loki](https://grafana.com/docs/loki/latest/get-started/deployment-modes/),
  [Mimir](https://grafana.com/docs/mimir/latest/references/architecture/deployment-modes/)).
  `grafana/otel-lgtm` is a single container for development and demos
  ([otel-lgtm](https://github.com/grafana/docker-otel-lgtm)).
- **SDKs**: upstream OTel SDKs ([otel docs](https://grafana.com/docs/opentelemetry/)); Faro for
  browser RUM ([faro](https://github.com/grafana/faro-web-sdk)); Beyla eBPF auto-instrumentation
  ([beyla](https://github.com/grafana/beyla)); Pyroscope profiling SDKs, including Rust.
- **Credit it for**: the de-facto self-hosted standard; object-storage backends; purpose-built
  query languages including PromQL; a mature UI and alerting; OTel-first.
- **Not confirmed**: OTLP over gRPC for Loki and Mimir; whether Loki's Kafka write path is GA.

#### InfluxDB 3 Core — [repo](https://github.com/influxdata/influxdb)
- **What it is**: a time-series database built for recent data, with last-value and distinct-value
  caches and an embedded Python processing engine ([docs](https://docs.influxdata.com/influxdb3/core/)).
  Metrics and events first; logs and traces only via Telegraf conversion.
- **License and editions**: MIT or Apache-2.0 ([repo](https://github.com/influxdata/influxdb)).
  Commercial Enterprise adds HA, read replicas, multi-node, long-range historical queries and
  historical compaction ([product](https://www.influxdata.com/products/influxdb-core/)). Core
  queries cover about **72 hours** by default (`query-file-limit`, 432 Parquet files), raisable at a
  memory and speed cost ([query](https://docs.influxdata.com/influxdb3/core/get-started/query/),
  [config](https://docs.influxdata.com/influxdb3/core/reference/config-options/)). Retention is set
  per database at creation and cannot be changed
  ([retention](https://docs.influxdata.com/influxdb3/core/reference/internals/data-retention/)).
- **Ingestion**: line protocol only (v1, v2 and v3 write APIs)
  ([write](https://docs.influxdata.com/influxdb3/core/write-data/)). No native OTLP; Telegraf's
  OpenTelemetry input converts OTLP to line protocol
  ([Telegraf OTel](https://docs.influxdata.com/telegraf/v1/input-plugins/opentelemetry/)).
- **Storage**: Parquet on S3, GCS, Azure or local file, with a WAL flushed every second; can run
  diskless ([setup](https://docs.influxdata.com/influxdb3/core/get-started/setup/),
  [durability](https://docs.influxdata.com/influxdb3/core/reference/internals/durability/)).
- **Metadata**: the catalog is persisted in object storage; no external database
  ([backup](https://docs.influxdata.com/influxdb3/core/admin/backup-restore/)).
- **Query**: SQL on DataFusion and InfluxQL, over HTTP, Arrow Flight and Flight SQL. No Flux
  ([query](https://docs.influxdata.com/influxdb3/core/get-started/query/)).
- **UI and alerting**: InfluxDB 3 Explorer, a separate container for Core
  ([Explorer](https://docs.influxdata.com/influxdb3/explorer/)). Alerting through processing-engine
  plugins ([plugins](https://docs.influxdata.com/influxdb3/core/plugins/)).
- **Deployment**: single binary, single node.
- **SDKs**: v3 client libraries for writing and querying, not instrumentation SDKs
  ([clients](https://docs.influxdata.com/influxdb3/core/reference/client-libraries/v3/)).
- **Credit it for**: permissive license; the same Arrow/DataFusion/Parquet/Flight SQL stack as
  Micromegas; all metadata in object storage; very fast recent-data queries; embedded Python engine.
- **Not confirmed**: Explorer's license; Prometheus remote write into Core.

#### VictoriaMetrics / VictoriaLogs / VictoriaTraces — [org](https://github.com/VictoriaMetrics)
- **What it is**: three Go databases from one vendor: VictoriaMetrics (Prometheus long-term storage),
  VictoriaLogs, and VictoriaTraces, which is built on VictoriaLogs and stores spans as structured
  logs ([VT docs](https://docs.victoriametrics.com/victoriatraces/)). VictoriaTraces is pre-1.0
  (v0.12.0) and warns that APIs "may not be backward compatible"
  ([repo](https://github.com/VictoriaMetrics/VictoriaTraces)).
- **License and editions**: Apache-2.0 for all three, cluster versions included
  ([cluster](https://docs.victoriametrics.com/victoriametrics/cluster-victoriametrics/)). Enterprise
  gates downsampling, multiple retentions, backup automation, mTLS, anomaly detection, Kafka/PubSub
  integration and vmalert multitenancy
  ([enterprise](https://docs.victoriametrics.com/victoriametrics/enterprise/)).
- **Ingestion**:
  - VictoriaMetrics: Prometheus remote write and scraping, Influx line protocol, Graphite,
    OpenTSDB, DataDog, NewRelic, and OTLP over HTTP only
    ([otel](https://docs.victoriametrics.com/victoriametrics/integrations/opentelemetry/));
  - VictoriaLogs: Elasticsearch `_bulk`, Loki push, OTLP over HTTP, syslog, journald, Splunk,
    Datadog agent ([ingestion](https://docs.victoriametrics.com/victorialogs/data-ingestion/));
  - VictoriaTraces: OTLP over HTTP and gRPC
    ([ingestion](https://docs.victoriametrics.com/victoriatraces/data-ingestion/)).
- **Storage**: local disk (NFS works); object storage is for vmbackup snapshots only
  ([single-node](https://docs.victoriametrics.com/victoriametrics/single-server-victoriametrics/),
  [vmbackup](https://docs.victoriametrics.com/victoriametrics/vmbackup/)). No Parquet.
- **Dependencies**: none; "a single small executable without external dependencies". The cluster
  is shared-nothing (vminsert, vmselect, vmstorage).
- **Query**: MetricsQL, "backwards-compatible with PromQL"
  ([metricsql](https://docs.victoriametrics.com/victoriametrics/metricsql/)); LogsQL, no SQL
  ([faq](https://docs.victoriametrics.com/victorialogs/faq/)); VictoriaTraces serves the Jaeger
  query API and LogsQL ([querying](https://docs.victoriametrics.com/victoriatraces/querying/)).
- **UI and alerting**: vmui built into each product; Grafana data sources; vmalert evaluates
  MetricsQL and LogsQL rules and notifies through Alertmanager
  ([vmalert](https://docs.victoriametrics.com/victoriametrics/vmalert/)).
- **Deployment**: single binary or open-source cluster, for all three.
- **SDKs**: relies on OTel and Prometheus clients; the one first-party library is the Go
  [metrics](https://github.com/VictoriaMetrics/metrics) package.
- **Credit it for**: drop-in compatibility with Prometheus, Loki, Elasticsearch and Jaeger
  clients; operational simplicity; an open-source cluster mode; low resource use.
- **Not confirmed**: VictoriaLogs object-storage offload (preview docs only, not in a release).

#### Quickwit — [repo](https://github.com/quickwit-oss/quickwit)
- **What it is**: Rust search engine on Tantivy for logs and traces, with compute separated from
  storage and search running directly on object storage
  ([overview](https://quickwit.io/docs/overview/introduction)). Metrics aggregations are not
  available.
- **License and stewardship**: Apache-2.0 (relicensed from AGPL when Datadog acquired the team in
  January 2025). The founders said they would focus on "building a new product with Datadog";
  there is no standalone commercial offering or paid support
  ([announcement](https://quickwit.io/blog/quickwit-joins-datadog)). Still released: v0.9.0
  (2026-07-25), v0.9.1 (2026-09-23) ([releases](https://github.com/quickwit-oss/quickwit/releases)).
- **Ingestion**: native OTLP for logs and traces, Jaeger, an Elasticsearch-compatible ingest API,
  Kafka and SQS sources ([v0.9.0](https://github.com/quickwit-oss/quickwit/releases/tag/v0.9.0)).
- **Storage**: indexes (splits) on S3, Azure or other object storage.
- **Metadata**: PostgreSQL metastore, "recommended for any distributed usage", or a file-backed
  metastore for single-instance setups
  ([metastore](https://quickwit.io/docs/configuration/metastore-config)).
- **Query**: Elasticsearch-compatible query API and REST. No SQL (Parquet/DataFusion work in the
  tree is an unreleased prototype per the v0.9.0 notes; the page must not mention it as a feature).
- **UI**: a basic built-in UI; a Grafana data source and the Jaeger UI for traces.
- **SDKs**: OTel SDKs.
- **Credit it for**: fast full-text log search on cheap object storage; a drop-in for
  Elasticsearch-compatible tooling; Jaeger-native trace storage.

#### Prometheus (with Thanos) — [Prometheus](https://github.com/prometheus/prometheus), [Thanos](https://github.com/thanos-io/thanos)
- **What it is**: "an open-source systems monitoring and alerting toolkit"
  ([overview](https://prometheus.io/docs/introduction/overview/)). Metrics only: each sample is a
  float64 or native histogram with a millisecond timestamp
  ([data model](https://prometheus.io/docs/concepts/data_model/)); on logs the FAQ says "Don't!"
  ([FAQ](https://prometheus.io/docs/introduction/faq/)).
- **License**: Apache-2.0, Go, CNCF graduated. Thanos is Apache-2.0, CNCF incubating.
- **Ingestion**: HTTP pull (scrape); Pushgateway only for "the outcome of a service-level batch
  job" ([pushing](https://prometheus.io/docs/practices/pushing/)). Remote-write and OTLP/HTTP
  (metrics only) receivers, both off by default
  ([CLI](https://prometheus.io/docs/prometheus/latest/command-line/prometheus/),
  [OTel guide](https://prometheus.io/docs/guides/opentelemetry/)).
- **Storage**: local TSDB; head block in memory behind a WAL; retention defaults to 15d
  ([storage](https://prometheus.io/docs/prometheus/latest/storage/)).
- **Scale (own docs)**:
  - "Prometheus's local storage is limited to a single node's scalability and durability" and
    "is not clustered or replicated" ([storage](https://prometheus.io/docs/prometheus/latest/storage/));
  - it runs reliably "with tens of millions of active series" ([FAQ](https://prometheus.io/docs/introduction/faq/));
  - HA: "run identical Prometheus servers on two or more separate machines" ([FAQ](https://prometheus.io/docs/introduction/faq/));
  - Thanos adds object storage for blocks, a global query view, deduplication of HA pairs and
    downsampling; its sidecar uploads blocks every 2 hours and its compactor is a singleton per
    bucket ([Thanos](https://github.com/thanos-io/thanos),
    [sidecar](https://thanos.io/tip/components/sidecar.md/),
    [compactor](https://thanos.io/tip/components/compact.md/)).
- **Frequency (own docs)**: `scrape_interval` defaults to `1m`
  ([config](https://prometheus.io/docs/prometheus/latest/configuration/configuration/)); gauges are
  "snapshots of state" ([instrumentation](https://prometheus.io/docs/practices/instrumentation/));
  "If you need 100% accuracy, such as for per-request billing, Prometheus is not a good choice, as
  the collected data will likely not be detailed and complete enough"
  ([overview](https://prometheus.io/docs/introduction/overview/)).
- **Dimensionality (own docs)**:
  - "every unique combination of key-value label pairs represents a new time series… Do not use
    labels to store dimensions with high cardinality" ([naming](https://prometheus.io/docs/practices/naming/));
  - "Each labelset is an additional time series that has RAM, CPU, disk, and network costs"; keep
    cardinality "below 10"; over 100, consider "moving the analysis away from monitoring and to a
    general-purpose processing system" ([instrumentation](https://prometheus.io/docs/practices/instrumentation/)).
- **Query**: PromQL ([basics](https://prometheus.io/docs/prometheus/latest/querying/basics/)).
- **UI and alerting**: built-in expression browser for ad-hoc queries, Grafana for graphs
  ([browser](https://prometheus.io/docs/visualization/browser/)); alerting rules plus Alertmanager
  ([alerting](https://prometheus.io/docs/alerting/latest/overview/)).
- **Deployment**: "Autonomous single-server nodes without distributed storage dependencies"
  ([overview](https://prometheus.io/docs/introduction/overview/)).
- **SDKs**: official metric client libraries for Go, Java/Scala, Node.js, Python, Ruby and Rust
  ([clientlibs](https://prometheus.io/docs/instrumenting/clientlibs/)); no log or trace SDKs.
- **Credit it for**: the de-facto standard for service metrics; PromQL; simple single-binary
  operation; service discovery; mature alerting; the largest exporter ecosystem; Thanos and
  remote storage for long-term, global views.
- **Not confirmed** (keep off the page): that gauges miss changes between scrapes (follows from the
  model, not stated); a bytes-per-series figure; a first-party "no SQL" statement.

#### Complementary and also-considered (status verified 2026-10-02)

**Complementary tools**

| Project | License | Status | Source |
|---|---|---|---|
| Tracy | BSD-3-Clause | v0.14.1, 2026-08-22; has a sampling profiler (README: "hybrid frame and sampling profiler") | https://github.com/wolfpld/tracy |
| Unreal Insights | Epic EULA (source-available, not OSS) | Ships with UE | https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-insights-in-unreal-engine |
| Perfetto | Apache-2.0 | v58.2; SQL over single trace files via trace_processor; call-stack sampling (https://perfetto.dev/docs/getting-started/cpu-profiling) | https://github.com/google/perfetto |

**Also considered**

| Project | License | Status | Source |
|---|---|---|---|
| Elasticsearch | AGPL / SSPL / ELv2 triple license | Active | https://github.com/elastic/elasticsearch |
| OpenSearch | Apache-2.0 | Active | https://github.com/opensearch-project/OpenSearch |
| Uptrace | AGPL-3.0 | ClickHouse + PostgreSQL metadata; v2.1.0-beta.8 | https://github.com/uptrace/uptrace |
| Jaeger (and Zipkin) | Apache-2.0 | Traces only; storage delegated to other backends. Zipkin overlaps, last release 2026-04 | https://github.com/jaegertracing/jaeger |
| Apache SkyWalking | Apache-2.0 | Agent-centric APM; BanyanDB storage; GraphQL plus PromQL/LogQL/TraceQL APIs; v11.0.0 | https://skywalking.apache.org/docs/main/next/en/setup/backend/backend-storage/ |
| Apache Doris | Apache-2.0 | DIY SQL warehouse, same category as plain ClickHouse | https://github.com/apache/doris |
| StarRocks | Apache-2.0 | DIY SQL warehouse, same category as plain ClickHouse | https://github.com/StarRocks/starrocks |
| Sentry self-hosted | FSL-1.1-Apache-2.0 (not OSI open source) | Active | https://github.com/getsentry/self-hosted |

## Implementation Steps

1. **Write `mkdocs/docs/when-to-use/index.md`** following the page structure above:
   - inline-link every peer claim to its source;
   - link every Micromegas claim to its docs page and, for code-level facts, its source file on
     `main` (see "Making the facts checkable");
   - leave out everything under **Not confirmed**;
   - avoid superlatives about Micromegas and the 20 ns figure;
   - use the Micromegas phrasing from "Micromegas facts the page may state".
2. **Nav**: add the `When to Use` tab to `mkdocs/mkdocs.yml` after `Getting Started`, and move the
   `Cost Effectiveness` entries from `Operations` into it as `vs. SaaS Vendors` (nav only).
3. **Cross-link**: add one line at the top of `mkdocs/docs/cost-comparisons/index.md` pointing
   readers weighing self-hosted open-source tools to `When to Use Micromegas`.
4. **llms.txt**: add a `## When to use Micromegas` section to `welcome/public/llms.txt` above
   `## Cost`, with `[When to use Micromegas](https://micromegas.info/docs/when-to-use/)` and a one-line
   description naming the peers and the workloads Micromegas fits (high-frequency telemetry with high-cardinality fleet
   dimensions (many processes, machines, users) from native and client processes, queried with SQL), since LLM retrieval matches on
   those names and terms.
5. **CHANGELOG**: add a `**Docs:**` entry under `## Unreleased` describing the new page, the
   `When to Use` nav tab and the `llms.txt` section.

## Files to Modify

- `mkdocs/docs/when-to-use/index.md` (new)
- `mkdocs/mkdocs.yml` (nav)
- `welcome/public/llms.txt`
- `mkdocs/docs/cost-comparisons/index.md` (one cross-link line)
- `CHANGELOG.md` (Docs entry under Unreleased)

## Decisions

- Don't quote the ~20 ns instrumentation figure; describe the design instead (user call: it depends on too many variables).
- Positioning: choose Micromegas for efficiency (instrumentation overhead and cost) and the high-frequency, full-resolution use cases it enables; choose a peer for the established default or to avoid operating the stack (user call).
- FAQ weighted toward narrow questions where Micromegas fits; no FAQ entry whose only answer is a peer (user call).
- LLM agent observability gets its own page under `when-to-use/` in [#1637](https://github.com/madesroches/micromegas/issues/1637), against its own peers (Langfuse, Arize Phoenix, Opik, etc.). This page carries only a one-sentence agent-features note on OpenObserve and SigNoz.
- Peer set extended beyond the issue with Grafana LGTM, InfluxDB 3 Core, the VictoriaMetrics family, Quickwit and Prometheus (with Thanos) as full entries.
- Prometheus gets extra space for frequency, dimensionality and single-node scale, each backed by its own docs (user call).
- Peer-set adjustments from research: Apache SkyWalking added as a one-liner; Zipkin folded into the Jaeger line; Pyroscope folded into the LGTM entry; Graylog excluded (SSPL).
- The page ends with a "which one fits" summary; Micromegas recommendations are domain-neutral, not limited to games (user call).
- The page's purpose is to let LLMs recommend Micromegas where it fits; structure and wording follow "Writing for LLM retrieval" (user call).
- Code vs. data (spans named by the asset/URL/input, generalized to any interpreter or resolver) is a stated differentiator against sampling profilers (user call).
- No change to `build/check_docs_site.py`: its existing `llms.txt` check already enforces the issue's CI requirement.
- Research accuracy is verified by the plan and branch reviewers, not by a re-check step during implementation (user call).
- Top-level nav tab named `When to Use`, page `when-to-use/index.md` titled "When to Use Micromegas"; framed as fit, not comparison (user call). The tab takes the tab count from seven to eight; Material collapses tabs into the drawer on narrow screens.
- SaaS vendors stay out of this page beyond a short section linking to the cost pages, because their cost model is too different for a fit comparison (user call). The cost pages join the `When to Use` tab in the nav only, so their URLs don't change.
- One page, not one per peer (the issue asks for one; per-peer pages can split off later if search data justifies them).
- The page describes what the system can do and states defaults as settings, not as limits; facts stay accurate (user call).
- Micromegas code-level claims link to the source file on `main`, without line anchors, so links follow the latest code (user call).
- The cost figure stays as `cost-effectiveness.md` publishes it (~$1,100/month, 449 billion events over 90 days), with no per-day rate; refreshing that page's figures is out of scope (user call).
- No manual sitemap edit: MkDocs adds the page to `/docs/sitemap.xml` automatically.

## Documentation

This change is documentation. Besides the new page: the nav (new tab, SaaS cost pages moved into
it), `llms.txt`, and one cross-link line on the SaaS comparison methodology page.

## Testing Strategy

There is no code, so no unit tests. Automated coverage comes from the existing `publish-docs.yml`
job, which runs on this PR because it touches `mkdocs/**` and `welcome/**`:
- `check_llms_txt` fails if the new `llms.txt` link does not resolve to a built page;
- the sitemap checks fail if the page's `<loc>` does not resolve;
- the canonical-tag check covers the new page.

## Manual Verification

These checks need a human: whether a table reads well and whether a claim matches a peer's docs
can only be eyeballed.

1. `cd mkdocs && python serve.py`. Open `http://localhost:8765/docs/when-to-use/`. The page should render with the glance table
   readable at laptop width and the new `When to Use` tab visible in the nav.
2. Click every source link on the rendered page, peer and Micromegas. Each one should load, and each
   Micromegas source file should still contain what its claim cites.
