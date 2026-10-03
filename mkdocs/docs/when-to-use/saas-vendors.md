---
description: How Micromegas delivers enterprise-grade observability at a fraction of commercial SaaS cost, with dollar-for-dollar estimates against Datadog, Dynatrace, Elastic, Grafana Cloud, New Relic and Splunk.
---

# Micromegas vs. SaaS Observability Vendors

Micromegas runs on your own infrastructure, so its cost is the direct cost of the cloud services it uses rather than a per-GB, per-host or per-user price. This page explains that cost model, states the methodology shared by every estimate, and compares it with six commercial vendors on one reference workload.

**TL;DR:** on the reference workload (logs and metrics, 90-day retention, 5 users) Micromegas costs about $1,100/month. The six vendors below come out between roughly 1.5× (Dynatrace, with custom metrics excluded) and roughly 20× (Splunk) that figure. All vendor numbers are estimates, not quotes.

*Last reviewed: June 2026*

Weighing self-hosted open-source tools rather than SaaS vendors? See [When to Use Micromegas](index.md).

## At a glance

| Vendor | Priced on | Est. monthly (logs + metrics) | vs. Micromegas ~$1,100 |
| :--- | :--- | :--- | :--- |
| [Datadog](#vs-datadog) | Per host, per GB ingested, per million events indexed | ~$7,950 | ~7× |
| [Dynatrace](#vs-dynatrace) | Memory-hours of monitored hosts, per GiB of logs (DPS rate card) | ~$1,600 (logs + monitoring only) | ~1.5× |
| [Elastic](#vs-elastic) | Per GB ingested (volume-tiered), per GB-month retained | ~$2,200 | ~2× |
| [Grafana Cloud](#vs-grafana-cloud) | Per 1,000 active series, per GB of logs, per user | ~$7,300+ (30-day retention only) | ~6.6× |
| [New Relic](#vs-new-relic) | Per GB ingested, per user seat | ~$6,025 | ~5.5× |
| [Splunk](#vs-splunk) | Daily index volume, billed annually | ~$22,000 | ~20× |

Every estimate excludes traces, and the Dynatrace figure excludes custom metrics at the reference volume; see [Methodology](#methodology).

## Cost Philosophy

Unlike traditional observability platforms that charge per GB ingested, per host, or per user, **Micromegas runs on your own infrastructure**. Your cost is simply the direct cost of the cloud services you consume.

### Why This Matters

- **Full transparency** - See every dollar spent on your cloud bill
- **No vendor margins** - Pay only for actual infrastructure usage
- **Predictable scaling** - Costs scale linearly with resource consumption
- **Data ownership** - Your telemetry data never leaves your cloud account

## Primary Cost Drivers

The infrastructure cost for Micromegas comes from standard cloud services:

### Compute Services

- **Ingestion Service** (`telemetry-ingestion-srv`) - Handles incoming telemetry data
- **Analytics Service** (`flight-sql-srv`) - Serves SQL queries and dashboards
- **Maintenance Daemon** (`telemetry-maintenance-srv`) - Background data processing and rollups

!!! tip "Simplified deployment"
    For smaller deployments or local development, `micromegas-monolith` runs all roles (ingestion, analytics, web, maintenance) in a single process, eliminating the need to manage multiple services.

### Storage Services

- **Database (PostgreSQL)** - Stores metadata about processes, streams, and data blocks
- **Object Storage (S3/GCS)** - Stores raw telemetry payloads and materialized Parquet files. Typically the largest portion of the cost.

### Supporting Infrastructure

- **Load Balancers** - Route traffic to services
- **Networking** - Data transfer and connectivity

Micromegas also supports **native OTLP/HTTP ingestion** (logs, metrics, and traces via the OpenTelemetry protocol), making it straightforward to migrate existing OpenTelemetry pipelines without re-instrumenting your code.

## Example Deployment Cost

Here's a real-world cost breakdown for a production Micromegas deployment on AWS Fargate:

### Data Scale

- **Retention Period:** 90 days
- **Total Storage:** 8.5 TB in 118 million objects
- **Log Entries:** 9 billion
- **Metric Events:** 275 billion
- **Trace Events:** 165 billion

### Monthly Infrastructure Costs

| Component | Specification | Monthly Cost |
|-----------|---------------|-------------|
| **Ingestion Services** | 2 × (1 vCPU, 2 GB) on Fargate | ~$66 |
| **Analytics Service** | 2 × (4 vCPU, 8 GB) on Fargate | ~$288 |
| **Maintenance Daemon** | 1 × (4 vCPU, 8 GB) on Fargate | ~$144 |
| **Analytics Web** | 1 × (0.5 vCPU, 1 GB) on Fargate | ~$18 |
| **Aurora Serverless v2** | 44 GB storage, 0.5–20 ACU (avg 18.7% ≈ 3.74 ACU) | ~$330 |
| **S3 Storage** | 8.5 TB @ $0.023/GB | ~$200 |
| **Application Load Balancer** | Fixed + LCU charges (shared) | ~$25 |
| **Data Transfer** | Minimal (internal) | ~$10 |
| **Total** | | **~$1,100/month** |

Resources are sized for redundancy, not peak utilization — a tighter deployment without redundancy would be cheaper. Conversely, autoscaling can add tasks under sustained load.

### Scale Perspective

This deployment handles:

- **449 billion total events** over 90 days
- **Spikes of ~266 million events per minute**
- **~58,000 events per second** average throughput

## Cost Management Features

### On-Demand Processing (Tail Sampling)

Micromegas supports storing all raw telemetry data in low-cost object storage and materializing it for analysis only when needed:

- **Raw data** stored cheaply in S3/GCS
- **Processing costs** only when querying specific data
- **Selective materialization** based on actual analysis needs

### Flexible Retention Policies

Configure retention periods independently for:

- **Raw telemetry data** - Keep longer in cheap storage
- **Materialized views** - Shorter retention for frequently accessed data
- **Metadata** - Configure based on compliance requirements

## Methodology

This section describes the methodology shared by every vendor comparison below. Each vendor section focuses only on that vendor's pricing.

### Pricing Models

Most commercial observability solutions use one or a combination of the following pricing models:

1.  **Per-GB Ingested:** Charged based on the volume of log, metric, and trace data sent to the platform each month.
    *   **Pros:** Simple to understand initially.
    *   **Cons:** Can lead to unpredictable costs. Often encourages aggressive sampling or dropping data to control costs, potentially losing valuable insights. Costs can spike during incidents — exactly when you need data most.

2.  **Per-Host / Per-Node:** A flat rate for each server, container, or agent monitored.
    *   **Pros:** Predictable monthly costs.
    *   **Cons:** Expensive for dynamic or containerized environments where node count fluctuates. Impractical for widely distributed applications (client-side instrumentation on desktop or mobile) where node count is massive and unpredictable.

3.  **Per-User:** Charged based on the number of users with platform access.
    *   **Pros:** Predictable and easy to manage for small teams.
    *   **Cons:** Discourages widespread access to observability data across an organization. Doesn't scale well as more engineers, SREs, and product managers need access.

4.  **Feature-Based Tiers:** Features bundled into tiers (Basic, Pro, Enterprise). Higher tiers unlock advanced features like longer retention or more sophisticated analytics.
    *   **Pros:** Pay for only the features you need.
    *   **Cons:** You may be forced into a much more expensive tier for a single critical feature. Cost jumps between tiers can be substantial.

Micromegas takes a fundamentally different approach: instead of abstracting away the infrastructure, it runs on your own cloud account (AWS, GCP, Azure), and **your cost is the direct cost of the underlying cloud services you consume.**

| Aspect | Commercial SaaS Platforms | Micromegas |
| :--- | :--- | :--- |
| **Cost Basis** | Abstracted (per-GB, per-host, per-user) | Concrete (direct cloud infrastructure spend) |
| **Transparency** | Opaque. The vendor's margin is built into the price. | Fully transparent. You see every dollar on your cloud bill. |
| **Control** | Limited. You control the data you send, but not the underlying infrastructure or its cost efficiency. | Full control. Fine-tune every component, choose instance types, optimize storage tiers. |
| **Scalability** | Scales automatically, but costs can become unpredictable and grow non-linearly. | Cost scales directly and predictably with resource consumption. |
| **Data Ownership** | Your data is in a third-party system. | Your data never leaves your cloud account. |
| **Cost Management** | Relies on sampling, filtering, and dropping data before ingestion. | Relies on **on-demand processing (tail sampling)**. Keep all raw data in cheap storage and only pay to process what you need. |

### Reference Workload

All comparisons use the same reference workload, based on a real Micromegas production deployment (costed in [Example Deployment Cost](#example-deployment-cost)):

*   **Infrastructure:** 20 hosts/nodes to monitor
*   **Retention:** 90 days (3 months)
*   **Total events over 90-day retention:**
    *   **Logs:** 9 billion log entries
    *   **Metrics:** 275 billion metric data points
    *   **Traces:** 165 billion trace events
*   **Monthly ingestion rate:**
    *   **Logs:** 3 billion log entries/month
    *   **Metrics:** ~92 billion metric data points/month
    *   **Traces:** ~55 billion trace events/month
*   **Users:** 5 active users
*   **Data size assumptions for billing estimates:**
    *   Average log entry size: 500 bytes
    *   Average metric data point size: 100 bytes
    *   Average trace event size: 1 KB

### Why Personnel Costs Are Excluded

All comparisons focus purely on **platform and infrastructure costs** — the numbers that are concrete and verifiable. Personnel costs are excluded from both sides.

Commercial observability platforms are not zero-ops. They require significant engineering effort to:

- Deploy and maintain agents/collectors across infrastructure
- Configure dashboards, alerts, and integrations
- Manage vendor relationships and contracts
- Optimize usage to control costs (sampling strategies, index management)
- Train teams on vendor-specific query languages and UIs
- Handle vendor API changes, deprecations, and migrations

An organization using a fragmented set of off-the-shelf solutions does not spend less on human resources than one running an integrated in-house platform. The operational overhead is simply distributed differently. Rather than trying to estimate and compare these inherently fuzzy costs, all comparisons use the numbers that can be objectively verified.

### The Challenge of Traces

Micromegas is designed to ingest and store a very high volume of raw trace events (165 billion total, or 55 billion per month in our reference workload) and process them on-demand. This is feasible due to its highly compact data representation and columnar storage, which keeps infrastructure costs manageable.

Commercial SaaS tracing solutions are typically priced based on ingested GB or spans, and their architectures are optimized for real-time analysis and high-cardinality indexing. While powerful, this comes at a significantly higher cost per unit, especially for long retention periods.

For 165 billion trace events (equivalent to ~165 TB of raw data at 1 KB/event) with 90-day retention, the estimated cost in a typical SaaS tracing solution would be **prohibitively expensive** — hundreds of thousands of dollars per month. This is why high-volume tracing in SaaS solutions relies heavily on **aggressive sampling**.

*   **SaaS Tracing Reality:** To manage costs, users implement head-based or tail-based sampling, meaning only a small fraction (1–10%) of traces are actually ingested and retained. This sacrifices data completeness for cost control.
*   **Micromegas Tracing Philosophy:** Micromegas retains a significantly larger volume of raw trace data, allowing comprehensive on-demand processing and analysis. This fundamental difference makes a direct dollar-for-dollar comparison for traces misleading — the two approaches optimize for different cost/completeness trade-offs.

For this reason, **all cost comparisons exclude traces** and focus on logs and metrics, where pricing is more directly comparable.

### When Micromegas is Cost Effective

The Micromegas model is particularly advantageous when:

- **High data volumes** - Direct infrastructure costs scale better than per-GB pricing
- **Cost predictability** is critical for budgeting
- **Data governance** requirements favor keeping data in your environment
- **Operational maturity** exists to manage distributed systems
- **Long-term retention** is needed (cheap object storage vs. expensive SaaS retention)

## vs. Datadog

**Disclaimer:** These are estimates, not quotes. Datadog's pricing is complex and modular. Actual costs vary based on specific product usage, negotiated enterprise agreements, and region. **Pricing figures were verified in early 2026 — confirm current rates at [datadoghq.com/pricing](https://www.datadoghq.com/pricing/) before use.**

Datadog's pricing combines per-host infrastructure monitoring with per-GB log ingestion and per-million-event log indexing.

*   **Infrastructure Monitoring (Pro Plan):**
    *   `20 hosts × $15/host/month (annual commitment)`
    *   **Subtotal:** **~$300/month**

*   **Log Management:**
    *   Ingestion: `1,500 GB/month × $0.10/GB = ~$150/month`
    *   Indexing (30-day retention): `3,000 million events/month × $2.50/million = ~$7,500/month`
    *   **Subtotal:** **~$7,650/month**

*   **Total Estimated Monthly Cost (Logs & Infrastructure):** **~$7,950/month**

Note: Log indexing dominates the cost. The effective cost of logs is ~$1.80–$2.94/GB when combining ingestion and indexing. The $2.50/million events indexing rate is approximate — Datadog does not transparently publish per-retention-tier indexing prices.

**Cost comparison.**

| Category | Micromegas | Datadog |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month | ~$7,950/month |
| **Ratio** | **1×** | **~7× more** |

**Qualitative differences.**

*   **Cost Driver:** Datadog's cost is dominated by log indexing fees. For organizations that generate large volumes of structured logs, these costs can escalate quickly.

*   **Platform Philosophy:**
    *   **Datadog** offers distinct, tightly integrated products for logs, metrics, and traces. The underlying data is stored in specialized systems, leading to separate billing for each.
    *   **Micromegas** uses a single, unified storage and query layer for all telemetry types, enabling cross-signal correlation and significantly better storage efficiency.

*   **Cost Complexity:** Datadog's pricing is famously complex and modular. While this offers flexibility, it can lead to unpredictable costs as you enable more features or as usage patterns change. Micromegas's cost maps directly to your cloud bill.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud environment — a critical requirement for many organizations.

Sources: [Datadog Pricing List](https://www.datadoghq.com/pricing/list/), [SigNoz: Datadog Pricing Analysis](https://signoz.io/blog/datadog-pricing/), [Last9: Datadog Pricing Breakdown](https://last9.io/blog/datadog-pricing-all-your-questions-answered/).

## vs. Dynatrace

**Disclaimer:** These are estimates, not quotes. Dynatrace pricing varies based on negotiated enterprise agreements, region, and specific product usage. **Pricing figures were verified in early 2026 — confirm current rates at [dynatrace.com/pricing](https://www.dynatrace.com/pricing/) before use.**

Dynatrace has transitioned from the legacy DDU (Davis Data Unit) model to DPS (Dynatrace Platform Subscription) with a rate card pricing structure. Costs are based on capabilities consumed rather than host units.

*   **Full-Stack Monitoring (memory-based):**
    *   `20 hosts × 8 GB memory × 730 hours/month × $0.01/GiB-hour`
    *   **Subtotal:** **~$1,168/month**

*   **Log Ingest & Process:**
    *   `1,500 GB/month × $0.20/GiB`
    *   **Subtotal:** **~$300/month**

*   **Log Retention (90 days):**
    *   `1,500 GB × 90 days × $0.0007/GiB-day`
    *   **Subtotal:** **~$95/month**

*   **Total Estimated Monthly Cost (Logs + Monitoring):** **~$1,563/month**

**Metric ingestion at extreme volume:** The reference workload includes ~92 billion metric data points per month. At the DPS list rate of $0.15/100k data points, this would cost ~$138,000/month. In practice, standard host metrics are included with Full-Stack monitoring — only custom metrics are billed separately. The 275 billion metric data points in the Micromegas workload include fine-grained instrumentation metrics that would not typically be sent to Dynatrace at this granularity.

**No per-user fees:** Dynatrace includes unlimited users in its DPS model, unlike most competitors.

**Log retention options:** Dynatrace also offers a "Retain with Included Queries" log option at $0.02/GiB-day (up to 35 days), which is significantly more expensive than the usage-based $0.0007/GiB-day option used in this estimate.

**Cost comparison.**

| Category | Micromegas | Dynatrace |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month | ~$1,600/month (logs + monitoring only) |
| **Ratio** | **1×** | **~1.5× more** |

Note: The Dynatrace estimate excludes custom metrics at scale. Including them at the reference workload's volume would dramatically increase the cost.

**Qualitative differences.**

*   **Closest Competitor:** Of all platforms compared, Dynatrace is the closest to Micromegas in cost at this reference workload — but only when custom metrics at extreme volume are excluded from the comparison.

*   **Platform Philosophy:**
    *   **Dynatrace** is known for its highly automated, AI-driven approach to observability, offering deep insights into application performance with minimal manual configuration.
    *   **Micromegas** provides a unified data model for all telemetry types within your own cloud environment, offering greater control and cost efficiency for high-volume data.

*   **Pricing Model:** The DPS rate card offers granular, capability-based pricing. This can be predictable for standard monitoring but diverges dramatically at extreme metric or trace volumes.

*   **Cost at Scale:** The gap between Dynatrace and Micromegas widens as data volume increases. At the reference workload, it's 1.5×. For organizations with high-cardinality custom metrics, the difference can be orders of magnitude.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud account — a critical requirement for many organizations.

Sources: [Dynatrace Rate Card](https://www.dynatrace.com/pricing/rate-card/), [Dynatrace DPS Overview](https://www.dynatrace.com/pricing/dynatrace-platform-subscription/).

## vs. Elastic

**Disclaimer:** These are estimates, not quotes. Actual costs vary based on usage patterns, cloud provider, region, and volume tiers. **Pricing figures reflect the November 2025 Elastic pricing update — confirm current rates at [elastic.co/pricing](https://www.elastic.co/pricing) before use.**

Elastic Cloud Serverless reached GA in December 2024, and pricing was updated in November 2025. The serverless model replaces the older resource-based pricing (RAM + storage) and is now Elastic's primary offering for new customers.

*   **Observability Complete Ingest:**
    *   Volume-tiered pricing ranging from ~$0.60/GB (first 50 GB) down to ~$0.09/GB at high volume
    *   At 10+ TB/month, the blended rate is approximately ~$0.15/GB
    *   `10,700 GB/month × ~$0.15/GB`
    *   **Subtotal:** **~$1,605/month**

*   **Retention (90 days):**
    *   `10,700 GB × 3 months × $0.019/GB-month`
    *   **Subtotal:** **~$610/month**

*   **Total Estimated Monthly Cost:** **~$2,215/month**

Note: The exact volume tiers for Observability ingest are not fully published — Elastic's pricing calculator provides the full breakdown. The ~$0.15/GB blended rate is approximate for this workload.

**Cost comparison.**

| Category | Micromegas | Elastic Cloud (Serverless) |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month | ~$2,200/month |
| **Ratio** | **1×** | **~2× more** |

**Qualitative differences.**

*   **Architectural Philosophy:**
    *   **Elastic** was built around the Lucene search index — exceptionally powerful for log search and text analysis. Metrics and traces support has been built on top of this foundation.
    *   **Micromegas** was designed from the ground up with a unified data model for logs, metrics, and traces. It uses columnar storage (Parquet) and a SQL query engine (DataFusion), which is inherently more efficient for analytical queries and data compression.

*   **Query Language:**
    *   **Elastic** uses KQL (Kibana Query Language) and Lucene query syntax, powerful for text search but requiring domain-specific knowledge.
    *   **Micromegas** uses **SQL**, making it immediately accessible to a broader range of engineers, analysts, and data scientists.

*   **Serverless vs. Self-Hosted:** Elastic Cloud Serverless removes the need to manage cluster sizing and scaling. Micromegas requires managing your own infrastructure but provides full cost transparency and control.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud account, simplifying data governance.

Sources: [Elastic Serverless Observability Pricing](https://www.elastic.co/pricing/serverless-observability), [Elastic Cloud Serverless GA Announcement](https://www.elastic.co/blog/elastic-cloud-serverless-ga), [Elastic Cloud Serverless Pricing Update (Nov 2025)](https://www.elastic.co/blog/elastic-cloud-serverless-pricing-packaging).

## vs. Grafana Cloud

**Disclaimer:** These are estimates, not quotes. Actual costs vary based on usage patterns, active series count, scrape frequency, and negotiated agreements. **Pricing figures were verified in early 2026 — confirm current rates at [grafana.com/pricing](https://grafana.com/pricing/) before use.**

Grafana Cloud Pro pricing is component-based, with separate charges for logs (Loki), metrics (Mimir), and users.

*   **Platform Fee:**
    *   **Subtotal:** **$19/month**

*   **Logs (Loki):**
    *   Pricing includes process ($0.05/GB) + write ($0.40/GB) + retain ($0.10/GB) = $0.50/GB total
    *   This rate includes **30-day retention only**
    *   `1,500 GB/month × $0.50/GB`
    *   **Subtotal:** **~$750/month**

*   **Extended Log Retention (beyond 30 days):**
    *   Grafana Cloud does not publicly list pricing for log retention beyond 30 days — it requires contacting sales
    *   The reference workload requires 90-day retention, so the actual cost would be **higher than shown**
    *   **Subtotal:** **Unknown (contact sales)**

*   **Metrics (Mimir):**
    *   Priced at $6.50 per 1,000 active series (at 1 data point per minute scrape frequency)
    *   Higher scrape frequencies multiply cost proportionally
    *   `1,000,000 active series × $6.50/1k`
    *   **Subtotal:** **~$6,500/month**

*   **Users:**
    *   `5 active users × $8/user`
    *   **Subtotal:** **$40/month**

*   **Total Estimated Monthly Cost (30-day retention only):** **~$7,309/month**

Note: This estimate uses only 30-day log retention. The reference workload requires 90 days — the actual cost with extended retention would be higher. Metric costs dominate at this scale; 1 million active series at $6.50/1k series = $6,500/month.

**Cost comparison.**

| Category | Micromegas | Grafana Cloud Pro |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month | ~$7,300+/month (30-day retention only) |
| **Ratio** | **1×** | **~6.6× more** |

Note: With 90-day log retention (not publicly priced), the actual ratio would be higher.

**Qualitative differences.**

*   **Cost Driver:** Metrics dominate the Grafana Cloud cost at this scale. At $6.50 per 1,000 active series, 1 million active series alone costs $6,500/month — nearly 6× the entire Micromegas deployment.

*   **Open Source Foundation:**
    *   **Grafana Cloud** is built on open-source components (Grafana, Loki, Mimir, Tempo), which means organizations can self-host these components to reduce costs — but at the expense of operational complexity.
    *   **Micromegas** is also open source and self-hosted, but its unified architecture means fewer components to manage.

*   **Retention Transparency:** Grafana Cloud's lack of publicly listed extended retention pricing makes it difficult to accurately estimate costs for workloads requiring 90+ day retention. Micromegas's retention cost is simply the cost of S3 storage.

*   **Ecosystem:** Grafana Cloud benefits from the rich Grafana visualization ecosystem. Micromegas provides SQL-based querying and integrates with standard analytics tools.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud account — a critical requirement for many organizations.

Sources: [Grafana Cloud Pricing](https://grafana.com/pricing/), [Grafana Cloud Logs](https://grafana.com/products/cloud/logs/).

## vs. New Relic

**Disclaimer:** These are estimates, not quotes. Actual costs vary based on data volume, user count, plan type, and negotiated enterprise agreements. **Pricing figures were verified in early 2026 — confirm current rates at [newrelic.com/pricing](https://newrelic.com/pricing) before use.**

New Relic's pricing combines per-GB data ingestion with per-user seat fees. They also offer a newer CCU (Compute Capacity Unit) model, but per-GB + per-seat pricing is still available and widely used.

*   **Data Ingest (Original Data option):**
    *   Logs: `1,500 GB/month`
    *   Metrics: `~9,200 GB/month`
    *   Total: `10,700 GB/month × $0.40/GB`
    *   **Subtotal:** **~$4,280/month**

*   **User Seats (Pro, Full Platform Users, annual):**
    *   `5 users × $349/user/month`
    *   **Subtotal:** **~$1,745/month**

*   **Total Estimated Monthly Cost:** **~$6,025/month**

Note: New Relic also offers a Data Plus option at $0.60/GB with additional features (extended retention, HIPAA eligibility, etc.). The CCU model provides compute-based pricing as an alternative, but its pricing is not transparently published.

**Cost comparison.**

| Category | Micromegas | New Relic |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month | ~$6,025/month |
| **Ratio** | **1×** | **~5.5× more** |

**Qualitative differences.**

*   **Cost Drivers:** Both data ingestion and user seats contribute significantly. At $349/user/month for Pro Full Platform Users, just 5 users cost $1,745/month — more than the entire Micromegas infrastructure.

*   **Platform Philosophy:**
    *   **New Relic** offers a broad, integrated SaaS platform covering APM, infrastructure, logs, and more, with a focus on providing a single pane of glass for observability.
    *   **Micromegas** provides a unified data model within your own cloud environment, offering greater control and cost efficiency for high-volume data.

*   **Pricing Model:** New Relic's combination of data volume and user seats means costs scale on two dimensions. For large teams with high data volumes, both dimensions compound. Micromegas's cost scales only with infrastructure usage.

*   **User Access:** New Relic's per-user pricing can discourage giving broad access to observability data. Micromegas has no per-user fees — anyone in your organization can query the data.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud account — a critical requirement for many organizations.

Sources: [New Relic Pricing](https://newrelic.com/pricing), [SigNoz: New Relic CCU Pricing Analysis](https://signoz.io/blog/new-relic-ccu-pricing-unpredictable-costs/).

## vs. Splunk

**Disclaimer:** These are estimates, not quotes. Splunk Cloud pricing is highly negotiable and not transparently published. Actual costs vary based on daily index volume, negotiated enterprise rates, and region. **Pricing figures are based on AWS Marketplace rates from early 2026 — verify current rates before use.**

Splunk charges based on daily index volume, billed annually. Representative pricing from AWS Marketplace:

| Daily Volume | Annual Cost | Effective $/GB/day/year |
|---|---|---|
| 50 GB/day | $50,000/yr | $1,000 |
| 100 GB/day | $80,000/yr | $800 |

The reference workload generates ~10,700 GB/month ÷ 30 = **~357 GB/day**.

*   **Ingest (~357 GB/day):**
    *   At scale pricing (~$700–$800/GB/day/year): ~$250k–$285k/year
    *   **Estimated Monthly Cost:** **~$22,000/month**

Note: Splunk Cloud pricing is heavily volume-discounted at scale. The AWS Marketplace figures ($50k/yr for 50 GB/day, $80k/yr for 100 GB/day) are representative but actual pricing may vary significantly through direct enterprise negotiations.

**Cisco acquisition context.** Cisco completed its acquisition of Splunk in March 2024 for ~$28 billion. Cisco is working on a "Data Fabric" approach that could affect future pricing. Verify current pricing directly with Splunk before making budget decisions.

**Cost comparison.**

| Category | Micromegas | Splunk Cloud |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month | ~$22,000/month |
| **Ratio** | **1×** | **~20× more** |

**Qualitative differences.**

*   **Cost at Scale:** Splunk is the most expensive option by a wide margin. At ~20× the cost of Micromegas, the difference is primarily driven by Splunk's ingest-based pricing applied to high data volumes.

*   **Platform Maturity:** Splunk has decades of investment in log analysis, security (SIEM), and IT operations. Its SPL (Search Processing Language) is powerful but requires specialized knowledge.

*   **Operational Burden:** Splunk Cloud is a managed SaaS — the vendor handles uptime, scaling, and patching. Micromegas requires managing your own infrastructure but provides full cost transparency.

*   **Control & Transparency:** With Micromegas, you see every dollar on your cloud bill. With Splunk, pricing is opaque and requires annual contract negotiations.

*   **Data Ownership:** Micromegas keeps all telemetry data within your own cloud account, which is a critical advantage for security and data governance.

Sources: [AWS Marketplace: Splunk Cloud](https://aws.amazon.com/marketplace/pp/prodview-jlaunompo5wbw), [Cisco Newsroom: Splunk Acquisition Complete](https://newsroom.cisco.com/c/r/newsroom/en/us/a/y2024/m03/cisco-completes-acquisition-of-splunk.html).
