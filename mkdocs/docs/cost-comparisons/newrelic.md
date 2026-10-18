---
title: New Relic Pricing and Cost Breakdown vs. Micromegas
description: New Relic pricing explained ($0.40/GB ingest, $349/month per Full Platform user) and applied to a measured production workload, ~$90,200/month for logs and metrics (30-day log retention) and ~$216,500/month with traces, vs. ~$1,100 for self-hosted Micromegas. Rates verified October 2026.
---

# New Relic Pricing vs. Micromegas

New Relic bills per GB of data ingested and per user seat. On the reference workload the estimate is ~$90,200/month for logs and metrics with 30-day log retention, about 82× the Micromegas figure, split between data ingest and per-seat fees. The with-traces figure adds the reference span volume at $0.40/GB and comes to ~$216,500/month, about 197×. This page uses the shared [reference workload](../when-to-use/saas-vendors.md#reference-workload) and [methodology](../when-to-use/saas-vendors.md#methodology); see [The Challenge of Traces](../when-to-use/saas-vendors.md#the-challenge-of-traces) for how traces are priced.

*Rates verified October 2026. Estimates, not quotes — confirm current rates at [newrelic.com/pricing](https://newrelic.com/pricing).*

## New Relic pricing on the reference workload

*   **Data Ingest (Original Data option, 100 GB/month free, 30-day retention):**
    *   Logs: `8,261 GB/month`; metrics: `129,980 GB/month`; total ~138,241 GB/month
    *   `(8,261 + 129,980 − 100) GB × $0.40/GB`
    *   **Subtotal:** **~$55,257/month**

*   **User Seats (Pro, Full Platform Users, annual):**
    *   `100 users × $349/user/month` ($418.80 month-to-month)
    *   **Subtotal:** **~$34,900/month**

*   **Total Estimated Monthly Cost:** **~$90,157/month**, or ~$90,200

**With traces: ~$216,500/month.** The span volume is ~315,837 GB per month at the same $0.40/GB: `315,837 GB × $0.40/GB` = **~$126,335/month**, for a total of **~$216,492/month**.

## What's included and what's billed separately

*   **Included:** 100 GB/month of free ingest, and free Basic users.
*   **Billed separately:** Full Platform users at $349/month (Pro, annual); Core users at $49/month; data beyond the free 100 GB.
*   **Data Plus:** $0.60/GB, with additional features such as extended retention (up to 120 days) and HIPAA eligibility. Original Data keeps logs only 30 days, so the 90-day reference retention needs Data Plus: `(138,241 − 100) GB × $0.60 = ~$82,885/month` for data, ~$117,800/month in total with seats (~107× Micromegas).
*   **Logs, metrics and traces** are all billed per GB at the same rate, with no host or custom-metric charges. Traces are priced in the with-traces figure above and are not in the logs-and-metrics estimate.
*   **CCU pricing:** New Relic also offers a Compute Capacity Unit (CCU) model, but no rates are published.

## Cost comparison

| Category | Micromegas | New Relic |
| :--- | :--- | :--- |
| **Logs + metrics** | ~$1,100/month (includes all traces) | ~$90,200/month (30-day log retention) |
| **With traces** | ~$1,100/month | ~$216,500/month |
| **Ratio** | **1×** | **~82× / ~197×** |

## Qualitative differences

*   **Cost Drivers:** Data ingest and user seats both matter. Data is ~$55,257/month on logs and metrics, and with traces it grows by another ~$126,335/month. At $349/user/month for Pro Full Platform Users, 100 users cost $34,900/month — over 30× the entire Micromegas infrastructure. Giving some users cheaper Core or free Basic seats lowers this, but those seats restrict access to parts of the platform.

*   **Platform Philosophy:**
    *   **New Relic** offers a broad, integrated SaaS platform covering APM, infrastructure, logs, and more, with a focus on providing a single pane of glass for observability.
    *   **Micromegas** provides a unified data model within your own cloud environment, offering greater control and cost efficiency for high-volume data.

*   **Pricing Model:** New Relic's combination of data volume and user seats means costs scale on two dimensions. For large teams with high data volumes, both dimensions compound. Micromegas's cost scales only with infrastructure usage.

*   **User Access:** New Relic's per-user pricing can discourage giving broad access to observability data. Micromegas has no per-user fees — anyone in your organization can query the data.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud account — a critical requirement for many organizations.

## Sources

[New Relic Pricing](https://newrelic.com/pricing); [New Relic data retention](https://docs.newrelic.com/docs/data-apis/manage-data/manage-data-retention/). Back to [all SaaS vendor comparisons](../when-to-use/saas-vendors.md#at-a-glance).
