# Comparison Methodology

*Last reviewed: February 2026*

This page describes the shared methodology used across all cost comparisons. Each individual comparison page references this methodology and focuses only on the competitor-specific pricing analysis.

---

## Common Pricing Models in Commercial Observability Platforms

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

---

## The Micromegas Cost Model: Direct Infrastructure Cost

Micromegas takes a fundamentally different approach. Instead of abstracting away the infrastructure, it runs on your own cloud account (AWS, GCP, Azure).

**Your cost is the direct cost of the underlying cloud services you consume.**

As detailed in the [Cost Effectiveness overview](../cost-effectiveness.md), these costs are primarily:

*   **Object Storage (S3, GCS):** Storing raw telemetry data and materialized views. Typically the largest portion of the cost.
*   **Compute (Fargate, Kubernetes):** Running the ingestion, analytics, and daemon services.
*   **Database (PostgreSQL, Aurora):** Storing metadata.
*   **Networking (Load Balancers, Data Transfer):** Routing traffic and moving data.

### Comparison of Philosophies

| Aspect | Commercial SaaS Platforms | Micromegas |
| :--- | :--- | :--- |
| **Cost Basis** | Abstracted (per-GB, per-host, per-user) | Concrete (direct cloud infrastructure spend) |
| **Transparency** | Opaque. The vendor's margin is built into the price. | Fully transparent. You see every dollar on your cloud bill. |
| **Control** | Limited. You control the data you send, but not the underlying infrastructure or its cost efficiency. | Full control. Fine-tune every component, choose instance types, optimize storage tiers. |
| **Scalability** | Scales automatically, but costs can become unpredictable and grow non-linearly. | Cost scales directly and predictably with resource consumption. |
| **Data Ownership** | Your data is in a third-party system. | Your data never leaves your cloud account. |
| **Cost Management** | Relies on sampling, filtering, and dropping data before ingestion. | Relies on **on-demand processing (tail sampling)**. Keep all raw data in cheap storage and only pay to process what you need. |

---

## Reference Workload

All comparisons use the same reference workload, based on a real Micromegas production deployment:

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

---

## Micromegas Baseline Cost

The Micromegas reference deployment runs on **AWS Fargate** with the following infrastructure:

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

---

## Why Personnel Costs Are Excluded

All comparisons focus purely on **platform and infrastructure costs** — the numbers that are concrete and verifiable. Personnel costs are excluded from both sides.

**Rationale:** Commercial observability platforms are not zero-ops. They require significant engineering effort to:

- Deploy and maintain agents/collectors across infrastructure
- Configure dashboards, alerts, and integrations
- Manage vendor relationships and contracts
- Optimize usage to control costs (sampling strategies, index management)
- Train teams on vendor-specific query languages and UIs
- Handle vendor API changes, deprecations, and migrations

An organization using a fragmented set of off-the-shelf solutions does not spend less on human resources than one running an integrated in-house platform. The operational overhead is simply distributed differently. Rather than trying to estimate and compare these inherently fuzzy costs, all comparisons use the numbers that can be objectively verified.

---

## The Challenge of Traces

Micromegas is designed to ingest and store a very high volume of raw trace events (165 billion total, or 55 billion per month in our reference workload) and process them on-demand. This is feasible due to its highly compact data representation and columnar storage, which keeps infrastructure costs manageable.

Commercial SaaS tracing solutions are typically priced based on ingested GB or spans, and their architectures are optimized for real-time analysis and high-cardinality indexing. While powerful, this comes at a significantly higher cost per unit, especially for long retention periods.

For 165 billion trace events (equivalent to ~165 TB of raw data at 1 KB/event) with 90-day retention, the estimated cost in a typical SaaS tracing solution would be **prohibitively expensive** — hundreds of thousands of dollars per month. This is why high-volume tracing in SaaS solutions relies heavily on **aggressive sampling**.

*   **SaaS Tracing Reality:** To manage costs, users implement head-based or tail-based sampling, meaning only a small fraction (1–10%) of traces are actually ingested and retained. This sacrifices data completeness for cost control.
*   **Micromegas Tracing Philosophy:** Micromegas retains a significantly larger volume of raw trace data, allowing comprehensive on-demand processing and analysis. This fundamental difference makes a direct dollar-for-dollar comparison for traces misleading — the two approaches optimize for different cost/completeness trade-offs.

For this reason, **all cost comparisons exclude traces** and focus on logs and metrics, where pricing is more directly comparable.

---

## Detailed Comparisons

*   [Micromegas vs. Datadog](./datadog.md)
*   [Micromegas vs. Dynatrace](./dynatrace.md)
*   [Micromegas vs. Elastic Observability](./elastic.md)
*   [Micromegas vs. Grafana Cloud](./grafana.md)
*   [Micromegas vs. New Relic](./newrelic.md)
*   [Micromegas vs. Splunk](./splunk.md)
