---
title: Elastic Cloud Pricing and Cost Breakdown vs. Micromegas
description: Elastic Cloud Serverless pricing explained (ingest from $0.09/GB, $0.019/GB-month retention) and applied to a measured 90-day production workload, ~$6,200-$10,400/month for logs and metrics and ~$52,600-$56,800/month with traces, vs. ~$1,100 for self-hosted Micromegas. Rates verified October 2026.
---

# Elastic Cloud Pricing vs. Micromegas

Elastic Cloud Serverless bills per GB ingested (volume-tiered) and per GB-month retained, with no per-user fees. On the reference workload the estimate is ~$6,200–$10,400/month for logs and metrics, about 5.6×–9.4× the Micromegas figure. The with-traces figure adds the reference span volume at the most favourable ingest rate, $0.09/GB, plus $0.019/GB-month retained, and comes to ~$52,600–$56,800/month, about 48×–52×. This page uses the shared [reference workload](../when-to-use/saas-vendors.md#reference-workload) and [methodology](../when-to-use/saas-vendors.md#methodology); see [The Challenge of Traces](../when-to-use/saas-vendors.md#the-challenge-of-traces) for how traces are priced.

*Rates verified October 2026. Estimates, not quotes — confirm current rates at [elastic.co/pricing](https://www.elastic.co/pricing/serverless-observability).*

## Elastic pricing on the reference workload

There is no single headline figure, because Elastic does not publish its log-ingest tier breakpoints. The estimate is a range, from the $0.09/GB floor to the $0.60/GB first-tier rate.

*   **Logs (Observability Complete):**
    *   Ingest is volume-tiered, "as low as $0.09/GB", and the tier breakpoints are not published: `8,261 GB × $0.09–$0.60/GB = $744–$4,957/month`.
    *   Retention: `8,261 GB × 3 months × $0.019/GB-month = ~$471/month`

*   **Metrics (time-series metrics at 25% of the standard rate, effective July 1, 2026):**
    *   Ingest: `129,980 GB × $0.023/GB = ~$2,990/month`
    *   Retention: `129,980 GB × 3 months × $0.005/GB-month = ~$1,950/month`

*   **Total Estimated Monthly Cost:** **~$6,154–$10,367/month**, or ~$6,200–$10,400

Elastic bills ingest and retention on "the fully enriched normalized data size … before Elasticsearch compression", which is why this estimate uses the measured billing sizes rather than raw payload sizes.

**With traces: ~$52,600–$56,800/month.** The span volume is ~315,837 GB per month, priced at the $0.09/GB floor, the most favourable rate at this volume.

*   **Span ingest:** `315,837 GB × $0.09/GB` = **~$28,425/month**
*   **Span retention:** `315,837 GB × 3 months × $0.019/GB-month` = **~$18,003/month**
*   **Traces subtotal:** **~$46,428/month**, for a total of **~$52,582–$56,795/month**

## What's included and what's billed separately

*   **Included:** no per-user fees. The first 50 GB of egress each month is free.
*   **Billed separately:** egress beyond 50 GB at $0.05/GB, and traces (priced in the with-traces figure above, not in the logs-and-metrics estimate).
*   **Cheaper tier:** Logs Essentials has ingest as low as $0.07/GB and retention as low as $0.017/GB-month, with reduced features compared with Observability Complete.
*   **Retention:** priced per GB-month stored, as above.

## Cost comparison

| Category | Micromegas | Elastic Cloud (Serverless) |
| :--- | :--- | :--- |
| **Logs + metrics** | ~$1,100/month (includes all traces) | ~$6,200–$10,400/month |
| **With traces** | ~$1,100/month | ~$52,600–$56,800/month |
| **Ratio** | **1×** | **~5.6×–9.4× / ~48×–52×** |

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
