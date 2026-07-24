# Speaker Guide: Interactive Notebooks for Observability

## Key Phrases

Weave these into the talk naturally:

- **"Composable"** — the cells are building blocks, not isolated panels
- **"No code changes"** — notebooks are built from existing cell types and the existing analytics backend
- **"In-browser query engine"** — DataFusion compiled to WASM runs locally, no server round-trips for downstream transforms
- **"Share via URL"** — variable state is encoded in the URL; paste a link, get the same view

## Don'ts

- Don't get lost in WASM/DataFusion internals — the audience cares about what it enables, not how it compiles
- Don't demo too many cell types — pick 3-4 that tell a story (table → chart → variable → composability)
- Don't apologize for missing features — present what works today
- Don't read the slides — the fragment lists are your cues, not your script

## Demo Prep Checklist

- [ ] Services running (`start_services.py`)
- [ ] Analytics web app running (`yarn dev` in `analytics-web-app/`)
- [ ] Two notebooks pre-built and saved:
  - Game telemetry notebook (variables, table, chart, logs)
  - Service self-monitoring notebook (dogfooding)
- [ ] Browser at 1920px width, dark theme
- [ ] No sensitive data visible in any notebook
- [ ] Test the full demo flow once before presenting

## Demo Script (~5 minutes)

1. **Open a notebook with game telemetry data**
   Show the full picture: variables in the title bar, a table, a chart, log entries.
   "This is one notebook — SQL cells feeding tables, charts, and logs."

2. **Change a variable dropdown**
   Watch auto-run cascade through all cells.
   "I changed one variable. Every cell that depends on it re-executed automatically."

3. **Drag-to-zoom on chart**
   The global time range updates; all cells re-execute.
   "Drag to zoom. The time range propagates to every cell."

4. **Show composability**
   Add a cell that queries another cell's results.
   "This query runs entirely in the browser. No server call. The previous cell's result is a local table."

5. **Switch to the service monitoring notebook**
   Same tool, different data source.
   "Same notebook interface, now showing our own telemetry pipeline's health."

6. **Show URL sharing**
   Copy URL, open in new tab — same state preserved.
   "The URL encodes the variable state. Paste it to a colleague, they see exactly what you see."

7. **(If time) Show source view toggle**
   Flip between rendered view and cell config.
   "You can inspect and edit the cell configuration directly."

## Objection Handling

| Objection | Response |
|-----------|----------|
| "Why not just use Grafana?" | Grafana is great for monitoring. Notebooks are for investigation — iterative, exploratory, composable. They complement each other. |
| "Why not just use Jupyter?" | Jupyter gives you maximum flexibility but requires Python boilerplate for every visualization. Notebooks give you specialized visual cells with zero boilerplate. |
| "How does the in-browser engine scale?" | It handles the result sets you'd typically visualize — thousands to low millions of rows. The heavy lifting (scanning the data lake) still happens server-side. |
| "Can I build dashboards with this?" | Notebooks with auto-run and URL sharing are effectively shareable dashboards. The line is blurry by design. |
| "What about collaboration?" | Notebooks are saved and shared via URL. Multiple people can work from the same notebook link. |
