# When to Use Layout and Agent Observability Page Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1637

## Overview

The `When to Use` nav tab holds 9 pages today: the open-source peer page plus 8 SaaS cost pages
(an overview, a methodology page and six vendor pages of ~400 words each). #1637 asks for one
more page, on observing LLM agents compared with Langfuse, Arize Phoenix, Opik and similar. Rather
than grow the tab to 10 pages of comparison material, this plan regroups it into three pages, one
per kind of alternative a reader is weighing:

```
When to Use
├─ When to Use Micromegas   when-to-use/index.md                (open-source peers; mostly unchanged)
├─ vs. LLM Agent Tools      when-to-use/agent-observability.md  (new, #1637)
└─ vs. SaaS Vendors         when-to-use/saas-vendors.md         (merge of 8 cost pages)
```

The 8 old cost URLs keep working through redirect stubs from the `mkdocs-redirects` plugin.

## Current State

- `mkdocs/mkdocs.yml:97-107` — the `When to Use` nav: `when-to-use/index.md`, then a
  `vs. SaaS Vendors` group with `cost-effectiveness.md`, `cost-comparisons/index.md` and
  `cost-comparisons/{datadog,dynatrace,elastic,grafana,newrelic,splunk}.md`. Plugins are
  `search`, `mkdocstrings`, `blog`, `tags`, `rss`; there is no redirect plugin.
- `mkdocs/docs/when-to-use/index.md` (~6,200 words): intro, `Micromegas in brief`, Limits,
  `At a glance` tables, one `Micromegas vs. <peer>` section per open-source peer, `Complementary
  tools`, `Also considered`, `Commercial SaaS` (`:179-181`, a paragraph linking the 8 cost pages),
  `Summary: which one fits`, FAQ. The OpenObserve (`:76`) and SigNoz (`:92`, `:94`) sections each
  have one sentence on their LLM observability features. The reference links `[cost]` and
  `[cost-ondemand]` (`:250-251`) point at `../cost-effectiveness.md#scale-perspective` and
  `#on-demand-processing-tail-sampling`.
- The cost pages repeat each other:
  - `cost-effectiveness.md` `Example Deployment Cost` → `Monthly Infrastructure Costs` and
    `cost-comparisons/index.md` `Micromegas Baseline Cost` have the same $1,100/month table.
  - `cost-effectiveness.md` `Pricing Model Differences` and `cost-comparisons/index.md`
    `Comparison of Philosophies` are the same SaaS-vs-Micromegas table, at different detail.
  - `A Note on Personnel Costs` and `Why Personnel Costs Are Excluded` make the same point.
  - Each vendor page opens with the same "see the Comparison Methodology page" line and has the
    same four H2 sections: `<Vendor> Pricing`, `Cost Comparison Summary` (a two-row table),
    `Qualitative Differences`, `References` (a numbered list). `dynatrace.md` (`Important
    Caveats`) and `splunk.md` (`Cisco Acquisition Context`) each add one H3 under the pricing
    section. There is no table across vendors.
- Inbound links to the cost URLs outside `mkdocs/docs/`:
  - `welcome/public/llms.txt:71-80`, the `## Cost` section: 8 links.
  - `welcome/src/components/Footer.tsx:31`: `/docs/cost-effectiveness/`.
  - `README.md:61`: `https://micromegas.info/docs/cost-effectiveness/`.
- `build/check_docs_site.py` checks the staged site in CI (`publish-docs.yml`, step
  "Check docs site URLs"). Check 4 requires every HTML file under `docs/` to carry a
  `<link rel="canonical">` whose absolute URL points at that same file; only `404.html` is
  exempt. Check 6 requires every on-site link in `llms.txt` to resolve to an existing file,
  ignoring the fragment. Tests are in `build/test_check_docs_site.py`, run by the same workflow.
- Agent observability material that exists today:
  - `mkdocs/docs/otlp/index.md` `### Claude Code` (`:255`): the OTLP recipe for Claude Code's
    metrics, logs and (beta) traces. `## Limitations` (`:691`): OTLP/HTTP only, histograms not
    materialized, `otel_spans` is JIT and per-process, so cross-process trace queries need a UNION.
  - `mkdocs/docs/blog/posts/2026-09-03-record-your-ai-agent-share-on-your-terms.md`: recording
    prompts, responses, tool calls and cost into `log_entries`/`measures`, private-by-default
    audiences, sharing by grant, splitting metrics and logs across team and personal audiences.
  - `mkdocs/docs/admin/authorization.md`: audiences and grants.

