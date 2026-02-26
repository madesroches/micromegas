# Execution & Auto-Run

## Execution Model

Cells execute **sequentially from top to bottom** until an error occurs. When you run a notebook (or run from a specific cell), each cell executes in order, and the results of earlier cells are available to later cells through the local WASM query engine. If a cell fails, execution stops and remaining cells are skipped.

### Flattened Execution Order

Before execution, the cell list is flattened: horizontal group (HG) containers are removed and their children are inserted in left-to-right order. This means an HG with children A, B, C followed by cell D executes as A, B, C, D.

### Cell States

Each cell has a status that reflects its current execution state:

| State | Description |
|-------|-------------|
| **idle** | Not yet executed |
| **loading** | Currently executing |
| **success** | Executed successfully — results available |
| **error** | Execution failed — error message displayed |

After successful execution, the cell header shows execution stats: row count, data size, and elapsed time (e.g., "1,234 rows (2.3 MB) in 125ms"). During loading, live progress shows rows and bytes received so far.

### Run Cell vs. Run From Here

- **Run Cell**: executes only the selected cell. Useful for re-running a single query after editing.
- **Run From Here**: executes the selected cell and all cells below it in sequence. This is the default when you want to refresh downstream results.

Running from the first cell resets the WASM engine (clearing all registered tables) before starting, ensuring a clean slate.

## Auto-Run

Auto-run automatically re-executes cells when their inputs change, keeping results up to date without manual intervention.

### Enabling Auto-Run

Each cell has an `autoRunFromHere` toggle. When enabled, changes to that cell or its inputs trigger automatic execution from that cell downward.

### Trigger Behavior

| Change Type | Behavior |
|-------------|----------|
| SQL or content edit | **Debounced** (300ms) — waits for the user to stop typing before executing |
| Variable value change | **Immediate** — executes as soon as the value changes |
| Time range change | **Immediate** — full re-execution from the first cell |
| Refresh button | **Immediate** — full re-execution from the first cell |

A re-entrance guard prevents recursive auto-run — if execution is already in progress from an auto-run trigger, additional triggers are queued rather than creating parallel executions.

## Time Range

The time range controls which time window is queried. It is managed at the page level and shared by all cells in the notebook.

### Relative vs. Absolute

- **Relative**: `now-5m`, `now-1h`, `now-7d` — evaluated fresh at each execution, so results always reflect the latest data.
- **Absolute**: specific ISO 8601 timestamps — fixed time window that doesn't change on re-execution.

### Implicit Time Filtering

When a query is sent to the server, the current time range is passed as **separate metadata** alongside the SQL. The server's query planner automatically injects time filters into the execution plan for all materialized views — you do not need to write `WHERE time >= '$begin' AND time < '$end'` in your SQL.

Each view defines which columns are filtered:

| View | Filtered Columns |
|------|-----------------|
| Metrics, logs, async events | `time BETWEEN begin AND end` |
| Blocks | `begin_time <= end AND insert_time >= begin` |
| Export logs | Configurable time column |

This means a simple query like `SELECT time, value FROM measures WHERE name = '$metric'` is automatically scoped to the selected time range.

### Explicit Time References

The `$begin` and `$end` macros are still available for cases where you need the time range values explicitly — for example, in markdown content, link URLs, or `date_bin()` aggregations:

```sql
SELECT
  date_bin('$bin_interval', time) as time,
  avg(value) as value
FROM measures
WHERE name = '$metric'
GROUP BY 1
ORDER BY 1
```

The macros are replaced with absolute ISO 8601 timestamps after resolving any relative expressions.

### Drag-to-Zoom

Chart and swimlane cells support drag-to-zoom: drag horizontally on the visualization to select a time sub-range. This updates the global time range, which triggers re-execution of all cells.

### URL Parameters

Time range is stored in URL parameters `from` and `to`:

```
/screen/my-notebook?from=now-1h&to=now
```

After saving, parameters matching the saved defaults are cleaned up.

## Data Sources

Each query cell can specify which data source to use, overriding the notebook default.

### Resolution Priority

1. **Cell-level override**: the cell's `dataSource` field, if set
2. **Variable reference**: if the data source starts with `$`, it resolves to the named variable's value
3. **Notebook default**: the default data source configured at the system level

### Variable-Based Routing

Combine a datasource variable with query cells to let users choose which backend to query:

1. Add a **variable** cell with `variableType: 'datasource'` — this creates a dropdown of available data sources.
2. Set query cells' data source to `$variableName` — queries route to whichever data source the user selects.

Chart cells with multiple queries can specify per-query data sources, allowing a single chart to compare data from different backends.

## Local WASM Query Engine

Every notebook has a local [DataFusion](https://datafusion.apache.org/) query engine compiled to WebAssembly. This is the core mechanism that makes notebooks interactive and composable.

### How It Works

1. When a data cell (table, transposed table, chart, log, etc.) executes a remote SQL query, the result is automatically **registered as a named table** in the local WASM engine under the cell's name.
2. Any downstream cell can query upstream results locally using `SELECT ... FROM cell_name` — no round-trip to the server.
3. This enables interactive data transformation: fetch data once from the server, then reshape, filter, join, and aggregate locally.

### Cell Results as Tables

Every data cell's result becomes a queryable table named after the cell:

| Cell Name | Queryable As |
|-----------|-------------|
| `raw_metrics` | `SELECT * FROM raw_metrics` |
| `process_info` | `SELECT * FROM process_info` |

**Reference table** cells work the same way — their CSV data is converted to Arrow format and registered by cell name.

**Chart cells** with multiple queries register each query as a separate table using the pattern `cellName.queryName`:

| Cell Name | Query Name | Queryable As |
|-----------|-----------|-------------|
| `cpu_chart` | `user` | `SELECT * FROM "cpu_chart.user"` |
| `cpu_chart` | `system` | `SELECT * FROM "cpu_chart.system"` |

Tables are deregistered when cells are renamed or deleted. Running from the first cell calls `engine.reset()` to clear all tables.

### Usage Pattern

A typical notebook follows this pattern:

1. **Fetch cells** — first cells execute remote SQL queries to fetch raw data from the server.
2. **Transform cells** — subsequent cells query upstream results locally for filtering, joining, aggregation, or reshaping.
3. **Display cells** — final cells present the transformed data as charts, tables, or other visualizations.

This avoids redundant server round-trips and enables rapid iteration on data shaping.

### Example

```
Cell 1: "raw_data" (Table)
  SQL: SELECT time, name, value FROM measures

Cell 2: "thresholds" (Reference Table)
  CSV: name,warn,error
       cpu_usage,80,95
       memory_usage,70,90

Cell 3: "alerts" (Table)
  SQL: SELECT r.time, r.name, r.value, t.warn, t.error
       FROM raw_data r
       JOIN thresholds t ON r.name = t.name
       WHERE r.value > t.warn
```

Cell 3 queries cells 1 and 2 locally — no additional server requests.

### Monitoring Data Size

Each cell's header shows row count and byte size after execution (e.g., "1,234 rows (2.3 MB) in 125ms"). During fetching, live progress shows rows and bytes received so far. Horizontal groups aggregate stats across all children.

### Memory Limits

There are no hard-coded limits on table sizes in the WASM engine. The practical limit is the browser's WebAssembly memory (typically 2-4 GB depending on browser and OS). Monitor cell data sizes via the header stats and keep total notebook memory usage in mind. Large datasets should be filtered or aggregated server-side before registering in the local engine.
