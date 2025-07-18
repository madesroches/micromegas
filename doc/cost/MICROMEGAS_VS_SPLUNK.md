# Cost Comparison: Micromegas vs. Splunk

**Author:** Gemini, a large language model from Google.

**Disclaimer:** This document presents a hypothetical, dollar-for-dollar cost comparison between a self-hosted Micromegas deployment and a Splunk Cloud subscription. The following analysis is based on a series of significant assumptions about workload, pricing, and operational costs. **These are estimates, not quotes.** Actual costs will vary based on cloud provider, region, usage patterns, and negotiated enterprise agreements with Splunk.

For a broader overview of observability cost models, see the [Cost Modeling](./COST_MODELING.md) document.

---

## Core Assumptions for this Comparison

1.  **Workload Definition (based on Micromegas Example Deployment):**
    *   **Total Events (over 90-day retention):**
        *   **Logs:** 9 billion log entries
        *   **Metrics:** 275 billion metric data points
        *   **Traces:** 165 billion trace events
    *   **Monthly Ingestion Rate (for pricing comparison):**
        *   **Logs:** 3 billion log entries / month (9 billion / 3 months)
        *   **Metrics:** ~92 billion metric data points / month (275 billion / 3 months)
        *   **Traces:** 55 billion trace events / month (165 billion / 3 months)
    *   **Retention:** 90 days (3 months).

2.  **Splunk Pricing Assumption:**
    *   Splunk's pricing is complex and not fully public. For this analysis, we assume an ingest-based pricing model for Splunk Cloud. This is supported by Splunk's official pricing page, which states: "Pay based on the amount of data you bring into the Splunk Platform." [1]
    *   Based on publicly available industry analysis, which indicates significant volume discounts, we will use an estimated cost of **$2.25 per GB of ingested data per month** for this high-volume workload. This is a critical assumption, as actual costs can vary significantly based on negotiated enterprise rates. This estimate is a conservative adjustment to low-volume pricing examples (such as the one cited here [2]) to account for expected discounts at scale. [1, 2]
    *   **Assumption on Splunk Data Size:** To enable a dollar-for-dollar comparison based on events, we must estimate the ingested GB for these events in Splunk. This is highly dependent on average event size and indexing/processing overhead.
        *   Average log entry size for Splunk (after indexing/overhead): 500 bytes
        *   Average metric data point size for Splunk: 100 bytes. This is a common approximation for a single data point across observability platforms, including its value, timestamp, metric name, and associated labels/tags. For example, Datadog's API documentation suggests that a metric data point, including its timestamp (8 bytes), value (8 bytes), metric name (approx. 20 bytes), and typical labels/tags (approx. 50 bytes of overhead per data point for unique identification), sums up to around 100 bytes when considering additional overhead. [3]
        *   Average trace event size for Splunk: 1 KB. While Splunk does not provide an exact average, this is a common industry approximation for a typical span (a trace event), considering it includes various attributes like operation name, start/end times, attributes, events, and links. Splunk APM has a maximum span size limit of 64KB, implying that typical spans are significantly smaller. [4]

3.  **Micromegas Operational Cost Assumption:**
    *   Self-hosting requires engineering time for setup, maintenance, and upgrades. This is a real cost.
    *   We assume this requires **20% of one full-time DevOps/SRE engineer's time**.
    *   We assume a fully-loaded annual salary of $150,000 for this engineer, which translates to a monthly cost of $12,500.

---

### The Challenge of Traces: Why Direct Comparison is Impractical

Micromegas is designed to ingest and store a very high volume of raw trace events (165 billion total, or 55 billion per month in our example) and process them on-demand. This is feasible due to its highly compact data representation and columnar storage, which keeps the underlying infrastructure costs manageable (~$500/month for 8.5 TB of total data, including traces).

Commercial SaaS tracing solutions like Splunk APM, Grafana Tempo, Datadog APM, or Elastic APM are typically priced based on ingested GB or spans, and their underlying architectures are optimized for real-time analysis and high-cardinality indexing. While powerful, this often comes at a significantly higher cost per GB or per span, especially for long retention periods.

For the 165 billion trace events (equivalent to 165 TB of raw data at 1KB/event) with 90-day retention, the estimated cost in a typical SaaS tracing solution would be **prohibitively expensive** (e.g., hundreds of thousands of dollars per month). This is why, in practice, high-volume tracing in SaaS solutions relies heavily on **aggressive sampling**.