## Design

### 1. Redirects

Add `mkdocs-redirects` to `mkdocs/docs-requirements.txt` and configure it in `mkdocs.yml` (in the
same phase that creates `saas-vendors.md`, so every phase builds clean under `--strict`):

```yaml
  - redirects:
      redirect_maps:
        cost-effectiveness.md: when-to-use/saas-vendors.md
        cost-comparisons/index.md: when-to-use/saas-vendors.md#methodology
        cost-comparisons/datadog.md: when-to-use/saas-vendors.md#vs-datadog
        cost-comparisons/dynatrace.md: when-to-use/saas-vendors.md#vs-dynatrace
        cost-comparisons/elastic.md: when-to-use/saas-vendors.md#vs-elastic
        cost-comparisons/grafana.md: when-to-use/saas-vendors.md#vs-grafana-cloud
        cost-comparisons/newrelic.md: when-to-use/saas-vendors.md#vs-new-relic
        cost-comparisons/splunk.md: when-to-use/saas-vendors.md#vs-splunk
```

The plugin (1.2.3) writes a stub at each old path after the build: meta refresh, a JS redirect that
carries the old URL's fragment over, and `<link rel="canonical">` with the **relative** target URL.
Stubs are not pages, so they never enter `sitemap.xml`.

The plugin's `on_post_build` only logs a warning and skips a redirect whose target file is missing,
and CI builds without `--strict`. `publish-docs.yml` therefore gets `--strict` on its `mkdocs build`
line, which turns a broken target into a build failure. `mkdocs build --strict` passes on the
current tree.

Since the plugin's script appends the old fragment, `cost-effectiveness/#scale-perspective` lands on
`saas-vendors/#scale-perspective` only if the merged page keeps that heading. The merged page
therefore keeps the `Scale Perspective` and `On-Demand Processing (Tail Sampling)` headings
verbatim, and the other `cost-effectiveness.md` headings it carries over (see §3).

### 2. Checker: accept redirect stubs

A stub's canonical is relative and points at a different file, so check 4 would reject all 8
stubs. Teach `check_canonical_tags` about them:

- A file is a redirect stub when it has a `<meta http-equiv="refresh" content="0; url=...">` tag.
- For a stub, don't apply the self-canonical rule. Instead, resolve the refresh URL with the
  existing `href_to_path` (fragment dropped) and fail if the target file does not exist. With
  `--strict` the build already catches a missing target, so this is a defensive check on the staged
  tree.
- `href_to_path` must first apply `_resolve_url_path` to relative hrefs too (today only the
  root-absolute branch does). The stubs' refresh URLs are relative directory URLs such as
  `../when-to-use/saas-vendors/`, and `.resolve()` drops the trailing slash, which would yield the
  directory rather than `index.html`. Compute the joined path from the resolved URL path so a
  trailing slash becomes `index.html`.
- Files with no refresh tag are checked exactly as today.

This keeps check 4 strict for real pages.

### 3. `when-to-use/saas-vendors.md`: merged cost page

The 8 pages become one, with duplicates kept only once. Prose is moved, not rewritten. The cost
figures are unchanged (see Decisions). Outline:

```
# Micromegas vs. SaaS Observability Vendors
  intro (one paragraph) · TL;DR · *Last reviewed* · pointer to the open-source peer page
## At a glance                    ← new: one table, a row per vendor, from the 6 summary tables
## Cost Philosophy                ← cost-effectiveness.md (keep heading text = keep anchors)
### Why This Matters
## Primary Cost Drivers            ← also absorbs cost-comparisons/index.md "The Micromegas Cost
                                    Model" (cost-driver bullets and the OTLP/HTTP note)
### Compute Services / Storage Services / Supporting Infrastructure
## Example Deployment Cost        ← the single copy of the $1,100 table
### Data Scale / Monthly Infrastructure Costs / Scale Perspective
## Cost Management Features
### On-Demand Processing (Tail Sampling) / Flexible Retention Policies
## Methodology                    ← cost-comparisons/index.md; target of the index redirect
### Pricing Models               ← "Common Pricing Models…" + the philosophy table, merged with
                                    cost-effectiveness "Pricing Model Differences"
### Reference Workload
### Why Personnel Costs Are Excluded  ← absorbs "A Note on Personnel Costs"
### The Challenge of Traces
### When Micromegas is Cost Effective
## vs. Datadog                    ← one H2 per vendor; slugs must match the redirect map
## vs. Dynatrace
## vs. Elastic
## vs. Grafana Cloud
## vs. New Relic
## vs. Splunk
```

