# Intro to Micromegas Presentation Plan

## Overview

A 30-minute introduction-to-Micromegas talk for an audience that has never used the platform. Mixes the **why** (capture enough detail to fix issues without reproducing them) with the **what** (the four-layer pipeline) and the **how it differs** (low-overhead instrumentation, SQL on Arrow/FlightSQL via DataFusion). Closes with a brief look at notebooks.

Reuses the Reveal.js + Vite + Micromegas brand-theme stack used in `doc/notebooks/`, `doc/unified-observability-for-games/`, and `doc/high-frequency-observability/`. New project lives at `doc/intro-micromegas/`.

## Goals

- Land four points in the audience's head: **unified**, **detailed**, **efficient**, **standard**.
- Differentiate from OpenTelemetry on the dimension where Micromegas wins: instrumentation overhead and event frequency.
- Justify the SQL bet without getting lost in DataFusion internals.
- Leave time for the notebook walkthrough (screenshot tour — the most visual part of the talk) and Q&A.

## Audience and Framing

**Mixed audience** — game devs, data scientists, data engineers, and managers. Don't tune the talk for any one group; the deck is structured so each person finds at least one section that speaks to them directly:

- **Game devs** → section 6 (instrumentation overhead, "what this unlocks": always-on tracing in production)
- **Data scientists** → section 7 (SQL), section 8 (Python API + `pip install micromegas`), section 9 (notebooks)
- **Data engineers** → section 5 (architecture, data lake / lakehouse split), section 8 (HTTP gateway + Databricks federation)
- **Managers** → section 2 (the "fix without reproducing" promise), section 4 (simpler-and-more-powerful operational pitch), section 5 cost slide ($1,750/900B-events)

Audience-management implications:
- **Avoid jargon that only one cohort knows.** No PromQL deep-cuts, no DataFusion internals, no game-engine-specific framing in the framing slides. Where domain terms appear, define them in passing.
- **Don't assume OTel familiarity.** Some attendees will, some won't. Section 6's framing is "the biggest difference from modern observability stacks" — that lands without requiring the audience to know OTel specifically.
- **The cost slide is the universal hook.** Game devs care about feasibility, data folks care about pipeline economics, managers care about budget. One number, four audiences nodding.
- **The "fix without reproducing" frame** in section 2 lands across cohorts — every role has watched a bug die in "cannot reproduce" purgatory.

Frame Micromegas as a **complement-then-replace** option, not a rip-and-replace — softer landing for managers wary of platform churn, and credible to engineers who've been burned by aggressive migration pitches.

## Time Budget (30 min hard cap)

| Section | Slides | Time | Cumulative |
|---|---|---|---|
| 1. Title + framing | 1–2 | 1:00 | 1:00 |
| 2. The problem (why it matters) | 3 | 3:30 | 4:30 |
| 3. What Micromegas is (4 layers) | 1 | 1:00 | 5:30 |
| 4. **Why unified: simpler AND more powerful** | 2 | 3:00 | 8:30 |
| 5. Architecture + cost-efficiency | 2 | 3:30 | 12:00 |
| 6. Instrumentation: the biggest difference | 4 | 4:00 | 16:00 |
| 7. SQL on Arrow/FlightSQL | 3 | 3:30 | 19:30 |
| 8. Interoperability (Python API + HTTP gateway) | 2 | 3:00 | 22:30 |
| 9. Notebooks (screenshot walkthrough) | 2 | 3:00 | 25:30 |
| 10. Closing | 2 | 2:00 | 27:30 |
| Q&A buffer | — | 2:30 | 30:00 |

If running long, the cuttable items are: section 9's second screenshot, the closer fragment of section 4, and the implicit-comparison line on the cost slide. **Do not** cut from the "why unified" thesis (section 4), the cost-efficiency slide (section 5), the "biggest difference" beat (section 6), or interoperability (section 8) — those are the load-bearing parts of the talk. Section 6's framing is intentionally about *the underlying cost-model assumption*, not a vendor-vs-vendor comparison; resist the urge to drift into specific-vendor takedowns on stage.

## Slide Outline

### 1. Title

