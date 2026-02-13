# Notebook Queries Plan

## Overview

Add support for SQL queries that reference other cells' results in notebooks. This enables iterative data exploration where Cell B can query Cell A's output by name.

**Approach:** Each notebook owns a client-side DataFusion WASM context that serves as the **single source of truth** for all cell data. Remote queries fetch raw IPC bytes from the server and register them directly in the WASM engine — no JS-side Arrow decoding. Local queries execute entirely in WASM against accumulated cell results. Renderers read data back from WASM via Arrow IPC initially, with a path to Arrow C Data Interface (FFI) zero-copy views implemented in our own code.

See [engine_analysis.md](engine_analysis.md) for the engine comparison and research that informed this plan. The analysis recommends a hybrid approach starting with server-side session caching, but that underestimates the complexity of managing stateful sessions across multiple server processes (sticky routing or shared session state). Client-side execution avoids this entirely — the state lives in the browser, dies with the tab.

## Motivation

Currently, every cell query executes independently against the server via FlightSQL. There's no way to compose cell results:

```
Cell A: SELECT * FROM log_entries WHERE level='ERROR'  → server round-trip
Cell B: SELECT host, count(*) FROM ??? GROUP BY host  → needs Cell A's result, can't reference it
```

With a local DataFusion context, Cell B references Cell A's result by name:

```
Cell A: dataSource=default  → fetches from data lake, result registered as "errors" in WASM context
Cell B: dataSource=notebook → executes in WASM context: SELECT host, count(*) FROM errors GROUP BY host
```

## Design Principles

### Cell Type = Display Type

Cell types (`table`, `chart`, `log`, `propertytimeline`, `swimlane`) define how data is visualized, not where data comes from. This follows the Grafana panel model where a panel's data source is orthogonal to its visualization.

### Data Source Routing

The existing `dataSource?: string` field on cell configs already supports arbitrary values. We add `'notebook'` as a reserved value. When `resolveCellDataSource()` returns `'notebook'`, execution routes to the WASM engine instead of the server. No new `Query` type union needed — the data source dropdown in the cell editor is the only UI change.

The existing `DataSourceSelector` dropdown already shows server data sources and `$variable` references. We add `'notebook'` as an option (shown only in notebook context). This fits naturally into the existing pattern — no separate "source toggle" UI.

### WASM Engine as Single Source of Truth

Every cell that produces data — regardless of query source — registers its result in the notebook's DataFusion WASM context. The WASM engine is the authoritative store for cross-cell references. Renderers currently receive a decoded JS Table copy via IPC (peak memory ~2x per cell). A future FFI output path could provide zero-copy views into WASM memory.

## Technical Design

### Arrow Data Flow

**Remote cell (any server data source):**
```
Server → fetchQueryIPC() → Uint8Array (raw IPC stream)
  → engine.register_table(name, ipcBytes)  [1 copy: JS→WASM via wasm-bindgen]
  → tableFromIPC(ipcBytes) → JS Arrow Table for renderer  [1 copy: IPC decode]
```

**Notebook cell (dataSource: 'notebook'):**
```
engine.execute_and_register(sql, name)
  → DataFusion executes in WASM, registers as MemTable  [0 copies]
  → Returns IPC bytes  [serialize + wasm-bindgen copy out]
  → tableFromIPC(ipcBytes) → JS Arrow Table for renderer  [1 copy: IPC decode]
```

The IPC output path adds copies vs today's direct `streamQuery` decode, but enables cross-cell references. A future FFI output path could eliminate the IPC serialize/deserialize overhead.

### Notebook DataFusion Context

Each notebook eagerly owns a `WasmQueryEngine` instance — created when the notebook mounts so that remote cell results are always registered for cross-cell references. The context lives for the lifetime of the notebook component. Cell results accumulate in it as named tables.

