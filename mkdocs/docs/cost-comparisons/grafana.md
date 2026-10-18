---
title: Grafana Cloud Pricing and Cost Breakdown vs. Micromegas
description: Grafana Cloud Pro pricing explained ($6.50 per 1,000 active series, $0.50/GB logs, $8/user) and applied to a ~100-user logs-and-metrics workload, ~$8,000+/month vs. ~$1,100 for self-hosted Micromegas. Rates verified October 2026.
---

# Grafana Cloud Pricing vs. Micromegas

Grafana Cloud Pro bills per 1,000 active metric series, per GB of logs and per active user. On the reference workload the estimate is ~$8,000+/month with 30-day log retention only, about 7.2× the Micromegas figure. This page uses the shared [reference workload](../when-to-use/saas-vendors.md#reference-workload) and [methodology](../when-to-use/saas-vendors.md#methodology); like every comparison on this site, it excludes traces ([why](../when-to-use/saas-vendors.md#the-challenge-of-traces)).

*Rates verified October 2026. Estimates, not quotes — confirm current rates at [grafana.com/pricing](https://grafana.com/pricing/).*

## Grafana Cloud pricing on the reference workload

*   **Platform Fee:** **$19/month**

*   **Logs (Loki):**
    *   $0.50/GB: process $0.05 + write $0.40 + retain $0.10. This rate includes **30-day retention only**, and 50 GB/month is included.
    *   `(1,500 − 50) GB × $0.50/GB`
    *   **Subtotal:** **~$725/month**

*   **Metrics (Mimir):**
    *   $6.50 per 1,000 active series at 1 data point per minute; 10,000 series are included and retention is 13 months. Higher scrape frequencies multiply the cost proportionally.
    *   `(1,000,000 − 10,000) series × $6.50/1k`
    *   **Subtotal:** **~$6,435/month**

*   **Users:**
    *   `(100 − 3 included) users × $8/user`
    *   **Subtotal:** **$776/month**

*   **Total Estimated Monthly Cost (30-day log retention only):** **~$7,955/month**

Retention beyond 30 days is billed per GB "for each additional 30-day increment" at a rate that is not published. The reference workload needs 90 days, so the actual cost would be higher than shown, which is why the figure is written as "~$8,000+".

## What's included and what's billed separately

*   **Included:** 3 users, 50 GB of logs, 10,000 metric series, 30-day log retention and 13-month metric retention.
*   **Billed separately:** additional users at $8 each (users of Enterprise plugins cost $55), log retention beyond 30 days, and traces.
*   **Traces:** billed at the same $0.50/GB split as logs, with 50 GB included; not in this estimate.
*   **Enterprise plan:** starts at a $25k/year commit, with metrics "as low as $3/1k series".

## Cost comparison

| Category | Micromegas | Grafana Cloud Pro |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month | ~$8,000+/month (30-day log retention only) |
| **Ratio** | **1×** | **~7.2× more** |

With 90-day log retention (price not published), the actual ratio would be higher.

## Qualitative differences

*   **Cost Driver:** Metrics dominate the Grafana Cloud cost at this scale. At $6.50 per 1,000 active series, 1 million active series costs about $6,435/month — nearly 6× the entire Micromegas deployment.

*   **Open Source Foundation:**
    *   **Grafana Cloud** is built on open-source components (Grafana, Loki, Mimir, Tempo), which means organizations can self-host these components to reduce costs — but at the expense of operational complexity.
    *   **Micromegas** is also open source and self-hosted, but its unified architecture means fewer components to manage.

*   **Retention Transparency:** Grafana Cloud does not publish extended retention pricing, which makes it difficult to estimate costs for workloads requiring 90+ day retention. Micromegas's retention cost is simply the cost of S3 storage.

*   **Ecosystem:** Grafana Cloud benefits from the rich Grafana visualization ecosystem. Micromegas provides SQL-based querying and integrates with standard analytics tools.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud account — a critical requirement for many organizations.

## Sources

[Grafana Cloud Pricing](https://grafana.com/pricing/), [Understand your Grafana Cloud invoice](https://grafana.com/docs/grafana-cloud/cost-management-and-billing/understand-your-invoice/). Back to [all SaaS vendor comparisons](../when-to-use/saas-vendors.md#at-a-glance).