*   **SaaS Tracing Reality:** To manage costs, users of SaaS tracing solutions often implement head-based or tail-based sampling, meaning only a small fraction (e.g., 1-10%) of traces are actually ingested and retained. This sacrifices data completeness for cost control.
*   **Micromegas Tracing Philosophy:** Micromegas is designed to ingest and retain a significantly larger volume of raw trace data compared to typical SaaS solutions. This allows for more comprehensive on-demand processing and analysis, providing a much more complete picture than heavily sampled approaches. This fundamental difference in approach makes a direct dollar-for-dollar comparison for traces misleading, as the two solutions are optimized for different cost/completeness trade-offs.

---

## Analysis for Hypothetical Workload

### 1. Estimated Monthly Cost: Micromegas (Self-Hosted)

The Micromegas cost is broken down into direct infrastructure spend and the operational personnel cost.

*   **Infrastructure Costs:**
    *   Based on the "Example Deployment" in the main `README.md`, the estimated monthly cloud spend for this workload is **~$1,000 / month**.
        *   *(Includes blob storage, compute, database, and load balancer)*

*   **Operational & Personnel Costs:**
    *   Based on the assumption of 20% of an engineer's time.
    *   `$12,500/month * 0.20`
    *   **Subtotal (Personnel):** **~$2,500 / month**

*   **Total Estimated Monthly Cost (TCO):** **~$3,500 / month**

---

### 2. Estimated Monthly Cost: Splunk Cloud (Logs & Metrics Only)

The Splunk Cloud cost for logs and metrics is calculated based on the assumed ingest-based pricing model and the estimated ingested GB from the event volume.

*   **Calculated Ingested GB for Splunk (Logs & Metrics):**
    *   Logs: `3 billion log entries/month * 500 bytes/entry = 1,500 GB/month`
    *   Metrics: `~92 billion metric data points/month * 100 bytes/point = ~9,200 GB/month`
    *   **Total Ingested GB (Logs & Metrics):** `1,500 + 9,200 = 10,700 GB/month`

*   **Ingestion Cost:**
    *   `10,700 GB/month * $2.25/GB`
    *   **Subtotal (Ingestion):** **~$24,075 / month**

*   **Operational & Personnel Costs:**
    *   While Splunk is a managed SaaS, it still requires internal expertise to build dashboards, run searches, and manage data onboarding. This cost is highly variable but generally lower than managing a full self-hosted solution. For this comparison, we will consider it part of the subscription's value.

*   **Total Estimated Monthly Cost (Logs & Metrics):** **~$24,075 / month**

---

## Dollar-for-Dollar Comparison Summary

| Category | Micromegas (Self-Hosted) | Splunk Cloud (SaaS) |
| :--- | :--- | :--- |
| **Infrastructure Cost** | ~$1,000 / month | (Included in subscription) |
| **Personnel / Ops Cost** | ~$2,500 / month | (Included in subscription) |
| **Licensing / Subscription** | $0 | ~$24,075 / month |
| **Total Estimated Cost** | **~$3,500 / month** | **~$24,075 / month** |

### Qualitative Differences

Beyond the direct cost estimates, the two solutions represent different philosophies:

*   **Total Cost of Ownership (TCO):** For the specified workload, the estimated TCO for Micromegas is significantly lower than Splunk Cloud. The primary driver is paying direct infrastructure costs versus a bundled SaaS price that includes the vendor's margin.
*   **Operational Burden:** Micromegas carries a higher operational burden. You are responsible for the uptime, scaling, and maintenance of the system. Splunk, as a SaaS, handles this for you.
*   **Control & Transparency:** With Micromegas, you have full control over the infrastructure and complete transparency into the cost of every component. You can fine-tune instance types and storage classes to optimize costs. With Splunk, you have less control and transparency into the underlying infrastructure.
*   **Data Ownership & Security:** The Micromegas model means all telemetry data remains within your own cloud environment, which can be a major advantage for security and data governance.
*   **Scalability:** Both solutions are designed to scale. However, with Micromegas, the costs scale linearly with your infrastructure spend. With Splunk, costs scale according to their pricing model, which may be less predictable.

---

## References

[1] [Splunk Pricing | Splunk](https://www.splunk.com/en_us/products/pricing.html)
[2] [Guide to Splunk Pricing and Costs in 2025 | Uptrace](https://uptrace.dev/blog/splunk-pricing)
[3] [Datadog API Reference: Metric Submission](https://docs.datadoghq.com/api/latest/metrics/#submit-metrics)

