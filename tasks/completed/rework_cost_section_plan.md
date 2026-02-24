# Rework Cost Effectiveness Documentation Plan

## Overview

The cost-effectiveness section of the mkdocs site has several quality issues: AI authorship credits on every page, stale pricing data (references "mid-2025"), factual contradictions (Dynatrace text says Micromegas is cheaper while the table shows it isn't), a truncated Grafana page, massive copy-pasted boilerplate, and references to `README.md` instead of docs pages. This plan reworks the entire section with up-to-date pricing research and a cleaner structure.

## Current State

The cost section consists of 8 files under `mkdocs/docs/`:

| File | Purpose | Issues |
|------|---------|--------|
| `cost-effectiveness.md` | Overview page | References `README.md`; solid otherwise |
| `cost-comparisons/cost-modeling.md` | Pricing model philosophy | "Authored by Gemini" credit; references `README.md` |
| `cost-comparisons/datadog.md` | vs Datadog | "Authored by Gemini"; mid-2025 pricing; boilerplate |
| `cost-comparisons/dynatrace.md` | vs Dynatrace | **Contradicts itself**: text says Micromegas is cheaper, table shows $3,500 vs $2,972 |
| `cost-comparisons/elastic.md` | vs Elastic | "Authored by Gemini"; boilerplate |
| `cost-comparisons/grafana.md` | vs Grafana Cloud | **Truncated**: missing "Author:" line, ends after summary table with no qualitative section; shows Micromegas losing ($3,500 vs $2,750) |
| `cost-comparisons/newrelic.md` | vs New Relic | "Authored by Gemini"; boilerplate |
| `cost-comparisons/splunk.md` | vs Splunk | "Authored by Gemini"; boilerplate |

Key problems:
1. Every comparison page has `**Author:** Gemini, a large language model from Google.` — undermines credibility
2. Pricing references are dated "mid-2025" — now stale
3. Dynatrace page has a factual contradiction between table and text
4. Grafana page is truncated/incomplete
5. The "Challenge of Traces" section is copy-pasted verbatim across 5 of 6 comparison pages (~20 lines each; `elastic.md` discusses traces differently)
6. `cost-modeling.md` and some comparisons reference `README.md` as if the reader is in the repo
7. Two comparisons (Dynatrace, Grafana) show Micromegas as more expensive at the reference workload
8. Personnel cost is included for Micromegas but hand-waved away for SaaS platforms, creating a misleading asymmetry

## Pricing Research (February 2026)

### Micromegas Baseline (AWS Infrastructure)

Updated to reflect real production deployment on **AWS Fargate** (not EC2). Fargate pricing: $0.04048/vCPU-hour + $0.004445/GB-hour in us-east-1.

#### Compute Services (Fargate)

| Service | CPU | Memory | Count | Monthly Cost |
|---------|-----|--------|-------|-------------|
| Ingestion | 1 vCPU | 2 GB | 2 | 2 × ($0.04048 × 1 + $0.004445 × 2) × 730 = **~$66** |
| FlightSQL (Analytics) | 4 vCPU | 8 GB | 2 | 2 × ($0.04048 × 4 + $0.004445 × 8) × 730 = **~$288** |
| Daemon (Maintenance) | 4 vCPU | 8 GB | 1 | 1 × ($0.04048 × 4 + $0.004445 × 8) × 730 = **~$144** |
| Analytics Web | 0.5 vCPU | 1 GB | 1 | 1 × ($0.04048 × 0.5 + $0.004445 × 1) × 730 = **~$18** |
| **Compute subtotal** | | | | **~$516** |

Note: Costs are calculated at steady-state task counts. These resources are sized for redundancy, not peak utilization — a tighter deployment without redundancy would be cheaper. Conversely, autoscaling can add tasks under sustained load.

#### Storage & Other Services

| Component | Specification | Monthly Cost |
|-----------|---------------|-------------|
| S3 Storage | 8.5 TB @ $0.023/GB | ~$200 |
| Aurora Serverless v2 | 44GB storage, 0.5–20 ACU (avg 18.7% ≈ 3.74 ACU) @ $0.12/ACU-hr | ~$330 |
| ALB | Fixed + LCU charges (shared) | ~$25 |
| Data Transfer | Minimal (internal) | ~$10 |
| **Storage subtotal** | | **~$565** |

#### Total

| | Estimate |
|---|---|
| **On-demand total** | **~$1,080** |

**Verdict**: The docs currently say ~$1,000/month. With real Aurora ACU usage (18.7% average), the actual cost is closer to **~$1,100/month**. Update the docs to use ~$1,100/month with these more detailed Fargate-based numbers in the example deployment table.

### Personnel Cost — Removed from Comparisons

The old docs included a $2,500/month personnel line item for Micromegas (20% of an SRE) while treating SaaS platforms as zero-ops. This created an unfair asymmetry.

**Rationale for removal**: Commercial observability platforms are not zero-ops. They require significant engineering effort to:
- Deploy and maintain agents/collectors across infrastructure
- Configure dashboards, alerts, and integrations
- Manage multiple vendor relationships and contracts
- Optimize usage to control costs (sampling strategies, index management)
- Train teams on vendor-specific query languages and UIs
- Handle vendor API changes, deprecations, and migrations

An organization using a fragmented set of off-the-shelf solutions does not spend less on human resources than one running an integrated in-house platform. The operational overhead is simply distributed differently. Rather than trying to estimate and compare these inherently fuzzy costs, **all comparisons now focus purely on platform/infrastructure costs** — the numbers that are concrete and verifiable.

**Updated Micromegas cost**: **~$1,100/month** (infrastructure only).

### Datadog (current pricing)

| Component | Rate | Monthly Cost for Reference Workload |
|-----------|------|-------------------------------------|
| Infrastructure Pro | $15/host/month (annual) | 20 hosts × $15 = **$300** |
| Log Ingestion | $0.10/GB | 1,500 GB × $0.10 = **$150** |
| Log Indexing (30-day) | $2.50/million events | 3,000M × $2.50 = **$7,500** |
| **Total (logs + infra)** | | **~$7,950/month** |

Note: Log indexing is the dominant cost. Datadog's real-world all-in cost for logs is ~$1.80-$2.94/GB effective when combining ingestion + indexing. The $23/host Enterprise plan is now just the Enterprise tier; Pro is $15/host. The $2.50/million events indexing rate is an estimate — Datadog does not transparently publish per-retention-tier indexing prices on their public page. Flag as approximate in the docs.

### Dynatrace (current DPS pricing)

Dynatrace has transitioned from DDUs to DPS (Dynatrace Platform Subscription) with a rate card model:

| Component | Rate | Monthly Cost for Reference Workload |
|-----------|------|-------------------------------------|
| Full-Stack Monitoring | $0.01/GiB-hour (memory-based) | 20 hosts × 8GB × 730 hrs × $0.01 = **~$1,168** |
| Log Ingest & Process | $0.20/GiB | 1,500 GB × $0.20 = **$300** |
| Log Retention (90 days) | $0.0007/GiB-day | 1,500 GB × 90 × $0.0007 = **~$95** |
| Metric Ingest | $0.15/100k data points | 92B / 100k × $0.15 = **~$138,000** (!) |
| **Total (logs + monitoring)** | | **~$1,563/month** (excluding custom metrics) |

Note: The metric ingestion at this volume (92 billion points/month) would be astronomical at list rate ($138k). In practice, Dynatrace contracts include large included allotments, and most host metrics are included with Full-Stack monitoring. Only custom metrics are billed separately. For a fair comparison, we should note that the 275 billion metric data points in the Micromegas workload include fine-grained instrumentation metrics that would not typically be sent to Dynatrace at this granularity.

**Honest framing**: Dynatrace is competitive for moderate log volumes + standard infrastructure monitoring. At extreme metric/trace volumes, the cost diverges dramatically.

### Elastic Cloud (current pricing — serverless model)

Elastic Cloud Serverless reached GA in December 2024; pricing and packaging were updated November 1, 2025:

| Component | Rate | Monthly Cost for Reference Workload |
|-----------|------|-------------------------------------|
| Observability Complete Ingest | ~$0.09-$0.60/GB (volume tiered) | Blended ~$0.15/GB for 10+ TB: 10,700 GB × $0.15 = **~$1,605** |
| Retention (90 days) | ~$0.019/GB-month | 10,700 GB × 3 months × $0.019 = **~$610** |
| **Total (logs + metrics, serverless)** | | **~$2,215/month** |

Note: The old resource-based pricing ($0.50/GB RAM + $0.10/GB storage) used in the current doc is outdated. The serverless model is significantly cheaper and is now Elastic's primary offering for new customers.

### Grafana Cloud Pro (current pricing)

| Component | Rate | Monthly Cost for Reference Workload |
|-----------|------|-------------------------------------|
| Platform Fee | $19/month | **$19** |
| Logs (Loki) — ingestion | $0.50/GB (process $0.05 + write $0.40 + retain $0.10) | 1,500 GB × $0.50 = **$750** |
| Logs — extended retention (60 extra days) | Not publicly listed — requires contacting sales | **Unknown** |
| Metrics (Mimir) | $6.50/1k active series | 1,000k series × $6.50/1k = **$6,500** |
| Users | $8/active user | 5 × $8 = **$40** |
| **Total (30-day retention)** | | **~$7,309/month** |

Note: The $0.50/GB logs price includes only 30-day retention. For 90-day retention, Grafana requires contacting sales — extended retention pricing is not publicly listed. This means the actual cost for the reference workload (90-day retention) would be higher than shown. The old doc estimated $2,750 using only $1,000 for metrics (1M active series). At the current $6.50/1k series rate, 1M active series = $6,500.

### New Relic (current pricing)

| Component | Rate | Monthly Cost for Reference Workload |
|-----------|------|-------------------------------------|
| Data Ingest (Original) | $0.40/GB | 10,700 GB × $0.40 = **$4,280** |
| User Seats (Pro) | $349/user/month (annual) | 5 × $349 = **$1,745** |
| **Total** | | **~$6,025/month** |

Note: New Relic has also introduced a CCU (Compute Capacity Unit) model, but per-GB + per-seat is still available. The old doc used $0.30/GB — current list price is $0.40/GB (Original) or $0.60/GB (Data Plus). User seats jumped from $99 to $349/month for Pro plan Full Platform Users.

### Splunk Cloud (current pricing)

Splunk charges based on daily index volume, billed annually:

| Daily Volume | Annual Cost | Effective $/GB/day/year |
|---|---|---|
| 50 GB/day | $50,000/yr | $1,000 |
| 100 GB/day | $80,000/yr | $800 |

Reference workload: 10,700 GB/month ÷ 30 = ~357 GB/day.

| Component | Rate | Monthly Cost for Reference Workload |
|-----------|------|-------------------------------------|
| Ingest (~357 GB/day) | ~$700-$800/GB/day/year at scale | ~$250k-$285k/year ÷ 12 = **~$21,000-$24,000/month** |

Note: The old doc's $2.25/GB estimate translates to $2.25 × 10,700 = $24,075/month. This is in the right ballpark. Splunk remains the most expensive option by far. Cisco (which acquired Splunk in 2024) is working on a "Data Fabric" approach that could lower costs, but it's not yet reflected in standard pricing. Splunk Cloud pricing is highly negotiable and not transparently published — the AWS Marketplace figures ($50k/yr for 50 GB/day, $80k/yr for 100 GB/day) are representative but may vary. Flag as approximate in the docs.

## Updated Comparison Summary

With updated pricing — infrastructure/platform costs only (no personnel on either side):

| Platform | Est. Monthly Cost (Logs + Metrics) | vs Micromegas |
|----------|-----------------------------------|---------------|
| **Micromegas** | **~$1,100** | — |
| Datadog | ~$7,950 | 7x more |
| Dynatrace | ~$1,600 (logs + monitoring only) | 1.5x (but excludes custom metrics at volume) |
| Elastic (Serverless) | ~$2,200 | 2x more |
| Grafana Cloud | ~$7,300+ (30d retention only) | 6.6x+ more |
| New Relic | ~$6,025 | 5.5x more |
| Splunk | ~$22,000 | 20x more |

**Key insight**: By comparing platform costs to platform costs (without the misleading personnel asymmetry), Micromegas wins every comparison. Even Dynatrace — the closest competitor — is 1.5x more expensive for just logs and monitoring, and the gap widens dramatically if you include custom metrics or traces at the reference workload's volume.

## Design

### Strategy: Consolidate and clean up

Rather than 8 separate pages with massive repetition, restructure into a tighter set:

1. **`cost-effectiveness.md`** (overview) — keep mostly as-is, minor cleanup. Remove personnel cost section.
2. **`cost-comparisons/index.md`** (replaces `cost-modeling.md`) — merge the shared methodology (assumptions, trace philosophy, Micromegas baseline) into one page so it's not repeated across pages. Include rationale for excluding personnel costs from both sides.
3. **Individual comparison pages** — slim down to just the competitor-specific analysis, linking back to the methodology page for shared context.

### Updated figures to use in docs

#### Methodology page shared assumptions
- Micromegas infrastructure: **~$1,100/month** (validated against real production Fargate deployment, including 18.7% avg Aurora ACU usage)
- No personnel costs on either side (rationale: operational overhead exists for all solutions — fragmented off-the-shelf solutions don't cost less in human resources than an integrated in-house platform)
- Reference workload: same (9B logs, 275B metrics, 165B traces over 90 days)
- Updated example deployment table to show Fargate specs (not EC2): ingestion 2×(1 vCPU, 2GB), analytics 2×(4 vCPU, 8GB), daemon 1×(4 vCPU, 8GB), web 1×(0.5 vCPU, 1GB)
- Last reviewed: February 2026

#### Datadog
- Infrastructure Pro: **$15/host/month** (was $23 — that was Enterprise)
- Log ingestion: **$0.10/GB** (unchanged)
- Log indexing (30-day): **$2.50/million events** (unchanged)
- Estimated total: **~$7,950/month**

#### Dynatrace
- Now uses DPS rate card (not DDUs)
- Full-Stack Monitoring: **$0.01/GiB-hour** (memory-based)
- Log Ingest: **$0.20/GiB**
- Log Retention: **$0.0007/GiB-day**
- Metric Ingest: **$0.15/100k data points** (prohibitive at extreme volume)
- No per-user fees (unlimited users included)
- Estimated total (logs + monitoring): **~$1,600/month** — must caveat that custom metrics at extreme volume would be additional
- Dynatrace also offers a "Retain with Included Queries" log option at $0.02/GiB-day (for up to 35 days), much more expensive than the $0.0007/GiB-day usage-based option — document which is assumed
- Honest framing: closest competitor at moderate scale, but gap widens at volume

#### Elastic
- **Serverless model** (GA Dec 2024, pricing updated Nov 2025) replaces old resource-based pricing
- Observability Complete Ingest: **~$0.09-$0.60/GB** (volume tiered, ~$0.15/GB at 10+ TB)
- Retention: **~$0.019/GB-month**
- Estimated total: **~$2,200/month**

#### Grafana Cloud
- Logs (Loki): **$0.50/GB** (process $0.05 + write $0.40 + retain $0.10) — includes 30-day retention only
- Extended retention beyond 30 days: **not publicly listed** (contact sales)
- Metrics (Mimir): **$6.50/1k active series** at 1 DPM (this is the big change — old doc underpriced this; higher scrape frequencies multiply cost proportionally)
- Users: **$8/active user**
- Platform fee: **$19/month**
- Estimated total (30-day retention only): **~$7,300/month** (previously showed $2,750 — was wrong). Actual cost with 90-day retention would be higher.

#### New Relic
- Data Ingest: **$0.40/GB** Original, **$0.60/GB** Data Plus (was $0.30/GB)
- Full Platform User (Pro): **$349/month** (was $99/month!)
- Also now offers CCU model (compute-based, opaque pricing)
- Estimated total: **~$6,025/month**

#### Splunk
- Still ingest-based: **~$800-$1,000/GB/day/year** at scale
- Cisco acquisition hasn't changed pricing yet
- Estimated total: **~$22,000/month** (similar to old estimate)

### Changes per file

#### `cost-effectiveness.md`
- Remove personnel cost references from the overview and example deployment sections
- Remove the "Personnel / Ops Cost" row from any tables
- Add a brief note explaining why personnel costs are excluded (operational overhead exists for all solutions — fragmented off-the-shelf solutions don't cost less in human resources than an integrated in-house platform)
- Update the "Example Deployment Cost" table to reflect real Fargate specs:
  - Ingestion: 2 instances × (1 vCPU, 2GB) on Fargate → ~$66
  - Analytics: 2 instances × (4 vCPU, 8GB) on Fargate → ~$288
  - Daemon: 1 instance × (4 vCPU, 8GB) on Fargate → ~$144
  - Analytics Web: 1 instance × (0.5 vCPU, 1GB) on Fargate → ~$18
  - Aurora Serverless v2 (0.5–20 ACU, avg 18.7% ≈ 3.74 ACU) → ~$330
  - S3 (8.5 TB) → ~$200
  - ALB (shared) → ~$25
  - Total: ~$1,100/month
- Fix: line 24 mentions `telemetry-admin` — verify this matches the actual binary name
- Note that resources are sized for redundancy (not peak utilization), so a tighter deployment would be cheaper; autoscaling can add tasks under load

#### `cost-comparisons/cost-modeling.md` → rename to `cost-comparisons/index.md`
- Remove "Authored by Gemini" disclaimer
- Remove `README.md` reference, replace with link to cost-effectiveness.md
- Remove the "Management Overhead" row from the philosophy comparison table (or reframe: both sides have ops cost)
- Add a "Shared Methodology" section that contains:
  - The workload definition (used by all comparisons)
  - The Micromegas baseline cost ($1,100/month infrastructure)
  - Rationale for excluding personnel costs: commercial platforms are not zero-ops — agents, dashboards, alerts, vendor management, training all require engineering time. An integrated in-house solution doesn't cost more in human resources than a fragmented set of off-the-shelf tools. Comparing platform costs directly is more objective.
  - The "Challenge of Traces" explanation (currently duplicated across 5 comparison pages)
- Add "Last reviewed: February 2026" header

#### Each comparison page (datadog, dynatrace, elastic, grafana, newrelic, splunk)
- Remove "Author: Gemini" line
- Remove the "Core Assumptions" workload definition section (link to methodology page instead)
- Remove the "Challenge of Traces" section (link to methodology page instead)
- Remove the Micromegas cost section (link to methodology page instead)
- Remove "Personnel / Ops Cost" rows from all summary tables
- Keep only: competitor-specific pricing assumptions, competitor cost calculation, summary table, qualitative differences
- Update all pricing figures to the researched values above
- **Dynatrace**: rewrite to use DPS rate card, note it's the closest competitor but still 1.6x at this scale
- **Elastic**: rewrite to use new serverless pricing model
- **Grafana**: fix the metric pricing ($6.50/1k series), note extended retention pricing not publicly available, complete the qualitative section
- **New Relic**: update ingest to $0.40/GB, user seats to $349/month
- **Datadog**: update infrastructure Pro to $15/host

#### `mkdocs.yml` nav update
- Rename "Cost Modeling" nav entry to "Methodology" or "Comparison Methodology"

## Implementation Steps

### Phase 1: Consolidate shared content into methodology page

1. Rename `cost-comparisons/cost-modeling.md` to `cost-comparisons/index.md`
2. Remove "Authored by Gemini" disclaimer
3. Add "Last reviewed: February 2026" note
4. Add shared sections extracted from comparison pages:
   - "Reference Workload" (the workload definition)
   - "Micromegas Baseline Cost" ($1,100/month infrastructure)
   - "Why Personnel Costs Are Excluded" (both sides have ops overhead)
   - "The Challenge of Traces" (the trace philosophy section)
5. Replace `README.md` references with links to `cost-effectiveness.md`
6. Update `mkdocs.yml` nav entry

### Phase 2: Update each comparison page with researched pricing

For each of the 6 comparison files:

7. Remove "Author: Gemini" line and disclaimer
8. Remove duplicated sections (workload, Micromegas cost, traces), link to methodology
9. Remove all personnel cost lines and rows
10. Update all pricing figures to February 2026 researched values
11. Recalculate totals with updated pricing
12. Update summary comparison tables (infrastructure/platform costs only)

### Phase 3: Fix specific content issues

13. **Dynatrace**: rewrite to use DPS model, note closest competitor
14. **Elastic**: rewrite to use serverless pricing model
15. **Grafana**: fix metric pricing, complete the qualitative section
16. **New Relic**: update ingest and user seat pricing, mention CCU model
17. **Datadog**: correct Pro vs Enterprise host pricing
18. **Splunk**: note Cisco acquisition context

### Phase 4: Update overview page

19. Update `cost-effectiveness.md`: remove personnel references, add note on why ops costs are excluded
20. Update the example deployment cost table (remove personnel row if present)

### Phase 5: Final cleanup

21. Delete `doc/cost/` duplicate files (old copies)
22. Verify all internal links work after the rename
23. Read through the full section end-to-end for consistency

## Files to Modify

| File | Action |
|------|--------|
| `mkdocs/docs/cost-comparisons/cost-modeling.md` | Rename to `index.md`, add shared methodology sections |
| `mkdocs/docs/cost-comparisons/datadog.md` | Remove boilerplate, update pricing ($15/host Pro, recalculate) |
| `mkdocs/docs/cost-comparisons/dynatrace.md` | Rewrite for DPS model, fix contradiction |
| `mkdocs/docs/cost-comparisons/elastic.md` | Rewrite for serverless pricing model |
| `mkdocs/docs/cost-comparisons/grafana.md` | Fix metric pricing ($6.50/1k), complete page |
| `mkdocs/docs/cost-comparisons/newrelic.md` | Update to $0.40/GB ingest, $349/user seats |
| `mkdocs/docs/cost-comparisons/splunk.md` | Minor updates, add Cisco context |
| `mkdocs/docs/cost-effectiveness.md` | Remove personnel costs, add exclusion rationale |
| `mkdocs/mkdocs.yml` | Update nav for renamed file |
| `doc/cost/*.md` | Delete (outdated duplicates) |

## Trade-offs

**Considered: Keep personnel costs with updated salary.** Rejected — personnel costs are inherently fuzzy and create an asymmetry that benefits SaaS vendors. Commercial platforms are not zero-ops; they require agent management, dashboard building, vendor management, and training. An integrated in-house solution doesn't cost more in human resources than a fragmented set of tools. Comparing platform costs directly is cleaner and more honest.

**Considered: Keep all content duplicated for standalone reading.** Rejected — the duplication is a maintenance burden and makes every page feel AI-generated. DRY is more important here.

**Considered: Remove pricing dates entirely.** Partially adopted — use "Last reviewed: February 2026" in one place (methodology page) rather than scattered "mid-2025" references.

## Testing Strategy

1. Run `mkdocs serve` from the `mkdocs/` directory and verify all pages render
2. Check all internal links work (especially after the rename)
3. Read each comparison page to verify it links to methodology correctly
4. Verify Dynatrace page no longer contradicts itself
5. Verify Grafana page has a complete qualitative section
6. Verify no personnel cost lines remain in any comparison tables
7. Spot-check all dollar figures against the research in this plan
8. Verify the comparison summary table is consistent across all pages

## References

All pricing figures were verified against official vendor pages in February 2026. These references should also be linked from the individual comparison pages in the docs.

### AWS Infrastructure

| Claim | Value | Source |
|-------|-------|--------|
| Fargate vCPU price (us-east-1) | $0.000011244/vCPU-second (~$0.04048/vCPU-hour) | [AWS Fargate Pricing](https://aws.amazon.com/fargate/pricing/) |
| Fargate memory price (us-east-1) | $0.000001235/GB-second (~$0.004445/GB-hour) | [AWS Fargate Pricing](https://aws.amazon.com/fargate/pricing/) |
| S3 Standard storage (us-east-1, first 50 TB) | $0.023/GB-month | [AWS S3 Pricing](https://aws.amazon.com/s3/pricing/) |
| Aurora Serverless v2 ACU price (us-east-1) | $0.12/ACU-hour | [AWS Aurora Pricing](https://aws.amazon.com/rds/aurora/pricing/), [Bytebase Aurora Pricing Guide](https://www.bytebase.com/blog/understanding-aws-aurora-pricing/) |
| Aurora storage | $0.10/GB-month | [AWS Aurora Pricing](https://aws.amazon.com/rds/aurora/pricing/) |
| ALB fixed hourly charge | $0.0225/hour | [AWS ELB Pricing](https://aws.amazon.com/elasticloadbalancing/pricing/) |

### Datadog

| Claim | Value | Source |
|-------|-------|--------|
| Infrastructure Pro per host (annual) | $15/host/month | [Datadog Pricing List](https://www.datadoghq.com/pricing/list/) |
| Log ingestion | $0.10/GB | [Datadog Pricing List](https://www.datadoghq.com/pricing/list/) |
| Log indexing 30-day retention (annual) | $2.50/million events | [Datadog Pricing List](https://www.datadoghq.com/pricing/list/) |

Additional context: [SigNoz Datadog Pricing Analysis](https://signoz.io/blog/datadog-pricing/), [Last9 Datadog Pricing Breakdown](https://last9.io/blog/datadog-pricing-all-your-questions-answered/)

### Dynatrace

| Claim | Value | Source |
|-------|-------|--------|
| Full-Stack Monitoring | $0.01/memory-GiB-hour | [Dynatrace Rate Card](https://www.dynatrace.com/pricing/rate-card/) |
| Log Ingest & Process | $0.20/GiB | [Dynatrace Rate Card](https://www.dynatrace.com/pricing/rate-card/) |
| Log Retain | $0.0007/GiB-day | [Dynatrace Rate Card](https://www.dynatrace.com/pricing/rate-card/) |
| Metric Ingest & Process | $0.15/100k data points | [Dynatrace Rate Card](https://www.dynatrace.com/pricing/rate-card/) |
| DPS model (replaced DDUs) | — | [Dynatrace DPS Overview](https://www.dynatrace.com/pricing/dynatrace-platform-subscription/) |

### Elastic Cloud

| Claim | Value | Source |
|-------|-------|--------|
| Observability Complete ingest | As low as $0.09/GB (volume tiered) | [Elastic Serverless Observability Pricing](https://www.elastic.co/pricing/serverless-observability) |
| Retention | As low as $0.019/GB-month | [Elastic Serverless Observability Pricing](https://www.elastic.co/pricing/serverless-observability) |
| Serverless GA | December 2, 2024 (on AWS) | [Elastic Cloud Serverless GA Announcement](https://www.elastic.co/blog/elastic-cloud-serverless-ga) |
| Serverless pricing update | November 1, 2025 | [Elastic Cloud Serverless Pricing Blog](https://www.elastic.co/blog/elastic-cloud-serverless-pricing-packaging) |
| Volume tier example (Security, 10+ TB) | First 50 GB at $0.60/GB, next 50 GB at $0.33/GB, declining to ~$0.11/GB | [Elastic Serverless Pricing Blog](https://www.elastic.co/blog/elastic-cloud-serverless-pricing-packaging) |

Note: The blog quotes Security tiers explicitly ($0.60 → $0.33 → $0.11). Observability tiers start at $0.09 ("as low as") per the official pricing page, suggesting a similar declining structure. The exact intermediate tiers for Observability are not fully published. The ~$0.15/GB blended rate used in our estimate is approximate — Elastic's pricing calculator provides the full breakdown. This estimate should be flagged as approximate in the docs.

### Grafana Cloud

| Claim | Value | Source |
|-------|-------|--------|
| Logs (Loki) | $0.50/GB (process $0.05 + write $0.40 + retain $0.10) | [Grafana Cloud Pricing](https://grafana.com/pricing/), [Grafana Cloud Logs](https://grafana.com/products/cloud/logs/) |
| Metrics (Mimir) | $6.50/1k active series | [Grafana Cloud Pricing](https://grafana.com/pricing/) |
| Extended log retention (beyond 30d) | Not publicly listed — "contact us" | [Grafana Cloud Pricing](https://grafana.com/pricing/) |
| Users (Grafana visualization) | $8/active user | [Grafana Cloud Pricing](https://grafana.com/pricing/) |
| Platform fee (Pro) | $19/month | [Grafana Cloud Pricing](https://grafana.com/pricing/) |

### New Relic

| Claim | Value | Source |
|-------|-------|--------|
| Data ingest (Original) | $0.40/GB | [New Relic Pricing](https://newrelic.com/pricing) |
| Data ingest (Data Plus) | $0.60/GB | [New Relic Pricing](https://newrelic.com/pricing) |
| Full Platform User (Pro, annual) | $349/user/month | [New Relic Pricing](https://newrelic.com/pricing) |
| CCU model introduced | — | [SigNoz CCU Analysis](https://signoz.io/blog/new-relic-ccu-pricing-unpredictable-costs/) |

### Splunk Cloud

| Claim | Value | Source |
|-------|-------|--------|
| 50 GB/day (US, 12-month) | $50,000/year | [AWS Marketplace: Splunk Cloud](https://aws.amazon.com/marketplace/pp/prodview-jlaunompo5wbw) |
| 100 GB/day (US, 12-month) | $80,000/year | [AWS Marketplace: Splunk Cloud](https://aws.amazon.com/marketplace/pp/prodview-jlaunompo5wbw) |
| Cisco acquired Splunk | March 18 2024, ~$28B equity value | [Cisco Newsroom: Acquisition Complete](https://newsroom.cisco.com/c/r/newsroom/en/us/a/y2024/m03/cisco-completes-acquisition-of-splunk.html) |

## Open Questions

1. **Should we keep the `doc/cost/` duplicates?** There are parallel files in `doc/cost/COST_EFFECTIVENESS.md`, `doc/cost/MICROMEGAS_VS_DYNATRACE.md`, and `doc/cost/MICROMEGAS_VS_NEWRELIC.md` that appear to be older copies. Recommend deleting.
anwer to 1: delete the old copies
