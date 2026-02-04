# Notebook Queries Plan

## Overview

Add support for client-side SQL queries in notebooks using DataFusion compiled to WebAssembly. This enables cells to query results from other cells without server round-trips, allowing fast iterative data exploration.

## Motivation

Currently, every cell query executes against the server via FlightSQL. For data transformations and exploration workflows, this creates unnecessary latency:

```
Cell A: SELECT * FROM log_entries WHERE level='ERROR'  → server round-trip
Cell B: SELECT host, count(*) FROM ??? GROUP BY host  → needs Cell A's result
```

With notebook queries, Cell B can reference Cell A's result directly in the browser:

```
Cell A: source=server  → fetches from data lake
Cell B: source=notebook, SQL: SELECT host, count(*) FROM A GROUP BY host → runs in WASM
```

## Design Principles

### Cell Type = Display Type

Cell types (table, chart, log) define how data is visualized, not where data comes from. This follows the Grafana panel model where a panel's data source is orthogonal to its visualization.

### Polymorphic Query Configuration

Queries are separate from cells and can come from different sources:

```typescript
interface ServerQuery {
  source: 'server'
  sql: string
}

interface NotebookQuery {
  source: 'notebook'
  sql: string
}

type Query = ServerQuery | NotebookQuery
```

This design allows future data sources without changing the cell model:

```typescript
// Future possibilities
interface CellRefQuery {
  source: 'cell'
  cell: string  // reference another cell's data directly
}

interface HttpQuery {
  source: 'http'
  url: string
  format: 'json' | 'csv' | 'parquet'
}

interface InlineQuery {
  source: 'inline'
  data: Record<string, unknown>[]
}
```

## Technical Design

### Cell Configuration Changes

Current structure (`notebook-types.ts`):

```typescript
interface QueryCellConfig extends CellConfigBase {
  type: 'table' | 'chart' | 'log'
  sql: string  // flat, always server
  options?: Record<string, unknown>
}
```

Proposed structure:

```typescript
interface QueryCellConfig extends CellConfigBase {
  type: 'table' | 'chart' | 'log'
  query: Query  // polymorphic
  options?: Record<string, unknown>
}
```

### Execution Context Changes

Current (`cell-registry.ts`):

```typescript
interface CellExecutionContext {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  runQuery: (sql: string) => Promise<Table>  // always server
}
```

Proposed:

```typescript
interface CellExecutionContext {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  runQuery: (query: Query) => Promise<Table>  // dispatch by source
  cellResults: Record<string, Table>  // results from cells above
}
```

### Query Execution Flow

In `useCellExecution.ts`, the `runQuery` function dispatches based on source:

```typescript
async function runQuery(query: Query, cellResults: Record<string, Table>): Promise<Table> {
  if (query.source === 'server') {
    return executeSql(query.sql, timeRange, abortSignal)
  } else if (query.source === 'notebook') {
    return executeNotebookQuery(query.sql, cellResults)
  }
}
```

### DataFusion WASM Integration

New module `lib/datafusion-wasm.ts`:

```typescript
import { Table } from 'apache-arrow'

// Singleton session context
let sessionContext: DataFusionContext | null = null

export async function initDataFusion(): Promise<void> {
  // Load WASM module, initialize context
}

export async function executeNotebookQuery(
  sql: string,
  cellResults: Record<string, Table>
): Promise<Table> {
  // Register each cell result as a table
  for (const [name, table] of Object.entries(cellResults)) {
    await sessionContext.registerArrowTable(name, table)
  }

  // Execute query
  const result = await sessionContext.sql(sql)
  return result.toArrowTable()
}
```

### Cell Result Registration

When a cell executes successfully, its result is stored for downstream cells:

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

Notebook queries can then reference any cell by name:

```sql
-- Cell named "errors" executed first with source=server
-- This cell uses source=notebook
SELECT host, count(*) as error_count
FROM errors  -- references the "errors" cell
GROUP BY host
ORDER BY error_count DESC
```

## Implementation Phases

### Phase 1: Polymorphic Query Model

1. Update `notebook-types.ts` with Query type union
2. Update `QueryCellConfig` to use `query: Query`
3. Add migration for existing configs (`sql` → `query: { source: 'server', sql }`)
4. Update cell metadata `execute` methods to use new structure
5. Update cell editors to show source selector

### Phase 2: DataFusion WASM Integration

