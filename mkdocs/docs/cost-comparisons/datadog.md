---
title: Datadog Pricing and Cost Breakdown vs. Micromegas
description: Datadog pricing explained ($15/host Infra Pro, $2.50 per million log events indexed, $1.70 per million spans indexed) and applied to a measured production workload, ~$25,100/month for logs and metrics (30-day log retention) and ~$568,000/month with traces, vs. ~$1,100 for self-hosted Micromegas. Rates verified October 2026.
---

# Datadog Pricing vs. Micromegas

Datadog bills per host for infrastructure monitoring, per GB for log ingestion and per million events for log indexing. On the reference workload the estimate is ~$25,100/month for logs and metrics with 30-day log retention, about 23× the Micromegas figure, with custom metrics excluded. The with-traces figure adds the reference span volume at $0.10/GB ingested and $1.70 per million spans indexed (15-day retention) and comes to ~$568,000/month, about 517×. This page uses the shared [reference workload](../when-to-use/saas-vendors.md#reference-workload) and [methodology](../when-to-use/saas-vendors.md#methodology); see [The Challenge of Traces](../when-to-use/saas-vendors.md#the-challenge-of-traces) for how traces are priced.

*Rates verified October 2026. Estimates, not quotes — confirm current rates at [datadoghq.com/pricing](https://www.datadoghq.com/pricing/list/).*

## Datadog pricing on the reference workload

*   **Infrastructure Monitoring (Pro plan, annual commitment):**
    *   `20 hosts × $15/host/month` ($18 month-to-month)
    *   **Subtotal:** **~$300/month**

*   **Log Management:**
    *   Ingestion: `8,261 GB/month × $0.10/GB = ~$826/month`
    *   Indexing (30-day retention): `9,606 million events/month × $2.50/million = ~$24,015/month`
    *   **Subtotal:** **~$24,841/month**

*   **Total Estimated Monthly Cost (Logs & Infrastructure):** **~$25,141/month**, or ~$25,100

Log indexing dominates the cost. Adding the $0.10/GB ingestion fee to the indexing fee gives an effective log cost of ~$3.01/GB (`$24,841 / 8,261 GB`).

Indexing is priced per retention period: $1.06 per million events for 3 days, $1.27 for 7 days, $1.70 for 15 days and $2.50 for 30 days. Retention beyond 30 days is "Contact Us". The 90-day path is Flex Logs, which stores events at $0.05 per million events per month: three months of events (`28,818 million × $0.05`) is about $1,441/month, plus Flex compute, which is contact-sales. That option is not part of the headline figure.

**Custom metrics are excluded from the headline.** The workload has about 1 million active metric series. Pro includes 100 custom metrics per host, or 2,000 for 20 hosts, so ~998,000 series would be billed as overage at $5 per 100: about **$49,900/month**. Fine-grained metrics like these wouldn't typically be sent to Datadog at this granularity, which is why the headline leaves them out.

**With traces: ~$568,000/month.** The span volume is ~300 billion spans per month, or ~315,837 GB at the measured span size.

*   **APM:** `20 hosts × $31/host/month` = **$620/month**
*   **Span ingestion:** `315,837 GB × $0.10/GB` = **~$31,584/month**
*   **Span indexing (15-day retention):** `300,500 million spans × $1.70/million` = **~$510,869/month**
*   **Traces subtotal:** **~$543,073/month**, for a total of **~$568,214/month**

Indexing is what keeps spans searchable. Ingesting without indexing costs about $32,000/month, but spans are then kept only 15 minutes. Included per-host allotments are not published on the list page, so none are subtracted.

## What's included and what's billed separately

*   **Included:** unlimited users, host metrics, and 100 custom metrics per host on Pro (overage $5 per 100).
*   **Billed separately:** log ingestion and indexing (priced per retention period), Flex Logs storage and compute, and APM ($31/host with Infrastructure, annual).
*   **Traces:** APM is billed per host, plus ingested and indexed span volume. They are priced in the with-traces figure above and are not in the logs-and-metrics estimate.

## Cost comparison

| Category | Micromegas | Datadog |
| :--- | :--- | :--- |
| **Logs + metrics** | ~$1,100/month (includes all traces) | ~$25,100/month (30-day log retention, custom metrics excluded) |
| **With traces** | ~$1,100/month | ~$568,000/month (15-day span index) |
| **Ratio** | **1×** | **~23× / ~517×** |

## Qualitative differences

*   **Cost Driver:** Without traces, Datadog's cost is dominated by log indexing fees ($24,015 of the $25,141 total). With traces, span indexing ($510,869) dominates. For organizations that generate large volumes of structured logs or spans, these costs can escalate quickly.

*   **Platform Philosophy:**
    *   **Datadog** offers distinct, tightly integrated products for logs, metrics, and traces. The underlying data is stored in specialized systems, leading to separate billing for each.
    *   **Micromegas** uses a single, unified storage and query layer for all telemetry types, enabling cross-signal correlation and significantly better storage efficiency.

*   **Cost Complexity:** Datadog's pricing is complex and modular. While this offers flexibility, it can lead to unpredictable costs as you enable more features or as usage patterns change. Micromegas's cost maps directly to your cloud bill.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud environment — a critical requirement for many organizations.

## Sources

[Datadog Pricing List](https://www.datadoghq.com/pricing/list/) (including the APM rows), [Datadog Log Management pricing](https://www.datadoghq.com/pricing/?product=log-management), [Datadog custom metrics billing](https://docs.datadoghq.com/account_management/billing/custom_metrics/). Back to [all SaaS vendor comparisons](../when-to-use/saas-vendors.md#at-a-glance).
