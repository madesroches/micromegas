# Notebook Documentation Plan

## Status
DONE

## Overview

Create comprehensive mkdocs documentation for the analytics web app's notebook feature. The documentation will cover notebook concepts, all 11 cell types, the variable system, execution model, drag-and-drop, and all interactive features. This fills a significant gap — the existing docs only cover web app deployment (`admin/web-app.md`) with no user-facing notebook documentation.

## Current State

- **Existing docs**: `mkdocs/docs/admin/web-app.md` covers deployment/config only
- **No notebook docs exist** — users must discover features by exploration
- **mkdocs setup**: Material theme with admonitions, mermaid diagrams, tabbed content, code highlighting
- **Nav structure**: Hierarchical sections (Query Guide, Unreal, Grafana, etc.) — notebooks need their own section

## Documentation Structure

Add a new top-level "Analytics Web App" nav section in `mkdocs/mkdocs.yml`:

```yaml
- Analytics Web App:
    - Overview: web-app/index.md
    - Notebooks:
        - Overview: web-app/notebooks/index.md
        - Cell Types: web-app/notebooks/cell-types.md
        - Variables: web-app/notebooks/variables.md
        - Execution & Auto-Run: web-app/notebooks/execution.md
    - Deployment: admin/web-app.md
```

The deployment page (`admin/web-app.md`) stays in its current file location but appears in both nav sections — under "Analytics Web App" for discoverability and under "Administration" where it currently lives. MkDocs supports referencing the same file in multiple nav locations. The Administration section keeps all three of its existing entries unchanged.

### Files to Create

| File | Content |
|------|---------|
| `mkdocs/docs/web-app/index.md` | Analytics web app overview, screen types, navigation |
| `mkdocs/docs/web-app/notebooks/index.md` | Notebook concepts, layout, creating/saving/sharing |
| `mkdocs/docs/web-app/notebooks/cell-types.md` | All 11 cell types with config and usage |
| `mkdocs/docs/web-app/notebooks/variables.md` | Variable system, expressions, macro substitution, URL params |
| `mkdocs/docs/web-app/notebooks/execution.md` | Execution model, auto-run, time range, data sources |

### Files to Modify

| File | Change |
|------|--------|
| `mkdocs/mkdocs.yml` | Add "Analytics Web App" nav section (deployment page referenced in both sections) |

## Page Content Outlines

### `web-app/index.md` — Analytics Web App Overview

- What the web app is (React SPA + Rust backend for querying observability data)
- Screen types: process_list, metrics, log, table, notebook
- Note: built-in screen types (process_list, metrics, log, table) are being phased out in favor of notebooks. Notebooks can replicate all built-in screen functionality with greater flexibility.
- Creating and managing screens
- Time range controls (relative/absolute, URL params `from`/`to`)
- Data sources
- Link to deployment docs and notebook docs

### `web-app/notebooks/index.md` — Notebook Overview

- What notebooks are (ordered list of cells, executed top-to-bottom)
- Notebooks are the primary screen type — built-in screen types (process_list, metrics, log, table) are being phased out and replaced by notebooks, which can replicate all their functionality with greater flexibility and composability
- Notebook layout: cell list on left, editor panel on right
- Creating a new notebook
- Adding, removing, duplicating cells
- Drag-and-drop reordering (vertical)
- Horizontal groups (HG cells) for side-by-side layout
- Cell collapse/expand
- Saving notebooks and URL-based state sharing (delta-based variable URLs)
- Source view (JSON editor) for advanced editing
- Config diff modal for reviewing changes

### `web-app/notebooks/cell-types.md` — Cell Types Reference

Document all 11 cell types with consistent structure per type:
- Description
- Configuration fields
- Behavior/features
- Example usage

**Cell types to cover:**

1. **Markdown** — Static text/documentation using GitHub Flavored Markdown. Supports `$variable` substitution.
2. **Variable** — User inputs (4 subtypes: text, combobox, expression, datasource). Renders in title bar. Populates downstream `$variable` references.
3. **Table** — SQL query results in sortable, paginated table. Column hiding, column format overrides.
4. **Transposed Table** — SQL results with rows/columns swapped. Useful for key-value property display.
5. **Chart** — Multi-query time series charts. Line/bar/area types, scale modes (p99, min-max, stddev), per-query units. Drag-to-zoom.
6. **Log** — SQL query results formatted as log entries with level coloring, auto-classified columns.
7. **Property Timeline** — Time-based property change visualization. JSON properties displayed as horizontal timelines.
8. **Swimlane** — Horizontal lane visualization for thread/async activity over time. Requires id, name, begin, end columns.
9. **Perfetto Export** — Export trace data to Perfetto UI or download. Requires process_id variable. Supports thread/async/both span types.
10. **Reference Table** — Embedded CSV data registered as a queryable table for use by downstream SQL cells.
11. **Horizontal Group (HG)** — Container cell arranging children side-by-side. Drag-in/drag-out support.

