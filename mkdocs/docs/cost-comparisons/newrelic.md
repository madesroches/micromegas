# Cost Comparison: Micromegas vs. New Relic

**Disclaimer:** These are estimates, not quotes. Actual costs vary based on data volume, user count, plan type, and negotiated enterprise agreements.

For shared methodology, workload definition, and Micromegas baseline cost, see the [Comparison Methodology](index.md) page.

---

## New Relic Pricing (Logs & Metrics)

New Relic's pricing combines per-GB data ingestion with per-user seat fees. They also offer a newer CCU (Compute Capacity Unit) model, but per-GB + per-seat pricing is still available and widely used.

*   **Data Ingest (Original Data option):**
    *   Logs: `1,500 GB/month`
    *   Metrics: `~9,200 GB/month`
    *   Total: `10,700 GB/month × $0.40/GB`
    *   **Subtotal:** **~$4,280/month**

*   **User Seats (Pro, Full Platform Users, annual):**
    *   `5 users × $349/user/month`
    *   **Subtotal:** **~$1,745/month**

*   **Total Estimated Monthly Cost:** **~$6,025/month**

Note: New Relic also offers a Data Plus option at $0.60/GB with additional features (extended retention, HIPAA eligibility, etc.). The CCU model provides compute-based pricing as an alternative, but its pricing is not transparently published.

---

## Cost Comparison Summary

| Category | Micromegas | New Relic |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month | ~$6,025/month |
| **Ratio** | **1×** | **~5.5× more** |

---

## Qualitative Differences

*   **Cost Drivers:** Both data ingestion and user seats contribute significantly. At $349/user/month for Pro Full Platform Users, just 5 users cost $1,745/month — more than the entire Micromegas infrastructure.

*   **Platform Philosophy:**
    *   **New Relic** offers a broad, integrated SaaS platform covering APM, infrastructure, logs, and more, with a focus on providing a single pane of glass for observability.
    *   **Micromegas** provides a unified data model within your own cloud environment, offering greater control and cost efficiency for high-volume data.

*   **Pricing Model:** New Relic's combination of data volume and user seats means costs scale on two dimensions. For large teams with high data volumes, both dimensions compound. Micromegas's cost scales only with infrastructure usage.

*   **User Access:** New Relic's per-user pricing can discourage giving broad access to observability data. Micromegas has no per-user fees — anyone in your organization can query the data.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud account — a critical requirement for many organizations.

---

## References

1. [New Relic Pricing](https://newrelic.com/pricing)
2. [SigNoz: New Relic CCU Pricing Analysis](https://signoz.io/blog/new-relic-ccu-pricing-unpredictable-costs/)
