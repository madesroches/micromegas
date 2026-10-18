---
title: Dynatrace Pricing and Cost Breakdown vs. Micromegas
description: Dynatrace pricing explained ($0.01 per memory-GiB-hour Full-Stack, $0.20/GiB log and span ingest) and applied to a measured 90-day production workload, ~$25,400/month for logs and monitoring and ~$95,300/month with traces, vs. ~$1,100 for self-hosted Micromegas. Rates verified October 2026.
---

# Dynatrace Pricing vs. Micromegas

Dynatrace bills on its Dynatrace Platform Subscription (DPS) rate card: memory-hours of monitored hosts, per GiB of logs, and per data point of custom metrics. On the reference workload the estimate is ~$25,400/month for logs and monitoring, about 23× the Micromegas figure, with custom metrics excluded. The with-traces figure adds the reference span volume at $0.20/GiB ingested plus $0.0007/GiB-day retained and comes to ~$95,300/month, about 87×. Full-Stack is billed on memory-hours, so it is priced on 400 hosts × 8 GiB, the approximate number of machines sending data concurrently. This page uses the shared [reference workload](../when-to-use/saas-vendors.md#reference-workload) and [methodology](../when-to-use/saas-vendors.md#methodology); see [The Challenge of Traces](../when-to-use/saas-vendors.md#the-challenge-of-traces) for how traces are priced.

*Rates verified October 2026. Estimates, not quotes — confirm current rates at [dynatrace.com/pricing](https://www.dynatrace.com/pricing/rate-card/).*

## Dynatrace pricing on the reference workload

*   **Full-Stack Monitoring (memory-based):**
    *   `400 hosts × 8 GiB × 730 hours/month × $0.01/GiB-hour`
    *   **Subtotal:** **~$23,360/month**

*   **Log Ingest & Process:**
    *   `7,694 GiB/month × $0.20/GiB`
    *   **Subtotal:** **~$1,539/month**

*   **Log Retain (90 days):**
    *   `7,694 GiB × 90 days × $0.0007/GiB-day`
    *   **Subtotal:** **~$485/month**

*   **Total Estimated Monthly Cost (Logs + Monitoring):** **~$25,383/month**, or ~$25,400

**Custom metrics are excluded from the headline.** Metric data points are billed at $0.15 per 100k. Full-Stack includes 900 custom-metric data points per GiB of memory per 15-minute interval, which for 400 hosts × 8 GiB over a month is about 8.4 billion data points (`400 × 8 × 900 × 2,920 intervals`). That covers about 6.1% of the workload's ~138 billion points per month. The remainder would cost about **$194,800/month**. The fine-grained instrumentation metrics in the Micromegas workload would not typically be sent to Dynatrace at this granularity.

**With traces: ~$95,300/month.** The span volume is ~294,146 GiB per month. Full-Stack includes 200 KiB per minute per GiB of memory, about 26,700 GiB/month (26,733) for the 400 hosts.

*   **Span ingest:** `(294,146 − 26,733) GiB × $0.20/GiB` = **~$53,483/month**
*   **Span retention:** the first 10 days are free, so days 11–90 are billed: `294,146 GiB × 80 days × $0.0007/GiB-day` = **~$16,472/month**
*   **Traces subtotal:** **~$69,955/month**, for a total of **~$95,338/month**

## What's included and what's billed separately

*   **Included:** unlimited users (no seat fees), and the Full-Stack custom-metric allotment described above.
*   **Billed separately:** log ingest, log retention and log queries (Log Query is $0.0035/GiB scanned), and custom metric data points beyond the allotment.
*   **Retention options:** "Retain with Included Queries" is $0.02/GiB-day, much more than the usage-based $0.0007/GiB-day used in this estimate.
*   **Traces:** $0.20/GiB ingest plus $0.0007/GiB-day retained, with the first 10 days of retention free. They are priced in the with-traces figure above and are not in the logs-and-monitoring estimate.

## Cost comparison

| Category | Micromegas | Dynatrace |
| :--- | :--- | :--- |
| **Logs + monitoring** | ~$1,100/month (includes all traces) | ~$25,400/month (custom metrics excluded) |
| **With traces** | ~$1,100/month | ~$95,300/month |
| **Ratio** | **1×** | **~23× / ~87×** |

The Dynatrace estimates exclude custom metrics at scale. Including them at the reference workload's volume would dramatically increase the cost.

## Qualitative differences

*   **Platform Philosophy:**
    *   **Dynatrace** is known for its highly automated, AI-driven approach to observability, offering deep insights into application performance with minimal manual configuration.
    *   **Micromegas** provides a unified data model for all telemetry types within your own cloud environment, offering greater control and cost efficiency for high-volume data.

*   **Pricing Model:** The DPS rate card offers granular, capability-based pricing. This can be predictable for standard monitoring but diverges dramatically at extreme metric or trace volumes.

*   **Cost at Scale:** The gap between Dynatrace and Micromegas widens as data volume increases. At the reference workload, it's ~23× on logs and monitoring and ~87× with traces. For organizations with high-cardinality custom metrics, the difference can be orders of magnitude.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud account — a critical requirement for many organizations.

## Sources

[Dynatrace Rate Card](https://www.dynatrace.com/pricing/rate-card/), [Dynatrace pricing](https://www.dynatrace.com/pricing/), [Full-Stack Monitoring license docs](https://docs.dynatrace.com/docs/license/capabilities/app-infra-observability/full-stack-monitoring) (included trace volume, 10-day free retention). Back to [all SaaS vendor comparisons](../when-to-use/saas-vendors.md#at-a-glance).
