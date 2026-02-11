# Notebook Queries Plan

## Overview

Add support for SQL queries that reference other cells' results in notebooks. This enables iterative data exploration where Cell B can query Cell A's output by name.

**Approach: hybrid.** Start with server-side session queries (simplest path, full UDF support, zero bundle size). Optionally add client-side WASM execution later for offline/low-latency use cases.

See [engine_analysis.md](engine_analysis.md) for the engine comparison and research that informed this plan.

## Motivation

Currently, every cell query executes independently against the server via FlightSQL. There's no way to compose cell results:

```
Cell A: SELECT * FROM log_entries WHERE level='ERROR'  → server round-trip
Cell B: SELECT host, count(*) FROM ??? GROUP BY host  → needs Cell A's result, can't reference it
```

With session queries, Cell B references Cell A's result by name:

```
Cell A: source=server  → fetches from data lake
Cell B: source=session, SQL: SELECT host, count(*) FROM errors GROUP BY host → server queries cached result
```

## Design Principles

### Cell Type = Display Type

Cell types (`table`, `chart`, `log`, `propertytimeline`, `swimlane`) define how data is visualized, not where data comes from. This follows the Grafana panel model where a panel's data source is orthogonal to its visualization.

### Polymorphic Query Configuration

Queries are separate from cells and can come from different sources:

```typescript
interface ServerQuery {
  source: 'server'
  sql: string
}

interface SessionQuery {
  source: 'session'
  sql: string
}

type Query = ServerQuery | SessionQuery
```

This design allows future data sources without changing the cell model:

```typescript
// Future possibilities
interface NotebookQuery {
  source: 'notebook'  // client-side WASM execution (Phase 4)
  sql: string
}

interface CellRefQuery {
  source: 'cell'
  cell: string  // reference another cell's data directly
}

interface HttpQuery {
  source: 'http'
  url: string
  format: 'json' | 'csv' | 'parquet'
}
```

## Technical Design

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

Proposed change — add an optional `query` field (backward compatible, no migration needed):

```typescript
export interface QueryCellConfig extends CellConfigBase {
  type: 'table' | 'chart' | 'log' | 'propertytimeline' | 'swimlane'
  sql: string               // kept for backward compat, used when query is absent
  query?: Query             // new: polymorphic query, takes precedence over sql
  options?: Record<string, unknown>
  dataSource?: string
}
```

When `query` is present, it takes precedence. When absent, the cell behaves as today (implicit `{ source: 'server', sql }`). No migration required for existing notebooks.

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

Proposed:

```typescript
export interface CellExecutionContext {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  runQuery: (query: Query) => Promise<Table>  // dispatch by source
  cellResults: Record<string, Table>          // results from cells above
}
```

### Cell Result Propagation

The `executeFromCell` function in `useCellExecution.ts` must accumulate cell results:

```typescript
// In useCellExecution.ts executeFromCell
const cellResults: Record<string, Table> = {}

for (let i = 0; i < cells.length; i++) {
  const cell = cells[i]
  const state = await executeCell(i, cellResults)

  if (state.data) {
    cellResults[cell.name] = state.data  // available to subsequent cells
  }
}
```

### Query Execution Flow

The `runQuery` function dispatches based on source:

```typescript
async function runQuery(query: Query, cellResults: Record<string, Table>): Promise<Table> {
  if (query.source === 'server') {
    return executeSql(query.sql, timeRange, abortSignal)
  } else if (query.source === 'session') {
    return executeSessionQuery(query.sql, cellResults)
  }
}
```

### Server-Side Session Query Design

Session queries send the SQL plus referenced cell result tables to the server, which registers them as temp tables in a DataFusion session context and executes the query.

**Client side** (`lib/session-query.ts`):

```typescript
import { Table, tableToIPC } from 'apache-arrow'

export async function executeSessionQuery(
  sql: string,
  cellResults: Record<string, Table>
): Promise<Table> {
  // Serialize referenced cell results as Arrow IPC
  const tables: Record<string, Uint8Array> = {}
  for (const [name, table] of Object.entries(cellResults)) {
    tables[name] = tableToIPC(table, 'stream')
  }

  // POST to server endpoint
  const response = await fetch('/api/v1/session-query', {
    method: 'POST',
    body: encodeSessionQueryRequest(sql, tables),
  })

  // Response is Arrow IPC stream (same as existing FlightSQL path)
  return decodeArrowResponse(response)
}
```

**Server side** (new endpoint in `analytics-web-srv`):

1. Receive SQL + named Arrow IPC byte arrays
2. Create an ephemeral `SessionContext`
3. For each named table: deserialize IPC → `RecordBatch` → `MemTable` → register
4. Execute SQL, stream results back as Arrow IPC
5. Drop the context (no server-side state between requests)

This is stateless — each request is self-contained. No session management, no cleanup, no memory leaks. The tradeoff is re-sending cell data on each query, but for notebook-scale data (thousands to low millions of rows) the serialization cost is negligible.

