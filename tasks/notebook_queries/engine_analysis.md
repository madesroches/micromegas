# Notebook Queries: Query Engine Analysis

Challenges the assumptions in [plan.md](plan.md), specifically the choice of DataFusion WASM as the client-side query engine.

## The Existing `datafusion-wasm` Package Is Useless For This

The plan references `datafusion-wasm` (the npm package from `datafusion-wasm-bindings`). That repo has **3 GitHub stars** and solves a different problem entirely — querying remote Parquet files. Key findings:

- ~670 lines of Rust total, of which ~385 are an unsafe `Send+Sync` wrapper for OpenDAL (object storage plumbing)
- The actual DataFusion wasm-bindgen surface is ~95 lines in `core.rs`
- Results are returned as **pretty-printed ASCII strings**, not Arrow data
- **No API to register tables from JavaScript** — only supports remote data sources
- No Arrow IPC, no zero-copy, no Arrow data crosses the WASM boundary in usable form

This is not a usable starting point. But that doesn't mean DataFusion in WASM is off the table — it just means building custom bindings from scratch.

## Building Custom DataFusion WASM Bindings

### DataFusion compiles to WASM

DataFusion's core SQL engine compiles to `wasm32-unknown-unknown` with `default-features = false`. The key is disabling everything that pulls in C/C++ dependencies:

```toml
datafusion = { version = "52", default-features = false, features = ["sql"] }
arrow = { version = "54", default-features = false, features = ["ipc"] }
tokio = { version = "1", features = ["macros", "rt", "sync"] }
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"
getrandom = { version = "0.2", features = ["js"] }
chrono = { version = "0.4", features = ["wasmbind"] }
```

No `compression` (kills `zstd-sys`), no `parquet`, no `crypto_expressions`. Just the SQL engine on in-memory Arrow tables. Single-threaded (`target_partitions=1`), no disk manager.

### What you'd build (~200-400 lines of Rust)

```rust
#[wasm_bindgen]
impl WasmQueryEngine {
    pub fn new() -> Self { /* SessionContext, partitions=1, disk disabled */ }
    pub fn register_table(&self, name: &str, ipc_bytes: &[u8]) -> Result<(), JsValue> {
        // Deserialize IPC -> Vec<RecordBatch> -> MemTable -> register
    }
    pub async fn execute_sql(&self, sql: &str) -> Result<Vec<u8>, JsValue> {
        // Execute SQL -> collect RecordBatches -> serialize to IPC bytes
    }
    pub fn deregister_table(&self, name: &str) -> Result<(), JsValue> { ... }
}
```

The JS side is trivial because the analytics-web-app already speaks Arrow IPC (`arrow-stream.ts` uses `RecordBatchReader.from()`):

```typescript
import { tableToIPC, tableFromIPC } from 'apache-arrow';

engine.register_table("errors", tableToIPC(cellResult, 'stream'));
const result = tableFromIPC(await engine.execute_sql("SELECT ..."));
```

### Arrow interop via IPC serialization

Two approaches exist for moving Arrow data across the JS-WASM boundary:

**IPC serialization (recommended):** JS calls `tableToIPC()`, passes bytes to WASM, Rust deserializes with `arrow_ipc::reader::StreamReader`. Results go back the same way. This is the same pattern used by DuckDB-WASM. For notebook-scale data (thousands to low millions of rows), serialization cost is negligible (~10ms for a million-row table).

**Arrow C Data Interface (zero-copy):** Uses `arrow-wasm` + `arrow-js-ffi` to share memory without copying. Faster for large datasets but the libraries are immature (v0.1.0, 28 stars), memory management is manual, and JS-to-WASM direction still needs a copy. Not worth the complexity.

### Effort estimate

| Task | Effort | Risk |
|------|--------|------|
| Get DataFusion to compile for `wasm32-unknown-unknown` | 1-2 days | Medium — dependency wrangling |
| Write the wasm-bindgen wrapper | Half a day | Low |
| Build pipeline (`cargo build --target wasm32`, `wasm-bindgen`, `wasm-opt`) | Half a day | Low |
| Integrate into analytics-web-app (Vite, lazy loading) | 1 day | Low |
| **Total** | **~3-4 days** | |