- Logo, title "**An Introduction to Micromegas**", subtitle "Unified observability built for high-frequency capture", presenter contact, micromegas.info.
- `data-state="hide-sidebar"` (matches the notebooks presentation, whose theme CSS this project copies).

### 2. The Problem — Why More Than Stats

**Slide: "Stats Tell You Something Is Wrong"**
- Fragment list:
  - Dashboards say: "p99 latency spiked at 14:32"
  - Metrics say: "errors went up 3x in region eu-west-1"
  - You still need to **reproduce** the issue to actually fix it
  - Reproduction is expensive — sometimes impossible (race conditions, specific hardware, network timing)

**Slide: "The Goal"**
- One line: **Capture enough detail to fix issues without reproducing them.**
- Fragment list (smaller font):
  - Quantify how often issues happen
  - Quantify how bad they are
  - Have enough context (logs + metrics + traces) to fix

**Slide: "Why This Is Hard"**
- Detailed traces are expensive to capture
- Detailed traces are expensive to store
- Detailed traces are expensive to query
- Most platforms force you to choose: detail OR scale OR cost. Pick two.
- Fragment: **Micromegas refuses to choose.** (Echo of the high-frequency-observability talk.)

### 3. What Micromegas Is

**Slide: "Unified Observability, End to End"**
- Four-pane visual showing the stages:
  1. **Instrumentation** — capture events from your apps
  2. **Ingestion** — receive and persist them
  3. **Analytics** — transform and query
  4. **Presentation** — Grafana, notebooks, Python
- Caption: "One pipeline. Logs, metrics, and traces share the same path."

### 4. Why Unified: Simpler AND More Powerful

This is the thesis of the talk. **Logs, metrics, and traces in one database — not three — is both simpler to operate AND strictly more powerful to query.** Borrows the structure of the games talk's "Easier AND More Powerful" beats but condenses for an intro audience.

**Slide: "Simpler"**
- One pipeline, not three.
- Fragment list:
  - **One SDK** to integrate — not one for logs, one for metrics, one for traces
  - **One query language** (SQL) — no PromQL + Lucene + vendor DSL
  - **One place to look** — stop asking "which tool has this data?"
  - **One retention policy, one budget, one team** — capacity planning in one place
- Closer: "Three observability stacks is three things to learn, three things to operate, three things to pay for."

**Slide: "More Powerful: Automatic Correlation"**
- Every event in Micromegas shares a schema model: `process_id`, `thread_id`, `time`, `session`, plus signal-specific fields.
- Fragment list of questions you can ask in **one query** that fragmented stacks can't answer without a manual join across tools:
  - "Show me the CPU trace **and** the logs **and** the metrics from the frame where this request hitched"
  - "What was the server doing when this client reported an error?"
  - "All events from this user's session, 10 seconds before the crash"
- Closer: **Fragmented**: hours hunting through three tools. **Unified**: one query.

### 5. Architecture

**Slide: "The Pipeline" (mermaid diagram)**
- Reuse the ingestion + analytics flow diagram from `high-frequency-observability/src/slides/presentation.md` (lines 107–177). It shows:
  - Apps → ingestion-srv → PostgreSQL (metadata) + S3 (payloads)
  - flight-sql-srv reads from data lake, writes lakehouse Parquet, serves clients via FlightSQL
- Speaker emphasis: cheap writes (data lake) + fast reads (lakehouse Parquet) — separated by a lazy ETL daemon.

**Slide: "Cost-Efficient by Design"** — load-bearing, not optional
- The cost story is core to the value proposition: always-on detail tracing only matters if you can actually afford it at scale. This slide proves the design holds up in production.
- Why it's cheap:
  - Raw payloads in **object storage** at object-storage prices (cents per GB-month, not dollars)
  - **On-demand ETL** — payloads sit in the data lake until queried; you don't pay to process data nobody looks at
  - **Lakehouse** materializes the parts you query repeatedly into Parquet, so repeated queries are fast and cheap
  - Self-hosted on your cloud — no per-event vendor pricing
