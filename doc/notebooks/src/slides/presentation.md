<!-- .slide: data-state="hide-sidebar" -->
<img src="./micromegas-vertical-compact.svg" alt="micromegas" class="plain" style="height: 340px; margin: 0;">

## Interactive Notebooks for Observability

<p style="font-size: 0.6em;">Marc-Antoine Desroches · <a href="mailto:madesroches@gmail.com">madesroches@gmail.com</a><br><a href="https://madesroches.github.io/micromegas/">madesroches.github.io/micromegas</a></p>

---

## Grafana Dashboards

<ul>
<li class="fragment"><strong>Self-service</strong>: teams build and maintain their own dashboards</li>
<li class="fragment">Great for <strong>monitoring</strong> — pre-built panels, alerts, familiar interface</li>
<li class="fragment">But <strong>queries can be difficult to write</strong> — big queries, no intermediate results, limited data types, hard to debug</li>
<li class="fragment">Dashboards are <strong>static</strong>: when something looks wrong, you need to explore elsewhere</li>
</ul>

---

## Python + Jupyter

<ul>
<li class="fragment">The Micromegas Python API makes Jupyter a <strong>powerful investigation tool</strong></li>
<li class="fragment">Full SQL access, DataFrames, arbitrary analysis — maximum flexibility</li>
<li class="fragment">But <strong>matplotlib is not quick and intuitive</strong> for throwaway queries</li>
<li class="fragment">10 lines of boilerplate to get a chart you'll look at once and throw away</li>
<li class="fragment">Great for deep analysis, overkill for "just show me what happened"</li>
</ul>

---

## Single-Purpose Tooling

<ul>
<li class="fragment">The analytics web app started here: <strong>dedicated screens</strong> for process list, metrics, logs, performance analytics</li>
<li class="fragment">Each screen does <strong>one thing well</strong> — purpose-built UI, no query writing needed</li>
<li class="fragment"><strong>Not self-service</strong>: every new workflow requires a developer to build a new screen</li>
</ul>

---

## The Solution

Monitoring (Grafana) ← **???** → Deep analysis (Jupyter)

Single-purpose screens sit in between but don't compose

<div class="fragment">

What if we took those **same components** — tables, charts, logs, timelines — and made them **composable**?

<ul>
<li>The <strong>iterative, exploratory</strong> nature of Jupyter</li>
<li>But with <strong>specialized visualizations</strong> that make results instantly readable</li>
<li>No boilerplate — write SQL, see a chart</li>
</ul>

</div>

---

## Notebooks

An ordered list of cells that execute top-to-bottom.

<ul>
<li class="fragment"><strong>11 cell types</strong>: tables, charts, logs, timelines, markdown, variables...</li>
<li class="fragment"><strong>Local query engine</strong>: cell results become queryable tables (DataFusion in WASM)</li>
<li class="fragment"><strong>Variables</strong>: parameterize queries, share state via URL</li>
<li class="fragment"><strong>Auto-run</strong>: change a variable, everything updates</li>
</ul>

--

## The Secret Sauce

**Composability through an in-browser query engine**

<div style="font-size: 0.75em; margin-top: 1em;">

```
┌─────────────────────────────┐
│  Cell A: SQL query           │
│  SELECT ... FROM server      │──→ result registered as table "A"
└─────────────────────────────┘
              │
              ▼
┌─────────────────────────────┐
│  Cell B: local query         │
│  SELECT * FROM A WHERE ...   │──→ runs entirely in-browser
└─────────────────────────────┘
              │
              ▼
┌─────────────────────────────┐
│  Cell C: visualization       │
│  Chart / Table / Timeline    │──→ no extra server round-trips
└─────────────────────────────┘
```

</div>

<p class="fragment" style="margin-top: 1em;">This is what makes notebooks more than just a list of queries.</p>

---

## Cell Types at a Glance

<img src="./notebook_overview.png" style="max-width: 75%; height: auto;">

Tables, charts, logs, markdown — all in one page

---

## Charts with Drag-to-Zoom

<img src="./chart_zoom.png" style="max-width: 75%; height: auto;">

Drag to select a time range. Everything re-executes.

---

## Variables

<img src="./variables.png" style="max-width: 75%; height: auto;">

Dropdowns, text inputs, computed expressions — all from SQL

---

## Horizontal Groups

<img src="./horizontal_group.png" style="max-width: 75%; height: auto;">

Side-by-side layout via drag-and-drop

---

## Swimlane & Timeline

<img src="./swimlane.png" style="max-width: 75%; height: auto;">

Thread activity and property changes over time

---

## Demo

**Switch to the running analytics web app**

---

<!-- .slide: data-state="hide-sidebar" -->
<img src="./micromegas-vertical-compact.svg" alt="micromegas" class="plain" style="height: 340px; margin: 0;">

**https://madesroches.github.io/micromegas/**
