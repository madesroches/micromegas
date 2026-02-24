# Cost Comparison: Micromegas vs. Elastic Observability

**Disclaimer:** These are estimates, not quotes. Actual costs vary based on usage patterns, cloud provider, region, and volume tiers.

For shared methodology, workload definition, and Micromegas baseline cost, see the [Comparison Methodology](index.md) page.

---

## Elastic Cloud Pricing (Serverless Model)

Elastic Cloud Serverless reached GA in December 2024, and pricing was updated in November 2025. The serverless model replaces the older resource-based pricing (RAM + storage) and is now Elastic's primary offering for new customers.

*   **Observability Complete Ingest:**
    *   Volume-tiered pricing ranging from ~$0.60/GB (first 50 GB) down to ~$0.09/GB at high volume
    *   At 10+ TB/month, the blended rate is approximately ~$0.15/GB
    *   `10,700 GB/month × ~$0.15/GB`
    *   **Subtotal:** **~$1,605/month**

*   **Retention (90 days):**
    *   `10,700 GB × 3 months × $0.019/GB-month`
    *   **Subtotal:** **~$610/month**

*   **Total Estimated Monthly Cost:** **~$2,215/month**

Note: The exact volume tiers for Observability ingest are not fully published — Elastic's pricing calculator provides the full breakdown. The ~$0.15/GB blended rate is approximate for this workload.

---

## Cost Comparison Summary

| Category | Micromegas | Elastic Cloud (Serverless) |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month | ~$2,200/month |
| **Ratio** | **1×** | **~2× more** |

---

## Qualitative Differences

*   **Architectural Philosophy:**
    *   **Elastic** was built around the Lucene search index — exceptionally powerful for log search and text analysis. Metrics and traces support has been built on top of this foundation.
    *   **Micromegas** was designed from the ground up with a unified data model for logs, metrics, and traces. It uses columnar storage (Parquet) and a SQL query engine (DataFusion), which is inherently more efficient for analytical queries and data compression.

*   **Query Language:**
    *   **Elastic** uses KQL (Kibana Query Language) and Lucene query syntax, powerful for text search but requiring domain-specific knowledge.
    *   **Micromegas** uses **SQL**, making it immediately accessible to a broader range of engineers, analysts, and data scientists.

*   **Serverless vs. Self-Hosted:** Elastic Cloud Serverless removes the need to manage cluster sizing and scaling. Micromegas requires managing your own infrastructure but provides full cost transparency and control.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud account, simplifying data governance.

---

## References

1. [Elastic Serverless Observability Pricing](https://www.elastic.co/pricing/serverless-observability)
2. [Elastic Cloud Serverless GA Announcement](https://www.elastic.co/blog/elastic-cloud-serverless-ga)
3. [Elastic Cloud Serverless Pricing Update (Nov 2025)](https://www.elastic.co/blog/elastic-cloud-serverless-pricing-packaging)
