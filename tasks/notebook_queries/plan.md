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
Cell A: source=remote  → fetches from data lake, result registered as "errors" in local context
Cell B: source=local   → executes in local context: SELECT host, count(*) FROM errors GROUP BY host
```

## Design Principles

### Cell Type = Display Type

Cell types (`table`, `chart`, `log`, `propertytimeline`, `swimlane`) define how data is visualized, not where data comes from. This follows the Grafana panel model where a panel's data source is orthogonal to its visualization.

### Polymorphic Query Configuration

Queries are separate from cells and can come from different sources:

```typescript
interface RemoteQuery {
  source: 'remote'
  sql: string
}

interface LocalQuery {
  source: 'local'
  sql: string
}

type Query = RemoteQuery | LocalQuery
```

**Data source routing:** The existing `dataSource` field stays on the cell config (not in the `Query` type). For remote queries, `resolveCellDataSource()` resolves the data source as it does today. For local queries, the `dataSource` field is set to `'notebook'` — a reserved value that `resolveCellDataSource()` recognizes to route execution to the WASM engine instead of the server.

This design allows future data sources without changing the cell model:

```typescript
// Future possibilities
interface CellRefQuery {
  source: 'cell'
  cell: string  // reference another cell's data directly, no SQL
}

interface HttpQuery {
  source: 'http'
  url: string
  format: 'json' | 'csv' | 'parquet'
}
```

### WASM Engine as Single Source of Truth

Every cell that produces data — regardless of query source — stores its result in the notebook's DataFusion WASM context. The WASM engine is the single location where cell data lives. Renderers don't receive a separate copy of the data; they get Arrow FFI views directly into WASM memory. This eliminates duplicate storage and unnecessary serialization round-trips.

## Technical Design

### Arrow Data Flow

The architecture routes all data through the WASM engine. Data enters as IPC bytes (from the server or from local query execution) and exits as Arrow FFI views that JS renderers consume directly.

**Remote query flow (Phase 1 — IPC output):**
```
Server (FlightSQL)
  → HTTP response (custom JSON-framed Arrow IPC)
  → fetchQueryIPC() strips JSON frames, collects raw IPC bytes
  → engine.register_table(name, ipcBytes)  [1 copy: wasm-bindgen JS→WASM]
  → Rust StreamReader parses IPC into RecordBatches  [near-zero-copy: slices into buffer]
  → Data lives in WASM memory as MemTable
  → Renderer reads: engine.read_table_ipc(name)  [IPC serialize + wasm-bindgen copy out]
  → tableFromIPC(ipcBytes) → JS Arrow Table
```

**Local query flow (Phase 1 — IPC output):**
```
engine.execute_sql(sql, registerAs)
  → DataFusion executes query in WASM  [data stays in WASM]
  → Result registered as named MemTable  [no boundary crossing]
  → Returns IPC bytes  [IPC serialize + wasm-bindgen copy out]
  → tableFromIPC(ipcBytes) → JS Arrow Table
```

**Future: FFI output (our own implementation):**
```
  → engine.get_table_ffi(name)  [0 copies: FFI struct pointers]
  → Our FFI reader creates JS Arrow Table from WASM memory views  [1 copy or zero-copy]
```

**Copy count comparison:**

| Path | Without WASM (today) | Phase 1 (WASM + IPC output) | Future (WASM + FFI output) |
|---|---|---|---|
| Server → WASM | N/A (no WASM) | 1-2 (wasm-bindgen + StreamReader) | 1-2 |
| WASM → renderer | N/A | 2-3 (StreamWriter + wasm-bindgen + tableFromIPC) | 0-1 (FFI) |
| Server → renderer (total) | 1 (streamQuery decodes directly) | 3-5 | 2-3 |
| Local query → renderer | N/A | 2-3 | 0-1 |

Phase 1 adds copies on the output path compared to today's direct decode, but enables the core capability (cross-cell references via WASM context). The input path saves the JS decode + re-encode that the original plan required. The FFI output path (implemented as our own ~200 lines of TypeScript, not an external dependency) recovers the output path cost later.

The key architectural win is that data lives in one place (WASM) and all query execution flows through the engine. The output serialization format (IPC vs FFI) is an internal detail that can change without affecting the rest of the system.

### Notebook DataFusion Context

Each notebook owns a `WasmQueryEngine` instance (DataFusion `SessionContext` compiled to WASM). The context lives for the lifetime of the notebook component. Cell results accumulate in it as named tables.

```typescript
// Notebook-level context, created once per notebook mount
const engine = new WasmQueryEngine()

