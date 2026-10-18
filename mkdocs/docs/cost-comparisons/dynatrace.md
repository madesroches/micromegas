---
title: Dynatrace Pricing and Cost Breakdown vs. Micromegas
description: Dynatrace pricing explained ($0.01 per memory-GiB-hour Full-Stack, $0.20/GiB log and span ingest) and applied to a measured 90-day production workload, ~$3,200/month for logs and monitoring and ~$78,200/month with traces, vs. ~$1,100 for self-hosted Micromegas. Rates verified October 2026.
---

# Dynatrace Pricing vs. Micromegas

Dynatrace bills on its Dynatrace Platform Subscription (DPS) rate card: memory-hours of monitored hosts, per GiB of logs, and per data point of custom metrics. On the reference workload the estimate is ~$3,200/month for logs and monitoring, about 2.9× the Micromegas figure, with custom metrics excluded. The with-traces figure adds the reference span volume at $0.20/GiB ingested plus $0.0007/GiB-day retained and comes to ~$78,200/month, about 71×. This page uses the shared [reference workload](../when-to-use/saas-vendors.md#reference-workload) and [methodology](../when-to-use/saas-vendors.md#methodology); see [The Challenge of Traces](../when-to-use/saas-vendors.md#the-challenge-of-traces) for how traces are priced.

*Rates verified October 2026. Estimates, not quotes — confirm current rates at [dynatrace.com/pricing](https://www.dynatrace.com/pricing/rate-card/).*

## Dynatrace pricing on the reference workload

*   **Full-Stack Monitoring (memory-based):**
    *   `20 hosts × 8 GiB × 730 hours/month × $0.01/GiB-hour`
    *   **Subtotal:** **~$1,168/month**

*   **Log Ingest & Process:**
    *   `7,694 GiB/month × $0.20/GiB`
    *   **Subtotal:** **~$1,539/month**

*   **Log Retain (90 days):**
    *   `7,694 GiB × 90 days × $0.0007/GiB-day`
    *   **Subtotal:** **~$485/month**

*   **Total Estimated Monthly Cost (Logs + Monitoring):** **~$3,191/month**, or ~$3,200

**Custom metrics are excluded from the headline.** Metric data points are billed at $0.15 per 100k. Full-Stack includes 900 custom-metric data points per GiB of memory per 15-minute interval, which for 160 GiB over a month is about 420 million data points (`160 × 900 × 2,920 intervals`). That covers about 0.3% of the workload's ~138.3 billion points per month. The remainder would cost about **$206,800/month** (`(138.3B − ~420M) / 100k × $0.15`). The fine-grained instrumentation metrics in the Micromegas workload would not typically be sent to Dynatrace at this granularity.

**With traces: ~$78,200/month.** The span volume is ~294,148 GiB per month. Full-Stack includes 200 KiB per minute per GiB of memory, about 1,337 GiB/month for the 20 hosts.

*   **Span ingest:** `(294,148 − 1,337) GiB × $0.20/GiB` = **~$58,562/month**
*   **Span retention:** the first 10 days are free, so days 11–90 are billed: `294,148 GiB × 80 days × $0.0007/GiB-day` = **~$16,472/month**
*   **Traces subtotal:** **~$75,034/month**, for a total of **~$78,226/month**

## What's included and what's billed separately

*   **Included:** unlimited users (no seat fees), and the Full-Stack custom-metric allotment described above.
*   **Billed separately:** log ingest, log retention and log queries (Log Query is $0.0035/GiB scanned), and custom metric data points beyond the allotment.
*   **Retention options:** "Retain with Included Queries" is $0.02/GiB-day, much more than the usage-based $0.0007/GiB-day used in this estimate.
*   **Traces:** $0.20/GiB ingest plus $0.0007/GiB-day retained, with the first 10 days of retention free. They are priced in the with-traces figure above and are not in the logs-and-monitoring estimate.

## Cost comparison

| Category | Micromegas | Dynatrace |
| :--- | :--- | :--- |
| **Logs + monitoring** | ~$1,100/month (includes all traces) | ~$3,200/month (custom metrics excluded) |
| **With traces** | ~$1,100/month | ~$78,200/month |
| **Ratio** | **1×** | **~2.9× / ~71×** |

The Dynatrace estimates exclude custom metrics at scale. Including them at the reference workload's volume would dramatically increase the cost.

## Qualitative differences

*   **Closest on Logs + Monitoring:** Dynatrace is the closest to Micromegas in cost on logs and monitoring at this reference workload, and only when custom metrics at extreme volume are excluded from the comparison.

*   **Platform Philosophy:**
    *   **Dynatrace** is known for its highly automated, AI-driven approach to observability, offering deep insights into application performance with minimal manual configuration.
    *   **Micromegas** provides a unified data model for all telemetry types within your own cloud environment, offering greater control and cost efficiency for high-volume data.

*   **Pricing Model:** The DPS rate card offers granular, capability-based pricing. This can be predictable for standard monitoring but diverges dramatically at extreme metric or trace volumes.

*   **Cost at Scale:** The gap between Dynatrace and Micromegas widens as data volume increases. At the reference workload, it's ~2.9× on logs and monitoring and ~71× with traces. For organizations with high-cardinality custom metrics, the difference can be orders of magnitude.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud account — a critical requirement for many organizations.

## Sources

[Dynatrace Rate Card](https://www.dynatrace.com/pricing/rate-card/), [Dynatrace pricing](https://www.dynatrace.com/pricing/), [Full-Stack Monitoring license docs](https://docs.dynatrace.com/docs/license/capabilities/app-infra-observability/full-stack-monitoring) (included trace volume, 10-day free retention). Back to [all SaaS vendor comparisons](../when-to-use/saas-vendors.md#at-a-glance).
