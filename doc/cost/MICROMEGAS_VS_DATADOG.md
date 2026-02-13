# Cost Comparison: Micromegas vs. Datadog

**Author:** Gemini, a large language model from Google.

**Disclaimer:** This document presents a hypothetical, dollar-for-dollar cost comparison between a self-hosted Micromegas deployment and the Datadog SaaS platform. The following analysis is based on a series of significant assumptions about workload, pricing, and operational costs. **These are estimates, not quotes.** Datadog's pricing is complex and modular. Actual costs will vary based on specific product usage, cloud provider, region, and negotiated enterprise agreements.

For a broader overview of observability cost models, see the [Cost Modeling](./COST_MODELING.md) document.

---

## Core Assumptions for this Comparison

1.  **Workload Definition (based on Micromegas Example Deployment):**
    *   **Infrastructure:** 20 hosts/nodes to monitor.
    *   **Total Events (over 90-day retention):**
        *   **Logs:** 9 billion log entries
        *   **Metrics:** 275 billion metric data points
        *   **Traces:** 165 billion trace events
    *   **Monthly Ingestion Rate (for pricing comparison):**
        *   **Logs:** 3 billion log entries / month (9 billion / 3 months)
        *   **Metrics:** ~92 billion metric data points / month (275 billion / 3 months)
        *   **Traces:** 55 billion trace events / month (165 billion / 3 months)
    *   **Retention:** 90 days for logs and traces (requires extended retention plans).

2.  **Datadog Pricing Assumption:**
    *   We will use the publicly available pricing for the **Datadog Pro** and **Enterprise** plans as of mid-2025, as different products are required. Datadog's pricing is highly modular, with costs varying based on specific product usage and consumption. [1, 2]
    *   Datadog's pricing is highly modular. We will estimate the cost by combining the necessary components.
    *   **Assumption on Datadog Data Size:** To enable a dollar-for-dollar comparison based on events, we must estimate the billable units for Datadog.
        *   Average log entry size for Datadog (after processing): 500 bytes. This is a common approximation for a typical, well-structured log entry after processing, including the message, timestamp, and various attributes/tags. Datadog's API supports log entries up to 1MB, implying that typical entries are significantly smaller. [3]
        *   Average trace event size for Datadog (after processing): 1 KB. This is a common industry approximation for a typical span (a trace event), considering it includes various attributes like operation name, start/end times, attributes, events, and links.

3.  **Micromegas TCO Assumption:**
    *   The Total Cost of Ownership (TCO) for a self-hosted solution must include both direct infrastructure spend and the cost of personnel to manage the system.
    *   **Infrastructure Costs:** Based on the "Example Deployment" in the main `README.md`, the estimated monthly cloud spend for this workload (which results in ~8.5 TB of storage for Micromegas) is **~$1,000 / month**.
    *   **Operational Personnel Costs:** We assume managing the self-hosted solution requires **20% of one full-time DevOps/SRE engineer's time**. At a fully-loaded annual salary of $150,000, this equates to **~$2,500 / month**.

---

### The Challenge of Traces: Why Direct Comparison is Impractical

Micromegas is designed to ingest and store a very high volume of raw trace events (165 billion total, or 55 billion per month in our example) and process them on-demand. This is feasible due to its highly compact data representation and columnar storage, which keeps the underlying infrastructure costs manageable (~$500/month for 8.5 TB of total data, including traces).

Commercial SaaS tracing solutions like Datadog APM, Grafana Tempo, or Elastic APM are typically priced based on ingested GB or spans, and their underlying architectures are optimized for real-time analysis and high-cardinality indexing. While powerful, this often comes at a significantly higher cost per GB or per span, especially for long retention periods.

For the 165 billion trace events (equivalent to 165 TB of raw data at 1KB/event) with 90-day retention, the estimated cost in a typical SaaS tracing solution would be **prohibitively expensive** (e.g., hundreds of thousands of dollars per month). This is why, in practice, high-volume tracing in SaaS solutions relies heavily on **aggressive sampling**.

*   **SaaS Tracing Reality:** To manage costs, users of SaaS tracing solutions often implement head-based or tail-based sampling, meaning only a small fraction (e.g., 1-10%) of traces are actually ingested and retained. This sacrifices data completeness for cost control.
*   **Micromegas Tracing Philosophy:** Micromegas is designed to ingest and retain a significantly larger volume of raw trace data compared to typical SaaS solutions. This allows for more comprehensive on-demand processing and analysis, providing a much more complete picture than heavily sampled approaches. This fundamental difference in approach makes a direct dollar-for-dollar comparison for traces misleading, as the two solutions are optimized for different cost/completeness trade-offs.

