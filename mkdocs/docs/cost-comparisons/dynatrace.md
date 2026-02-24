# Cost Comparison: Micromegas vs. Dynatrace

**Disclaimer:** These are estimates, not quotes. Dynatrace pricing varies based on negotiated enterprise agreements, region, and specific product usage.

For shared methodology, workload definition, and Micromegas baseline cost, see the [Comparison Methodology](index.md) page.

---

## Dynatrace Pricing (DPS Rate Card)

Dynatrace has transitioned from the legacy DDU (Davis Data Unit) model to DPS (Dynatrace Platform Subscription) with a rate card pricing structure. Costs are based on capabilities consumed rather than host units.

*   **Full-Stack Monitoring (memory-based):**
    *   `20 hosts × 8 GB memory × 730 hours/month × $0.01/GiB-hour`
    *   **Subtotal:** **~$1,168/month**

*   **Log Ingest & Process:**
    *   `1,500 GB/month × $0.20/GiB`
    *   **Subtotal:** **~$300/month**

*   **Log Retention (90 days):**
    *   `1,500 GB × 90 days × $0.0007/GiB-day`
    *   **Subtotal:** **~$95/month**

*   **Total Estimated Monthly Cost (Logs + Monitoring):** **~$1,563/month**

### Important Caveats

**Metric ingestion at extreme volume:** The reference workload includes ~92 billion metric data points per month. At the DPS list rate of $0.15/100k data points, this would cost ~$138,000/month. In practice, standard host metrics are included with Full-Stack monitoring — only custom metrics are billed separately. The 275 billion metric data points in the Micromegas workload include fine-grained instrumentation metrics that would not typically be sent to Dynatrace at this granularity.

**No per-user fees:** Dynatrace includes unlimited users in its DPS model, unlike most competitors.

**Log retention options:** Dynatrace also offers a "Retain with Included Queries" log option at $0.02/GiB-day (up to 35 days), which is significantly more expensive than the usage-based $0.0007/GiB-day option used in this estimate.

---

## Cost Comparison Summary

| Category | Micromegas | Dynatrace |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month | ~$1,600/month (logs + monitoring only) |
| **Ratio** | **1×** | **~1.5× more** |

Note: The Dynatrace estimate excludes custom metrics at scale. Including them at the reference workload's volume would dramatically increase the cost.

---

## Qualitative Differences

*   **Closest Competitor:** Of all platforms compared, Dynatrace is the closest to Micromegas in cost at this reference workload — but only when custom metrics at extreme volume are excluded from the comparison.

*   **Platform Philosophy:**
    *   **Dynatrace** is known for its highly automated, AI-driven approach to observability, offering deep insights into application performance with minimal manual configuration.
    *   **Micromegas** provides a unified data model for all telemetry types within your own cloud environment, offering greater control and cost efficiency for high-volume data.

*   **Pricing Model:** The DPS rate card offers granular, capability-based pricing. This can be predictable for standard monitoring but diverges dramatically at extreme metric or trace volumes.

*   **Cost at Scale:** The gap between Dynatrace and Micromegas widens as data volume increases. At the reference workload, it's 1.5×. For organizations with high-cardinality custom metrics, the difference can be orders of magnitude.

*   **Control & Data Ownership:** Micromegas provides full data ownership within your own cloud account — a critical requirement for many organizations.

---

## References

1. [Dynatrace Rate Card](https://www.dynatrace.com/pricing/rate-card/)
2. [Dynatrace DPS Overview](https://www.dynatrace.com/pricing/dynatrace-platform-subscription/)
