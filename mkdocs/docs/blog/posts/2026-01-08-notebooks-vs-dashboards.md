---
date: 2026-01-08
authors:
  - madesroches
categories:
  - Engineering
tags:
  - observability
  - sql
  - analytics
  - grafana
  - open-source
---

# Your Observability Dashboard Can't Do This

Open a notebook. Write SQL. See logs, charts, swimlanes, and Perfetto traces — in one view. Drag a chart to zoom. The entire notebook updates. No PromQL. No KQL. No NRQL. Just SQL.

<!-- more -->

## The Problem with Traditional Observability UIs

Grafana dashboards look great — until you need to investigate something they weren't designed for. You hit the wall: wrong query type, wrong panel, wrong data source. You open a second tool. Then a third. You're now switching between Grafana, Kibana, and a trace viewer, copy-pasting timestamps to correlate events across all three.

Datadog notebooks exist, but good luck when your custom metrics cardinality bill arrives. And their query language is still proprietary.

JupyterLab gives you the exploratory workflow — but you need to be a programmer to use it. Pandas, matplotlib, SQL connectors — these are complex libraries with steep learning curves. And the files aren't easily shared. You email `.ipynb` files around, hope everyone has the right Python environment — and pray nobody embedded credentials in a cell output. Notebook files floating through Slack and email are a security nightmare. That's not accessible to the SRE who just needs to investigate a production spike.

## Notebooks Built for Observability

Micromegas notebooks are purpose-built for investigating telemetry data. Each notebook is a composable document with typed cells:

- **SQL cells** — full Apache DataFusion SQL, not a crippled subset
- **Chart cells** — time-series visualization with drag-to-zoom that updates the notebook's time range
- **Table cells** — sortable, with column formatting and duration display
- **Log cells** — color-coded severity levels, directly from your telemetry lake
- **Swimlane cells** — thread-level activity visualization
- **Perfetto export cells** — generate Chrome Trace Viewer files from your span data
- **Variable cells** — cascading dropdowns, text inputs, and computed expressions that feed into every query below them
- **Markdown cells** — document your analysis inline

These aren't generic widgets. Each cell type is designed for a specific observability task. A log cell knows about severity levels. A swimlane cell knows about threads and time ranges. A Perfetto export cell knows how to transform span data into Chrome's trace format.

## Variables and Drag-to-Zoom

Two features make notebooks feel interactive instead of static.

**Variables** go at the top of the notebook. A dropdown selects which process to analyze. A text input sets a filter. A computed expression derives a value from other variables. Every SQL cell below can reference these variables — change a dropdown, and the whole notebook re-executes with the new context.

**Drag-to-zoom** on any chart updates the global time range. Every cell sees it. You spot an anomaly in a chart, drag to zoom in, and immediately your log cells show the logs for that window, your swimlane cells show thread activity for that window, your metrics update. No manually editing time range pickers across three tools.

These two features together turn a notebook from a static report into an interactive investigation tool.

## Share by URL

There's no file to email. Notebook state — time range, variable selections, cell configuration — is encoded in the URL using delta-based parameter encoding. Copy the URL, send it to a colleague, and they see exactly what you saw. Same time range, same variable selections, same analysis.

No Python environments to set up. No `.ipynb` files to manage. No credentials accidentally embedded in cell output.

## Why Not Just Use Grafana?

Grafana is good at dashboards — pre-built views that monitor known metrics. That's a different job than investigation.

When something goes wrong, you need to explore. You don't know which query to write yet. You need to look at logs, then correlate with metrics, then drill into traces, then go back and filter differently. Dashboards are rigid. Notebooks let you build up the analysis step by step, keeping context as you go.

Micromegas doesn't replace Grafana (there's even a [Grafana plugin](../../grafana/index.md) for it). But when you need to investigate rather than monitor, notebooks are the better tool.

## The Cost Angle

Arrow IPC streaming with compression means you can pull large datasets without choking. Queries run on Apache DataFusion against S3/Parquet. No per-query pricing. No cardinality traps. No surprise bills.

Companies spend 20-30% of their infrastructure budget on observability tools. Most of that goes to vendors charging by volume. Micromegas stores everything on object storage and gives you full SQL to query it. See our [cost comparisons](../../cost-effectiveness.md) for specifics.

## One Notebook, One Query Language, All Your Telemetry

Exploratory analysis shouldn't require three tools and a PhD in PromQL.

[Get started](../../getting-started.md) or explore the [notebook documentation](../../web-app/notebooks/index.md) for the full walkthrough.
