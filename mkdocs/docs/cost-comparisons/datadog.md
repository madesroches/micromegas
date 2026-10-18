---
title: Datadog Pricing and Cost Breakdown vs. Micromegas
description: Datadog pricing explained ($15/host Infra Pro, $2.50 per million log events indexed) and applied to a ~100-user logs-and-metrics workload, ~$7,950+/month at 30-day log retention vs. ~$1,100 for self-hosted Micromegas. Rates verified October 2026.
---

# Datadog Pricing vs. Micromegas

Datadog bills per host for infrastructure monitoring, per GB for log ingestion and per million events for log indexing. On the reference workload the estimate is ~$7,950+/month with 30-day log retention, about 7.2× the Micromegas figure. This page uses the shared [reference workload](../when-to-use/saas-vendors.md#reference-workload) and [methodology](../when-to-use/saas-vendors.md#methodology); like every comparison on this site, it excludes traces ([why](../when-to-use/saas-vendors.md#the-challenge-of-traces)).

*Rates verified October 2026. Estimates, not quotes — confirm current rates at [datadoghq.com/pricing](https://www.datadoghq.com/pricing/list/).*

## Datadog pricing on the reference workload

*   **Infrastructure Monitoring (Pro plan, annual commitment):**
    *   `20 hosts × $15/host/month` ($18 month-to-month)
    *   **Subtotal:** **~$300/month**

*   **Log Management:**
    *   Ingestion: `1,500 GB/month × $0.10/GB = ~$150/month`
    *   Indexing (30-day retention): `3,000 million events/month × $2.50/million = ~$7,500/month`
    *   **Subtotal:** **~$7,650/month**

*   **Total Estimated Monthly Cost (Logs & Infrastructure):** **~$7,950+/month**

Log indexing dominates the cost. The 1,500 GB of logs is 3,000 million events, or 2 million events per GB, so indexing comes to $5.00/GB. Adding the $0.10/GB ingestion fee gives an effective cost of ~$5.10/GB (`$7,650 / 1,500 GB`).

Indexing is priced per retention period: $1.06 per million events for 3 days, $1.27 for 7 days, $1.70 for 15 days and $2.50 for 30 days. Retention beyond 30 days is "Contact Us". The 90-day path is Flex Logs, which stores events at $0.05 per million events per month: three months of events (`9,000 million × $0.05`) is about $450/month, plus Flex compute, which is contact-sales. That option is not part of the headline figure.

**Custom metrics are excluded from the headline.** The workload has about 1 million active metric series. Pro includes 100 custom metrics per host, or 2,000 for 20 hosts, so ~998,000 series would be billed as overage at $5 per 100: about **$49,900/month**. Fine-grained metrics like these wouldn't typically be sent to Datadog at this granularity, which is why the headline leaves them out.

## What's included and what's billed separately

*   **Included:** unlimited users, host metrics, and 100 custom metrics per host on Pro (overage $5 per 100).
*   **Billed separately:** log ingestion and indexing (priced per retention period), Flex Logs storage and compute, and APM ($31/host with Infrastructure, annual).
*   **Traces:** APM is billed per host, plus ingested and indexed span volume; none of it is in this estimate.

## Cost comparison

| Category | Micromegas | Datadog |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month | ~$7,950+/month (30-day log retention, custom metrics excluded) |
| **Ratio** | **1×** | **~7.2× more** |

## Qualitative differences

*   **Cost Driver:** Datadog's cost is dominated by log indexing fees. For organizations that generate large volumes of structured logs, these costs can escalate quickly.

*   **Platform Philosophy:**
    *   **Datadog** offers distinct, tightly integrated products for logs, metrics, and traces. The underlying data is stored in specialized systems, leading to separate billing for each.
    *   **Micromegas** uses a single, unified storage and query layer for all telemetry types, enabling cross-signal correlation and significantly better storage efficiency.

*   **Cost Complexity:** Datadog's pricing is complex and modular. While this offers flexibility, it can lead to unpredictable costs as you enable more features or as usage patterns change. Micromegas's cost maps directly to your cloud bill.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud environment — a critical requirement for many organizations.

## Sources

[Datadog Pricing List](https://www.datadoghq.com/pricing/list/), [Datadog Log Management pricing](https://www.datadoghq.com/pricing/?product=log-management), [Datadog custom metrics billing](https://docs.datadoghq.com/account_management/billing/custom_metrics/). Back to [all SaaS vendor comparisons](../when-to-use/saas-vendors.md#at-a-glance).
