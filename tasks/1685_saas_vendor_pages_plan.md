# Split SaaS Vendor Comparison into Per-Vendor Pages Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1685

## Status

**Done**: Implementation Steps 1–6, on branch `comparisons`, not pushed.
- Six vendor pages restored at `/docs/cost-comparisons/<vendor>/`; the hub is reworked; redirects,
  nav, anchor validation and the `mkdocs>=1.6.0` floor are in place; `llms.txt` links the new
  pages; the #1637 changelog entry is amended.
- Review-loop fixes are in: 30-day log-retention labels on Datadog, New Relic and Grafana; a
  Datadog custom-metrics caveat; consistent caveats in `llms.txt`; factual README wording.
- A #1685 `CHANGELOG.md` entry is written. It still says Elastic is ~0.6× and gets updated in
  Step 10.
- `mkdocs build --strict` and `pytest build/test_check_docs_site.py` pass.
- Commits: `d6252153a`, `c1f2374f0`, `36bb6a084`, `676dafde4`, `35913d9ff`.

**Done (fairness corrections)**: Step 7. Billing sizes were measured on the production deployment
on 2026-10-18; see **Billing-size measurement**. The script is at
`tasks/1685_billing_sizes/measure_billing_sizes.py`.

**Done (fairness corrections, continued)**: Steps 8–11, in commit `d619c5c78`.
- Workload refreshed from the last 30 days.
- Measured billing sizes applied; traces priced where a rate is published; Elastic shown as a
  range.
- Micromegas costed at ~$1,100/month on a single provider's public list prices.
- Hub, vendor pages, `llms.txt`, README and changelog rewritten.
- `mkdocs build --strict` and the docs-site tests pass.

**Left to do**: push and open the PR.

The pricing figures in **Pricing Re-verification** are the rates as verified. Its per-vendor
totals and ratios are *pre-correction* numbers, superseded by **Decisions**.

## Overview

#1641 folded the six SaaS cost comparisons into one ~4,300-word page,
`when-to-use/saas-vendors.md`, and turned the old `/docs/cost-comparisons/<vendor>/` URLs into
meta-refresh stubs pointing at `#vs-<vendor>` anchors. The only search demand on the site that
isn't for the Micromegas name is "<vendor> pricing / cost" queries, and AI Overviews cite the
comparison content. A section anchor on a page titled "vs. SaaS Vendors" can't rank for those
queries. This plan restores six standalone vendor pages at the original URLs. They share one
structure that puts the vendor's pricing first. `saas-vendors.md` stays the hub for the shared
methodology. All six vendors' rates are re-verified together, and every page carries the same
verification month (October 2026).

## Current State