```typescript
// Remote cell: IPC bytes from server → register in WASM → decode for renderer
const ipcBytes = await fetchQueryIPC(params, signal)
engine.register_table("errors", ipcBytes)
const table = tableFromIPC(ipcBytes)

// Notebook cell: execute in WASM → result auto-registered → decode for renderer
const resultIPC = await engine.execute_and_register(
  "SELECT host, count(*) FROM errors GROUP BY host", "by_host"
)
const table = tableFromIPC(resultIPC)
```

On re-execution (e.g., time range change), the context is reset and cells re-execute top-to-bottom, re-registering their results.

### Cell Configuration

No schema changes needed. The existing `dataSource?: string` field on `QueryCellConfig` is used directly:

```typescript
export interface QueryCellConfig extends CellConfigBase {
  type: 'table' | 'chart' | 'log' | 'propertytimeline' | 'swimlane'
  sql: string
  options?: Record<string, unknown>
  dataSource?: string  // set to 'notebook' for local WASM execution
}
```

When `dataSource` is `'notebook'`, the cell's SQL executes in the WASM engine. Any other value (or undefined) routes to the server as before. No migration needed — existing notebooks work unchanged.

### DataFusion WASM Engine

Implemented in `rust/datafusion-wasm/src/lib.rs`. Custom wasm-bindgen wrapper around DataFusion's `SessionContext` + `MemTable`. Uses IPC for data input/output.

**API:**

```typescript
class WasmQueryEngine {
  constructor()
  register_table(name: string, ipc_bytes: Uint8Array): number  // returns row count
  execute_sql(sql: string): Promise<Uint8Array>                 // returns IPC bytes
  execute_and_register(sql: string, register_as: string): Promise<Uint8Array>  // execute + register + return IPC
  deregister_table(name: string): boolean                       // returns whether table existed
  reset(): void                                                 // deregister all tables
}
```

**Notable implementation details:**
- Works around DataFusion 52.1 LimitPushdown bug by filtering the optimizer rule
- `chrono/wasmbind` feature routes `Utc::now()` through `js_sys::Date` (avoids `std::time` panic on wasm32)
- `getrandom/wasm_js` feature routes entropy to `crypto.getRandomValues()`
- Single-threaded execution (`wasm-bindgen-futures` provides `spawn_local`)

