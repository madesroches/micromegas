# Notebook Presentation Plan

## Overview

A 10-15 minute presentation showcasing the notebook feature in the analytics web app. Follows the same Reveal.js + Vite + Micromegas brand theme used in recent presentations (`doc/unified-observability-for-games/`). The arc: motivate **why** we built notebooks, **wow** with screenshots showing key capabilities, then **live demo**.

## Presentation Structure

### Slide Outline (~12 minutes + Q&A)

#### 1. Title Slide (~30s)
- Micromegas logo (vertical compact)
- "Interactive Notebooks for Observability"
- Name, email, GitHub link
- `data-state="hide-sidebar"` like other presentations

#### 2. Existing Tools: Strengths and Gaps (~3 min)

**Slide: "Grafana Dashboards"**
- Fragment list:
  - **Self-service**: teams build and maintain their own dashboards
  - Great for **monitoring** — pre-built panels, alerts, familiar interface
  - But **queries can be difficult to write** — big queries, no intermediate results, limited data types, hard to debug
  - Dashboards are **static**: when something looks wrong, you need to explore elsewhere

**Slide: "Python + Jupyter"**
- Fragment list:
  - The Micromegas Python API makes Jupyter a **powerful investigation tool**
  - Full SQL access, DataFrames, arbitrary analysis — maximum flexibility
  - But **matplotlib is not quick and intuitive** for throwaway queries
  - 10 lines of boilerplate to get a chart you'll look at once and throw away
  - Great for deep analysis, overkill for "just show me what happened"

**Slide: "Single-Purpose Tooling"**
- Fragment list:
  - The analytics web app started here: **dedicated screens** for process list, metrics, logs, performance analytics
  - Each screen does **one thing well** — purpose-built UI, no query writing needed
  - **Not self-service**: every new workflow requires a developer to build a new screen

**Slide: "The Solution"**
- Monitoring (Grafana) ← **???** → Deep analysis (Jupyter)
- Single-purpose screens sit in between but don't compose
- Fragment: What if we took those **same components** — tables, charts, logs, timelines — and made them **composable**?
  - The **iterative, exploratory** nature of Jupyter
  - But with **specialized visualizations** that make results instantly readable
  - No boilerplate — write SQL, see a chart

#### 3. What We Built (~2 min)

**Slide: "Notebooks"**
- One-liner: ordered list of cells that execute top-to-bottom
- Fragment list of key capabilities:
  - **11 cell types**: tables, charts, logs, timelines, markdown, variables...
  - **Local query engine**: cell results become queryable tables (DataFusion in WASM)
  - **Variables**: parameterize queries, share via URL
  - **Auto-run**: change a variable, everything updates

**Slide: "The Secret Sauce" (vertical sub-slide)**
- Composability diagram (ASCII or simple visual):
  - Cell A runs SQL → registers result as table "A"
  - Cell B runs `SELECT * FROM A WHERE ...` → entirely in-browser
  - No extra server round-trips for downstream transformations
- This is what makes notebooks more than just a list of queries

#### 4. Screenshots (~3-4 min, one per slide)

**Slide: "Cell Types at a Glance"**
- Screenshot of a notebook with mixed cell types visible (table, chart, variables in title bar)
- Caption: "Tables, charts, logs, markdown — all in one page"

**Slide: "Charts with Drag-to-Zoom"**
- Screenshot of a chart cell with time-series data
- Caption: "Drag to select a time range. Everything re-executes."

**Slide: "Variables"**
- Screenshot showing variable cells rendered as compact inputs in the title bar + a combobox dropdown
- Caption: "Dropdowns, text inputs, computed expressions — all from SQL"

**Slide: "Horizontal Groups"**
- Screenshot showing cells laid out side-by-side
- Caption: "Side-by-side layout via drag-and-drop"

**Slide: "Swimlane & Timeline"**
- Screenshot of swimlane or property timeline visualization
- Caption: "Thread activity and property changes over time"

#### 5. Live Demo (~4-5 min)

