---
title: Grafana Cloud Pricing and Cost Breakdown vs. Micromegas
description: Grafana Cloud Pro pricing explained ($6.50 per 1,000 active series, $0.50/GB logs and traces, $8/user) and applied to a measured production workload, ~$25,400/month for logs and metrics (30-day log retention only) and ~$183,000/month with traces, vs. ~$1,100 for self-hosted Micromegas. Rates verified October 2026.
---

# Grafana Cloud Pricing vs. Micromegas

Grafana Cloud Pro bills per 1,000 active metric series, per GB of logs and per active user. On the reference workload the estimate is ~$25,400/month for logs and metrics with 30-day log retention only, about 23× the Micromegas figure. The with-traces figure adds the reference span volume at $0.50/GB (30-day retention) and comes to ~$183,000/month, about 167×. This page uses the shared [reference workload](../when-to-use/saas-vendors.md#reference-workload) and [methodology](../when-to-use/saas-vendors.md#methodology); see [The Challenge of Traces](../when-to-use/saas-vendors.md#the-challenge-of-traces) for how traces are priced.

*Rates verified October 2026. Estimates, not quotes — confirm current rates at [grafana.com/pricing](https://grafana.com/pricing/).*

## Grafana Cloud pricing on the reference workload

*   **Platform Fee:** **$19/month**

*   **Logs (Loki):**
    *   $0.50/GB: process $0.05 + write $0.40 + retain $0.10. This rate includes **30-day retention only**, and 50 GB/month is included.
    *   `(8,261 − 50) GB × $0.50/GB`
    *   **Subtotal:** **~$4,106/month**

*   **Metrics (Mimir):**
    *   $6.50 per 1,000 active series at 1 data point per minute; 10,000 series are included and retention is 13 months. Higher scrape frequencies multiply the cost proportionally. Billing uses the greater of active series or total data points per minute divided by the included data points per minute.
    *   ~138.3 billion points per month is ~3.16 million points per minute, so ~3.16 million billable series.
    *   `(3,157 − 10 included) thousand series × $6.50/1k`
    *   **Subtotal:** **~$20,456/month**

*   **Users:**
    *   `(100 − 3 included) users × $8/user`
    *   **Subtotal:** **$776/month**

*   **Total Estimated Monthly Cost (30-day log retention only):** **~$25,356/month**, or ~$25,400

Retention beyond 30 days is billed per GB "for each additional 30-day increment" at a rate that is not published. The reference workload needs 90 days, so the actual cost would be higher than shown.

**With traces: ~$183,000/month.** The span volume is ~315,837 GB per month, billed at the same $0.50/GB as logs with 30-day retention: `(315,837 − 50) GB × $0.50/GB` = **~$157,894/month**, for a total of **~$183,250/month**.

## What's included and what's billed separately

*   **Included:** 3 users, 50 GB of logs, 10,000 metric series, 30-day log retention and 13-month metric retention.
*   **Billed separately:** additional users at $8 each (users of Enterprise plugins cost $55), log retention beyond 30 days, and traces.
*   **Traces:** billed at the same $0.50/GB split as logs, with 50 GB included; priced in the with-traces figure above, not in the logs-and-metrics estimate.
*   **Enterprise plan:** starts at a $25k/year commit, with metrics "as low as $3/1k series".

## Cost comparison

| Category | Micromegas | Grafana Cloud Pro |
| :--- | :--- | :--- |
| **Logs + metrics** | ~$1,100/month (includes all traces) | ~$25,400/month (30-day log retention only) |
| **With traces** | ~$1,100/month | ~$183,000/month (30-day) |
| **Ratio** | **1×** | **~23× / ~167×** |

With 90-day log retention (price not published), the actual ratios would be higher.

## Qualitative differences

*   **Cost Driver:** Metrics dominate the logs-and-metrics cost at this scale. The reference workload's ~3.16 million points per minute bill as ~3.16 million series, about $20,456/month — roughly 19× the entire Micromegas deployment. With traces, span ingestion at $0.50/GB dominates.

*   **Open Source Foundation:**
    *   **Grafana Cloud** is built on open-source components (Grafana, Loki, Mimir, Tempo), which means organizations can self-host these components to reduce costs — but at the expense of operational complexity.
    *   **Micromegas** is also open source and self-hosted, but its unified architecture means fewer components to manage.

*   **Retention Transparency:** Grafana Cloud does not publish extended retention pricing, which makes it difficult to estimate costs for workloads requiring 90+ day retention. Micromegas's retention cost is simply the cost of S3 storage.

*   **Ecosystem:** Grafana Cloud benefits from the rich Grafana visualization ecosystem. Micromegas provides SQL-based querying and integrates with standard analytics tools.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud account — a critical requirement for many organizations.

## Sources

[Grafana Cloud Pricing](https://grafana.com/pricing/), [Understand your Grafana Cloud invoice](https://grafana.com/docs/grafana-cloud/cost-management-and-billing/understand-your-invoice/). Back to [all SaaS vendor comparisons](../when-to-use/saas-vendors.md#at-a-glance).
