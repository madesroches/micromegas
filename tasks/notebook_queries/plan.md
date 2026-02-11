# Notebook Queries Plan

## Overview

Add support for SQL queries that reference other cells' results in notebooks. This enables iterative data exploration where Cell B can query Cell A's output by name.

**Approach:** Each notebook owns a client-side DataFusion WASM context. Remote queries fetch data from the server and register the result locally. Local queries execute entirely in the browser against accumulated cell results.

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

### Every Cell Registers Its Result

Every cell that produces data — regardless of query source — registers its result as a named table in the notebook's local DataFusion context. This builds up a queryable namespace as cells execute top-to-bottom.

## Technical Design

### Notebook DataFusion Context

Each notebook owns a `WasmQueryEngine` instance (DataFusion `SessionContext` compiled to WASM). The context lives for the lifetime of the notebook component. Cell results accumulate in it as named tables.

```typescript
// Notebook-level context, created once per notebook mount
const engine = new WasmQueryEngine()

// After any cell executes and produces data:
engine.register_table("cell_name", tableToIPC(result, 'stream'))

// Local queries execute directly:
const ipcBytes = await engine.execute_sql("SELECT * FROM errors GROUP BY host")
const result = tableFromIPC(ipcBytes)
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
    let result: Table

    if (isLocal) {
      const ipcBytes = await engine.execute_sql(sql)
      result = tableFromIPC(ipcBytes)
    } else {
      result = await executeSql(sql, timeRange, abortSignal, cellDataSource)
    }

    // Every cell's result is registered in the local context
    // (even empty results — downstream cells can still reference the schema)
    engine.register_table(cellName, tableToIPC(result, 'stream'))

    return result
  },
}
```

### Cell Result Registration

Both remote and local query results are registered:

```sql
-- Cell "errors": source=remote
-- Fetches from data lake via FlightSQL, result registered as table "errors"
SELECT time, host, level, msg FROM log_entries WHERE level IN ('ERROR', 'FATAL')

-- Cell "by_host": source=local
-- Executes in local DataFusion context, references "errors" table
-- Result registered as table "by_host"
SELECT host, count(*) as error_count FROM errors GROUP BY host ORDER BY error_count DESC

-- Cell "error_chart": source=local
-- References "by_host" table, also in local context
SELECT * FROM by_host LIMIT 10
```

### DataFusion WASM Engine

