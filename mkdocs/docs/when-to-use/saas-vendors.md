---
description: Micromegas's self-hosted cost model compared with Datadog, Dynatrace, Elastic, Grafana Cloud, New Relic and Splunk on one reference workload, with per-vendor pricing pages.
---

# Micromegas vs. SaaS Observability Vendors

Micromegas runs on your own infrastructure, so its cost is the direct cost of the cloud services it uses rather than a per-GB, per-host or per-user price. This page explains that cost model, states the methodology shared by every estimate, and compares it with six commercial vendors on one reference workload.

**TL;DR:** on the reference workload (90-day retention, ~100 users) Micromegas costs about $1,100/month. The six vendors range from below that figure (Elastic Serverless at ~0.6×, on logs and metrics alone), through Dynatrace at ~1.5× (with custom metrics excluded), up to roughly 36× (New Relic, driven by per-seat fees). The comparison is heavily skewed in the vendors' favor: the Micromegas figure includes 165 billion trace events, while the vendor estimates cover only logs and metrics, because pricing that trace volume in a SaaS product would not be a realistic comparison (see [The Challenge of Traces](#the-challenge-of-traces)). All vendor numbers are estimates, not quotes.

*Last reviewed: October 2026*

Weighing self-hosted open-source tools rather than SaaS vendors? See [When to Use Micromegas](index.md).

## At a glance

| Vendor | Priced on | Est. monthly (logs + metrics), rates verified October 2026 | vs. Micromegas ~$1,100 |
| :--- | :--- | :--- | :--- |
| [Datadog](../cost-comparisons/datadog.md) | Per host, per GB ingested, per million events indexed | ~$7,950 (custom metrics excluded) | ~7.2× |
| [Dynatrace](../cost-comparisons/dynatrace.md) | Memory-hours of monitored hosts, per GiB of logs (DPS rate card) | ~$1,600 (logs + monitoring only) | ~1.5× |
| [Elastic](../cost-comparisons/elastic.md) | Per GB ingested (volume-tiered), per GB-month retained | ~$660 | ~0.6× |
| [Grafana Cloud](../cost-comparisons/grafana.md) | Per 1,000 active series, per GB of logs, per user | ~$8,000+ (30-day retention only) | ~7.2× |
| [New Relic](../cost-comparisons/newrelic.md) | Per GB ingested, per user seat | ~$39,100 | ~36× |
| [Splunk](../cost-comparisons/splunk.md) | Daily index volume, billed annually | ~$22,000 | ~20× |

Every estimate excludes traces, and the Datadog and Dynatrace figures exclude custom metrics at the reference volume; see [Methodology](#methodology).

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
*   **Users:** ~100 active users
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