### `web-app/notebooks/variables.md` — Variable System

- Variable cell types: text, combobox, expression, datasource
- SQL macro substitution: `$variable`, `$variable.column`, `$begin`, `$end`
- Longest-name-first matching, SQL single-quote escaping
- Multi-column variables (combobox with multi-column SQL results)
- Expression evaluation: available bindings (`$begin`, `$end`, `$duration_ms`, `$innerWidth`, `$devicePixelRatio`, plus each upstream variable as `$variableName`), functions (`snap_interval()`, `Math.*`, `new Date()`)
- URL parameter sync: delta-based encoding, reserved params (`from`, `to`, `type`)
- Variable scope: cells only see variables from cells above them
- Available Variables panel in editor

### `web-app/notebooks/execution.md` — Execution & Auto-Run

- Execution model: sequential top-to-bottom, flattened (HG children expanded)
- Cell states: idle, loading, success, error, blocked
- Run cell vs. run from here
- Blocking: cells with `canBlockDownstream` block downstream on failure
- Auto-run: per-cell `autoRunFromHere` flag, debounced SQL changes, immediate variable changes
- Time range: relative/absolute, drag-to-zoom in charts, `$begin`/`$end` in queries
- Data sources: per-cell override, variable-based routing (`$datasource_var`)
- Refresh behavior: full re-execution with WASM engine reset

#### Local WASM Query Engine (major subsection)

Every notebook has a local DataFusion query engine compiled to WebAssembly. This is a core concept that should be documented prominently — it's what makes notebooks interactive and composable.

**How it works:**
- When a data cell (table, chart, log, etc.) executes a remote SQL query, the result is automatically registered as a named table in the local WASM engine under the cell's name
- Reference table cells register their CSV data directly into the WASM engine
- Any downstream cell can query upstream cell results locally using `SELECT ... FROM cell_name` — no round-trip to the server
- This enables interactive data transformation: fetch data once from the server, then reshape, filter, join, and aggregate locally

**Cell results as queryable tables:**
- Every data cell's result becomes a table named after the cell (e.g., a cell named `raw_metrics` becomes queryable as `SELECT * FROM raw_metrics`)
- Reference tables work the same way — CSV data is converted to Arrow format and registered by cell name
- Chart cells with multiple queries register each query under a custom name
- Tables are deregistered when cells are renamed or deleted
- `engine.reset()` clears all tables on full re-execution

**Intended usage pattern:**
- First cells fetch raw data from the server (remote queries)
- Subsequent cells query upstream results locally for transformation, filtering, joining, aggregation
- This avoids redundant server round-trips and enables rapid iteration on data shaping

**Monitoring data size:**
- Each cell's header shows row count and byte size after execution: e.g., "1,234 rows (2.3 MB) in 125ms"
- During fetch, live progress is shown: row count and bytes received so far
- Horizontal groups aggregate stats across all child cells
- Byte size is calculated from Arrow IPC batch sizes (`batch.data.byteLength`)

**Memory limits:**
- There are no hard-coded limits on table sizes in the WASM engine
- The practical limit is the browser's WebAssembly memory (typically 2-4 GB depending on browser and OS)
- Users should monitor cell data sizes via the header stats and be mindful of total notebook memory usage
- Large datasets should be filtered/aggregated server-side before registration in the local engine

## Implementation Steps

1. Create directory structure: `mkdocs/docs/web-app/notebooks/`
2. Write `web-app/index.md`
3. Write `web-app/notebooks/index.md`
4. Write `web-app/notebooks/cell-types.md`
5. Write `web-app/notebooks/variables.md`
6. Write `web-app/notebooks/execution.md`
7. Update `mkdocs/mkdocs.yml` nav to add new section (keep deployment page in Administration too)
8. Build and verify: `cd mkdocs && mkdocs build` (or `mkdocs serve` for local preview)

## Trade-offs

**Separate pages vs. single page**: Chose separate pages for cell types, variables, and execution to keep each page focused and scannable. A single monolithic page would exceed 2000 lines and be hard to navigate.

**New top-level section vs. under Administration**: Notebooks are a user-facing feature, not an admin concern. A dedicated "Analytics Web App" section makes more sense and gives room to grow (future: screen types, local query docs).

**Deployment page in both nav sections**: The deployment page appears under both "Analytics Web App" and "Administration" rather than being moved. This keeps the Administration section intact (3 entries) while making deployment docs discoverable from the web app section too. MkDocs handles dual nav references to the same file without issues.

## Decisions

1. **No screenshots for first pass.** Add them later once the text content is stable.
