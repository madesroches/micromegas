<!-- .slide: data-state="hide-sidebar" -->
<img src="./micromegas-vertical-compact.svg" alt="micromegas" class="plain" style="height: 340px; margin: 0;">

## Interactive Notebooks for Observability

<p style="font-size: 0.6em;">Marc-Antoine Desroches · <a href="mailto:madesroches@gmail.com">madesroches@gmail.com</a><br><a href="https://micromegas.info/">micromegas.info</a></p>

---

## Agenda

1. **Existing tools** — strengths and gaps
2. **The solution** — composable notebooks
3. **Screenshots** — what we built
4. **Live demo**

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

## The Gap

Monitoring (Grafana) ← **???** → Deep analysis (Jupyter)

Single-purpose screens sit in between but don't compose

--

## The Solution

What if we took those **same components** — tables, charts, logs, timelines — and made them **composable**?

<ul>
<li class="fragment">The <strong>iterative, exploratory</strong> nature of Jupyter</li>
<li class="fragment">But with <strong>specialized visualizations</strong> that make results instantly readable</li>
<li class="fragment">No boilerplate — write SQL, see a chart</li>
</ul>

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

<div style="font-size: 0.5em; margin-top: 0.5em; display: grid; grid-template-columns: auto 1fr; column-gap: 1em; row-gap: 0; align-items: center;">
  <div style="border: 1px solid var(--color-border); border-radius: 6px; padding: 0.3em 0.8em; background: var(--color-bg-2); font-family: var(--font-mono); text-align: left;">
    <div>Cell A: <span style="color: var(--color-wheat);">SQL query</span></div>
    <div><code>SELECT ... FROM server</code></div>
  </div>
  <div>→ result registered as table <strong>"A"</strong></div>
  <div style="text-align: center; font-size: 1.3em; line-height: 1.2;">↓</div>
  <div></div>
  <div style="border: 1px solid var(--color-border); border-radius: 6px; padding: 0.3em 0.8em; background: var(--color-bg-2); font-family: var(--font-mono); text-align: left;">
    <div>Cell B: <span style="color: var(--color-horizon);">local query</span></div>
    <div><code>SELECT * FROM A WHERE ...</code></div>
  </div>
  <div>→ runs entirely <strong>in-browser</strong></div>
  <div style="text-align: center; font-size: 1.3em; line-height: 1.2;">↓</div>
  <div></div>
  <div style="border: 1px solid var(--color-border); border-radius: 6px; padding: 0.3em 0.8em; background: var(--color-bg-2); font-family: var(--font-mono); text-align: left;">
    <div>Cell C: <span style="color: var(--color-rust);">visualization</span></div>
    <div><code>Chart / Table / Timeline</code></div>
  </div>
  <div>→ <strong>no</strong> extra server round-trips</div>
</div>

<p style="margin-top: 0.3em; font-size: 0.8em;">This is what makes notebooks more than just a list of queries.</p>

---

## Cell Types at a Glance

<img src="./notebook_overview.png" style="max-width: 75%; height: auto;">

Tables, charts, logs, markdown — all in one page

---

## The Table Cell

SQL in, rows out

<img src="./table_cell.png" style="max-width: 75%; height: auto;">

3.7M rows · 175 MB · <code>SELECT * FROM measures</code>

---

## Charts

<img src="./chart_zoom.png" style="max-width: 75%; height: auto;">

Drag to select a time range. Everything re-executes.

---

## Logs

<img src="./log_cell.png" style="max-width: 75%; height: auto;">

Compact and color-coded automatically.

---

## Variables

<img src="./variables.png" style="max-width: 75%; height: auto;">

Dropdowns, text inputs, computed JS expressions

---

## Horizontal Groups

<img src="./horizontal_group.png" style="max-width: 75%; height: auto;">

Side-by-side layout via drag-and-drop

---

## Swimlane

<img src="./swimlane.png" style="max-width: 75%; height: auto;">

Thread activity over time

---

## Property Timeline

<img src="./property_timeline.png" style="max-width: 75%; height: auto;">

Property changes over time

---

<!-- .slide: data-state="hide-sidebar" -->
<img src="./micromegas-vertical-compact.svg" alt="micromegas" class="plain" style="height: 340px; margin: 0;">

**https://micromegas.info/**
