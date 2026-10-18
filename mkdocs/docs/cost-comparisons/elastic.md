---
title: Elastic Cloud Pricing and Cost Breakdown vs. Micromegas
description: Elastic Cloud Serverless pricing explained (ingest from $0.09/GB, $0.019/GB-month retention) and applied to a 90-day, ~100-user logs-and-metrics workload, ~$660/month vs. ~$1,100 for self-hosted Micromegas. Rates verified October 2026.
---

# Elastic Cloud Pricing vs. Micromegas

Elastic Cloud Serverless bills per GB ingested (volume-tiered) and per GB-month retained, with no per-user fees. On the reference workload the estimate is ~$660/month, about 0.6× the Micromegas figure, so Elastic is cheaper on logs and metrics alone. This page uses the shared [reference workload](../when-to-use/saas-vendors.md#reference-workload) and [methodology](../when-to-use/saas-vendors.md#methodology); like every comparison on this site, it excludes traces ([why](../when-to-use/saas-vendors.md#the-challenge-of-traces)).

*Rates verified October 2026. Estimates, not quotes — confirm current rates at [elastic.co/pricing](https://www.elastic.co/pricing/serverless-observability).*

## Elastic pricing on the reference workload

The total range is **~$570–$1,340/month** (~0.5×–1.2× Micromegas), because Elastic does not publish its log-ingest tier breakpoints. The headline of ~$660/month rests on an assumed blended log-ingest rate.

*   **Logs (Observability Complete):**
    *   Ingest is volume-tiered, "as low as $0.09/GB", and the tier breakpoints are not published. This estimate assumes a blended ~$0.15/GB: `1,500 GB × ~$0.15 = ~$225/month`. Across $0.09–$0.60/GB the range is $135–$900/month.
    *   The ~$0.15/GB figure assumes the volume tiers count combined ingest (~10.7 TB/month). That is an assumption, not a verified fact. If the tiers count only the 1.5 TB of logs, the rate is higher, toward the upper end of the range.
    *   Retention: `1,500 GB × 3 months × $0.019/GB-month = ~$86/month`

*   **Metrics (time-series metrics at 25% of the standard rate, effective July 1, 2026):**
    *   Ingest: `9,200 GB × $0.023/GB = ~$212/month`
    *   Retention: `9,200 GB × 3 months × $0.005/GB-month = ~$138/month`

*   **Total Estimated Monthly Cost:** **~$660/month** (`$225 + $86 + $212 + $138`)

The range's lower bound uses $0.09/GB for logs, and its upper bound uses the $0.60/GB first-tier rate on all logs. "Below Micromegas" holds under the blended-rate assumption; at the top of the range Elastic costs more. The Micromegas figure also includes 165 billion trace events, while this estimate excludes traces.

## What's included and what's billed separately

*   **Included:** no per-user fees. The first 50 GB of egress each month is free.
*   **Billed separately:** egress beyond 50 GB at $0.05/GB, and traces (not in this estimate).
*   **Cheaper tier:** Logs Essentials has ingest as low as $0.07/GB and retention as low as $0.017/GB-month, with reduced features compared with Observability Complete.
*   **Retention:** priced per GB-month stored, as above.

## Cost comparison

| Category | Micromegas | Elastic Cloud (Serverless) |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month (includes 165B trace events) | ~$660/month (range ~$570–$1,340; excludes traces) |
| **Ratio** | **1×** | **~0.6×** |

## Qualitative differences

*   **Architectural Philosophy:**
    *   **Elastic** was built around the Lucene search index — exceptionally powerful for log search and text analysis. Metrics and traces support has been built on top of this foundation.
    *   **Micromegas** was designed from the ground up with a unified data model for logs, metrics, and traces. It uses columnar storage (Parquet) and a SQL query engine (DataFusion), which is inherently more efficient for analytical queries and data compression.

*   **Query Language:**
    *   **Elastic** uses KQL (Kibana Query Language) and Lucene query syntax, powerful for text search but requiring domain-specific knowledge.
    *   **Micromegas** uses **SQL**, making it immediately accessible to a broader range of engineers, analysts, and data scientists.

*   **Serverless vs. Self-Hosted:** Elastic Cloud Serverless removes the need to manage cluster sizing and scaling. Micromegas requires managing your own infrastructure but provides full cost transparency and control.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud account, simplifying data governance.

## Sources

[Elastic Serverless Observability Pricing](https://www.elastic.co/pricing/serverless-observability), [Elastic Cloud Serverless pricing and packaging](https://www.elastic.co/blog/elastic-cloud-serverless-pricing-packaging), [Elastic Observability billing dimensions](https://www.elastic.co/docs/deploy-manage/cloud-organization/billing/elastic-observability-billing-dimensions). Back to [all SaaS vendor comparisons](../when-to-use/saas-vendors.md#at-a-glance).