// Remote query: IPC bytes from server go straight to WASM
const ipcBytes = await fetchQueryIPC(params, signal)
engine.register_table("errors", ipcBytes)

// Local query: executes in WASM, result registered automatically
const resultIPC = await engine.execute_sql(
  "SELECT host, count(*) FROM errors GROUP BY host", "by_host"
)
const table = tableFromIPC(resultIPC)

// Read a registered table back for rendering
const errorsIPC = engine.read_table_ipc("errors")
const errorsTable = tableFromIPC(errorsIPC)
```

On re-execution (e.g., time range change), the context is reset and cells re-execute top-to-bottom, re-registering their results.

### Cell Configuration Changes

Current structure (`notebook-types.ts`):

```typescript
export interface QueryCellConfig extends CellConfigBase {
  type: 'table' | 'chart' | 'log' | 'propertytimeline' | 'swimlane'
  sql: string
  options?: Record<string, unknown>
  dataSource?: string
}
```

New versioned structure:

```typescript
// v1: legacy format (existing notebooks)
interface QueryCellConfigV1 extends CellConfigBase {
  type: 'table' | 'chart' | 'log' | 'propertytimeline' | 'swimlane'
  sql: string
  options?: Record<string, unknown>
  dataSource?: string
}

// v2: polymorphic query
interface QueryCellConfigV2 extends CellConfigBase {
  version: 2
  type: 'table' | 'chart' | 'log' | 'propertytimeline' | 'swimlane'
  query: Query
  options?: Record<string, unknown>
  dataSource?: string
}

export type QueryCellConfig = QueryCellConfigV2
```

Notebooks are migrated on load. V1 cells (no `version` field) are upgraded to V2:

```typescript
function migrateCellConfig(cell: QueryCellConfigV1): QueryCellConfigV2 {
  return {
    ...cell,
    version: 2,
    query: { source: 'remote', sql: cell.sql },
  }
}
```

After migration, all runtime code works exclusively with the V2 format. The migration runs once per load and the upgraded config is saved back on the next notebook save.

### DataFusion WASM Engine

Custom wasm-bindgen wrapper around DataFusion's `SessionContext` + `MemTable`. Phase 1 uses IPC for data output (proven, simple). The API is designed so FFI output can be added alongside without changing callers.

```rust
#[wasm_bindgen]
pub struct WasmQueryEngine {
    ctx: SessionContext,
}

#[wasm_bindgen]
impl WasmQueryEngine {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        let config = SessionConfig::new()
            .with_target_partitions(1);
        let ctx = SessionContext::new_with_config(config);
        Self { ctx }
    }

    /// Register Arrow IPC bytes as a named table.
    /// IPC bytes come directly from the server — no JS-side decoding needed.
    pub fn register_table(&self, name: &str, ipc_bytes: &[u8]) -> Result<(), JsValue> {
        let _ = self.ctx.deregister_table(name);

        let reader = StreamReader::try_new(Cursor::new(ipc_bytes), None)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let schema = reader.schema();
        let batches: Vec<RecordBatch> = reader.collect::<Result<_, _>>()
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let table = MemTable::try_new(schema, vec![batches])
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        self.ctx.register_table(name, Arc::new(table))
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(())
    }

    /// Execute SQL, register result as named table, return IPC bytes.
    pub async fn execute_sql(
        &self,
        sql: &str,
        register_as: &str,
    ) -> Result<Vec<u8>, JsValue> {
        let df = self.ctx.sql(sql).await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let schema: Schema = df.schema().into();
        let batches = df.collect().await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        // Register result as named table
        let _ = self.ctx.deregister_table(register_as);
        let mem_table = MemTable::try_new(Arc::new(schema.clone()), vec![batches.clone()])
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        self.ctx.register_table(register_as, Arc::new(mem_table))
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        // Serialize result to IPC for JS consumption
        serialize_to_ipc(&schema, &batches)
    }

    /// Read a registered table's data as IPC bytes.
    pub fn read_table_ipc(&self, name: &str) -> Result<Vec<u8>, JsValue> {
        // Read batches and schema from registered MemTable
        // Serialize to IPC stream bytes
        // ...
    }

    pub fn deregister_table(&self, name: &str) -> Result<(), JsValue> {
        self.ctx.deregister_table(name)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(())
    }

    pub fn reset(&self) {
        for name in self.ctx.table_names() {
            let _ = self.ctx.deregister_table(&name);
        }
    }
}

