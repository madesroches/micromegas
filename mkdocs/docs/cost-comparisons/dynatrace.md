---
title: Dynatrace Pricing and Cost Breakdown vs. Micromegas
description: Dynatrace pricing explained ($0.01 per memory-GiB-hour Full-Stack, $0.20/GiB log ingest) and applied to a 90-day, ~100-user logs-and-metrics workload, ~$1,600/month vs. ~$1,100 for self-hosted Micromegas. Rates verified October 2026.
---

# Dynatrace Pricing vs. Micromegas

Dynatrace bills on its Dynatrace Platform Subscription (DPS) rate card: memory-hours of monitored hosts, per GiB of logs, and per data point of custom metrics. On the reference workload the estimate is ~$1,600/month for logs and monitoring, about 1.5× the Micromegas figure, with custom metrics excluded. This page uses the shared [reference workload](../when-to-use/saas-vendors.md#reference-workload) and [methodology](../when-to-use/saas-vendors.md#methodology); like every comparison on this site, it excludes traces ([why](../when-to-use/saas-vendors.md#the-challenge-of-traces)).

*Rates verified October 2026. Estimates, not quotes — confirm current rates at [dynatrace.com/pricing](https://www.dynatrace.com/pricing/rate-card/).*

## Dynatrace pricing on the reference workload

*   **Full-Stack Monitoring (memory-based):**
    *   `20 hosts × 8 GiB × 730 hours/month × $0.01/GiB-hour`
    *   **Subtotal:** **~$1,168/month**

*   **Log Ingest & Process:**
    *   `1,500 GiB/month × $0.20/GiB`
    *   **Subtotal:** **~$300/month**

*   **Log Retain (90 days):**
    *   `1,500 GiB × 90 days × $0.0007/GiB-day`
    *   **Subtotal:** **~$95/month**

*   **Total Estimated Monthly Cost (Logs + Monitoring):** **~$1,563/month**

**Custom metrics are excluded from the headline.** Metric data points are billed at $0.15 per 100k. Full-Stack includes 900 custom-metric data points per GiB of memory per 15-minute interval, which for 160 GiB over a month is about 420 million data points (`160 × 900 × 2,920 intervals`). That covers about 0.5% of the workload's ~92 billion points per month. The remaining ~91.6 billion points would cost about **$137,000/month** (`91.6B / 100k × $0.15`). The fine-grained instrumentation metrics in the Micromegas workload would not typically be sent to Dynatrace at this granularity.

## What's included and what's billed separately

*   **Included:** unlimited users (no seat fees), and the Full-Stack custom-metric allotment described above.
*   **Billed separately:** log ingest, log retention and log queries (Log Query is $0.0035/GiB scanned), and custom metric data points beyond the allotment.
*   **Retention options:** "Retain with Included Queries" is $0.02/GiB-day, much more than the usage-based $0.0007/GiB-day used in this estimate.
*   **Traces:** $0.20/GiB ingest plus $0.0007/GiB-day retained; none of it is in this estimate.

## Cost comparison

| Category | Micromegas | Dynatrace |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month | ~$1,600/month (logs + monitoring only) |
| **Ratio** | **1×** | **~1.5× more** |

The Dynatrace estimate excludes custom metrics at scale. Including them at the reference workload's volume would dramatically increase the cost.

## Qualitative differences

*   **Second-Closest Competitor:** Dynatrace is the second-closest to Micromegas in cost at this reference workload, after Elastic Cloud Serverless, and only when custom metrics at extreme volume are excluded from the comparison.

*   **Platform Philosophy:**
    *   **Dynatrace** is known for its highly automated, AI-driven approach to observability, offering deep insights into application performance with minimal manual configuration.
    *   **Micromegas** provides a unified data model for all telemetry types within your own cloud environment, offering greater control and cost efficiency for high-volume data.

*   **Pricing Model:** The DPS rate card offers granular, capability-based pricing. This can be predictable for standard monitoring but diverges dramatically at extreme metric or trace volumes.

*   **Cost at Scale:** The gap between Dynatrace and Micromegas widens as data volume increases. At the reference workload, it's 1.5×. For organizations with high-cardinality custom metrics, the difference can be orders of magnitude.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud account — a critical requirement for many organizations.

## Sources

[Dynatrace Rate Card](https://www.dynatrace.com/pricing/rate-card/), [Dynatrace pricing](https://www.dynatrace.com/pricing/), [Full-Stack Monitoring license docs](https://docs.dynatrace.com/docs/license/capabilities/app-infra-observability/full-stack-monitoring). Back to [all SaaS vendor comparisons](../when-to-use/saas-vendors.md#at-a-glance).
