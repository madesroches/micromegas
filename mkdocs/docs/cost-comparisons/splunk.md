---
title: Splunk Cloud Pricing and Cost Breakdown vs. Micromegas
description: Splunk Cloud pricing explained (daily ingest volume billed annually, rates now private-offer only) and applied to a 90-day, ~100-user logs-and-metrics workload, ~$22,000/month vs. ~$1,100 for self-hosted Micromegas. Rates verified October 2026.
---

# Splunk Cloud Pricing vs. Micromegas

Splunk Cloud bills on daily index volume, billed annually. On the reference workload the estimate is ~$22,000/month, about 20× the Micromegas figure. Splunk no longer publishes rates, so this estimate is extrapolated from the last public list prices. This page uses the shared [reference workload](../when-to-use/saas-vendors.md#reference-workload) and [methodology](../when-to-use/saas-vendors.md#methodology); like every comparison on this site, it excludes traces ([why](../when-to-use/saas-vendors.md#the-challenge-of-traces)).

*Rates verified October 2026. Estimates, not quotes — confirm current pricing with Splunk at [splunk.com/en_us/products/pricing.html](https://www.splunk.com/en_us/products/pricing.html).*

## Splunk pricing on the reference workload

The AWS Marketplace listing now shows only "Custom pricing options" through private offers. splunk.com publishes the pricing models (ingest, workload/SVC, and a newer activity-based option) but no rates. The estimate below is therefore built from the last publicly listed AWS Marketplace tiers (early 2026), and current prices are available by private offer only.

| Daily Volume | Annual Cost | Effective $/GB/day/year |
|---|---|---|
| 50 GB/day | $50,000/yr | $1,000 |
| 100 GB/day | $80,000/yr | $800 |

*Last published AWS Marketplace list prices (early 2026).*

The reference workload generates ~10,700 GB/month ÷ 30 = **~357 GB/day**.

*   **Ingest (~357 GB/day):**
    *   Extrapolating at ~$700–$800/GB/day/year: ~$250k–$285k/year
    *   **Estimated Monthly Cost:** **~$22,000/month**

Splunk Cloud pricing is heavily volume-discounted at scale, so actual pricing through direct negotiation may differ significantly from this extrapolation.

## What's included and what's billed separately

*   **Included:** unlimited users on Splunk Cloud Platform.
*   **Metrics-side product:** Splunk Observability Cloud is priced per host-month, "starts at": Infrastructure $15, APM & Infrastructure $60, End-to-End $75. It is a separate product and is not in this estimate.
*   **Retention and traces:** not itemized in public pricing; traces are not in this estimate.

**Cisco acquisition.** Cisco completed its acquisition of Splunk in March 2024. Verify current pricing directly with Splunk before making budget decisions.

## Cost comparison

| Category | Micromegas | Splunk Cloud |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month | ~$22,000/month (extrapolated from last list prices) |
| **Ratio** | **1×** | **~20× more** |

## Qualitative differences

*   **Cost at Scale:** On data volume alone, Splunk is the most expensive option. At ~20× the cost of Micromegas, the difference is driven by Splunk's ingest-based pricing applied to high data volumes.

*   **Platform Maturity:** Splunk has decades of investment in log analysis, security (SIEM), and IT operations. Its SPL (Search Processing Language) is powerful but requires specialized knowledge.

*   **Operational Burden:** Splunk Cloud is a managed SaaS — the vendor handles uptime, scaling, and patching. Micromegas requires managing your own infrastructure but provides full cost transparency.

*   **Control & Transparency:** With Micromegas, you see every dollar on your cloud bill. With Splunk, pricing is opaque and requires annual contract negotiations.

*   **Data Ownership:** Micromegas keeps all telemetry data within your own cloud account, which is a critical advantage for security and data governance.

## Sources

[AWS Marketplace: Splunk Cloud](https://aws.amazon.com/marketplace/pp/prodview-jlaunompo5wbw), [Splunk pricing](https://www.splunk.com/en_us/products/pricing.html), [Splunk Observability Cloud pricing](https://www.splunk.com/en_us/products/pricing/observability.html). Back to [all SaaS vendor comparisons](../when-to-use/saas-vendors.md#at-a-glance).