fn serialize_to_ipc(schema: &Schema, batches: &[RecordBatch]) -> Result<Vec<u8>, JsValue> {
    let mut buf = Vec::new();
    let mut writer = StreamWriter::try_new(&mut buf, schema)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    for batch in batches {
        writer.write(batch).map_err(|e| JsValue::from_str(&e.to_string()))?;
    }
    writer.finish().map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok(buf)
}
```

**Future: Arrow FFI output** — When profiling shows the IPC serialize/copy overhead matters, add FFI-based output methods alongside the IPC ones. The FFI reader on the JS side is ~200 lines of TypeScript implementing the [Arrow C Data Interface](https://arrow.apache.org/docs/format/CDataInterface.html) spec — reading `FFI_ArrowArray`/`FFI_ArrowSchema` structs from WASM memory via `DataView` and building `arrow-js` arrays from buffer pointers. No external dependency needed. See [Arrow FFI Output Path](#arrow-ffi-output-path-future) in Considerations.

### Data Boundary

**JS → WASM (register_table):** Arrow IPC bytes via `&[u8]`. wasm-bindgen copies the bytes into WASM linear memory. This is unavoidable — WASM cannot read JS heap memory. IPC is the right format here because the server already produces it, and we skip JS-side decoding entirely.

**WASM → JS (execute_sql / read_table_ipc):** Arrow IPC bytes via `Vec<u8>`. Rust serializes RecordBatches to IPC, wasm-bindgen copies the bytes out, JS decodes via `tableFromIPC()`. This is the proven path (same approach as DuckDB-WASM).

**Future: WASM → JS via FFI** — see [Arrow FFI Output Path](#arrow-ffi-output-path-future) in Considerations for the zero-copy upgrade path.

### Fetch Pipeline Changes

The current fetch pipeline (`arrow-stream.ts`) decodes Arrow IPC into JS Arrow objects. For the WASM-first architecture, remote queries need the raw IPC bytes instead.

Current `streamQuery()` → `executeSql()` flow:
```
HTTP response → BufferedReader → strip JSON frames → push IPC bytes to queue
  → RecordBatchReader decodes into JS RecordBatch objects → collect into Table
```

New `fetchQueryIPC()` flow:
```
HTTP response → BufferedReader → strip JSON frames → collect raw IPC bytes
  → return Uint8Array (no JS-side Arrow decoding)
```

```typescript
/**
 * Fetches query results as raw Arrow IPC bytes, without JS-side decoding.
 * The bytes go directly to the WASM engine for registration.
 */