**Full UDF support:** Since the query runs in the server's DataFusion, all 26 UDFs (jsonb, histogram, properties, lakehouse) are available. No porting needed.

### Cell Result Registration

Session queries can reference any cell above them by name:

```sql
-- Cell named "errors" executed first with source=server
-- This cell uses source=session
SELECT host, count(*) as error_count
FROM errors  -- references the "errors" cell result
GROUP BY host
ORDER BY error_count DESC
```

## Implementation Phases

### Phase 1: Cell Result Propagation (frontend only)

Add `cellResults: Record<string, Table>` to `CellExecutionContext`. Modify `useCellExecution.ts` to accumulate results from each cell and pass them to subsequent cells' execution contexts.

This is the foundation — no new query sources yet, but it unblocks everything else.

Files: `cell-registry.ts`, `useCellExecution.ts`

### Phase 2: Server-Side Session Queries

1. Add `Query` type union to `notebook-types.ts`
2. Add `query?: Query` to `QueryCellConfig` (backward compatible)
3. New server endpoint `POST /api/v1/session-query` accepting SQL + named Arrow IPC tables
4. `executeSessionQuery()` client function in `lib/session-query.ts`
5. Update `runQuery` dispatch in `useCellExecution.ts`

Files: `notebook-types.ts`, `useCellExecution.ts`, new `lib/session-query.ts`, new server endpoint in `analytics-web-srv`

### Phase 3: UI for Source Selection

1. Source toggle in cell editor (server/session dropdown)
2. Cell name autocomplete for session queries (list cells above current)
3. Visual indicator for session vs server queries
4. Error handling for invalid cell references, empty results

Files: cell editor components

### Phase 4: Client-Side WASM (optional — spike first)

This phase is deferred and contingent on a spike. See [engine_analysis.md](engine_analysis.md) for full details.

**Spike (1-2 days):** Validate that DataFusion compiles to `wasm32-unknown-unknown` with `default-features = false, features = ["sql"]`. Build a minimal test crate, attempt compilation, assess dependency wrangling effort.

**If spike succeeds (~3-4 days total):**
- Build custom wasm-bindgen wrapper (~200-400 lines of Rust) around `SessionContext` + `MemTable`
- Arrow IPC for data transfer across JS-WASM boundary
- Add `source: 'notebook'` to the `Query` union
- Lazy-load WASM module only when notebook query cells exist
- Path to compiling the 14 WASM-suitable Rust UDFs directly into the browser

**If spike fails:** DuckDB WASM as fallback (npm install, 1 day integration). Different SQL dialect but battle-tested. Arrow version mismatch (`apache-arrow@17` vs app's `@21`) handled via IPC serialization.

**UDF availability in WASM (14 of 26):** JSONB functions (7), properties functions (4), histogram functions (3). The remaining 12 require PostgreSQL, object storage, or lakehouse context and cannot run in the browser. Full details in the analysis doc.

**Expected bundle size:** 4-6 MB gzipped for custom DataFusion WASM, 5-10 MB for DuckDB WASM. Lazy-loaded only when needed.

### Phase 5: Polish

1. Performance optimization (avoid re-sending unchanged cell results)
2. Memory management warnings for large cell results
3. Error messages for common issues (circular references, missing cells)
4. Documentation

## Considerations

### Bundle Size

Phase 2 (server-side session queries) adds zero bundle size — it's just a fetch call. Phase 4 (WASM) would add 4-6 MB gzipped, lazy-loaded only when a notebook query cell exists.

### Session Lifecycle

Session queries are stateless: cell data is sent with each request, the server creates an ephemeral context, executes, and discards. No server-side session management. This trades bandwidth for simplicity — acceptable for notebook-scale data.

### Memory Limits

Browser memory is limited. Large datasets should remain server-side. Consider:
- Warning when cell results exceed threshold (e.g., 100MB)
- Option to not register large results as tables

### Hydration on Page Reload

Cell results are lost on page reload. Options:
- Re-execute cells on load (current behavior)
- Cache results in IndexedDB (future enhancement)

### Circular References

Cells execute top-to-bottom. A cell can only reference cells above it. This prevents circular references by design.

### Time Range Variables

Session queries don't have implicit time range filtering like server queries. The `$begin` and `$end` macros can still be substituted, but filtering must be explicit in the SQL.

## Example Notebook

```yaml
cells:
  - name: errors
    type: table
    query:
      source: server
      sql: |
        SELECT time, host, level, msg
        FROM log_entries
        WHERE level IN ('ERROR', 'FATAL')
        AND time BETWEEN '$begin' AND '$end'
    layout: { height: 200 }

  - name: by_host
    type: table
    query:
      source: session
      sql: |
        SELECT host, count(*) as error_count
        FROM errors
        GROUP BY host
        ORDER BY error_count DESC
    layout: { height: 150 }

  - name: error_chart
    type: chart
    query:
      source: session
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