- The production number, called out big: **900 billion events / 90 days / ~$1,750 a month**
- Implicit comparison (don't name vendors): at this volume, traditional vendor SaaS pricing is **orders of magnitude** higher. The cost-model inversion in section 6 is what makes this feasible end-to-end.

### 6. Instrumentation — The Biggest Difference

This is where Micromegas diverges sharply from every modern observability stack the audience already knows — OpenTelemetry, Datadog, Honeycomb, Prometheus, Splunk, you name it. They're all built around the same implicit assumption: **events are relatively rare and individually expensive to record.** Micromegas inverts that. The whole platform is designed around events being **cheap, plentiful, and always on.** Frame the slide as that contrast — not as "why not OTel" specifically.

**Slide: "The Biggest Difference"** (~60s)
- Modern observability stacks share an unstated assumption:
  - Events are **expensive** — pay per event sent, allocate/format/serialize per event, sampling is mandatory at scale
  - Tuned for **service-level** telemetry at ~1k events/sec/process — requests, RPCs, queues
  - Always-on detail tracing is **infeasible** in production — too expensive on the host, too expensive at the vendor
- Micromegas was built on the opposite assumption:
  - Events should be **cheap enough to never sample by default**
  - Designed for **60k–200k events/sec/process**, sub-microsecond spans
  - Always-on tracing in production is the **default**, not a special mode
- Closer: "Same goals — logs, metrics, traces. Very different cost model. Everything else follows from that."

**Slide: "How: Sub-microsecond Instrumentation"**
- The mechanics that make the claim credible (condensed from `high-frequency-observability/presentation.md` lines 181–199):
  - **~20 ns per event** in the calling thread
  - Events are tiny (a few bytes) and use **native memory layout**
  - The hot path is a **memcpy into a thread-local buffer** — no allocation, no formatting, no serialization
  - Background thread drains buffers, compresses with LZ4, ships to ingestion
  - **Sampling decisions happen at the batch level** — was this trace section interesting? — not per event

**Slide: "What This Unlocks"**
- Fragment list:
  - **100k events/second per process** without breaking the host app
  - **Always-on tracing** in production — not just in dev / under flag
  - Sub-microsecond span durations are visible — you can see what your inner loop is doing

**Slide: "Code Example" (vertical sub-slide)**
- Reuse the Rust `#[span_fn]` snippet from `high-frequency-observability/presentation.md` lines 206–220.
- Don't add a side-by-side comparison with another stack — keep the focus on what Micromegas does, not on what others don't.

### 7. SQL via DataFusion + Arrow + FlightSQL

**Slide: "Why SQL"**
- Fragment list:
  - Everyone knows it (or AI can help)
  - No proprietary DSL (no PromQL, no Lucene-flavored query bar, no vendor SDK)
  - Joins across logs, metrics, traces — for free, because they share a schema model
  - Standard tooling Just Works: Grafana, Python, BI tools, Jupyter

**Slide: "Built on Standards"**
- Logos + one-liner each:
  - **Apache Arrow** — columnar, zero-copy, the lingua franca of analytics
  - **Apache Parquet** — durable columnar storage; compresses well, scans fast
  - **DataFusion** — Rust-native SQL engine, embeddable, fast
  - **FlightSQL** — gRPC-based wire protocol, language-agnostic clients
- Closer: "We didn't reinvent the analytics stack. We assembled it."

**Slide: "What That Buys You"**
- Fragment list:
  - **No lock-in** — your data is in Parquet, query it with anything that speaks DataFusion or Arrow
  - **The same engine runs in the browser via WASM** — used by notebooks (foreshadow section 9)
  - **Domain UDFs where it matters** — JSONB (`jsonb_path_query`, `jsonb_array_elements`), histogram aggregates (median, p99 over very large samples)

### 8. Interoperability — Beyond FlightSQL

The standards-based foundation makes integration easy. Two concrete on-ramps people can use today.

**Slide: "Python API"**
- Fragment list:
  - `pip install micromegas` — official client; thin wrapper over FlightSQL returning Pandas / PyArrow
  - **CLI included**: `micromegas-query "SELECT ..."` for ad-hoc queries from the shell
  - Use cases: Jupyter notebooks, automation scripts, ML feature pipelines, custom dashboards
  - One-liner code snippet (pseudo): `client.query("SELECT * FROM log_entries WHERE ...")` returning a DataFrame

**Slide: "HTTP Gateway + Databricks Lakehouse Federation"**
- The pitch:
  - Not every client speaks gRPC. The HTTP gateway (`http-gateway-srv`) translates JSON-over-HTTP to FlightSQL.
  - Curl-friendly, browser-friendly, firewall-friendly.
- The bigger payoff:
  - **Databricks Lakehouse Federation** can register Micromegas as a catalog via the gateway
  - Run Databricks queries that **join Micromegas telemetry with your business data** — no ETL, no copies
  - Move queries, not data — the federation pushes predicates down, transfers only the result
- Closer: "Your observability data joins your data warehouse, on demand."

### 9. Presentation Layer — Notebooks

Light touch. The full notebook story has its own talk (`doc/notebooks/`); here we just show that the platform has a polished investigation surface.

**Slide: "Notebooks at a Glance"** (one screenshot, ~1:15)
- Reuse `doc/notebooks/media/notebook_overview.png`.
- Open with the framing: Grafana handles monitoring/alerting; notebooks are for the iterative investigation that monitoring dashboards can't support.
- Caption: "SQL cells feed tables, charts, and logs. Variables in the title bar. One page, one URL — share like a dashboard."
- Narrate the cell types visible in the screenshot (tables, charts, logs, swimlanes, flame graphs — all composable in one page).

**Slide: "Composability"** (one screenshot, ~1:45)
- Reuse `doc/notebooks/media/variables.png` (variable bar + cell editor visible).
- Three things to call out:
  - Change a variable in the title bar → every cell that depends on it re-runs
  - Drag-to-zoom on any chart → the time range propagates to every cell
  - **Cell results are queryable by other cells, in-browser** — DataFusion in WASM, same engine as the server. The "same engine, end to end" beat lands here.
- Closing fragment: investigations, shared via URL — the iterative complement to Grafana dashboards.

### 10. Closing

**Slide: "What You Get"**
- Five short lines:
  - **Detail without overhead** — instrumentation that disappears
  - **Unified storage** — one schema model, automatic correlation
  - **Standard query interface** — SQL on Arrow, no lock-in
  - **Open integration** — Python API, HTTP gateway, Databricks federation
  - **Affordable at scale** — 900B events for $1,750/month, on your own cloud

**Slide: Closing / Contact**
- `data-state="hide-sidebar"`
- Logo, **micromegas.info**, GitHub, contact email
- "Open source. Apache 2.0. Self-hosted on your cloud."

## Speaker Guide (separate file)

Write a `presentation-plan.md` next to `presentation.md`. Should include:

- **Key phrases to land**:
  - "Capture enough to fix without reproducing"
  - "Simpler AND more powerful" (the unified-database thesis)
  - "One query, complete context" (the unified payoff in five words)
  - "Events should be cheap enough to never sample by default" (the cost-model inversion)
  - "Same goals, very different cost model" (the biggest-difference framing)
  - "**900 billion events for $1,750 a month**" (the proof — say the number, then pause)
  - "Memcpy into a thread-local buffer"
  - "We didn't reinvent the analytics stack — we assembled it"
  - "Same engine, end to end" (DataFusion server + DataFusion in browser)
  - "Move queries, not data" (federation pitch)

- **Don'ts**:
  - **Don't name-and-compare specific vendors.** Section 6 is framed as a contrast with the *shared assumption* of modern observability stacks — not as Micromegas-vs-OTel or Micromegas-vs-Datadog. Naming a competitor on stage invites a feature-by-feature derail.
  - Don't dive into LZ4/Arrow IPC encoding mechanics
  - Don't run a live demo — this talk is screenshot-only by design (see trade-offs)
  - Don't read the slides

- **Do's**:
  - Use the audience's vocabulary (services, requests, spans) and bridge to Micromegas's (events, streams, blocks)
  - Pause on the 20 ns number — let it land
  - **Pause on the cost number** — "$1,750 a month for 900 billion events" is the line the audience will quote to their boss. Don't bury it in a list.
  - Pause on "one query, complete context" — let the audience think of their last cross-tool investigation
  - When asked "how does this compare to <X>?" in Q&A, answer it then. On stage, stay focused on what Micromegas does.

- **Pre-stage checklist** (no live demo — screenshots only):
  - All screenshots captured and embedded in `presentation.md`
  - Standalone HTML build verified (`yarn build:standalone`) — opens cleanly from `file://`
  - Backup copy of standalone HTML on USB / cloud drive
  - Run through the full deck on the actual presentation laptop once

- **Objection handling table** (lift the relevant rows from `unified-observability-for-games/presentation-plan.md` and add):
  - "How does this compare to OTel / Datadog / Honeycomb / Prometheus?" → Same goals, different cost model. The others assume events are individually expensive (sampling mandatory at scale). Micromegas assumes events are cheap and always-on. Different starting assumption, different design. Can run alongside.
  - "Why SQL and not PromQL?" → SQL covers PromQL's analytical use cases plus joins across signals. PromQL specializes; SQL generalizes.
  - "Is FlightSQL mature?" → Yes — Arrow ecosystem standard. Clients in Python, Go (Grafana plugin), Rust, Java.
  - "Can I query from non-gRPC environments?" → Yes — HTTP gateway translates JSON-over-HTTP to FlightSQL. Curl works.
  - "Can I join with my warehouse data?" → Yes — Databricks Lakehouse Federation registers Micromegas via the gateway; queries push down, only results transfer.

## Project Setup

Mix-and-match from the existing presentations: take Mermaid wiring from `doc/high-frequency-observability/` (its `src/main.js` and `package.json` already have the plugin and deps), take the rest from `doc/notebooks/`.

```
doc/intro-micromegas/
├── index.html              # Copy from notebooks/, retitle  (no Mermaid wiring lives here)
├── package.json            # Copy from high-frequency-observability/, rename  (has mermaid + reveal.js-mermaid-plugin deps)
├── vite.config.js          # Copy from either (publicDir: 'media' is identical)
├── build-inline.js         # Copy, retitle
├── src/
│   ├── main.js             # Copy from high-frequency-observability/  (imports + registers RevealMermaid)
│   ├── themes/
│   │   └── micromegas.css  # Copy from notebooks/  (has the .reveal-viewport.hide-sidebar rule used by title/closing slides)
│   └── slides/
│       └── presentation.md # NEW — slide content per outline above
├── media/
│   ├── micromegas-vertical-compact.svg   # copy
│   ├── micromegas-vertical-sidebar.svg   # copy
│   ├── arrow-logo.png                    # copy from high-frequency-observability/media/
│   ├── datafusion-logo.png               # copy from high-frequency-observability/media/
│   ├── parquet-logo.svg                  # copy from high-frequency-observability/media/
│   ├── flightsql-logo.* (find or omit)   # may need to source
│   ├── notebook_overview.png             # copy from notebooks/media/  — used by §9
│   └── variables.png                     # copy from notebooks/media/  — used by §9 Composability slide
└── presentation-plan.md   # speaker guide
```

Mermaid is required for the section 5 architecture diagram (lifted verbatim from `high-frequency-observability/src/slides/presentation.md` lines 107–177). Confirming the wiring lives in `main.js` (the `RevealMermaid` plugin import + registration) and the deps in `package.json` (`mermaid`, `reveal.js-mermaid-plugin`) — that's why those two files come from high-frequency-observability and not notebooks.

## Files to Create

| File | Source | Action |
|---|---|---|
| `doc/intro-micromegas/index.html` | `notebooks/index.html` | Copy + retitle |
| `doc/intro-micromegas/package.json` | `high-frequency-observability/package.json` | Copy + rename (has Mermaid deps); rewrite `build:standalone` to `"yarn build && node build-inline.js"` (high-freq-obs uses npm, this project uses yarn — don't carry over `package-lock.json`) |
| `doc/intro-micromegas/vite.config.js` | either | Copy as-is |
| `doc/intro-micromegas/build-inline.js` | either | Copy + retitle |
| `doc/intro-micromegas/src/main.js` | `high-frequency-observability/src/main.js` | Copy as-is (has RevealMermaid plugin) |
| `doc/intro-micromegas/src/themes/micromegas.css` | `notebooks/src/themes/micromegas.css` | Copy as-is |
| `doc/intro-micromegas/src/slides/presentation.md` | — | **Write new** |
| `doc/intro-micromegas/presentation-plan.md` | — | **Write new** (speaker guide) |
| `doc/intro-micromegas/media/*` | high-freq-obs/ + notebooks/ | Copy per project-setup tree |

No source-code changes. No documentation site updates required (this is a standalone presentation; if hosted under `micromegas.info/intro/` the mkdocs nav can be updated separately, but is out of scope for this plan).

## Implementation Steps

1. **Scaffold project** per the source map in "Files to Create": `index.html` and `themes/micromegas.css` from `doc/notebooks/`; `package.json` and `src/main.js` from `doc/high-frequency-observability/` (Mermaid wiring); `vite.config.js` and `build-inline.js` from either. Update titles in `index.html`, `package.json`, `build-inline.js`. Rewrite `package.json`'s `build:standalone` script from `"npm run build && node build-inline.js"` to `"yarn build && node build-inline.js"` so the project is yarn-only (don't copy `package-lock.json`).
2. **Copy media**: pull logos and screenshots from `high-frequency-observability/media/` and `notebooks/media/` into `doc/intro-micromegas/media/`. (Don't symlink — the standalone build inlines.)
3. **Write `presentation.md`** following the outline above. Lift the architecture mermaid diagram and the Rust `#[span_fn]` code snippet verbatim from `high-frequency-observability/`.
4. **Write `presentation-plan.md`** (speaker guide).
5. **Dry run with timer**: `yarn dev`, run through start to finish. Trim until under 28 min so there is real Q&A space.
6. **Build standalone**: `yarn build:standalone` produces a single self-contained HTML for offline presenting.

## Trade-offs

- **30 minutes is tight for an end-to-end intro.** The bigger risk than going long is going shallow on every section. The plan accepts shallow coverage of presentation-layer surfaces (notebooks gets light treatment, only Grafana mentioned alongside) to spend real time on the unified-database thesis, the instrumentation mechanics, and interoperability — those are the parts the audience will remember.
- **The differentiation slide is framed against an assumption, not a vendor.** A natural temptation is to compare Micromegas head-to-head with OpenTelemetry (or Datadog, or Honeycomb). The plan deliberately frames section 6's "biggest difference" beat as a contrast with the *shared cost-model assumption* of modern observability stacks — events expensive, sampling mandatory — and inverts that assumption rather than naming competitors. Reasoning: (a) the contrast is more durable (the assumption holds across vendors and across years; specific vendor features change quarterly); (b) the talk is an intro to Micromegas, not a teardown of anyone; (c) once the audience hears the numbers (20 ns, 100k events/sec), they'll do the comparison in their head. If asked about a specific stack in Q&A, that's the right venue.
- **Screenshots only, no live demo.** A live notebook demo would be high-impact but failure-prone (network, services, browser quirks, demo-data freshness). For an intro talk this is the wrong place to spend that risk budget. The notebook walkthrough is two screenshots with narration — gives the audience the visual without putting the talk on a tightrope.
- **Cost is part of the story, not an aside.** Earlier drafts treated the cost slide as optional. It's not — always-on detail tracing is only interesting if it's *affordable* at production scale, and the $1,750/900B-events number is the proof that the design (efficient instrumentation + object-storage data lake + on-demand ETL) actually pays off end-to-end. The intro keeps it to one slide rather than the full breakdown from the high-frequency-observability talk, but it's load-bearing, not optional.
- **Reusing slides from existing talks.** Where outlines overlap (architecture diagram, Rust code example, "why unified" beats from the games talk), copy verbatim — they're already polished. Inventing parallel versions risks drift.

## Open Questions

_None — outstanding decisions resolved._

## Decisions

- **Title**: "An Introduction to Micromegas" — plain and descriptive over marketing-flavored alternatives. Sets the tone for the talk: substance, not pitch.