export async function fetchQueryIPC(
  params: StreamQueryParams,
  signal?: AbortSignal,
): Promise<Uint8Array> {
  const response = await authenticatedFetch(`${getApiBase()}/query-stream`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      sql: params.sql,
      params: params.params || {},
      begin: params.begin,
      end: params.end,
      data_source: params.dataSource || '',
    }),
    signal,
  })

  // ... HTTP error handling (same as streamQuery) ...

  const bufferedReader = new BufferedReader(response.body!.getReader())
  const ipcChunks: Uint8Array[] = []
  let totalSize = 0

  try {
    while (true) {
      const line = await bufferedReader.readLine()
      if (line === null) break

      const frame: Frame = JSON.parse(line)

      switch (frame.type) {
        case 'schema':
        case 'batch': {
          const bytes = await bufferedReader.readBytes(frame.size)
          ipcChunks.push(bytes)
          totalSize += bytes.length
          break
        }
        case 'done': {
          // Concatenate IPC chunks into single buffer
          const result = new Uint8Array(totalSize)
          let offset = 0
          for (const chunk of ipcChunks) {
            result.set(chunk, offset)
            offset += chunk.length
          }
          return result
        }
        case 'error':
          throw new Error(frame.message)
      }
    }
  } finally {
    bufferedReader.release()
  }

  throw new Error('Unexpected end of stream')
}
```

The existing `streamQuery()` and `executeStreamQuery()` remain for non-notebook callers (other screen types that don't use the WASM engine).

### Execution Context Changes

Current (`cell-registry.ts`):

```typescript
export interface CellExecutionContext {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  runQuery: (sql: string) => Promise<Table>
}
```

**Critical gap:** Cell results do NOT propagate today. Only variable values propagate between cells (via `variableValuesRef`). The `executeFromCell` loop in `useCellExecution.ts` runs cells sequentially but discards each cell's result data — downstream cells cannot access upstream results.

Proposed — the interface stays the same:

```typescript
export interface CellExecutionContext {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  runQuery: (sql: string) => Promise<Table>  // unchanged signature
}
```

The `runQuery` signature does not change. Individual cell execute functions (table, chart, log, variable, etc.) continue to call `runQuery(sql)` exactly as they do today. The query source dispatch and result registration happen in `useCellExecution.ts` when constructing the context — the cell's `runQuery` is bound to either the remote or local path based on the cell's config. This means **variable cells and perfetto export cells require zero changes**.

Note: `cellResults` is not in the context — the DataFusion WASM engine IS the context. Cells don't need explicit access to previous results; they reference them via SQL table names in local queries.

### Query Execution Flow

The dispatch happens in `useCellExecution.ts` when building the `CellExecutionContext` for each cell. The cell's `execute()` function just calls `runQuery(sql)` — it doesn't know or care about the source:

```typescript
// In useCellExecution.ts, per-cell context construction:
const query = (config as QueryCellConfigV2).query
const isLocal = query?.source === 'local'

const context: CellExecutionContext = {
  variables: availableVariables,
  timeRange,
  runQuery: async (sql: string) => {
    if (isLocal) {
      // Execute in WASM, register result, return IPC bytes
      const ipcBytes = await engine.execute_sql(sql, cellName)
      return tableFromIPC(ipcBytes)
    } else {
      // Fetch raw IPC bytes from server, register in WASM
      const ipcBytes = await fetchQueryIPC(
        { sql, params: { begin: timeRange.begin, end: timeRange.end },
          begin: timeRange.begin, end: timeRange.end, dataSource: cellDataSource },
        abortSignal
      )
      engine.register_table(cellName, ipcBytes)

      // Read back via IPC for the renderer
      const resultIPC = engine.read_table_ipc(cellName)
      return tableFromIPC(resultIPC)
    }
  },
}
```

Both paths end the same way: data lives in WASM, renderer gets a JS Table from IPC deserialization. The output path (IPC vs FFI) is an internal detail that can be swapped later without changing the `runQuery` signature or any cell execute functions.

### Cell Result Registration

Both remote and local query results are registered:

```sql
-- Cell "errors": source=remote
-- Fetches from data lake via FlightSQL, IPC bytes registered directly in WASM
SELECT time, host, level, msg FROM log_entries WHERE level IN ('ERROR', 'FATAL')

-- Cell "by_host": source=local
-- Executes in local DataFusion context, references "errors" table
-- Result registered as table "by_host"
SELECT host, count(*) as error_count FROM errors GROUP BY host ORDER BY error_count DESC