- `mkdocs/docs/when-to-use/saas-vendors.md` (487 lines):
  - Front matter `description:`, TL;DR, `*Last reviewed: June 2026*` (line 11).
  - "At a glance" table (lines 17–24), linking to `#vs-<vendor>` anchors.
  - Cost Philosophy, Primary Cost Drivers, Example Deployment Cost, Cost Management Features.
  - Methodology (line 118): Pricing Models, Reference Workload, Why Personnel Costs Are
    Excluded, The Challenge of Traces, When Micromegas is Cost Effective.
  - Six `## vs. <Vendor>` sections (lines 211–487). Each has a disclaimer ("verified in early
    2026"), the pricing breakdown, a cost-comparison table, qualitative differences, and
    sources.
- `mkdocs/mkdocs.yml`:
  - `redirects.redirect_maps` (lines 57–65) map `cost-comparisons/{datadog,dynatrace,elastic,grafana,newrelic,splunk}.md`
    to the anchors. They also map `cost-effectiveness.md` and `cost-comparisons/index.md` to
    the hub.
  - The nav lists `vs. SaaS Vendors: when-to-use/saas-vendors.md` (line 111).
  - The build runs with `strict: true`.
- The pre-#1641 page files (`git show 23c5ff839^:mkdocs/docs/cost-comparisons/<vendor>.md`) are
  where the original filenames come from. Note `grafana.md` and `newrelic.md`, not
  `grafana-cloud.md` / `new-relic.md`.
- `welcome/public/llms.txt` lines 72–77 link to the six `#vs-<vendor>` anchors.
- `build/check_docs_site.py` check 6 requires every on-site `llms.txt` link to resolve to a
  staged file. Check 4 requires canonical tags on real pages and resolvable targets on
  refresh stubs. Neither needs code changes.
- `mkdocs/overrides/main.html` only overrides `<title>` for blog posts. Other pages use
  Material's `base.html`, which uses `page.meta.title` (front matter `title:`) for `<title>`
  when set. So a page can have a short nav label and a long, search-oriented HTML title.
- Other inbound links (`when-to-use/index.md:182,259-260`, `README.md:61`,
  `welcome/src/components/Footer.tsx:31`) point at the hub or at its non-vendor anchors, which
  stay. The test fixture `build/test_check_docs_site.py:269` uses `#vs-datadog` as an
  arbitrary string and is not tied to the docs.

## Pricing Re-verification (October 2026)

All rates below were checked against official pricing pages on 2026-10-18. Unless marked
otherwise, they are unchanged from "early 2026". Reference workload: 20 hosts × 8 GiB, 90-day retention, ~1,500 GB/month
of logs (3B entries), ~9,200 GB/month of metrics (~92B points, ~1M active series), ~100 users,
~10,700 GB/month in total, ~357 GB/day.

Where a vendor publishes a free or included allotment, the estimate now subtracts it.

### Datadog — ~$7,950/month, ~7.2× (unchanged)
- Infra Pro $15/host-month annual ($18 month-to-month) → `20 × $15 = $300`.
- Log ingest $0.10/GB → `1,500 × $0.10 = $150`.
- Log indexing, 30-day retention: $2.50 per 1M events annual → `3,000 × $2.50 = $7,500`.
  This is now confirmed on the list page, so drop the old "not transparently published" note.
  Other retention periods are published: 3d $1.06, 7d $1.27, 15d $1.70. A retention of more
  than 30 days is "Contact Us".
- Longer retention option: Flex Logs storage costs $0.05 per 1M events stored per month. Three
  months stored comes to `9,000 × $0.05 = ~$450/month`, plus Flex compute, which is
  contact-sales. Mention this as the 90-day path; it is not part of the headline figure.
- Included: unlimited users, host metrics, and 100 custom metrics per host on Pro (overage $5
  per 100). Billed separately: log ingest and indexing (priced per retention period), Flex
  storage/compute, and APM at $31/host annual with Infra.
- Custom metrics caveat: the headline excludes the workload's ~998k custom-metric series
  (~1M minus the 2,000 included for 20 hosts), ~$49,900/month at the $5 per 100 overage. Say
  so on the page, with the usual note that fine-grained metrics like these wouldn't typically be
  sent to Datadog at this granularity.
- The old "effective cost of logs ~$1.80–$2.94/GB" figure is wrong. Replace it with ~$5.10/GB:
  3,000M events / 1,500 GB = 2M events/GB, so indexing is $5.00/GB plus $0.10 ingest
  (`$7,650 / 1,500`).
- Sources: datadoghq.com/pricing/list/,
  datadoghq.com/pricing/?product=log-management,
  docs.datadoghq.com/account_management/billing/custom_metrics/.

### Dynatrace — ~$1,600/month (logs + monitoring), ~1.5× (unchanged)
- Full-Stack $0.01/memory-GiB-hour → `20 × 8 × 730 × $0.01 = $1,168`.
- Log Ingest & Process $0.20/GiB → `$300`. Log Retain $0.0007/GiB-day → `1,500 × 90 × $0.0007 = ~$95`.
  Total ~$1,563.
- Metrics: $0.15 per 100k data points. Full-Stack includes 900 custom-metric data points per GiB
  per 15-minute interval: `160 GiB × 900 × 2,920 intervals ≈ 420M/month`. That covers about
  0.5% of the workload's ~92B points/month, so the remainder would cost
  `~91.6B / 100k × $0.15 ≈ $137,000/month`. Keep the existing caveat that the headline excludes
  custom metrics, and use the included-allotment figure to make the point concrete.
- Other confirmed rates: Retain with Included Queries $0.02/GiB-day, Log Query $0.0035/GiB
  scanned, traces $0.20/GiB ingest + $0.0007/GiB-day retain, unlimited users (no seat fees).
- Sources: dynatrace.com/pricing/rate-card/, dynatrace.com/pricing/, and
  docs.dynatrace.com/docs/license/capabilities/app-infra-observability/full-stack-monitoring.

### Elastic Cloud Serverless — ~$660/month, ~0.6× (CHANGED: metrics repriced 2026-07-01)
- Logs, Observability Complete:
  - Ingest is volume-tiered, "as low as $0.09/GB", and the tier breakpoints are not published.
    Keep the prior blended assumption of ~$0.15/GB, stated as an assumption:
    `1,500 × ~$0.15 = ~$225` (range `$135`–`$900` across $0.09–$0.60/GB). The ~$0.15/GB figure
    assumes the volume tiers count combined ingest (~10.7 TB/month); that is an assumption, not
    a verified fact. If the tiers count only the 1.5 TB of logs, the rate is higher, toward the
    range's upper end. The page must say the same.
  - Retention $0.019/GB-month → `1,500 × 3 × $0.019 = ~$86`.
- Metrics. New pricing effective July 1, 2026: time-series metrics cost 25% of the standard rate.
  - Ingest $0.023/GB → `9,200 × $0.023 = ~$212`.
  - Retention $0.005/GB-month → `9,200 × 3 × $0.005 = ~$138`.
- Total ~$660/month, which is **below** the Micromegas ~$1,100. The total range is
  ~$570–$1,340/month (~0.5×–1.2×): log ingest $135–$900 plus fixed $86 + $212 + $138. "Below
  Micromegas" holds under the blended-rate assumption (headline ~$660, ~0.6×); the upper bound
  assumes the $0.60 first-tier rate on all logs. The page states the headline together with that
  caveat, and also says that the Micromegas figure includes 165B trace events while this one
  excludes traces. The pricing section opens with the range, because the log-ingest tiers are
  unpublished.
- Also mention Logs Essentials (ingest as low as $0.07/GB, retention as low as $0.017/GB-month)
  as the cheaper, reduced-feature tier. No per-user fees. Egress: first 50 GB free, then $0.05/GB.
- Sources: elastic.co/pricing/serverless-observability,
  elastic.co/blog/elastic-cloud-serverless-pricing-packaging, and
  elastic.co/docs/deploy-manage/cloud-organization/billing/elastic-observability-billing-dimensions.

### Grafana Cloud Pro — ~$8,000+/month (30-day log retention only), ~7.2×
- Platform fee: $19.
- Logs: $0.50/GB ($0.05 process + $0.40 write + $0.10 retain), with 30-day retention and 50 GB
  included → `1,450 × $0.50 = $725`.
- Metrics: $6.50 per 1k active series at 1 DPM, with 10k series included and 13-month retention
  → `990 × $6.50 = $6,435`.
- Users: $8/active user, with 3 included → `97 × $8 = $776`.
- Total ~$7,955. Retention beyond 30 days is billed per GB "for each additional 30-day
  increment" at an unpublished rate. Keep the "+" and the note that 90 days costs more.
- Also confirmed: traces are billed at the same $0.50/GB split as logs (50 GB included).
  Enterprise-plugin users cost $55/user. The Enterprise plan starts at a $25k/yr commit, with
  metrics "as low as $3/1k series".
- Sources: grafana.com/pricing/, and the Grafana Cloud metrics- and logs-invoice docs
  (grafana.com/docs/grafana-cloud/cost-management-and-billing/understand-your-invoice/).

### New Relic — ~$39,100/month, ~36×
- Original Data costs $0.40/GB above the 100 GB/month free → `10,600 × $0.40 = $4,240`.
- Pro Full Platform users cost $349/user-month annual ($418.80 monthly pay-as-you-go) →
  `100 × $349 = $34,900`.
- Total ~$39,140.
- Other prices: Core users $49, Basic users free, Data Plus $0.60/GB. CCU pricing is still
  offered but has no published rates. Logs, metrics and traces are all billed per GB at the same
  rate, with no host or custom-metric charges.
- Source: newrelic.com/pricing.

### Splunk Cloud — ~$22,000/month, ~20× (CHANGED: public list prices withdrawn)
- The AWS Marketplace listing (prodview-jlaunompo5wbw) now shows only "Custom pricing options",
  through private offers. splunk.com publishes the pricing *models* (ingest, workload/SVC, and a
  newer activity-based option) but no rates.
- Keep the ~$22,000/month estimate, built from the last publicly listed Marketplace tiers
  (50 GB/day $50k/yr, 100 GB/day $80k/yr; extrapolated at ~$700–$800 per GB/day-year to
  ~357 GB/day). The Splunk page carries the tier table itself, labelled as the last published
  AWS Marketplace list prices (early 2026), and says that current prices are private-offer only.
- Mention Splunk Observability Cloud per-host pricing (Infrastructure $15, App & Infra $60,
  End-to-End $75 per host-month, "starts at") as the metrics-side product. Unlimited users on
  Cloud Platform. Keep the Cisco acquisition note short.
- Sources: AWS Marketplace listing, splunk.com/en_us/products/pricing.html,
  splunk.com/en_us/products/pricing/observability.html.

## Fairness Corrections (follow-up, to do)

The first implementation (commits `d6252153a`–`676dafde4`) puts Elastic at ~0.6× Micromegas.
That result comes from three assumptions that favour Elastic. This phase removes them.

1. **Byte-billed vendors are priced on Micromegas's compact event sizes.** The reference
   workload assumes 500 B per log entry, 100 B per metric point and 1 KB per trace event, which
   is close to how Micromegas stores data. Elastic bills ingest *and* retention on "the fully
   enriched normalized data size at the end of the ingest pipeline, before Elasticsearch
   compression", and notes that this is "higher than the volumes traditionally reported by
   Elasticsearch index size" (elastic-observability-billing-dimensions, fetched 2026-10-18).
   Every billed document carries its own timestamp, field names and resource attributes. Metrics
   are ~9,200 of the ~10,700 GB/month, so the 100 B figure alone sets most of Elastic's total.
2. **Traces are excluded even where a vendor publishes a linear per-GB trace rate.** The
   Micromegas ~$1,100 includes 165B trace events. The "Challenge of Traces" rationale ("hundreds
   of thousands of dollars per month") was never computed. For Elastic it would be roughly
   $8–11k/month, not prohibitive, so leaving traces out is what makes Elastic look cheaper.
3. **The Elastic log rate assumes a combined-volume discount.** The ~$0.15/GB headline assumes
   the unpublished ingest tiers count logs and metrics together.

### Billing-size measurement

Replace the assumed event sizes with measured ones and apply them to every charge billed per GB.
Charges billed per event, data point, series or host are unaffected.

| Vendor | Charges billed per GB (re-priced) | Charges billed some other way (unchanged) |
| :--- | :--- | :--- |
| Elastic | logs, metrics, traces: ingest and retention | — |
| New Relic | all data ingest | user seats |
| Splunk | daily index volume | — |
| Dynatrace | log ingest/retain, trace ingest/retain | host memory-hours, metric data points |
| Grafana Cloud | logs, traces | metric series, users |
| Datadog | log ingest ($0.10/GB) | log indexing per event, hosts, custom metrics |

Method: take samples of `log_entries`, `measures` and thread spans from a deployment running the
reference workload with `micromegas-query --format json`. Include the process columns, because
OTLP resource attributes ride along on every document. Measure the mean serialized bytes per
row. Serialized JSON with the resource context is a stand-in for a vendor's pre-compression,
enriched document size; it is not a vendor's exact number. Write the script to
`tasks/1685_billing_sizes/measure_billing_sizes.py`, run it with the poetry venv in
`python/micromegas`, and record its output. It is a task artifact and is not shipped.

**Measured 2026-10-18** (Step 7, done). Twelve one-hour windows over the previous 6 days were
sampled: 30,000 log entries, 30,000 metric points, and 50,000 thread spans from the busiest CPU
streams.

| Signal | Old assumption | Measured mean | p50 | p95 |
| :--- | :--- | :--- | :--- | :--- |
| Log entry | 500 B | **~860 B** | 775 B | 1,184 B |
| Metric point | 100 B | **~940 B** | 904 B | 1,299 B |
| Span | 1 KB | **~1,050 B** | 916 B | 2,120 B |

The same run measured **~1.8 CPU stream events per thread span** (19.7M spans against 35.1M
events). Edge effects make this approximate. Vendors bill spans, not Micromegas stream events,
so convert trace event counts to spans with this ratio.

On the hub:
- The Reference Workload's "Data size assumptions" become **measured billing sizes**, each with
  a one-line method note.
- The Methodology states the rule: per-GB charges use the measured billing size. Micromegas's
  own cost is its actual bill and does not change.

### Reference deployment refresh

The hub's "Example Deployment Cost" and "Reference Workload" describe the production deployment
as it was in early 2026 (8.5 TB, 9B logs, 275B metric events, 165B trace events, ~$1,100/month).
Replace them with the current deployment's figures.

**Confidentiality.** Everything about this deployment stays anonymous ("a production
deployment"). Don't publish:
- the organization, the project, or what is instrumented;
- hostnames, account IDs or discount programmes;
- instance types;
- anything else beyond aggregate volumes, sizes and list-price infrastructure cost.

**Volumes, last 30 days** (2026-09-18 → 2026-10-18; `SUM(blocks.nb_objects)` by stream tag,
second and last prod query, approved by the user):

| Signal | Per 30 days | Retained (90 days) |
| :--- | :--- | :--- |
| Log entries | ~9.6B | ~29B |
| Metric points | ~138B | ~415B |
| CPU trace events | ~535B (~300B spans at 1.8 events/span) | ~1.6T |
| Network-trace events | ~10.8B (left out of vendor estimates) | — |

- Object storage is ~27.5 TB (bucket-size metric, mid-October).
- A whole-retention query was discarded: blocks date back to 2026-04-10, past the 90-day
  retention, so cumulative totals were ambiguous.
- The trace row counts stream events, not spans. Label it "trace events", and use the span
  conversion only for vendor pricing.

**Cost basis: a single cloud provider's public monthly list prices** (DigitalOcean) for resources
equivalent to the deployment. That means no commitment and no negotiated discounts: the same
footing as the vendors' list prices. The user chose this basis on 2026-10-18.

| Component | Equivalent resource | $/month |
| :--- | :--- | :--- |
| Object storage | ~27.5 TB S3-compatible | ~$512 |
| Services | 16 vCPU / 32 GiB | $336 |
| PostgreSQL | managed, 4 vCPU / 8 GiB, single node | $122 |
| Object cache | 2 vCPU / 16 GiB, local NVMe | $131 |
| Load balancer | | ~$12 |
| **Total** | | **~$1,100** |

- The provider is named only in the hub's bottom "Pricing References" section, not as an
  endorsement.
- **AWS, where the deployment runs:** ~$2,200/month at on-demand list prices. The docs always state
  it as "no discounts, rebates or Savings Plans".
- **Caveats** published on the hub:
  - a single PostgreSQL node;
  - writes to object storage count against the compute transfer allowance (same-region reads are
    free);
  - the sizing was translated from the deployment, not measured on that provider.

With the measured sizes, monthly billable volume on per-GB vendors is ~8.3 TB of logs, ~130 TB of
metrics and ~316 TB of spans. Full figures and arithmetic are in the scratch spec the
implementation followed. Its totals are recorded in **Decisions**.

### Traces priced where a linear rate is published

Each vendor page gets a second, like-for-like figure: logs + metrics + the reference trace volume
(~55B spans/month, 90-day retention, measured span size). Price it only where the vendor
publishes a per-GB or per-span rate:

- **Elastic:** standard per-GB ingest rate plus $0.019/GB-month retention. Traces are not
  metrics, so they don't get the 25% time-series rate.
- **Dynatrace:** $0.20/GiB ingest plus $0.0007/GiB-day retain. Subtract the Full-Stack included
  trace volume only if the docs confirm it; the design research did not fetch that page.
- **Grafana Cloud:** $0.50/GB with 50 GB included and 30-day retention. Label it 30-day like its
  logs.
- **New Relic:** $0.40/GB, Original Data, 30-day retention; Data Plus for 90 days, as its logs
  already do.
- **Datadog:** look up the ingested-span and indexed-span list rates and the per-host allotments
  on datadoghq.com/pricing/list/. If they are not published, keep traces excluded and say so.
- **Splunk:** no published rates, so traces stay excluded, stated on the page.

Rewrite "The Challenge of Traces" around this rule:
- Traces are priced wherever a vendor publishes a linear rate, and excluded only where it
  doesn't.
- The sampling argument stays, as the reason real deployments of those vendors would not ingest
  this volume.
- Drop the uncomputed "hundreds of thousands" claim.

### Elastic log tier

Drop the single ~$0.15/GB headline for Elastic logs. The tier breakpoints are unpublished, so the
Elastic page and its At a glance cell show a range: the $0.09/GB floor up to the $0.60/GB
first-tier rate, applied to the actual per-signal volumes. The Elastic page does not pick a point
inside the range.

### Presentation

- **At a glance:** keep the "logs + metrics" column and add a "with traces" column. Use "n/a
  (no published rate)" where traces stay excluded. The ratio column compares the with-traces
  figure to Micromegas where one exists, because that is the like-for-like number.
- **Rewrite after recomputing:** the TL;DR, the hub `description:`, each vendor page's lead,
  description, cost table and qualitative bullets, the `llms.txt` glosses, and the README
  paragraph. These follow the recomputed figures; do not carry forward "Elastic is cheaper" or
  "second-closest" wording that no longer holds.
- **Template:** the lead sentence "like every comparison on this site, it excludes traces" becomes
  vendor-specific: either "includes the reference trace volume at <rate>" or "excludes traces:
  <vendor> publishes no trace rate".
- **CHANGELOG:** update the #1685 entry, which currently says Elastic is ~0.6×, to the
  recomputed figures.

## Design

### URLs and files

The six pages go back to their original paths, so the existing URLs (which Search Console and AI
citations already know) become real pages again:

```
mkdocs/docs/cost-comparisons/datadog.md     → /docs/cost-comparisons/datadog/
mkdocs/docs/cost-comparisons/dynatrace.md   → /docs/cost-comparisons/dynatrace/
mkdocs/docs/cost-comparisons/elastic.md     → /docs/cost-comparisons/elastic/
mkdocs/docs/cost-comparisons/grafana.md     → /docs/cost-comparisons/grafana/
mkdocs/docs/cost-comparisons/newrelic.md    → /docs/cost-comparisons/newrelic/
mkdocs/docs/cost-comparisons/splunk.md      → /docs/cost-comparisons/splunk/
```

`cost-comparisons/index.md` and `cost-effectiveness.md` keep their redirects to the hub.

### Shared page template

Every vendor page uses exactly this skeleton, in this order:

```markdown
---
title: <Vendor> Pricing and Cost Breakdown vs. Micromegas
description: <Vendor> pricing explained (<1–2 key rates>) and applied to a 90-day, ~100-user
  logs-and-metrics workload: ~$X/month vs. ~$1,100 for self-hosted Micromegas. Rates verified October 2026.
---

# <Vendor> Pricing vs. Micromegas

<One sentence: what <Vendor> bills on.> <One sentence: the headline estimate and ratio.>
This page uses the shared [reference workload](../when-to-use/saas-vendors.md#reference-workload)
and [methodology](../when-to-use/saas-vendors.md#methodology); like every comparison on this
site, it excludes traces ([why](../when-to-use/saas-vendors.md#the-challenge-of-traces)).

*Rates verified October 2026. Estimates, not quotes — confirm current rates at <pricing URL>.*

## <Vendor> pricing on the reference workload
<bulleted line items with the arithmetic shown, subtotals, total>

## What's included and what's billed separately
<short bullets covering: users, host metrics, retention tiers, custom metrics, traces>

## Cost comparison
| Category | Micromegas | <Vendor> |
| :--- | :--- | :--- |
| **Platform/Infrastructure Cost** | ~$1,100/month | ~$X/month |
| **Ratio** | **1×** | **~N×** |

## Qualitative differences
<existing bullets from saas-vendors.md, carried over and adjusted where the numbers changed>

## Sources
<links>. Back to [all SaaS vendor comparisons](../when-to-use/saas-vendors.md#at-a-glance).
```

- **Title.** The front matter `title:` becomes the HTML `<title>`, so it can match "<vendor>
  pricing / cost" queries. The nav label stays short ("Datadog"). Use the vendor's product name
  where it differs: "Grafana Cloud", "Elastic Cloud", "Splunk Cloud".
- **No year in the title.** The verification month sits in the body and the description,
  so the title doesn't go stale and need re-editing every refresh.
- **Description** is a single line in the actual front matter. The wrapping above is only for
  readability here.
- **Qualitative content.** Bring each vendor's qualitative bullets over as-is, except where the
  new numbers contradict them:
  - Elastic: say that it is cheaper on logs and metrics alone.
  - Dynatrace: the "Closest Competitor" bullet becomes "second-closest". Elastic is now the
    closest, and cheaper.
  - Splunk: the "most expensive by a wide margin" claim conflicts with New Relic's ~36×. Reword
    it to "most expensive on data volume alone", which is what the current text means.

### Hub changes (`when-to-use/saas-vendors.md`)

- Delete the six `## vs. <Vendor>` sections (lines 211–487).
- **At a glance.**
  - Link each vendor to `../cost-comparisons/<vendor>.md`.
  - Update the figures: Datadog ~$7,950 / ~7.2×, Elastic ~$660 / ~0.6×, Grafana ~$8,000+ / ~7.2×, New Relic ~$39,100 / ~36×.
  - Add a column header or a note saying "Rates verified October 2026".
- **TL;DR** (line 9). The range now starts *below* Micromegas: Elastic Serverless at ~0.6× on
  logs and metrics alone, then Dynatrace at ~1.5×, up to New Relic at ~36×. Keep the
  traces-skew sentence, which matters more now that Elastic comes in below Micromegas.
- `*Last reviewed: June 2026*` → `*Last reviewed: October 2026*`.
- Methodology intro (line 120): "every vendor comparison below… Each vendor section" →
  "every vendor page… Each vendor page".
- Reword the front matter `description:` to match the new figures. Drop "at a fraction of
  commercial SaaS cost" and use a factual framing, e.g. "Micromegas's self-hosted cost model
  compared with Datadog, Dynatrace, Elastic, Grafana Cloud, New Relic and Splunk on one
  reference workload, with per-vendor pricing pages."
- The note at line 26 ("the Dynatrace figure excludes custom metrics") names Datadog alongside
  Dynatrace as excluding custom metrics.

### `mkdocs.yml`

- Remove the six vendor lines from `redirect_maps`. Keep `cost-effectiveness.md` and
  `cost-comparisons/index.md`.
- Add `validation:` / `  anchors: warn` so `--strict` fails on broken hub-anchor links. This
  needs MkDocs 1.6, so raise the floor in `mkdocs/docs-requirements.txt` to `mkdocs>=1.6.0`.
- Nav: turn `vs. SaaS Vendors` into a section.

```yaml
  - When to Use:
    - When to Use Micromegas: when-to-use/index.md
    - vs. LLM Agent Tools: when-to-use/agent-observability.md
    - vs. SaaS Vendors:
      - Overview: when-to-use/saas-vendors.md
      - Datadog: cost-comparisons/datadog.md
      - Dynatrace: cost-comparisons/dynatrace.md
      - Elastic: cost-comparisons/elastic.md
      - Grafana Cloud: cost-comparisons/grafana.md
      - New Relic: cost-comparisons/newrelic.md
      - Splunk: cost-comparisons/splunk.md
```

### `welcome/public/llms.txt`

Point lines 72–77 at `https://micromegas.info/docs/cost-comparisons/<vendor>/`. Append a short
gloss to each line with the headline figure, e.g. `: ~$7,950/month on the reference workload,
~7.2× Micromegas`, matching the gloss style of the neighbouring entries. Replace the line-71 hub
gloss, which says "each vendor", with "where the money goes, the comparison methodology and an
at-a-glance cost table, with a pricing page per vendor below."

## Implementation Steps

Steps 1–6: **done**.

1. Create the six `mkdocs/docs/cost-comparisons/<vendor>.md` pages from the template. Take the
   pricing from **Pricing Re-verification** and the qualitative bullets from the current
   `saas-vendors.md` sections.
2. Edit `mkdocs/docs/when-to-use/saas-vendors.md`: delete the vendor sections and update At a
   glance, the TL;DR, Last reviewed, the Methodology intro, the front matter `description:`, and
   the line-26 custom-metrics note (name Datadog alongside Dynatrace).
3. Edit `mkdocs/mkdocs.yml`: change the redirects and nav, and add `validation:` / `  anchors: warn`.
   Raise `mkdocs/docs-requirements.txt` to `mkdocs>=1.6.0`.
4. Edit `welcome/public/llms.txt`, lines 71–77.
5. Amend the Unreleased #1637 `CHANGELOG.md` entry so it says only `/cost-effectiveness/` and
   `/cost-comparisons/` redirect to the merged page.
6. Run `mkdocs build --strict` from `mkdocs/` and `python3 -m pytest build/test_check_docs_site.py`.
   The PR's "Build Docs" CI job (`publish-docs.yml`) is the authoritative full-stage
   `check_docs_site.py` run; a partial local stage would fail for unrelated reasons.
   After the build, grep the six built `site/cost-comparisons/<vendor>/index.html` files and
   confirm none contains `http-equiv="refresh"` (a leftover vendor redirect would overwrite the
   real page with a stub). Also confirm each has a `<title>` starting with "<Vendor> Pricing and
   Cost Breakdown vs. Micromegas" and a `<meta name="description">` tag.

Steps 7–11: **done**.

7. ~~Write and run `tasks/1685_billing_sizes/measure_billing_sizes.py` on the production
   deployment, and record the bytes per log entry, metric point and span.~~ Done. Results are in
   **Billing-size measurement**.
8. Look up Datadog's span rates, and check Dynatrace's included trace volume (see **Traces priced
   where a linear rate is published**).
9. Refresh the Micromegas side per **Reference deployment refresh**: list-price cost with S3 at
   its current size, and the new volumes. Recompute every vendor's per-GB charges with the new
   volumes and measured sizes, recompute per-event charges (e.g. Datadog log indexing, Dynatrace
   metric data points) with the new event counts, add the with-traces figure where a rate is
   published, and switch Elastic logs to the range. Update the six vendor pages accordingly.
10. Update the hub: Example Deployment Cost, Scale Perspective, the Reference Workload volumes and
    sizes, the Methodology rule, "The Challenge of Traces",
    At a glance (new column), the TL;DR and `description:`. Then update the `llms.txt` glosses,
    the README paragraph and the #1685 `CHANGELOG.md` entry.
11. Re-run step 6's checks.

## Files to Modify

- `mkdocs/docs/cost-comparisons/{datadog,dynatrace,elastic,grafana,newrelic,splunk}.md` (new)
- `mkdocs/docs/when-to-use/saas-vendors.md`
- `mkdocs/mkdocs.yml`
- `mkdocs/docs-requirements.txt` (`mkdocs>=1.6.0`)
- `welcome/public/llms.txt`
- `CHANGELOG.md` (amend the Unreleased #1637 entry; update the #1685 entry)
- `README.md` (cost paragraph, after recomputing)
- `tasks/1685_billing_sizes/measure_billing_sizes.py` (new task artifact, not shipped)

## Documentation

This change is documentation. `CHANGELOG.md` gets an entry from the `pr` stage. Nothing else
links to the vendor anchors.

## Testing Strategy

The shipped change has no new code, so no unit tests. The step-7 measurement script is a one-off
task artifact whose output is recorded in the docs, not maintained code. Implementation Step 6
runs the automated checks. Beyond
those, `--strict` with `validation: anchors: warn` fails on broken anchors, including the
hub-anchor links, and the "Build Docs" CI job's Check 6 confirms the six new `llms.txt` URLs
resolve.

## Manual Verification

These checks are rendering output that a person has to look at.
1. Run `cd mkdocs && mkdocs serve`. Expected: the nav shows `When to Use › vs. SaaS Vendors` expanded, with Overview plus six
   vendors.
2. Open `/docs/when-to-use/saas-vendors/`. Expected: the At a glance links open the vendor
   pages, and no `## vs.` sections are left.
3. Open `/docs/cost-comparisons/` and `/docs/cost-effectiveness/`. Expected: both still redirect
   to the hub.

## Decisions

- Pricing figures were re-verified on 2026-10-18 during design (see **Pricing
  Re-verification**). The implementation uses them as given and does not re-research them. The
  exceptions are the step-8 lookups (Datadog span rates, Dynatrace included trace volume).
- Pages return to the original URLs rather than new ones.
- The methodology stays on the hub; vendor pages link to it.
- Splunk keeps a labelled estimate based on the last published list prices.
- Free allotments are subtracted for every vendor (Grafana ~$8,069 to ~$7,955, New Relic
  ~$39,180 to ~$39,140).
- The user judged the ~0.6× Elastic result unfair to Micromegas. It is replaced by **Fairness
  Corrections**: measured billing sizes for every per-GB charge, traces priced wherever a
  linear rate is published, and the Elastic log tier shown as a range.
- Measured billing sizes apply to every vendor, not just Elastic, even where that raises other
  vendors' estimates.
- Micromegas is costed at ~$1,100/month on a single cloud provider's public monthly list prices
  for equivalent resources (user decision, 2026-10-18). AWS is quoted only as ~$2,200 on-demand
  with no discounts.
- The inherited "20 hosts" assumption (in the docs since 2025, never measured) is replaced by
  **400 hosts** for every host-priced product (user decision, 2026-10-18), with 8 GiB per host
  still assumed.
  - Measured per minute over 2026-09-18 → 2026-10-18: ~393 average, ~435 p99, 499 max concurrent
    machines.
  - Hourly distinct counts (p99 ~856) were rejected. Short-lived machines inflate them.
- Recomputed vendor estimates, per month, logs + metrics / with traces:
  - Datadog ~$30,800 / ~$586,000
  - Dynatrace ~$25,400 / ~$95,300
  - Elastic ~$6,200–$10,400 / ~$52,600–$56,800
  - Grafana Cloud ~$25,400 / ~$183,000
  - New Relic ~$90,200 / ~$216,500
  - Splunk ~$22,100–$24,400 / n/a
- Grafana metrics are billed on the DPM rule (~3.16M billable series), not the 1M series assumed
  earlier.
- Splunk metrics are priced in Observability Cloud per host, with custom metrics excluded, rather
  than indexed in Splunk Cloud.
- Elastic traces are priced at the $0.09/GB floor, the rate most favourable to Elastic.
- Network-trace events are left out of every vendor estimate.
- The reference deployment stays anonymous. Only aggregate volumes, sizes and list-price cost
  are published.
- Vendor trace pricing uses spans: stream events converted at the measured ~1.8 events/span.

## Open Questions

None. The current S3 size came from the user on 2026-10-18. Refreshing the August volumes would
need a new go-ahead for one prod `blocks` query, and is optional (see **Reference deployment
refresh**).
