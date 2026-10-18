---
description: Micromegas's self-hosted cost model compared with Datadog, Dynatrace, Elastic, Grafana Cloud, New Relic and Splunk on one workload measured on a production deployment, with a pricing page per vendor.
---

# Micromegas vs. SaaS Observability Vendors

Micromegas runs on your own infrastructure, so its cost is the direct cost of the cloud services it uses rather than a per-GB, per-host or per-user price. This page explains that cost model, states the methodology shared by every estimate, and compares it with six commercial vendors on one reference workload.

**TL;DR:** on the reference workload (90-day retention, ~100 users) Micromegas costs about $1,100/month at a single cloud provider's public list prices, with all traces included. On logs and metrics alone, the six vendors range from ~5.6× (Elastic) to ~82× (New Relic). With traces priced wherever a vendor publishes a rate, the range is ~48× (Elastic) to ~532× (Datadog). All vendor numbers are estimates, not quotes.

*Last reviewed: October 2026*

Weighing self-hosted open-source tools rather than SaaS vendors? See [When to Use Micromegas](index.md).

## At a glance

| Vendor | Priced on | Logs + metrics | With traces | vs. Micromegas ~$1,100 |
| :--- | :--- | :--- | :--- | :--- |
| [Datadog](../cost-comparisons/datadog.md) | Per host, per GB ingested, per million events indexed | ~$30,800 (30-day logs, custom metrics excl.) | ~$586,000 (15-day span index) | ~28× / ~532× |
| [Dynatrace](../cost-comparisons/dynatrace.md) | Memory-hours of monitored hosts, per GiB of logs and spans (DPS rate card) | ~$25,400 (logs + monitoring, custom metrics excl.) | ~$95,300 | ~23× / ~87× |
| [Elastic](../cost-comparisons/elastic.md) | Per GB ingested (volume-tiered), per GB-month retained | ~$6,200–$10,400 | ~$52,600–$56,800 | ~5.6–9.4× / ~48–52× |
| [Grafana Cloud](../cost-comparisons/grafana.md) | Per 1,000 active series, per GB of logs and traces, per user | ~$25,400 (30-day logs) | ~$183,000 (30-day) | ~23× / ~167× |
| [New Relic](../cost-comparisons/newrelic.md) | Per GB ingested, per user seat | ~$90,200 (30-day logs) | ~$216,500 | ~82× / ~197× |
| [Splunk](../cost-comparisons/splunk.md) | Daily index volume, billed annually | ~$22,100–$24,400 (custom metrics excl.) | n/a (no published rate) | ~20–22× / — |

Rates verified October 2026. The ratio column is logs + metrics / with traces. The Micromegas figure includes all traces. Datadog, Dynatrace and Splunk exclude custom metrics at the reference volume, and Splunk's trace cost is not estimated because no per-volume rate is published; see [Methodology](#methodology).

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
- **Object Storage (S3/GCS/Azure)** - Stores raw telemetry payloads and materialized Parquet files. Typically the largest portion of the cost.

### Supporting Infrastructure

- **Load Balancers** - Route traffic to services
- **Networking** - Data transfer and connectivity

Micromegas also supports **native OTLP/HTTP ingestion** (logs, metrics, and traces via the OpenTelemetry protocol), making it straightforward to migrate existing OpenTelemetry pipelines without re-instrumenting your code.

## Example Deployment Cost

Here's the cost of running the reference workload, measured on a production Micromegas deployment between 2026-09-18 and 2026-10-18.

### Data Scale

- **Retention Period:** 90 days
- **Total Storage:** ~27.5 TB in object storage
- **Log Entries:** ~9.6 billion per month (~29 billion retained)
- **Metric Points:** ~138 billion per month (~415 billion retained)
- **CPU Trace Events:** ~535 billion per month (~1.6 trillion retained)

### Monthly Infrastructure Costs

The table prices resources equivalent to the production deployment using a single cloud provider's public monthly list prices, with no commitment and no negotiated discounts. This is the same footing as the vendors' list prices.

| Component | Equivalent resource | Monthly Cost |
|-----------|---------------------|-------------|
| **Object storage** | ~27.5 TB S3-compatible object storage | ~$512 |
| **Services** (ingestion, analytics, maintenance, web) | 16 vCPU / 32 GiB compute | $336 |
| **PostgreSQL** (metadata) | Managed PostgreSQL, 4 vCPU / 8 GiB, single node | $122 |
| **Object cache** | 2 vCPU / 16 GiB with local NVMe | $131 |
| **Load balancer** | | ~$12 |
| **Total** | | **~$1,100/month** |

The production deployment itself runs on AWS. There it would cost about $2,200/month at AWS on-demand list prices if no rebates of any kind applied (no discounts or Savings Plans).

Caveats:

- PostgreSQL is a single node with no standby.
- Writes to object storage count against the compute transfer allowance; reads within the region are free.
- The sizing is translated from the measured deployment, not separately measured.

### Scale Perspective

This deployment handles:

- **~263,000 events per second** on average
- **Bursts of up to ~240 million trace events per minute** and ~13 million metric points per minute

## Cost Management Features

### On-Demand Processing (Tail Sampling)

Micromegas supports storing all raw telemetry data in low-cost object storage and materializing it for analysis only when needed:

- **Raw data** stored cheaply in S3/GCS/Azure
- **Processing costs** only when querying specific data
- **Selective materialization** based on actual analysis needs

### Flexible Retention Policies

Configure retention periods independently for:

- **Raw telemetry data** - Keep longer in cheap storage
- **Materialized views** - Shorter retention for frequently accessed data
- **Metadata** - Configure based on compliance requirements

## Methodology

This section describes the methodology shared by every vendor page. Each vendor page focuses only on that vendor's pricing.

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

Every estimate follows one rule: per-GB charges use the measured billing sizes from the [reference workload](#reference-workload), while per-event, per-data-point, per-series and per-host charges use counts.

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

All comparisons use the same reference workload, measured on a production Micromegas deployment (costed in [Example Deployment Cost](#example-deployment-cost)):

*   **Infrastructure:** about 400 machines send data concurrently, measured per minute from 2026-09-18 to 2026-10-18 (average ~393, 99th percentile ~435, never above 499). Counting distinct machines per hour gives higher numbers because short-lived machines come and go; the estimates use ~400, which favors host-priced vendors. Memory per host is not recorded, so 8 GiB per host is an assumption (for host-priced products).
*   **Retention:** 90 days (3 months)
*   **Monthly volume:**
    *   **Logs:** ~9.6 billion log entries (~29 billion over 90 days)
    *   **Metrics:** ~138 billion metric data points (~415 billion over 90 days)
    *   **Traces:** ~535 billion CPU trace events (~1.6 trillion over 90 days). Vendors bill spans, not stream events. At a measured ~1.8 events per span, that is ~300 billion spans per month.
*   **Users:** ~100 active users
*   **Custom metrics:** ~1 million series, where a vendor bills per series or custom metric
*   **Left out:** network-trace events (~10.8 billion per month) are not included in any vendor estimate, which favors the vendors.
*   **Measured billing sizes:** the mean serialized JSON document size, including resource attributes (executable, user, host, process properties), sampled on 2026-10-18:
    *   Log entry: ~860 bytes
    *   Metric point: ~940 bytes
    *   Span: ~1,050 bytes
*   **Monthly billable volume:** logs ~8.3 TB (~275 GB/day), metrics ~130 TB, spans ~316 TB.

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

Micromegas is designed to ingest and store a very high volume of raw trace events (~535 billion per month, ~1.6 trillion over 90 days in the reference workload) and process them on-demand. This is feasible due to its highly compact data representation and columnar storage, which keeps infrastructure costs manageable.

Commercial SaaS tracing solutions are priced per ingested GB or per span, and their architectures are optimized for real-time analysis and high-cardinality indexing. This comes at a significantly higher cost per unit.

Each vendor page therefore gives a second figure that adds traces to the logs-and-metrics estimate wherever the vendor publishes a linear (per-GB or per-span) rate. The reference span volume is ~300 billion spans per month, or ~316 TB at the measured span size. Traces are excluded only where no such rate is published, which is the case for Splunk.

These figures price the full volume. Real deployments of these vendors would not ingest it:

*   **SaaS Tracing Reality:** To manage costs, users implement head-based or tail-based sampling, meaning only a small fraction (1–10%) of traces are actually ingested and retained. This sacrifices data completeness for cost control.
*   **Micromegas Tracing Philosophy:** Micromegas retains the full volume of raw trace data, allowing comprehensive on-demand processing and analysis. The two approaches optimize for different cost/completeness trade-offs, so the with-traces figures show what full-volume tracing would cost at each vendor, not what a sampled deployment would pay.

### When Micromegas is Cost Effective

The Micromegas model is particularly advantageous when:

- **High data volumes** - Direct infrastructure costs scale better than per-GB pricing
- **Cost predictability** is critical for budgeting
- **Data governance** requirements favor keeping data in your environment
- **Operational maturity** exists to manage distributed systems
- **Long-term retention** is needed (cheap object storage vs. expensive SaaS retention)

## Pricing References

The Micromegas figure in [Example Deployment Cost](#example-deployment-cost) uses these public list prices from DigitalOcean:

- [Spaces object storage](https://www.digitalocean.com/pricing/spaces-object-storage): $5/month including 250 GiB, then $0.02/GiB
- [Droplets](https://www.digitalocean.com/pricing/droplets): CPU-Optimized 16 vCPU / 32 GiB at $336; Storage-Optimized 2 vCPU / 16 GiB at $131
- [Managed Databases](https://www.digitalocean.com/pricing/managed-databases): PostgreSQL 4 vCPU / 8 GiB at $122.10
- [Load Balancers](https://www.digitalocean.com/pricing/load-balancers): from $12/month
- [Spaces pricing details](https://docs.digitalocean.com/products/spaces/details/pricing/): same-region reads are free
