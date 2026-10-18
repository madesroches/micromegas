---
title: Splunk Cloud Pricing and Cost Breakdown vs. Micromegas
description: Splunk Cloud pricing explained (daily ingest volume billed annually, rates now private-offer only) and applied to a measured 90-day production workload, ~$22,100-$24,400/month for logs and metrics vs. ~$1,100 for self-hosted Micromegas. Traces are excluded because no per-volume rate is published. Rates verified October 2026.
---

# Splunk Cloud Pricing vs. Micromegas

Splunk Cloud bills on daily index volume, billed annually. On the reference workload the estimate is ~$22,100–$24,400/month for logs and metrics, about 20×–22× the Micromegas figure, with custom metrics excluded. Splunk no longer publishes rates, so this estimate is extrapolated from the last public list prices. Traces are excluded: Splunk publishes no per-volume trace rate. This page uses the shared [reference workload](../when-to-use/saas-vendors.md#reference-workload) and [methodology](../when-to-use/saas-vendors.md#methodology); see [The Challenge of Traces](../when-to-use/saas-vendors.md#the-challenge-of-traces) for how traces are treated.

*Rates verified October 2026. Estimates, not quotes — confirm current pricing with Splunk at [splunk.com/en_us/products/pricing.html](https://www.splunk.com/en_us/products/pricing.html).*

## Splunk pricing on the reference workload

The AWS Marketplace listing now shows only "Custom pricing options" through private offers. splunk.com publishes the pricing models (ingest, workload/SVC, and a newer activity-based option) but no rates. The estimate below is therefore built from the last publicly listed AWS Marketplace tiers (early 2026), and current prices are available by private offer only.

| Daily Volume | Annual Cost | Effective $/GB/day/year |
|---|---|---|
| 50 GB/day | $50,000/yr | $1,000 |
| 100 GB/day | $80,000/yr | $800 |

*Last published AWS Marketplace list prices (early 2026).*

The reference workload generates ~8,261 GB of logs per month, about **~275 GB/day**. Metrics are priced in Splunk Observability Cloud, not indexed in Splunk Cloud, because that is where Splunk sells them.

*   **Logs in Splunk Cloud (~275 GB/day):**
    *   Extrapolating at ~$700–$800/GB/day/year
    *   **Estimated Monthly Cost:** **~$16,065–$18,360/month**

*   **Metrics in Splunk Observability Cloud:**
    *   Infrastructure: `400 hosts × $15/host/month` = **$6,000/month**
    *   The metric time series included per host are not published, so custom metrics are excluded, the same treatment as Datadog and Dynatrace.

*   **Total Estimated Monthly Cost:** **~$22,065–$24,360/month**, or ~$22,100–$24,400

**Traces are excluded.** Splunk publishes per-host APM prices but no per-volume trace rate, so no with-traces figure is given.

Splunk Cloud pricing is heavily volume-discounted at scale, so actual pricing through direct negotiation may differ significantly from this extrapolation.

## What's included and what's billed separately

*   **Included:** unlimited users on Splunk Cloud Platform.
*   **Metrics-side product:** Splunk Observability Cloud is priced per host-month, "starts at": Infrastructure $15, APM & Infrastructure $60, End-to-End $75. The Infrastructure rate is in this estimate for the 400 hosts; custom metrics beyond the included allotment are not.
*   **Retention and traces:** not itemized in public pricing; traces are not in this estimate because no per-volume rate is published.

**Cisco acquisition.** Cisco completed its acquisition of Splunk in March 2024. Verify current pricing directly with Splunk before making budget decisions.

## Cost comparison

| Category | Micromegas | Splunk Cloud |
| :--- | :--- | :--- |
| **Logs + metrics** | ~$1,100/month (includes all traces) | ~$22,100–$24,400/month (extrapolated from last list prices, custom metrics excluded) |
| **With traces** | ~$1,100/month | n/a (no published per-volume rate) |
| **Ratio** | **1×** | **~20×–22× / —** |

## Qualitative differences

*   **Cost at Scale:** At ~20×–22× the cost of Micromegas on logs and metrics, the difference is driven mostly by Splunk's ingest-based pricing applied to ~275 GB/day of logs, with per-host Observability Cloud pricing adding ~$6,000.

*   **Platform Maturity:** Splunk has decades of investment in log analysis, security (SIEM), and IT operations. Its SPL (Search Processing Language) is powerful but requires specialized knowledge.

*   **Operational Burden:** Splunk Cloud is a managed SaaS — the vendor handles uptime, scaling, and patching. Micromegas requires managing your own infrastructure but provides full cost transparency.

*   **Control & Transparency:** With Micromegas, you see every dollar on your cloud bill. With Splunk, pricing is opaque and requires annual contract negotiations.

*   **Data Ownership:** Micromegas keeps all telemetry data within your own cloud account, which is a critical advantage for security and data governance.

## Sources

[AWS Marketplace: Splunk Cloud](https://aws.amazon.com/marketplace/pp/prodview-jlaunompo5wbw), [Splunk pricing](https://www.splunk.com/en_us/products/pricing.html), [Splunk Observability Cloud pricing](https://www.splunk.com/en_us/products/pricing/observability.html). Back to [all SaaS vendor comparisons](../when-to-use/saas-vendors.md#at-a-glance).