The dependency wrangling is the only real risk. If DataFusion won't compile cleanly in a day, you bail and use DuckDB WASM as the fallback.

### Expected bundle size

With `sql` feature only + `wasm-opt -Oz` + LTO + `opt-level = "z"` + `strip = true`: estimated **4-6 MB gzipped**. WASM compresses 60-70% with gzip/brotli. Lazy-loaded only when a notebook query cell exists.

### Known WASM constraints

- **Single-threaded**: No `rt-multi-thread`. DataFusion's async model is cooperative and yields between batches, so it won't block the main thread on moderate datasets.
- **No disk spill**: Everything in WASM linear memory. Fine for notebook-scale data.
- **No filesystem**: `DiskManagerConfig::Disabled`.
- **Memory**: WASM linear memory can grow to 4 GB but browsers may limit. Warning on large datasets is still warranted.

### Path to custom UDFs

The real payoff: the 14 WASM-suitable UDFs identified in the plan (jsonb, histogram, properties functions) are Rust code in `rust/analytics/`. They can be compiled directly into the WASM build and registered with the DataFusion session context. Same functions, same behavior, client and server. DuckDB WASM can never offer this.

## Custom DataFusion WASM vs DuckDB WASM

| | Custom DataFusion WASM | DuckDB WASM |
|--|--|--|
| **Build effort** | ~3-4 days | ~1 day (npm install) |
| **Maintenance burden** | You own it | Community maintains it |
| **SQL dialect match with server** | Identical (both DataFusion) | Different dialect |
| **Arrow version** | You control it | Pins arrow@17 (app uses @21) |
| **Custom UDFs** | Can compile Rust UDFs directly | JS UDFs only |
| **Bundle size** | ~4-6 MB gzipped | ~5-10 MB gzipped |
| **Battle-tested** | No | Yes |
| **Risk if it doesn't compile** | Bail to DuckDB in 1-2 days | N/A |

## Full Engine Comparison

| Criterion | DuckDB WASM | sql.js | AlaSQL | Arquero | DataFusion WASM |
|-----------|------------|--------|--------|---------|----------------|
| **Bundle (gzipped)** | 5-10 MB | ~315 KB | ~80-100 KB | ~50-80 KB | <10 MB |
| **Maturity** | High | High | Moderate | Moderate | Very Low |
| **SQL Richness** | Excellent | Basic (SQLite) | Moderate | No SQL | Good (limited in WASM) |
| **Arrow Integration** | Native, excellent | None | None | Good (fromArrow/toArrow) | Immature |
| **Analytical Perf** | Best-in-class | Poor | Poor | Good for small data | Unknown |
| **WASM/JS** | WASM | WASM | Pure JS | Pure JS | WASM |
| **Register Arrow Tables** | Yes (insertArrowTable) | No | No | Yes (fromArrow) | Immature |
| **JavaScript UDFs** | Yes | Yes | Yes | N/A (fluent API) | No |
| **Maintenance** | Very Active (commercial) | Maintained | Low/Volunteer | Active | Minimal |
| **GitHub Stars** | ~1.9k (wasm) / 30k (core) | ~13.5k | ~5k | ~1.7k | 3 |
| **npm Downloads/week** | ~150-300k | ~177-330k | ~150-175k | ~10k | Negligible |

### sql.js
SQLite compiled to WASM. Small bundle (~315 KB gzipped) and battle-tested, but row-oriented with no Arrow integration. Designed for OLTP, not analytics. DuckDB outperforms it by 10-100x on analytical workloads.

### AlaSQL
Pure JavaScript, smallest payload (~80-100 KB gzipped), but degrades on complex queries and has no Arrow support. Maintained by unpaid volunteers with infrequent updates.

### Arquero
Not SQL — uses a fluent JavaScript API. Doesn't fit a SQL-cell notebook model.

### Newer entrants

**PGlite** (`@electric-sql/pglite`): Full PostgreSQL in WASM, <3 MB gzipped. Impressive but row-oriented — wrong workload profile for columnar analytics.