-- Cell "error_chart": source=local
-- References "by_host" table, also in local context
SELECT * FROM by_host LIMIT 10
```

### Variable Substitution

Macro substitution (`$begin`, `$end`, `$variable`) happens client-side before the query reaches either path. For remote queries this is the existing behavior. For local queries, `substituteMacros()` runs on the SQL string before passing it to `engine.execute_sql()`. The WASM engine sees fully resolved SQL only.

## Implementation Phases

### Phase 1: DataFusion WASM Engine

Build the WASM engine — this is the foundation everything else depends on.

1. **Spike:** Validate DataFusion compiles to `wasm32-unknown-unknown` with `default-features = false, features = ["sql"]`. Build a minimal test crate, attempt compilation, assess dependency wrangling.
2. **If spike succeeds:** Build the `WasmQueryEngine` wasm-bindgen wrapper with IPC-based input/output. Set up build pipeline (`cargo build --target wasm32-unknown-unknown`, `wasm-bindgen`, `wasm-opt`).
3. **If spike fails:** Fall back to DuckDB WASM (`npm install @duckdb/duckdb-wasm`). Different SQL dialect but battle-tested. Arrow version mismatch (`apache-arrow@17` vs app's `@21`) handled via IPC serialization. The rest of the plan still works — only the engine implementation changes.
4. Integrate into analytics-web-app (Vite config, lazy-load WASM module).

**Expected bundle size:** 4-6 MB gzipped (custom DataFusion) or 5-10 MB (DuckDB). Lazy-loaded only when a notebook is open. No additional JS dependencies beyond what the app already has.

Files: new `rust/datafusion-wasm/` crate, Vite config, new `lib/wasm-engine.ts`

### Phase 2: Fetch Pipeline + Notebook Context

Wire the WASM engine into the notebook execution loop with the WASM-first data flow.

1. Add `fetchQueryIPC()` to `arrow-stream.ts` — strips JSON frames, returns raw IPC bytes without JS-side Arrow decoding. Existing `streamQuery()` / `executeStreamQuery()` stay for non-notebook callers.
2. Add `Query` type union (`RemoteQuery | LocalQuery`) to `notebook-types.ts`
3. Add versioned `QueryCellConfigV2` with `query: Query` field
4. Add V1→V2 migration (runs on notebook load, saved back on next save). Migration only applies to `QueryCellConfig` cells — `MarkdownCellConfig`, `VariableCellConfig`, and `PerfettoExportCellConfig` are unchanged.
5. Create `WasmQueryEngine` instance per notebook in `useCellExecution.ts`
6. Update `useCellExecution.ts` to dispatch by source: remote queries use `fetchQueryIPC()` + `register_table()`, local queries use `execute_sql()`. Both paths return Tables via FFI. The `CellExecutionContext.runQuery` signature stays as `(sql: string) => Promise<Table>` — individual cell execute functions require no changes.
7. Reset the engine context on full re-execution (time range change, refresh)
8. Deregister tables on cell deletion
9. Update `CellRendererProps.sql` to derive from `query.sql` (the prop stays as `string` for backward compatibility with renderers — the V2→prop mapping extracts `config.query.sql`)

Files: `arrow-stream.ts`, `notebook-types.ts`, `cell-registry.ts`, `useCellExecution.ts`, `NotebookRenderer.tsx`, `lib/wasm-engine.ts`

### Phase 3: UI for Source Selection

1. Source toggle in cell editor (remote/local dropdown)
2. Cell name display showing available tables from cells above
3. Visual indicator for local vs remote queries
4. Error handling: invalid table references, empty results, WASM engine errors

Files: cell editor components

### Phase 4: UDFs in WASM

Register the WASM-suitable Rust UDFs in the client-side DataFusion context. Same functions, same behavior, client and server.

**14 of 26 UDFs are WASM-suitable:**
- JSONB functions (7): `jsonb_get`, `jsonb_get_str`, etc.
- Properties functions (4): `property_get`, `property_get_str`, etc.
- Histogram functions (3): `histogram_bucket_count`, etc.

The remaining 12 require PostgreSQL, object storage, or lakehouse context and cannot run in the browser.

Files: `rust/datafusion-wasm/` crate, UDF registration

### Phase 5: Polish

1. Memory management warnings for large cell results
2. Error messages for common issues (missing tables, type mismatches)
3. Graceful handling when referenced cell has error/blocked status
4. Documentation

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

If the spike succeeds and we use DataFusion WASM, local and remote queries share the same SQL dialect. If we fall back to DuckDB WASM, local queries use DuckDB's dialect (PostgreSQL-compatible, but not identical to DataFusion). This is the main cost of the fallback path.

## Example Notebook

```yaml
cells:
  - name: errors
    type: table
    query:
      source: remote
      sql: |
        SELECT time, host, level, msg
        FROM log_entries
        WHERE level IN ('ERROR', 'FATAL')
        AND time BETWEEN '$begin' AND '$end'
    layout: { height: 200 }

  - name: by_host
    type: table
    query:
      source: local
      sql: |
        SELECT host, count(*) as error_count
        FROM errors
        GROUP BY host
        ORDER BY error_count DESC
    layout: { height: 150 }

  - name: error_chart
    type: chart
    query:
      source: local
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