1. Add `datafusion-wasm` npm dependency
2. Create `lib/datafusion-wasm.ts` wrapper module
3. Implement lazy loading of WASM module
4. Add `executeNotebookQuery` function
5. Update `useCellExecution.ts` to track cell results
6. Update `runQuery` to dispatch by source

### Phase 3: UI Updates

1. Add source toggle in cell editor (server/notebook dropdown)
2. Show available cell names for notebook queries
3. Add visual indicator for notebook vs server queries
4. Add error handling for invalid cell references

### Phase 4: Polish

1. Performance optimization (avoid re-registering unchanged tables)
2. Memory management (clear unused tables)
3. Error messages for common issues (circular references, missing cells)
4. Documentation

## Dependencies

- [datafusion-wasm](https://www.npmjs.com/package/datafusion-wasm) - DataFusion compiled to WASM
- Existing: `apache-arrow` (already in use for result handling)

## UDF Availability in WASM

Not all server-side UDFs can be ported to WASM. This section documents which UDFs from `rust/analytics` are suitable for inclusion in the browser-based DataFusion instance.

### UDFs Suitable for WASM (14 total)

These UDFs are pure data transformations with no external dependencies:

#### JSONB Functions (7)

| Function | Description | Source |
|----------|-------------|--------|
| `jsonb_get` | Get value from JSONB object by key | `dfext/jsonb/get.rs` |
| `jsonb_parse` | Parse JSON string to JSONB binary | `dfext/jsonb/parse.rs` |
| `jsonb_object_keys` | Extract keys from JSONB object | `dfext/jsonb/keys.rs` |
| `jsonb_as_string` | Cast JSONB to string | `dfext/jsonb/cast.rs` |
| `jsonb_as_f64` | Cast JSONB to float64 | `dfext/jsonb/cast.rs` |
| `jsonb_as_i64` | Cast JSONB to int64 | `dfext/jsonb/cast.rs` |
| `jsonb_format_json` | Format JSONB as JSON string | `dfext/jsonb/format_json.rs` |

#### Properties Functions (4)

| Function | Description | Source |
|----------|-------------|--------|
| `properties_to_jsonb` | Convert List<Struct> to JSONB | `properties/properties_to_jsonb_udf.rs` |
| `properties_to_dict` | Dictionary-encode properties | `properties/properties_to_dict_udf.rs` |
| `properties_to_array` | Expand dictionary back to array | `properties/properties_to_dict_udf.rs` |
| `properties_length` | Get property count | `properties/properties_to_dict_udf.rs` |

#### Histogram Functions (3)

| Function | Description | Source |
|----------|-------------|--------|
| `make_histogram` | Aggregate values into histogram buckets | `dfext/histogram/histogram_udaf.rs` |
| `sum_histograms` | Combine multiple histograms | `dfext/histogram/sum_histograms_udaf.rs` |
| `expand_histogram` | Expand histogram to (bin_center, count) rows | `dfext/histogram/expand.rs` |

### UDFs NOT Suitable for WASM (12 total)

These UDFs require server-side resources and cannot run in the browser:

#### Require PostgreSQL Database (10)

| Function | Reason |
|----------|--------|
| `delete_duplicate_blocks` | Modifies database |
| `delete_duplicate_processes` | Modifies database |
| `delete_duplicate_streams` | Modifies database |
| `retire_partition_by_file` | Modifies database + object storage |
| `retire_partition_by_metadata` | Modifies database |
| `list_partitions` | Reads from database |
| `list_view_sets` | Reads ViewFactory |
| `materialize_partitions` | Writes to database + object storage |
| `retire_partitions` | Modifies database |
| `view_instance` | Requires LakehouseContext |

#### Require Object Storage (1)

| Function | Reason |
|----------|--------|
| `get_payload` | Fetches block payloads from blob storage |

#### Require Lakehouse Context (1)

| Function | Reason |
|----------|--------|
| `perfetto_trace_chunks` | Generates Perfetto traces from telemetry data |

## JSONB Crate WASM Compatibility

The micromegas project uses the [jsonb crate](https://github.com/databendlabs/jsonb) (v0.5.3) from Databend. This crate doesn't have explicit WASM support, but analysis of its dependencies shows it should be compatible with configuration.

### Dependency Analysis

| Dependency | Version | WASM Status | Notes |
|------------|---------|-------------|-------|
| `byteorder` | 1.5.0 | ✅ Compatible | Pure Rust byte ordering |
| `ethnum` | 1.5.2 | ✅ Compatible | Pure Rust extended numerics |
| `fast-float2` | 0.2.3 | ✅ Compatible | Pure Rust float parsing |
| `itoa` | 1.0 | ✅ Compatible | Pure Rust integer formatting |
| `nom` | 8.0.0 | ✅ Compatible | Parser combinator library |
| `num-traits` | 0.2.19 | ✅ Compatible | Pure Rust numeric traits |
| `ordered-float` | 5.1.0 | ✅ Compatible | Pure Rust ordered floats |
| `serde` | 1.0 | ✅ Compatible | Serialization framework |
| `serde_json` | 1.0 | ✅ Compatible | JSON serialization |
| `jiff` | 0.2.10 | ⚠️ Needs config | Requires `js` feature for browser |
| `rand` | 0.9.2 | ⚠️ Needs config | Requires getrandom configuration |
| `zmij` | 1.0 | ❓ Unknown | Needs verification |

### Configuration Required

#### 1. `rand` / `getrandom` Setup

The `rand` crate depends on `getrandom` which doesn't compile for `wasm32-unknown-unknown` by default. Configure via `.cargo/config.toml`:

```toml
[target.wasm32-unknown-unknown]
rustflags = ['--cfg', 'getrandom_backend="wasm_js"']
```

Or add getrandom as a direct dependency with the `js` feature:

```toml
[dependencies]
getrandom = { version = "0.2", features = ["js"] }
```

#### 2. `jiff` DateTime Library

[Jiff has explicit WASM support](https://docs.rs/jiff/latest/jiff/_documentation/platform/index.html) for browser targets:

- Enable `js` feature for `wasm32-unknown-unknown` target
- Uses JavaScript's `Date.now()` for current time
- Uses `Intl.DateTimeFormat` for timezone detection
- Automatically bundles IANA timezone database for WASM

```toml
[dependencies]
jiff = { version = "0.2", features = ["js"] }
```

### WASM Build Verification Steps

Before implementing, verify WASM compatibility:

1. Create a minimal test crate with jsonb UDFs
2. Add wasm32-unknown-unknown target: `rustup target add wasm32-unknown-unknown`
3. Attempt build: `cargo build --target wasm32-unknown-unknown`
4. Identify and resolve any compilation errors
5. Test in browser environment with wasm-pack

### Alternative: Pure JavaScript Implementation

If Rust WASM proves problematic, consider:

- Using DataFusion's existing WASM bindings without custom UDFs initially
- Implementing critical UDFs (like jsonb functions) in JavaScript/TypeScript
- Adding Rust UDFs incrementally as compatibility is verified

## Considerations

### Bundle Size

DataFusion WASM is approximately 2-5MB. Options:
- Lazy load only when notebook source is used
- Code split into separate chunk

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

Notebook queries don't have implicit time range filtering like server queries. The `$begin` and `$end` macros can still be substituted, but filtering must be explicit in the SQL.

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
      source: notebook
      sql: |
        SELECT host, count(*) as error_count
        FROM errors
        GROUP BY host
        ORDER BY error_count DESC
    layout: { height: 150 }

  - name: error_chart
    type: chart
    query:
      source: notebook
      sql: SELECT * FROM by_host LIMIT 10
    options:
      xColumn: host
      yColumns: [error_count]
    layout: { height: 300 }
```

## References

### DataFusion WASM
- [datafusion-wasm-bindings](https://github.com/datafusion-contrib/datafusion-wasm-bindings) - Official WASM bindings
- [datafusion-wasm-playground](https://github.com/datafusion-contrib/datafusion-wasm-playground) - Live demo
- [DataFusion Discussion #9834](https://github.com/apache/datafusion/discussions/9834) - Web playground discussion

### JSONB Crate
- [databendlabs/jsonb](https://github.com/databendlabs/jsonb) - JSONB implementation used by micromegas

### Rust WASM Resources
- [Jiff Platform Documentation](https://docs.rs/jiff/latest/jiff/_documentation/platform/index.html) - DateTime WASM support
- [getrandom WASM Configuration](https://docs.rs/getrandom/latest/getrandom/#webassembly-support) - Random number generation in WASM
- [Which Crates Work with WASM](https://rustwasm.github.io/book/reference/which-crates-work-with-wasm.html) - General compatibility guide
