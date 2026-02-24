# Cost Comparison: Micromegas vs. Grafana Cloud

**Disclaimer:** These are estimates, not quotes. Actual costs vary based on usage patterns, active series count, scrape frequency, and negotiated agreements.

For shared methodology, workload definition, and Micromegas baseline cost, see the [Comparison Methodology](index.md) page.

---

## Grafana Cloud Pro Pricing (Logs & Metrics)

Grafana Cloud Pro pricing is component-based, with separate charges for logs (Loki), metrics (Mimir), and users.

*   **Platform Fee:**
    *   **Subtotal:** **$19/month**

*   **Logs (Loki):**
    *   Pricing includes process ($0.05/GB) + write ($0.40/GB) + retain ($0.10/GB) = $0.50/GB total
    *   This rate includes **30-day retention only**
    *   `1,500 GB/month × $0.50/GB`
    *   **Subtotal:** **~$750/month**

*   **Extended Log Retention (beyond 30 days):**
    *   Grafana Cloud does not publicly list pricing for log retention beyond 30 days — it requires contacting sales
    *   The reference workload requires 90-day retention, so the actual cost would be **higher than shown**
    *   **Subtotal:** **Unknown (contact sales)**

*   **Metrics (Mimir):**
    *   Priced at $6.50 per 1,000 active series (at 1 data point per minute scrape frequency)
    *   Higher scrape frequencies multiply cost proportionally
    *   `1,000,000 active series × $6.50/1k`
    *   **Subtotal:** **~$6,500/month**

*   **Users:**
    *   `5 active users × $8/user`
    *   **Subtotal:** **$40/month**

*   **Total Estimated Monthly Cost (30-day retention only):** **~$7,309/month**

Note: This estimate uses only 30-day log retention. The reference workload requires 90 days — the actual cost with extended retention would be higher. Metric costs dominate at this scale; 1 million active series at $6.50/1k series = $6,500/month.

---

## Cost Comparison Summary

| Category | Micromegas | Grafana Cloud Pro |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month | ~$7,300+/month (30-day retention only) |
| **Ratio** | **1×** | **~6.6× more** |

Note: With 90-day log retention (not publicly priced), the actual ratio would be higher.

---

## Qualitative Differences

*   **Cost Driver:** Metrics dominate the Grafana Cloud cost at this scale. At $6.50 per 1,000 active series, 1 million active series alone costs $6,500/month — nearly 6× the entire Micromegas deployment.

*   **Open Source Foundation:**
    *   **Grafana Cloud** is built on open-source components (Grafana, Loki, Mimir, Tempo), which means organizations can self-host these components to reduce costs — but at the expense of operational complexity.
    *   **Micromegas** is also open source and self-hosted, but its unified architecture means fewer components to manage.

*   **Retention Transparency:** Grafana Cloud's lack of publicly listed extended retention pricing makes it difficult to accurately estimate costs for workloads requiring 90+ day retention. Micromegas's retention cost is simply the cost of S3 storage.

*   **Ecosystem:** Grafana Cloud benefits from the rich Grafana visualization ecosystem. Micromegas provides SQL-based querying and integrates with standard analytics tools.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud account — a critical requirement for many organizations.

---

## References

1. [Grafana Cloud Pricing](https://grafana.com/pricing/)
2. [Grafana Cloud Logs](https://grafana.com/products/cloud/logs/)
