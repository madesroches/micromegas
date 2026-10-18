# Split SaaS Vendor Comparison into Per-Vendor Pages Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1685

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
otherwise, they are unchanged from "early 2026". The implementer uses these figures as given and
does not re-research them. Reference workload: 20 hosts × 8 GiB, 90-day retention, ~1,500 GB/month
of logs (3B entries), ~9,200 GB/month of metrics (~92B points, ~1M active series), ~100 users,
~10,700 GB/month in total, ~357 GB/day.

Where a vendor publishes a free or included allotment, the estimate now subtracts it. The
previous estimates ignored allotments; applying them the same way for every vendor is
part of making all six pages share one structure.

### Datadog — ~$7,950/month, ~7× (unchanged)
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
    `1,500 × ~$0.15 = ~$225` (range `$135`–`$900` across $0.09–$0.60/GB).
  - Retention $0.019/GB-month → `1,500 × 3 × $0.019 = ~$86`.
- Metrics. New pricing effective July 1, 2026: time-series metrics cost 25% of the standard rate.
  - Ingest $0.023/GB → `9,200 × $0.023 = ~$212`.
  - Retention $0.005/GB-month → `9,200 × 3 × $0.005 = ~$138`.
- Total ~$660/month, which is **below** the Micromegas ~$1,100. The page must say so plainly,
  and also say that the Micromegas figure includes 165B trace events while this one excludes
  traces. The pricing section opens with the range, because the log-ingest tiers are unpublished.
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
  ~357 GB/day). Label it explicitly as based on the last published list prices and say that
  current prices are private-offer only. Cite the 2025-07-19 blog post
  (`blog/posts/...how-costly-really-is-splunk`) as the dated record of those tiers, and do not
  edit that post.
- Mention Splunk Observability Cloud per-host pricing (Infrastructure $15, App & Infra $60,
  End-to-End $75 per host-month, "starts at") as the metrics-side product. Unlimited users on
  Cloud Platform. Keep the Cisco acquisition note short.
- Sources: AWS Marketplace listing, splunk.com/en_us/products/pricing.html,
  splunk.com/en_us/products/pricing/observability.html.

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

`cost-comparisons/index.md` and `cost-effectiveness.md` keep their redirects to the hub. The
vendor pages live in a different folder from the hub (`when-to-use/`) because preserving
URLs matters more than having them in the same folder.

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
  - Update the figures: Elastic ~$660 / ~0.6×, Grafana ~$8,000+ / ~7.2×, New Relic ~$39,100 / ~36×.
  - Add a column header or a note saying "Rates verified October 2026".
- **TL;DR** (line 9). The range now starts *below* Micromegas: Elastic Serverless at ~0.6× on
  logs and metrics alone, then Dynatrace at ~1.5×, up to New Relic at ~36×. Keep the
  traces-skew sentence, which matters more now that Elastic comes in below Micromegas.
- `*Last reviewed: June 2026*` → `*Last reviewed: October 2026*`.
- Methodology intro (line 120): "every vendor comparison below… Each vendor section" →
  "every vendor page… Each vendor page".
- Update the front matter `description:` if it states a range; the current one doesn't.

### `mkdocs.yml`

- Remove the six vendor lines from `redirect_maps`. Keep `cost-effectiveness.md` and
  `cost-comparisons/index.md`.
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
~7× Micromegas`, matching the gloss style of the neighbouring entries. Update the line-71 hub
gloss if it still says "each vendor".

## Implementation Steps

1. Create the six `mkdocs/docs/cost-comparisons/<vendor>.md` pages from the template. Take the
   pricing from **Pricing Re-verification** and the qualitative bullets from the current
   `saas-vendors.md` sections.
2. Edit `mkdocs/docs/when-to-use/saas-vendors.md`: delete the vendor sections and update At a
   glance, the TL;DR, Last reviewed, and the Methodology intro.
3. Edit `mkdocs/mkdocs.yml`: change the redirects and nav.
4. Edit `welcome/public/llms.txt`, lines 71–77.
5. Run `mkdocs build --strict` from `mkdocs/`. Then stage the site the way
   `publish-docs.yml` does, as far as practical (a docs-only `public_docs/docs` + the welcome
   build), and run `python3 build/check_docs_site.py public_docs`.

## Files to Modify

- `mkdocs/docs/cost-comparisons/{datadog,dynatrace,elastic,grafana,newrelic,splunk}.md` (new)
- `mkdocs/docs/when-to-use/saas-vendors.md`
- `mkdocs/mkdocs.yml`
- `welcome/public/llms.txt`

## Trade-offs

- **Original URLs vs. `when-to-use/saas-vendors/<vendor>/`.** Put the pages back at the original
  URLs. Search Console's 122 impressions and the AI citations already point there. New URLs
  would need a second round of redirects and would let that accumulated signal fade.
- **Hub keeps the methodology vs. repeating it on each page.** The issue asks for the hub. Each
  vendor page links three anchors (reference workload, methodology, traces) instead of carrying
  ~1,500 words of copied boilerplate, which is what #1641 removed.
- **Splunk: keep the dollar estimate, or say "unpriced".** Keep it, labelled as based on the last
  published list rates. "Contact sales" would make the comparison table useless, and the
  published tiers are recorded in the dated blog post.
- **Elastic coming out cheaper.** Publish the honest figure. The page's credibility, and the
  whole comparison's, depends on applying the same arithmetic to every vendor. The
  traces exclusion explains why that figure isn't a like-for-like loss.
- **Subtracting free allotments.** This moves Grafana from ~$8,069 to ~$7,955 and New Relic from
  ~$39,180 to ~$39,140. The changes are small, but the same rule now applies to every vendor.

## Documentation

This change is documentation. `CHANGELOG.md` gets an entry from the `pr` stage. Nothing else
links to the vendor anchors.

## Testing Strategy

There is no new code, so no unit tests. Existing automated checks cover the change:
- `mkdocs build --strict` catches broken relative links (including the hub ↔ vendor anchors),
  nav entries pointing at missing files, and a redirect source that collides with a real page.
- `python3 -m pytest build/test_check_docs_site.py` should still pass; it doesn't change.
- `python3 build/check_docs_site.py public_docs` runs on a staged tree. Check 6 confirms the
  six new `llms.txt` URLs resolve. Check 4 confirms the new pages carry their own canonical tags
  instead of being refresh stubs.

## Manual Verification

These checks are rendering output that a person has to look at.
1. Run `cd mkdocs && mkdocs serve` and open `/docs/cost-comparisons/dynatrace/`. Expected: the
   browser tab reads "Dynatrace Pricing and Cost Breakdown vs. Micromegas - Micromegas
   Documentation", the page source has `<meta name="description">` with the front matter text,
   and it has no `http-equiv="refresh"`.
2. Expected: the nav shows `When to Use › vs. SaaS Vendors` expanded, with Overview plus six
   vendors.
3. Open `/docs/when-to-use/saas-vendors/`. Expected: the At a glance links open the vendor
   pages, and no `## vs.` sections are left.
4. Open `/docs/cost-comparisons/` and `/docs/cost-effectiveness/`. Expected: both still redirect
   to the hub.

## Decisions

- Pricing figures were re-verified on 2026-10-18 during design (see **Pricing
  Re-verification**). The implementation uses them as given and does not re-research.

## Open Questions

- None blocking. Elastic now comes out at ~0.6× Micromegas on logs and metrics. This
  reverses part of the hub's message, so the maintainer may want to review the reworded TL;DR
  in the PR.