**Squirreling**: Pure JS SQL engine, ~9 KB gzipped. Brand new (late 2025), single developer, no benchmarks. Too immature.

## DuckDB WASM Is the Obvious Client-Side Choice

It dominates on every practical axis: adoption, performance, Arrow-native data path, rich SQL dialect (CTEs, window functions, QUALIFY, PIVOT), and active commercial support.

**One real concern:** DuckDB WASM pins `apache-arrow@17.0.0`, and the analytics-web-app uses `apache-arrow@^21.1.0`. This version mismatch means Arrow Table objects can't be passed directly. Data would need to go through IPC serialization — which works fine since the streaming code already deals in IPC frames, but it's an extra step.

## The Server-Side Alternative Deserves More Weight

The plan's motivation is avoiding "unnecessary latency," but consider:

- A notebook cell referencing another cell's result could send a **server-side query that operates on cached results**. The server caches cell results in DataFusion's session context and lets subsequent cells reference them by name — same UX, zero WASM.
- Round-trip latency for small result-set queries is ~50-200ms. For an interactive notebook, that's barely noticeable.
- Full UDF support (all 14 WASM-suitable ones **plus** all 12 server-only ones) with zero effort.
- Zero added bundle size. Zero cold-start delay. Zero browser memory pressure.

The tradeoff is server load, but this is an internal analytics tool, not a public SaaS product.

## Recommended Options

### Option 1: Server-side cell result caching (simplest)

Add a session-scoped cache to the analytics server. When Cell A returns results, the server registers them as a named temp table in the DataFusion session context. Cell B's query says `FROM errors` and the server resolves it.

The polymorphic query model from the plan still works — `source: 'notebook'` becomes `source: 'session'` and goes to the same server with a session ID.

**Pros:** Zero frontend complexity, full UDF support, no bundle size impact, works with existing infrastructure.
**Cons:** Requires server-side session management, still has network latency per cell.

### Option 2: Custom DataFusion WASM (best long-term fit)

Build ~200-400 lines of custom wasm-bindgen bindings around DataFusion's `SessionContext` + `MemTable` APIs. Use Arrow IPC for data transfer across the JS-WASM boundary. ~3-4 days of effort with a clear bail-out point (if dependency wrangling fails after day 1-2, switch to DuckDB).

**Pros:** Same SQL dialect as server, no Arrow version mismatch, path to compiling Rust UDFs into WASM, smaller bundle than DuckDB, you control the dependency tree.
**Cons:** You own the maintenance, not battle-tested, 3-4 day upfront investment, risk of dependency issues.

### Option 3: DuckDB WASM (safe fallback)

Drop-in npm package. The polymorphic query model stays the same. Lazy-load the WASM binary only when a notebook query cell exists. Handle the Arrow version mismatch through IPC serialization.

**Pros:** Battle-tested, excellent perf, native Arrow, strong community.
**Cons:** 5-10 MB bundle (lazy-loaded), Arrow version gap (pins arrow@17 vs app's @21), different SQL dialect than server, no path to custom Rust UDFs.

### Option 4: Hybrid (recommended)

Start with Option 1 (server-side caching) to get the notebook query UX working immediately. In parallel, spike Option 2 (custom DataFusion WASM) — spend 1-2 days on dependency wrangling to validate it compiles. If it works, use it as the client-side engine. If it doesn't, fall back to Option 3 (DuckDB WASM). The polymorphic query model in the plan supports all three backends cleanly.

## Conclusion

The plan's architecture (polymorphic query types, cell result registration, sequential execution) is solid. The original choice of the `datafusion-wasm` npm package is a non-starter — the package is a proof-of-concept that returns ASCII strings, not Arrow data. But building custom DataFusion WASM bindings from scratch is a ~3-4 day effort with a clear risk profile: the dependency wrangling either works in a day or it doesn't, and DuckDB WASM is always available as the fallback. The architectural payoff (dialect consistency, Rust UDF portability, controlled Arrow version) makes it worth attempting before reaching for DuckDB.