**Slide: "Demo" (transition slide)**
- Switch to the running analytics web app
- Two notebooks prepared: one with game-like telemetry data, one with service self-monitoring (dogfooding)
- Demo script:
  1. Open a notebook with game telemetry data — show the full picture (variables, table, chart, logs)
  2. Change a variable dropdown → watch auto-run cascade through all cells
  3. Drag-to-zoom on chart → global time range updates → all cells re-execute
  4. Show composability: add a cell that queries another cell's results (in-browser, no server)
  5. Switch to the service monitoring notebook — same tool, different data source
  6. Show URL sharing — copy URL, open in new tab → same state preserved
  7. (If time) Show source view toggle, config diff

#### 6. Closing (~30s)

**Slide: Closing / Contact**
- Logo, GitHub link, contact
- `data-state="hide-sidebar"`

### Speaker Guide (separate file)

Include a `presentation-plan.md` with:
- Key phrases: "composable", "no code changes", "in-browser query engine", "share via URL"
- Don'ts: don't get lost in WASM/DataFusion internals, don't demo too many cell types
- Demo prep checklist: services running, two notebooks pre-built (game telemetry + service monitoring), browser at 1920px, dark theme

## Project Setup

Copy the structure from `doc/unified-observability-for-games/`:

```
doc/notebooks/
├── index.html              # Entry point (copy + update title)
├── package.json            # Dependencies: reveal.js, vite
├── vite.config.js          # Same config (copy)
├── build-inline.js         # Standalone builder (copy + update title)
├── src/
│   ├── main.js             # Reveal.js init (copy)
│   ├── themes/
│   │   └── micromegas.css  # Brand theme (symlink or copy)
│   └── slides/
│       └── presentation.md # Slide content (NEW)
├── media/                  # Screenshots + logos
│   ├── micromegas-vertical-compact.svg  # (copy from existing)
│   ├── micromegas-vertical-sidebar.svg  # (copy from existing)
│   ├── notebook_overview.png            # (capture)
│   ├── chart_zoom.png                   # (capture)
│   ├── variables.png                    # (capture)
│   ├── horizontal_group.png             # (capture)
│   └── swimlane.png                     # (capture)
└── presentation-plan.md   # Speaker guide
```

## Screenshots to Capture

Capture from the analytics web app with dark theme, 1920px browser width, no browser chrome.

| Screenshot | What to show |
|------------|-------------|
| `notebook_overview.png` | A notebook with a mix of cell types: variable in title bar, table, chart, markdown |
| `chart_zoom.png` | A chart cell with time-series data, ideally mid-drag or showing the zoom selection |
| `variables.png` | Variable cells: a combobox dropdown open, a text input, showing compact title-bar rendering |
| `horizontal_group.png` | An HG cell with 2-3 children side-by-side |
| `swimlane.png` | A swimlane or property timeline showing thread activity |

## Files to Create

| File | Action |
|------|--------|
| `doc/notebooks/index.html` | Copy from unified-observability, update title |
| `doc/notebooks/package.json` | Copy, update name and description |
| `doc/notebooks/vite.config.js` | Copy as-is |
| `doc/notebooks/build-inline.js` | Copy, update title |
| `doc/notebooks/src/main.js` | Copy as-is |
| `doc/notebooks/src/themes/micromegas.css` | Copy from unified-observability |
| `doc/notebooks/src/slides/presentation.md` | Write new — the actual slides |
| `doc/notebooks/presentation-plan.md` | Write new — speaker guide |
| `doc/notebooks/media/*.svg` | Copy logo SVGs from unified-observability |
| `doc/notebooks/media/*.png` | Capture screenshots manually |

## Implementation Steps

1. **Scaffold project**: Copy boilerplate files from `doc/unified-observability-for-games/`
2. **Write presentation.md**: The actual slide content following the outline above
3. **Write presentation-plan.md**: Speaker guide with demo script, key phrases, objection handling
4. **Capture screenshots**: Requires running analytics web app with real data — manual step
5. **Test**: `yarn dev` to preview, iterate on content
6. **Build**: `yarn build:standalone` for self-contained HTML

## Resolved Questions

1. **Screenshots**: Local instance available with real data — will capture manually.
2. **Roadmap slide**: Removed — no roadmap discussion in this presentation.
3. **Audience**: Game-dev audience, but motivation for analytics varies — keep the "why" broad (not just debugging hitches).
4. **Demo data**: Both — one notebook with game telemetry, one with service self-monitoring. Shows versatility.
