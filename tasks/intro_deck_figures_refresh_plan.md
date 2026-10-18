# Intro Deck Cost and Scale Figures Refresh Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1688

## Overview

The intro presentation quotes cost, scale, and throughput figures that predate the reference
workload in the vendor comparison docs (refreshed in #1687). This updates the deck to the docs'
figures and aligns the per-process throughput claim across the deck, landing page, `llms.txt`,
README, and docs home so every public surface tells the same story.

## Current State

`doc/intro-micromegas/src/slides/presentation.md`:
- Line 194 (Cost-Efficient by Design): `900 billion events / 90 days / ~$1,750 a month`
- Line 225 (How: Sub-microsecond Instrumentation): `~20 ns per event` in the calling thread — stays.
- Line 239 (What This Unlocks): `60k–200k events/second per process`
- Line 377 (What You Get): `900B events for $1,750/month, on your own cloud`

Source of truth, `mkdocs/docs/when-to-use/saas-vendors.md`:
- Line 9: ~$1,100/month at a single cloud provider's public list prices.
- Lines 69–74: 90-day retention, ~27.5 TB object storage, ~29B logs, ~415B metric points,
  ~1.6T CPU trace events retained (≈ 2 trillion events total).
- Line 19: Datadog ~28× Micromegas on logs + metrics.

`mkdocs/docs/when-to-use/instrumentation-overhead.md` calling-thread medians: static log
44.6 ns vs. 293.2 ns (~6.6×), static span 27.7 ns vs. 146.3 ns (~5.3×). Dynamic cases narrow
to ~2–3×.

Throughput claim of 100k events/sec elsewhere:
- `welcome/src/components/HowItWorks.tsx:45` — `"20ns per event, 100k events/sec"`
- `welcome/public/llms.txt:9` — `sustains roughly 100k events/second per process`
- `mkdocs/docs/index.md:32` — `Supports up to 100k events/second per process`
- `README.md:35` — `up to **100,000 events per second** from a single instrumented process`

(`welcome/dist/` is gitignored build output — not edited.)

`doc/intro-micromegas/presentation-plan.md` (speaker guide):
- Line 14: `"900 billion events for $1,750 a month"`
- Line 32: `"$1,750 a month for 900 billion events"`
- Line 80: `900B events / 90 days / ~$1,750/month`

`mkdocs/docs/when-to-use/index.md`:
- Line 198: `one production deployment on AWS runs at about $1,100/month for 449 billion events over 90 days`
- Line 216: `one production deployment runs at about $1,100/month for 449 billion events over 90 days`

## Design

Text-only edits; no structural slide changes.

1. **Line 194** →
   `~2 trillion events / 90 days / ~27.5 TB / ~$1,100 a month`
   Replace the secondary line (196, "orders of magnitude higher") with
   `At this volume, Datadog costs ~28× more on logs and metrics alone — before counting traces.`
2. **Line 377** → `~2 trillion events for ~$1,100/month, on your own cloud`
3. **Line 239** → `Up to 300k events/second per process` (keep "without breaking the host app").
4. **Line 225** → keep `~20 ns per event` and add a fragment bullet right after:
   `~5–6× less calling-thread cost than the OpenTelemetry Rust SDK for logs and spans`
   (static cases; the ratio is the portable claim, the absolute ns are laptop numbers).
5. **Throughput alignment** (100k → up to 300k):
   - `HowItWorks.tsx`: `"20ns per event, up to 300k events/sec"`
   - `llms.txt`: `sustains up to 300k events/second per process`
   - `mkdocs/docs/index.md`: `Supports up to 300k events/second per process`
   - `README.md`: `up to **300,000 events per second**`

6. **Speaker guide** `doc/intro-micromegas/presentation-plan.md` lines 14, 32, 80: replace
   `900 billion events` / `900B events` with `~2 trillion events` and `$1,750` with `~$1,100`,
   keeping each line's phrasing.
7. **`mkdocs/docs/when-to-use/index.md`** lines 198 and 216: rewrite to
   `about $1,100/month for ~2 trillion events over 90 days`; drop `on AWS` from line 198 to
   match the list-price footing.

The $1,100 public-list-price figure is used deliberately; the ~$2,200 AWS on-demand figure is
not quoted anywhere.

## Implementation Steps

1. Edit the four lines in `doc/intro-micromegas/src/slides/presentation.md` per Design 1–4.
2. Edit the throughput claim in the four files listed in Design 5.
   Apply Design 6 and 7 to the speaker guide and `when-to-use/index.md`.
3. `grep -rn --exclude-dir=node_modules --exclude-dir=dist --exclude-dir=blog "900 billion\|900B\|1,750\|449 billion\|100k events\|100,000 events\|60k" doc/intro-micromegas welcome/src welcome/public README.md mkdocs/docs`
   — expect no hits.
4. Add a `**Docs:**` bullet under `## Unreleased` in `CHANGELOG.md` (same `* **Docs:** ... (#1688).`
   format as the existing entries) summarizing the refreshed deck cost/scale figures and the
   aligned 300k events/second throughput claim. No SQL change.
5. Build the deck (`cd doc/intro-micromegas && yarn build`) and the landing page
   (`cd welcome && yarn build`) to confirm nothing broke.

## Files to Modify

- `doc/intro-micromegas/src/slides/presentation.md`
- `welcome/src/components/HowItWorks.tsx`
- `welcome/public/llms.txt`
- `mkdocs/docs/index.md`
- `README.md`
- `doc/intro-micromegas/presentation-plan.md`
- `mkdocs/docs/when-to-use/index.md`
- `CHANGELOG.md`

## Trade-offs

- **Fold the Datadog comparison into the existing secondary line** rather than adding a new
  fragment: keeps the slide's density unchanged and replaces a vague claim with a sourced one.
- **Extend the throughput fix to README and mkdocs home**, beyond the issue's explicit list:
  these carry the same claim and leaving them at 100k would recreate the inconsistency the issue
  is fixing.
- **Quote "~5–6×" for static logs/spans** rather than a single blended ratio: matches the issue
  and the static medians; dynamic cases are smaller and stay on the overhead page.

## Decisions

## Documentation

The edited files are themselves the docs; no new pages. CHANGELOG entry under Unreleased docs.

## Testing Strategy

Content-only change with no logic; no automated tests apply. The grep in step 3 and the two
builds in step 4 are the checks.

## Manual Verification

1. `cd doc/intro-micromegas && yarn dev` — open the Cost-Efficient, How, What This Unlocks, and
   What You Get slides; confirm the new figures render and fragments step correctly. Visual
   layout can only be judged by eye.