---

## Analysis for Hypothetical Workload

### 1. Estimated Monthly Cost: Micromegas (Self-Hosted)

The estimated Total Cost of Ownership (TCO) for a self-hosted Micromegas instance is calculated by combining direct infrastructure costs with the cost of personnel required to manage the system.

*   **Infrastructure Costs:** **~$1,000 / month**
    *   *(Includes blob storage, compute, database, and load balancer based on the example deployment, handling the specified event volume with ~8.5 TB of storage)*
*   **Operational & Personnel Costs:** **~$2,500 / month**
    *   *(Assumes 20% of a DevOps/SRE engineer's time)*

*   **Total Estimated Monthly Cost (TCO):** **~$3,500 / month**

---

### 2. Estimated Monthly Cost: Datadog (Logs & Metrics Only)

The Datadog cost is calculated by summing the costs for its individual products based on the defined workload and assumed data sizes, including the cost of extended retention.

*   **Infrastructure Monitoring:**
    *   `20 hosts * ~$23/host/month (Pro Plan)`
    *   *Estimated Cost: ~$460 / month*

*   **Log Management:**
    *   Ingestion Volume: `3 billion log entries/month * 500 bytes/entry = 1,500 GB/month`
    *   Ingestion Cost: `1,500 GB/month * ~$0.10/GB = ~$150 / month`
    *   Retention Cost (90 days): Datadog charges for log retention beyond 15 days. Assuming `~$2.50 per million log events` for 60 extra days, this is a rough estimate.
    *   `3,000 million log events/month * ~$2.50/million = ~$7,500 / month`
    *   *Estimated Cost (including retention): ~$7,650 / month*

*   **Subtotal (Platform):** **~$8,110 / month**

*   **Operational & Personnel Costs:**
    *   Datadog is a feature-rich but complex platform that requires significant internal expertise to manage effectively. This cost is not zero but is considered part of the value of the SaaS subscription for this comparison.

*   **Total Estimated Monthly Cost (Logs & Metrics):** **~$8,110 / month**

---

## Dollar-for-Dollar Comparison Summary

| Category | Micromegas (Self-Hosted) | Datadog (SaaS) |
| :--- | :--- | :--- |
| **Infrastructure Cost** | ~$1,000 / month | (Included in subscription) |
| **Personnel / Ops Cost** | ~$2,500 / month | (Included in subscription) |
| **Licensing / Subscription** | $0 | ~$8,110 / month |
| **Total Estimated Cost** | **~$3,500 / month** | **~$8,110 / month** |

### Qualitative Differences

This comparison highlights the dramatic cost difference for high-volume telemetry between a self-hosted, compact solution and a feature-rich SaaS platform.

*   **Total Cost of Ownership (TCO):** For the same volume of events, the estimated TCO for Micromegas is **dramatically lower** than for Datadog. This difference is primarily driven by the much more compact data representation and storage efficiency of Micromegas, especially for logs and traces, which directly translates to lower infrastructure costs.

*   **Platform Philosophy:**
    *   **Datadog (Federated but Integrated):** Datadog's platform consists of distinct products for logs, metrics, and traces. While highly integrated for user experience, the underlying data is stored in specialized systems, leading to separate billing for each.
    *   **Micromegas (Unified):** Micromegas uses a single, unified storage and query layer for all telemetry types. This can offer more powerful and fundamental data correlation capabilities and significantly better storage efficiency.

*   **Cost Complexity:** Datadog's pricing model is famously complex and modular. While this offers flexibility, it can also lead to unpredictable and extremely high costs as you enable more features or as usage patterns change, especially for high-volume data. Micromegas's cost is simpler to understand, as it maps directly to your cloud bill.

*   **Control & Data Ownership:** This remains a primary differentiator. Micromegas offers full data ownership and control within your own cloud environment, which is a critical requirement for many organizations.

*   **Cost at Extreme Scale:** The cost dynamics are heavily skewed towards Micromegas at extreme scale due to its superior data compactness. In a real-world scenario, to manage these costs, a Datadog deployment handling such high volumes would rely heavily on aggressive sampling of logs and traces, potentially sacrificing data completeness for cost efficiency.

---

## References

[1] [Datadog Pricing: A Comprehensive Guide | Middleware](https://middleware.io/blog/datadog-pricing)
[2] [Datadog Pricing Main Caveats Explained [Updated for 2025] | SigNoz](https://signoz.io/blog/datadog-pricing)
[3] [Datadog API Reference: Log Submission](https://docs.datadoghq.com/api/latest/logs/#send-logs)