**Future: Arrow FFI output** — When profiling shows the IPC serialize/copy overhead matters, add FFI-based output methods alongside the IPC ones. See [Arrow FFI Output Path](#arrow-ffi-output-path-future) in Considerations.

### Data Boundary

**JS → WASM (register_table):** Arrow IPC bytes via `&[u8]`. wasm-bindgen copies into WASM linear memory. Unavoidable — WASM cannot read JS heap memory.

**WASM → JS (execute_sql / execute_and_register):** Arrow IPC bytes via `Vec<u8>`. Rust serializes RecordBatches to IPC, wasm-bindgen copies out, JS decodes via `tableFromIPC()`. Same approach as DuckDB-WASM.

**Future: WASM → JS via FFI** — see [Arrow FFI Output Path](#arrow-ffi-output-path-future) in Considerations.

### Fetch Pipeline

Implemented in `arrow-stream.ts`. `fetchQueryIPC()` fetches raw Arrow IPC streaming-format bytes from the server without JS-side Arrow decoding. The bytes go directly to the WASM engine's `register_table()` and are also decoded via `tableFromIPC()` for the renderer.

The existing `streamQuery()` and `executeStreamQuery()` remain unchanged for non-notebook callers.

### Execution Context and Query Flow

The `CellExecutionContext` interface is unchanged:

```typescript
export interface CellExecutionContext {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  runQuery: (sql: string) => Promise<Table>
}
```

The dispatch happens in `useCellExecution.ts` when building the per-cell `runQuery` closure. The cell's `execute()` function just calls `runQuery(sql)` — it doesn't know the data source:

```typescript
runQuery: async (sql: string) => {
  if (isNotebookSource) {
    // Execute locally in WASM engine
    const ipcBytes = await engine.execute_and_register(sql, cell.name)
    return tableFromIPC(ipcBytes)
  } else if (engine) {
    // Remote execution, register result in WASM for downstream notebook cells
    const ipcBytes = await fetchQueryIPC({ sql, ... }, abortSignal)
    engine.register_table(cell.name, ipcBytes)
    return tableFromIPC(ipcBytes)
  } else {
    // Remote execution without WASM (no notebook cells exist)
    return executeSql(sql, timeRange, abortSignal, cellDataSource)
  }
}
```

Variable cells and perfetto export cells require zero changes — they call `runQuery` as before.

### Cell Result Registration

Both remote and notebook query results are registered in the WASM engine:

```sql
-- Cell "errors": dataSource=default (remote)
-- Fetches from data lake via FlightSQL, IPC bytes registered directly in WASM
SELECT time, host, level, msg FROM log_entries WHERE level IN ('ERROR', 'FATAL')

-- Cell "by_host": dataSource=notebook (local)
-- Executes in WASM DataFusion context, references "errors" table
-- Result registered as table "by_host"
SELECT host, count(*) as error_count FROM errors GROUP BY host ORDER BY error_count DESC

-- Cell "error_chart": dataSource=notebook (local)
-- References "by_host" table, also in WASM context
SELECT * FROM by_host LIMIT 10
```

### Variable Substitution

Macro substitution (`$begin`, `$end`, `$variable`) happens client-side before the query reaches either path. For remote queries this is the existing behavior. For local queries, `substituteMacros()` runs on the SQL string before passing it to `engine.execute_sql()`. The WASM engine sees fully resolved SQL only.

## Implementation Phases

### Phase 1: DataFusion WASM Engine — COMPLETE

See [wasm_query_poc.md](wasm_query_poc.md) for full details and validation results.

- DataFusion compiles to `wasm32-unknown-unknown` and runs in the browser
- WASM crate at `rust/datafusion-wasm/` with 8 integration tests (all passing)
- Build pipeline: `python3 build.py` → cargo build → wasm-bindgen → wasm-opt → copy to web app
- Standalone `local_query` screen type validates end-to-end flow
- Bundle size: 24 MB raw, 5.9 MB gzipped (lazy-loaded only when needed)

### Phase 2: Notebook Integration — COMPLETE

Wired the WASM engine into the notebook execution loop. `'notebook'` is a reserved data source option in the existing `DataSourceSelector` dropdown. Manually tested end-to-end and covered by automated tests.

**What's done:**
1. Added `execute_and_register` and `deregister_table` to WASM engine
2. Added `'notebook'` option to `DataSourceSelector` (shown via `showNotebookOption` prop)
3. Updated `useCellExecution.ts`: accepts optional WASM engine, dispatches by data source, registers all results in WASM when engine exists, re-executes all cells when engine becomes available
4. Updated `NotebookRenderer.tsx`: eagerly loads WASM engine on notebook mount, passes engine to `useCellExecution`, resets on full re-execution, deregisters on cell deletion
5. WASM engine load error banner in notebook UI
6. Download progress and execution time in cell title bars: live row/byte progress during remote fetch (via `fetchQueryIPC` onProgress callback), total elapsed time shown on completion. `CellState` extended with `elapsedMs` and `fetchProgress` fields.
7. Automated tests for WASM engine routing (notebook vs remote vs no-engine paths, reset behavior, error on missing engine)

Files: `rust/datafusion-wasm/src/lib.rs`, `DataSourceSelector.tsx`, `CellEditor.tsx`, `useCellExecution.ts`, `NotebookRenderer.tsx`, `notebook-types.ts`, `CellContainer.tsx`

### Phase 3: UDFs in WASM — DEFERRED (separate PR)

Register the WASM-suitable Rust UDFs in the client-side DataFusion context. Same functions, same behavior, client and server.

**14 of 26 UDFs are WASM-suitable:**
- JSONB functions (7): `jsonb_get`, `jsonb_get_str`, etc.
- Properties functions (4): `property_get`, `property_get_str`, etc.
- Histogram functions (3): `histogram_bucket_count`, etc.

The remaining 12 require PostgreSQL, object storage, or lakehouse context and cannot run in the browser.

Files: `rust/datafusion-wasm/` crate, UDF registration

### Phase 4: SHOW TABLES Support — DEFERRED (separate PR)

Add `SHOW TABLES` support to the WASM engine so users can inspect which cell results are available for cross-cell references. When executed in a notebook-local cell, `SHOW TABLES` returns the list of registered table names (i.e., cell names whose results are in the WASM context).

### Phase 5: Polish — DEFERRED (separate PR)

1. ~~Download progress and execution time feedback in cell title bars~~ — DONE (Phase 2)
2. Memory management warnings for large cell results
3. Error messages for common issues (missing tables, type mismatches)
4. Graceful handling when referenced cell has error/blocked status
5. Documentation

## Considerations

### Bundle Size

The WASM engine adds 4-6 MB gzipped, lazy-loaded only when a notebook is open. No impact on other screen types.

### Memory

The WASM engine is the single source of truth for cell data. With IPC output, renderers also hold a decoded JS Table copy, so peak memory per cell is ~2x the data size (WASM MemTable + JS Table). The JS Table can be garbage-collected when the component unmounts.

Consider warning when total registered table size exceeds a threshold (e.g., 100MB).

### Arrow FFI Output Path (Future)

When profiling shows the IPC output overhead matters, add FFI-based output methods to the WASM engine. This eliminates IPC serialization/deserialization on the output path, reducing copies from 2-3 to 0-1.

The Arrow C Data Interface is a stable, simple spec: two C structs (`ArrowArray`, `ArrowSchema`) containing buffer pointers and metadata. Implementing a JS reader is ~200 lines of TypeScript:

1. Rust side: `arrow::ffi::to_ffi()` converts RecordBatch to FFI structs pointing at existing WASM memory (0 copies)
2. `FFITable` wrapper exposes pointer addresses via wasm-bindgen
3. JS side: read FFI structs from WASM memory via `DataView`, follow buffer pointers, build `arrow-js` `Data` objects from `TypedArray` views

With `copy=true` (recommended): create JS-owned copies of each buffer — immune to `memory.grow()` invalidation. With `copy=false` (zero-copy): create `TypedArray` views directly into WASM memory — invalidated if WASM allocates and triggers `memory.grow()`.

**memory.grow() risk with copy=false:** WASM linear memory is an `ArrayBuffer` exposed to JS. When `memory.grow()` is called, the old buffer is detached and all views become invalid. The safe window for zero-copy is within a single synchronous microtask (no intervening WASM calls). React rendering is synchronous within a frame, so render-only access is viable, but any concurrent cell execution could invalidate views.

**No external dependency needed.** The FFI spec is stable and small. Existing references for implementation: [arrow-js-ffi](https://github.com/kylebarron/arrow-js-ffi) (Kyle Barron, ~200 lines core), [Arrow C Data Interface spec](https://arrow.apache.org/docs/format/CDataInterface.html).

### Context Lifecycle

The WASM context is created when the notebook mounts and destroyed on unmount. On full re-execution (time range change, refresh), the context is reset (all tables deregistered) and cells re-execute top-to-bottom. Individual cell re-execution deregisters the cell's old table and registers the new result.

**Cell deletion:** When a cell is deleted, its table is deregistered from the WASM context. Downstream cells that reference the deleted table will error on their next execution — this is the correct behavior (the dependency is gone).

### Hydration on Page Reload

Cell results and the WASM context are lost on page reload. Cells re-execute on load (current behavior).

### Circular References

Cells execute top-to-bottom. A cell can only reference cells above it. This prevents circular references by design.

### Cell Name Uniqueness

Cell names must be unique within a notebook — they map to table names in the DataFusion context. This is already enforced by `createDefaultCell()` in `cell-registry.ts` which generates unique names, and by the rename validation in the UI.

### Time Range Variables

Local queries don't have implicit time range filtering. `$begin` and `$end` macros are substituted client-side before execution, but filtering must be explicit in the SQL.

### SQL Dialect

Local (notebook) and remote queries share the same DataFusion SQL dialect. No dialect mismatch.

## Example Notebook

```yaml
cells:
  - name: errors
    type: table
    sql: |
      SELECT time, host, level, msg
      FROM log_entries
      WHERE level IN ('ERROR', 'FATAL')
      AND time BETWEEN '$begin' AND '$end'
    layout: { height: 200 }

  - name: by_host
    type: table
    dataSource: notebook
    sql: |
      SELECT host, count(*) as error_count
      FROM errors
      GROUP BY host
      ORDER BY error_count DESC
    layout: { height: 150 }

  - name: error_chart
    type: chart
    dataSource: notebook
    sql: SELECT * FROM by_host LIMIT 10
    options:
      xColumn: host
      yColumns: [error_count]
    layout: { height: 300 }
```

## Appendix: Future Optimizations

### WASM-Side HTTP Fetch

The WASM engine could fetch remote query results directly from the server, eliminating the JS intermediary and the wasm-bindgen JS→WASM copy for `register_table`.

**Current flow (Phase 1):**
```
JS fetchQueryIPC() → HTTP response → Uint8Array in JS heap
  → wasm-bindgen copies to WASM memory → register_table parses IPC
```

**Optimized flow:**
```
WASM engine calls fetch() via web-sys → HTTP response body read into WASM memory
  → parse JSON frames + IPC in Rust → register as MemTable
```

The response bytes land directly in WASM linear memory — no JS→WASM copy.

**How:** `reqwest` with `features = ["wasm"]` uses the browser's `fetch()` under the hood. Same-origin requests share the browser's cookie jar, so session cookies work. The JSON-framed protocol parsing (strip `{"type":"schema","size":N}\n` headers, extract raw IPC bytes) is straightforward in Rust.

**Complications:**
- **Auth token refresh:** The JS `authenticatedFetch` wrapper handles 401→refresh→retry. This logic would need to be reimplemented in Rust or delegated to JS via a wasm-bindgen callback.
- **Bundle size:** Adding `reqwest` + `web-sys` HTTP types to the WASM binary increases its size.
- **Streaming:** `web-sys` exposes `ReadableStream` but the Rust ergonomics are rough. `reqwest` on WASM supports `.bytes()` for the full response body but chunk-by-chunk streaming requires more glue code.

**When to consider:** When profiling shows the `register_table` wasm-bindgen copy is a bottleneck (unlikely for notebook-scale data, but relevant for very large remote query results). The WASM engine already knows the SQL — it could own the full fetch→parse→register cycle, making `runQuery` for remote cells a single WASM call with no JS-side data handling.

## References

- [engine_analysis.md](engine_analysis.md) — Engine comparison, WASM research, DuckDB analysis
- [Arrow C Data Interface spec](https://arrow.apache.org/docs/format/CDataInterface.html) — The FFI struct format (for future zero-copy output path)
- [arrow-js-ffi](https://github.com/kylebarron/arrow-js-ffi) — Reference implementation of JS-side FFI reader (~200 lines core)
- [Zero-copy Apache Arrow with WebAssembly](https://observablehq.com/@kylebarron/zero-copy-apache-arrow-with-webassembly) — Kyle Barron's detailed walkthrough of the approach
- [datafusion-wasm-bindings](https://github.com/datafusion-contrib/datafusion-wasm-bindings) — Existing WASM bindings (not usable as-is, see analysis doc)
- [datafusion-wasm-playground](https://github.com/datafusion-contrib/datafusion-wasm-playground) — Live demo
- [DataFusion Discussion #9834](https://github.com/apache/datafusion/discussions/9834) — Web playground discussion
- [databendlabs/jsonb](https://github.com/databendlabs/jsonb) — JSONB implementation used by micromegas
