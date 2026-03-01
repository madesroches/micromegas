---
date: 2026-02-15
authors:
  - madesroches
categories:
  - Engineering
tags:
  - observability
  - rust
  - webassembly
  - datafusion
  - sql
  - open-source
---

# We Put a Data Warehouse in Your Browser

Apache DataFusion already supports compiling to WebAssembly — so we took advantage of it. Full SQL running client-side, the same query engine that powers the Micromegas backend, now running in a browser tab. And it scales your analytics compute for free.

<!-- more -->

## The Problem with Monolithic Queries

I've always found it frustrating to get observability data into the shape I actually need. You start with a simple question, end up wrestling a monolithic query full of nested subqueries and brittle joins. It works. Until someone needs to change it and nobody can read the thing.

Notebooks fix this. Instead of one giant query, you take baby steps. One cell fetches the raw data. The next one filters. The next one aggregates. Each cell does one simple thing, and the next builds on it. Same result — but you can actually read it six months later.

The catch: every cell that runs a query means a round-trip to the server.

```
Traditional:
  Browser → Server → Analytics Engine → Server → Browser
  (repeat for every cell)
```

That's fine for the first query that fetches raw data. But the second cell that filters those results? The third cell that aggregates? Those don't need the server — the data is already in the browser.

## DataFusion, Compiled to WASM

DataFusion already compiles to `wasm32-unknown-unknown` — the hard work was done by the DataFusion community. We just wrapped it with `wasm-bindgen` and plugged it into the notebook. The server delivers data once via Arrow IPC. After that, all exploration and reshaping happens locally in the browser.

```
WASM Path:
  Cell 1: Browser → Server → Analytics Engine (fetch raw data)
  Cell 2: Browser → WASM DataFusion (filter locally, instant)
  Cell 3: Browser → WASM DataFusion (aggregate locally, instant)
```

This isn't a toy SQL subset. It's the full DataFusion query engine — the same one that processes terabytes on the backend — running in your tab. JOINs, window functions, CTEs, aggregations, all of it.

### The API

The engine exposes a clean interface via `wasm-bindgen`:

```rust
pub struct WasmQueryEngine {
    ctx: SessionContext,
}

impl WasmQueryEngine {
    pub fn new() -> Self
    pub fn register_table(&self, name: &str, ipc_bytes: &[u8]) -> Result<usize, JsValue>
    pub async fn execute_sql(&self, sql: &str) -> Result<Vec<u8>, JsValue>
    pub async fn execute_and_register(
        &self, sql: &str, register_as: &str
    ) -> Result<Vec<u8>, JsValue>
    pub fn deregister_table(&self, name: &str) -> Result<bool, JsValue>
    pub fn reset(&self)
}
```

When a server-side cell returns results, the browser calls `register_table()` to deserialize the Arrow IPC bytes into an in-memory table. Subsequent cells can query that table — or any combination of registered tables — with standard SQL. `execute_and_register()` lets you chain transformations: run a query and immediately make its results available as a new table for downstream cells.

## Free Horizontal Scaling

Here's what I didn't expect: it scales for free.

Every user running a notebook is a compute node. More users means more distributed compute, not more server cost. Fewer queries hit the FlightSQL server. Less load, better latency for everyone.

Most observability tools scale costs linearly with users. We're scaling the warehouse horizontally — for free.

## The Engineering Details

### Binary Size

The WASM module is ~24 MB raw, **5.9 MB gzipped**. We use LTO and `opt-level = "s"` to keep it reasonable. The module is lazy-loaded — it only downloads when a user first creates a local query cell, so it doesn't affect initial page load.

### Limitations

WASM is single-threaded (no spawning threads), so queries run on the main thread. In practice this is fine — local queries operate on filtered subsets that are already in memory. We also disable Parquet and compression features, keeping only Arrow IPC for data exchange. This helps with binary size.

### Tracing in WASM

`micromegas-tracing` compiles to WASM without modification. Right now it's a hack — logs route to the browser console via a `ConsoleEventSink` instead of proper telemetry collection. But it means instrumented Rust code works on both native and WASM targets without `#[cfg]` gates everywhere. Proper browser-to-server telemetry is on the roadmap.

## How It Fits Into Notebooks

Each notebook owns a `WasmQueryEngine` instance. The workflow:

1. A SQL cell runs a query against the server (FlightSQL) and gets back Arrow data
2. The results are registered as a named table in the WASM engine
3. A local query cell references that table with SQL — executes instantly in the browser
4. Drag-to-zoom on a chart updates the time range, which re-fetches server data and cascades through all cells

This cross-cell composition pattern is what makes notebooks more powerful than dashboards for investigation. You build up context step by step, and each step is readable and modifiable.

## Compared to DuckDB WASM

DuckDB WASM is the established player here — more mature, 15+ companies in production, larger community. If you need a standalone in-browser SQL engine, DuckDB WASM is a safe choice.

What Micromegas brings is the integration. The WASM engine isn't a standalone tool — it's embedded in an observability notebook where server-side and client-side queries compose naturally. Cross-cell references, drag-to-zoom cascading, variable injection — these are features of the notebook, not the query engine. The query engine just needs to be embeddable and fast, and DataFusion fits that role well.

## Try It

The WASM query engine ships as part of Micromegas v0.21.0. The [analytics web app](../../web-app/index.md) uses it automatically when you create local query cells.

- [Source code](https://github.com/madesroches/micromegas/tree/main/rust/datafusion-wasm)
- [Crate on crates.io](https://crates.io/crates/micromegas-datafusion-wasm)
- [API documentation](https://docs.rs/micromegas-datafusion-wasm/latest/micromegas_datafusion_wasm/struct.WasmQueryEngine.html)