Custom wasm-bindgen wrapper around DataFusion's `SessionContext` + `MemTable`:

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

    pub fn register_table(&self, name: &str, ipc_bytes: &[u8]) -> Result<(), JsValue> {
        // Deregister first in case the table already exists (cell re-execution)
        let _ = self.ctx.deregister_table(name);

        // Deserialize Arrow IPC → Vec<RecordBatch> → MemTable → register
        // StreamReader schema is available even when there are zero batches
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

    pub async fn execute_sql(&self, sql: &str) -> Result<Vec<u8>, JsValue> {
        let df = self.ctx.sql(sql).await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        // Get schema before collect() consumes the DataFrame
        let schema: Schema = df.schema().into();
        let batches = df.collect().await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        // Serialize to Arrow IPC — works even with zero batches
        let mut buf = Vec::new();
        let mut writer = StreamWriter::try_new(&mut buf, &schema)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        for batch in &batches {
            writer.write(batch).map_err(|e| JsValue::from_str(&e.to_string()))?;
        }
        writer.finish().map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(buf)
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
```

### Arrow IPC Boundary

Data crosses the JS↔WASM boundary via Arrow IPC serialization:

- **JS → WASM** (register_table): `tableToIPC(table, 'stream')` → `Uint8Array` → Rust `StreamReader`
- **WASM → JS** (execute_sql): Rust `StreamWriter` → `Vec<u8>` → `tableFromIPC(bytes)`

For notebook-scale data (thousands to low millions of rows), IPC serialization is ~10ms for a million-row table. The app already uses IPC throughout (`arrow-stream.ts`).

### Variable Substitution

Macro substitution (`$begin`, `$end`, `$variable`) happens client-side before the query reaches either path. For remote queries this is the existing behavior. For local queries, `substituteMacros()` runs on the SQL string before passing it to `engine.execute_sql()`. The WASM engine sees fully resolved SQL only.

## Implementation Phases

### Phase 1: DataFusion WASM Engine

Build the WASM engine — this is the foundation everything else depends on.

1. **Spike (1-2 days):** Validate DataFusion compiles to `wasm32-unknown-unknown` with `default-features = false, features = ["sql"]`. Build a minimal test crate, attempt compilation, assess dependency wrangling.
2. **If spike succeeds:** Build the `WasmQueryEngine` wasm-bindgen wrapper (~200-400 lines of Rust). Set up build pipeline (`cargo build --target wasm32-unknown-unknown`, `wasm-bindgen`, `wasm-opt`).
3. **If spike fails:** Fall back to DuckDB WASM (`npm install @duckdb/duckdb-wasm`). Different SQL dialect but battle-tested. Arrow version mismatch (`apache-arrow@17` vs app's `@21`) handled via IPC serialization. The rest of the plan still works — only the engine implementation changes.
4. Integrate into analytics-web-app (Vite config, lazy-load WASM module).

**Expected bundle size:** 4-6 MB gzipped (custom DataFusion) or 5-10 MB (DuckDB). Lazy-loaded only when a notebook is open.

Files: new `rust/datafusion-wasm/` crate, Vite config, new `lib/wasm-engine.ts`

### Phase 2: Notebook Context + Local Queries

Wire the WASM engine into the notebook execution loop.

1. Add `Query` type union (`RemoteQuery | LocalQuery`) to `notebook-types.ts`
2. Add versioned `QueryCellConfigV2` with `query: Query` field
3. Add V1→V2 migration (runs on notebook load, saved back on next save). Migration only applies to `QueryCellConfig` cells — `MarkdownCellConfig`, `VariableCellConfig`, and `PerfettoExportCellConfig` are unchanged.
4. Create `WasmQueryEngine` instance per notebook in `useCellExecution.ts`
5. Update `useCellExecution.ts` to dispatch by source and register every cell's result. The `CellExecutionContext.runQuery` signature stays as `(sql: string) => Promise<Table>` — individual cell execute functions require no changes.
6. Reset the engine context on full re-execution (time range change, refresh)
7. Deregister tables on cell deletion; reset + re-execute on cell reorder
8. Update `CellRendererProps.sql` to derive from `query.sql` (the prop stays as `string` for backward compatibility with renderers — the V2→prop mapping extracts `config.query.sql`)

Files: `notebook-types.ts`, `cell-registry.ts`, `useCellExecution.ts`, `NotebookRenderer.tsx`, new `lib/wasm-engine.ts`

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

### Memory Limits

Browser memory is limited. Large datasets from remote queries accumulate in both the JS Arrow Table and the WASM engine's MemTable. Consider:
- Warning when total registered table size exceeds threshold (e.g., 100MB)
- Row count limits on remote query results that get registered

### Context Lifecycle

The WASM context is created when the notebook mounts and destroyed on unmount. On full re-execution (time range change, refresh), the context is reset (all tables deregistered) and cells re-execute top-to-bottom. Individual cell re-execution deregisters the cell's old table and registers the new result.

**Cell deletion:** When a cell is deleted, its table is deregistered from the WASM context. Downstream cells that reference the deleted table will error on their next execution — this is the correct behavior (the dependency is gone).

**Cell reorder (drag-and-drop):** Reordering cells resets the context and triggers a full re-execution from the top. This is necessary because the dependency order may have changed — a cell that was above a dependent may now be below it. The existing `executeFromCell(0)` path handles this.

### Hydration on Page Reload

Cell results and the WASM context are lost on page reload. Cells re-execute on load (current behavior). Future enhancement: cache results in IndexedDB.

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

## References

- [engine_analysis.md](engine_analysis.md) — Engine comparison, WASM research, DuckDB analysis
- [datafusion-wasm-bindings](https://github.com/datafusion-contrib/datafusion-wasm-bindings) — Existing WASM bindings (not usable as-is, see analysis doc)
- [datafusion-wasm-playground](https://github.com/datafusion-contrib/datafusion-wasm-playground) — Live demo
- [DataFusion Discussion #9834](https://github.com/apache/datafusion/discussions/9834) — Web playground discussion
- [databendlabs/jsonb](https://github.com/databendlabs/jsonb) — JSONB implementation used by micromegas