Each vendor section: the vendor's disclaimer sentence, the pricing breakdown, then
`**Cost comparison.**` (the two-row table) and `**Qualitative differences.**` as bold lead-ins,
which is the same style the open-source peer page uses. Its `References` list becomes a single
`Sources:` line of links. Bold lead-ins keep the TOC at one entry per vendor, so it doesn't fill
with 6 copies of the same three H3s whose slugs Material would number `_1`…`_5`. The repeated
"see the Comparison Methodology page" lines are removed, since methodology is now on the same page.

The `At a glance` table:

| Vendor | Priced on | Est. monthly (logs + metrics) | vs. Micromegas ~$1,100 |
|---|---|---|---|
| Datadog | … | ~$7,950 | ~7× |
| … | | | |

Its cells come from each vendor page's existing `Cost Comparison Summary` and pricing section,
including the caveats in parentheses (Dynatrace "logs + monitoring only", Grafana Cloud "30-day
retention only"). Rows are sorted by vendor name, not by ratio.

`cost-effectiveness.md`, `cost-comparisons/index.md` and the six vendor files are deleted (the
redirect map replaces them). The `cost-comparisons/` directory goes away. The link-list sections
`## Detailed Cost Comparisons` (`cost-effectiveness.md`) and `## Detailed Comparisons`
(`cost-comparisons/index.md`) are deleted, not carried over: the merged page replaces them.
`## Commercial Platform Comparison` (`cost-effectiveness.md`) is dissolved into `## Methodology`,
which absorbs its `Pricing Model Differences`, `A Note on Personnel Costs` and `When Micromegas is
Cost Effective` subsections.

### 4. `when-to-use/agent-observability.md`: new page (#1637)

Same skeleton as `when-to-use/index.md`, so a reader who has seen one page knows how to read the
others, and every claim carries a link:

```
# Micromegas for LLM Agent Observability
  intro · TL;DR · *Last reviewed* · sourcing note (same wording as index.md)
## What Micromegas records from an agent
## At a glance                    ← one table: license, self-hosted deps, OTLP, SQL access,
                                    evals, prompt mgmt, token/cost, per-row access control
## Micromegas vs. Langfuse
## Micromegas vs. Arize Phoenix
## Micromegas vs. Opik
## Micromegas vs. Laminar         ← not named in #1637, but it ships a SQL editor over traces,
                                    which is the closest overlap with Micromegas's SQL pitch
## Also considered                ← Helicone, OpenLLMetry/Traceloop, MLflow Tracing
## Using them together
## Summary: which one fits
## FAQ
```

**What Micromegas records from an agent**: only what exists today.
- Agents that emit OTLP/HTTP, with Claude Code as the worked example: API requests with model,
  token counts and cost, landing as `log_entries` and `measures` rows; beta traces as `otel_spans`
  ([OTLP recipe](../otlp/index.md#claude-code)). Prompt, response and tool content is redacted by
  default; it is recorded only when `OTEL_LOG_USER_PROMPTS`, `OTEL_LOG_ASSISTANT_RESPONSES` and
  `OTEL_LOG_TOOL_DETAILS` are set, as the blog post
  `2026-09-03-record-your-ai-agent-share-on-your-terms.md` does.
- Per-row audiences: private by default, with sharing done by editing grants rather than rewriting
  data, and metrics split from prompt-bearing logs across a team audience and a personal one
  ([authorization](../admin/authorization.md), and the blog post).
- The same SQL surface and notebooks as all other telemetry, next to the game servers and CI
  runners the agent touches. Agent analysis is done by pointing an agent at the data through the
  CLI and a skill file (blog post, `from-o11y-to-candor`).

**Limits** (bold lead-in inside `## What Micromegas records from an agent`, as in `index.md`;
agent-specific, stated plainly; general limits link to `index.md#micromegas-in-brief`):
- No evaluations (LLM-as-judge, datasets, experiments), no prompt management or versioning, no
  playground, no annotation queues.
- No LLM-specific trace UI: no chat-transcript view, no tool-call tree. Agent data is shown with
  SQL, notebooks and Grafana.
- No GenAI semantic-convention mapping: `gen_ai.*` attributes land in the generic JSONB
  `properties` column like any other attribute and are read with the `jsonb_*` UDFs
  ([attribute encoding](../otlp/index.md#attribute-encoding); `rust/` has no `gen_ai` handling).
- No token-cost price table: cost is whatever the agent reports (Claude Code reports
  `cost.usage`). An agent that only reports tokens gets no dollar figure.
- OTLP histograms are not materialized, and `otel_spans` is per-process, so a multi-agent trace
  spanning processes needs a UNION ([OTLP limitations](../otlp/index.md#limitations)).

**Peer sections**: each one follows the issue's three parts, using the bold lead-ins from
`index.md`: a description paragraph (license, paid-tier gating, self-hosted dependencies,
ingestion), `**Choose <peer> when**`, `**How Micromegas differs.**`. The "choose the peer" side
should be generous: evals, prompt management and a purpose-built trace UI are real reasons to pick
these tools.

Peer facts are gathered in [Peer research](#peer-research) below. Every one of them must be
re-checked against the peer's current docs when the page is written, and linked.

**Using them together**: the realistic answer for many teams. A purpose-built tool handles
evals and the prompt-level UI, while Micromegas keeps the full-resolution, access-controlled record
next to the rest of the fleet's telemetry. Describe only setups that work with what exists:
pointing the agent's OTLP exporter at one backend, or at a collector that fans out to both.
Don't claim an integration that doesn't exist.

**FAQ** (each answer is a short paragraph that links into the page). Questions are phrased
the way a user would ask an LLM:
- What is a self-hosted alternative to Langfuse that I can query with SQL?
- How do I record Claude Code prompts and tool calls without exposing them to my whole team?
- How do I track AI agent token cost per developer or team?
- How do I keep agent telemetry next to the rest of my application telemetry?

### 5. Main page, llms.txt and other links

`when-to-use/index.md`:
- Replace the `## Commercial SaaS` paragraph's 8 links with one link to `saas-vendors.md` (plus
  `#methodology`). Keep the paragraph.
- Add `## LLM agent tools` next to it: two sentences and a link to `agent-observability.md`.
- OpenObserve `:76`, SigNoz `:94`: link the "LLM observability" phrase to the agent page.
- Repoint `[cost]` / `[cost-ondemand]` to `saas-vendors.md#scale-perspective` /
  `#on-demand-processing-tail-sampling`.
- FAQ: add one agent question, with a one-paragraph answer that links to the agent page.

`welcome/public/llms.txt`:
- Fold the `## Cost` section into `## When to use Micromegas`, as three entries:
  - the existing peer-page entry;
  - `vs. LLM agent tools`: one line naming Langfuse, Phoenix, Opik and Laminar, saying where each fits
    and where Micromegas fits;
  - `vs. SaaS vendors`: one line, then a sub-list of 6 anchor links
    (`…/when-to-use/saas-vendors/#vs-datadog`, …) so each vendor name still has its own link.
- Check 6 resolves these, ignoring fragments.

Other: `welcome/src/components/Footer.tsx:31` and `README.md:61` move to
`/docs/when-to-use/saas-vendors/`. The redirects would cover them, but links we own should point at
the canonical URL.

## Peer research

*(Gathered October 2026 for planning; re-verify each fact and link the primary source when writing
the page.)*

Items marked *unconfirmed* were not found in a primary source and must not reach the page
unless a source turns up.

| | Langfuse | Arize Phoenix | Opik | Laminar |
|---|---|---|---|---|
| License | MIT; `ee/` proprietary ([repo](https://github.com/langfuse/langfuse)) | **ELv2**, source-available, not OSI ([LICENSE](https://github.com/Arize-ai/phoenix/blob/main/LICENSE)) | Apache-2.0 ([repo](https://github.com/comet-ml/opik)) | Apache-2.0 ([repo](https://github.com/lmnr-ai/lmnr)) |
| Paid gating | Project RBAC, protected prompt labels, retention policies, audit logs, data masking, SCIM ([license key](https://langfuse.com/self-hosting/license-key)) | Self-hosting free with all features ([self-hosting](https://arize.com/docs/phoenix/self-hosting)); Arize AX is separate | Enterprise via Comet sales; gating *unconfirmed* | Signals, Slack/email alerts ([hosting](https://laminar.sh/docs/hosting-options)) |
| Self-hosted deps | Postgres, ClickHouse, Redis/Valkey, S3 ([self-hosting](https://langfuse.com/self-hosting)) | SQLite or PostgreSQL ([config](https://arize.com/docs/phoenix/self-hosting/configuration)) | ClickHouse, MySQL, Redis, MinIO, ZooKeeper; Helm for production ([local](https://www.comet.com/docs/opik/self-host/local_deployment)) | Postgres, ClickHouse, Quickwit; Helm adds RabbitMQ, Redis ([hosting](https://laminar.sh/docs/hosting-options)) |
| OTLP | HTTP only; `langfuse.*`, `gen_ai.*`, OpenInference attrs ([otel](https://langfuse.com/integrations/native/opentelemetry)) | HTTP and gRPC (config); uses OpenInference as its instrumentation and semantic-convention layer | HTTP only ([otel](https://www.comet.com/docs/opik/tracing/opentelemetry/overview)) | HTTP and gRPC |
| Native SDKs | Python, JS/TS | Python, TS/JS (Java/Go through OpenInference instrumentation) | Python, TS | Python, TS/JS |
| Agent features | Tracing, prompt mgmt, evals, datasets, playground; token/cost mapping | Tracing, evals, datasets, experiments, playground, prompt mgmt | Agent traces, datasets, experiments, LLM-as-judge, prompt mgmt, playground, guardrails | Evals, datasets, labeling queues, browser recording |
| SQL over traces | Not listed ([API overview](https://langfuse.com/docs/api-and-data-platform/overview)) | *Unconfirmed* | No; OQL search, REST, export ([export](https://www.comet.com/docs/opik/tracing/export_data)) | **Yes**: SQL editor ([docs](https://laminar.sh/docs/platform/sql-editor)), SQL dashboards, MCP |
| Retention | Self-hosted: indefinite; policies are EE ([retention](https://langfuse.com/docs/data-retention)) | `PHOENIX_DEFAULT_RETENTION_POLICY_DAYS` | *Unconfirmed* | *Unconfirmed* |

Also considered:
- **Helicone**: Apache-2.0, AI gateway plus observability. Supabase, ClickHouse, MinIO
  ([repo](https://github.com/Helicone/helicone)).
- **OpenLLMetry / Traceloop**: Apache-2.0, OTel instrumentation for LLM libraries, not a backend.
  It complements Micromegas, since its output can go to any OTLP backend
  ([repo](https://github.com/traceloop/openllmetry)).
- **MLflow Tracing**: Apache-2.0, needs only a SQL backend store. OTLP/HTTP with GenAI semconv,
  sampling via `MLFLOW_TRACE_SAMPLING_RATIO` ([tracing](https://mlflow.org/docs/latest/genai/tracing/)).

Claude Code's own exporter ([monitoring](https://code.claude.com/docs/en/monitoring-usage)):
- Metrics: `claude_code.cost.usage` and `token.usage`, among others.
- Events: `user_prompt`, `assistant_response`, `api_request`, `tool_result`, among others.
- Traces are beta.
- Content is redacted unless the matching `OTEL_LOG_*` flag is set.
- It supports gRPC too, so the Micromegas recipe must keep `http/protobuf`.

What this means for the page:
- Phoenix and MLflow have the lightest self-hosted footprint, which counts against the
  "Micromegas needs PostgreSQL" limit. Say so.
- Laminar already offers SQL over agent traces ([SQL editor](https://laminar.sh/docs/platform/sql-editor)). The Micromegas difference is therefore not "SQL",
  but agent data in the same store and SQL surface as the rest of the fleet's telemetry, plus
  per-row audiences.
- No peer was found with per-row access control set by the ingestion credential. Langfuse gates
  even project-level RBAC to EE. Phrase this as "not found in their docs", not as "they can't".

## Implementation Steps

One PR, closing #1637. The phases are ordered so the build stays green after each one; commit
locally per phase as rollback points.

**Phase 1: checker and strict build**
1. `build/check_docs_site.py`: add redirect-stub handling to `check_canonical_tags` (§2), and add
   tests for it in `build/test_check_docs_site.py`.
2. `.github/workflows/publish-docs.yml`: add `--strict` to the `mkdocs build` line.
   `mkdocs/docs-requirements.txt`: add `mkdocs-redirects>=1.2.3`.

**Phase 2: SaaS merge**
3. Write `mkdocs/docs/when-to-use/saas-vendors.md` from the 8 sources (§3). Then delete
   `cost-effectiveness.md` and `cost-comparisons/`, and replace the nav's `vs. SaaS Vendors`
   group with `- vs. SaaS Vendors: when-to-use/saas-vendors.md`.
4. `mkdocs.yml`: add the `redirects` plugin with the map in §1.
5. `when-to-use/index.md`: repoint the `## Commercial SaaS` paragraph's links and the `[cost]` /
   `[cost-ondemand]` references (§5), so no page links to a deleted file.

**Phase 3: agent page**
6. Re-verify the peer research. Write `mkdocs/docs/when-to-use/agent-observability.md` (§4).

**Phase 4: nav and links**
7. `mkdocs.yml` nav: add `- vs. LLM Agent Tools: when-to-use/agent-observability.md` between the
   two other entries.
8. Update the rest of `when-to-use/index.md` (§5), `llms.txt`, `Footer.tsx` and `README.md`.
9. Build and run the checker locally (Testing Strategy).

## Files to Modify

- `build/check_docs_site.py`, `build/test_check_docs_site.py`
- `.github/workflows/publish-docs.yml` (`--strict` on `mkdocs build`)
- `mkdocs/docs-requirements.txt`, `mkdocs/mkdocs.yml`
- `mkdocs/docs/when-to-use/index.md`
- `mkdocs/docs/when-to-use/saas-vendors.md` (new)
- `mkdocs/docs/when-to-use/agent-observability.md` (new)
- `mkdocs/docs/cost-effectiveness.md`, `mkdocs/docs/cost-comparisons/*.md` (deleted)
- `welcome/public/llms.txt`, `welcome/src/components/Footer.tsx`, `README.md`
- `CHANGELOG.md` (docs entry)

## Trade-offs

- **Three pages vs. one page.** One page for everything would put agent tools and SaaS cost
  models next to storage engines, with `At a glance` columns that apply to one group only. Three
  pages, one per kind of alternative, keep each comparison table meaningful.
- **Merged SaaS page vs. one page per vendor.** A standalone `/cost-comparisons/datadog/` URL
  matches a "Datadog alternative" query slightly better than an anchor does. Each vendor keeps a
  `vs. <Vendor>` H2 and its own `llms.txt` link. In exchange, the duplicates go away and the
  merged page gets a table across all six vendors, which no page has today.
- **`mkdocs-redirects` vs. hand-written stubs.** Hand-written HTML could carry absolute canonicals
  and skip the checker change, but the stubs would live outside the build, so a drifting
  target would go unnoticed. The plugin is the standard tool, and `--strict` makes it fail the build
  on a missing target. The checker change is small.

## Decisions

- Cost figures ($1,100/month, 449B events) are carried over unchanged. Refreshing them is
  separate work.
- One PR for the layout, the SaaS merge and the agent page (user call).
- The agent page title is "Micromegas for LLM Agent Observability" (user call); the nav label is
  `vs. LLM Agent Tools`.

## Documentation

All the work is documentation. `CHANGELOG.md` gets an entry under docs noting the merge and
the redirects. Grep `mkdocs/docs`, `doc/` and `welcome/` once more for `cost-comparisons` and
`cost-effectiveness` before opening the PR.

## Testing Strategy

Unit tests in `build/test_check_docs_site.py` (run in CI by `publish-docs.yml`), using the
existing synthetic-tree helpers:
- a redirect stub whose refresh target exists passes;
- a redirect stub whose target is missing fails, and the message names the stub;
- a redirect target with a fragment (`../saas-vendors/#vs-datadog`) resolves, fragment ignored;
- a relative directory URL (trailing slash) resolves to that directory's `index.html`;
- a non-stub page with a relative or foreign canonical still fails (stub handling must not
  loosen check 4).

The real build is covered by the existing CI step, which builds the full staged tree and runs the
checker. That is the end-to-end test of the redirect map, nav, sitemap and `llms.txt`.

## Manual Verification

Whether a merged page reads well and whether an anchor lands on the right heading are things to
check by eye. To do it locally (from repo root, in the docs venv):

1. `mkdocs build --strict --config-file mkdocs/mkdocs.yml --site-dir /tmp/mm_site/docs` (strict
   turns the plugin's missing-target warning into an error), then stage the rest of the tree by
   following the staging step in `publish-docs.yml` (`welcome/dist`, `CNAME`, ...) and run
   `python3 build/check_docs_site.py /tmp/mm_site`. Expected: `OK`.
2. `python mkdocs/serve.py`, then open `/cost-comparisons/datadog/`. It should land on
   `/when-to-use/saas-vendors/#vs-datadog`. Open `/cost-effectiveness/#scale-perspective`: it
   should land on the `Scale Perspective` heading.
3. Check the `When to Use` tab shows three entries and the merged page's TOC has one entry per
   vendor.
