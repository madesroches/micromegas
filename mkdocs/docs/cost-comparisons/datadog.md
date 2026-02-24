# Cost Comparison: Micromegas vs. Datadog

**Disclaimer:** These are estimates, not quotes. Datadog's pricing is complex and modular. Actual costs vary based on specific product usage, negotiated enterprise agreements, and region.

For shared methodology, workload definition, and Micromegas baseline cost, see the [Comparison Methodology](index.md) page.

---

## Datadog Pricing (Logs & Infrastructure)

Datadog's pricing combines per-host infrastructure monitoring with per-GB log ingestion and per-million-event log indexing.

*   **Infrastructure Monitoring (Pro Plan):**
    *   `20 hosts × $15/host/month (annual commitment)`
    *   **Subtotal:** **~$300/month**

*   **Log Management:**
    *   Ingestion: `1,500 GB/month × $0.10/GB = ~$150/month`
    *   Indexing (30-day retention): `3,000 million events/month × $2.50/million = ~$7,500/month`
    *   **Subtotal:** **~$7,650/month**

*   **Total Estimated Monthly Cost (Logs & Infrastructure):** **~$7,950/month**

Note: Log indexing dominates the cost. The effective cost of logs is ~$1.80–$2.94/GB when combining ingestion and indexing. The $2.50/million events indexing rate is approximate — Datadog does not transparently publish per-retention-tier indexing prices.

---

## Cost Comparison Summary

| Category | Micromegas | Datadog |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month | ~$7,950/month |
| **Ratio** | **1×** | **~7× more** |

---

## Qualitative Differences

*   **Cost Driver:** Datadog's cost is dominated by log indexing fees. For organizations that generate large volumes of structured logs, these costs can escalate quickly.

*   **Platform Philosophy:**
    *   **Datadog** offers distinct, tightly integrated products for logs, metrics, and traces. The underlying data is stored in specialized systems, leading to separate billing for each.
    *   **Micromegas** uses a single, unified storage and query layer for all telemetry types, enabling cross-signal correlation and significantly better storage efficiency.

*   **Cost Complexity:** Datadog's pricing is famously complex and modular. While this offers flexibility, it can lead to unpredictable costs as you enable more features or as usage patterns change. Micromegas's cost maps directly to your cloud bill.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud environment — a critical requirement for many organizations.

---

## References

1. [Datadog Pricing List](https://www.datadoghq.com/pricing/list/)
2. [SigNoz: Datadog Pricing Analysis](https://signoz.io/blog/datadog-pricing/)
3. [Last9: Datadog Pricing Breakdown](https://last9.io/blog/datadog-pricing-all-your-questions-answered/)
