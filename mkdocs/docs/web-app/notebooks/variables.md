# Variables

The variable system lets you parameterize notebooks. Variable cells define named values that are substituted into downstream SQL queries, markdown content, and expressions. Variables appear in the cell title bar as compact inputs — dropdowns, text fields, or computed labels — and their values are synced to the URL for sharing.

## Variable Types

| Type | Input | Execution |
|------|-------|-----------|
| **Text** | Free-form text field | None — value updates immediately |
| **Combobox** | Dropdown from SQL query results | Runs SQL to populate options |
| **Expression** | Computed (read-only display) | Evaluates JavaScript expression |
| **Datasource** | Dropdown of available data sources | Fetches data source list from API |

### Text

The simplest variable type. The user types a value directly. Supports an optional `defaultValue`. Input is debounced (300ms) before propagating to downstream cells.

### Combobox

A dropdown populated by a SQL query. The query can return one or more columns:

- **Single column**: each row becomes a string option.
- **Multiple columns**: each row becomes an object option. The dropdown label shows all column values joined by `|`. Individual columns are accessible via `$variable.column` syntax.

After execution, the current value is validated against the returned options. If the current value is no longer valid, the default value or first option is auto-selected.

### Expression

A computed variable whose value is derived from a JavaScript expression. The value is displayed as a read-only label in the title bar. Expressions are re-evaluated when the time range or upstream variables change.

See [Expression Evaluation](#expression-evaluation) below.

### Datasource

A dropdown populated with available data sources from the API. Use this to let users choose which backend to query. Reference the variable in a query cell's data source field using `$variableName` syntax.

## SQL Macro Substitution

SQL queries, markdown content, and chart unit labels all support macro substitution. Macros are replaced before the query is sent to the server.

### Syntax

| Macro | Description |
|-------|-------------|
| `$cellName[N].column` | Replaced with a value from an upstream cell's result table (row N, named column) |
| `$cellName.selected.column` | Replaced with a value from the selected row in an upstream table cell |
| `$variableName` | Replaced with the variable's value |
| `$variableName.column` | Replaced with a specific column from a multi-column variable |
| `$from` | Start of the current time range (ISO 8601 timestamp) |
| `$to` | End of the current time range (ISO 8601 timestamp) |
| `$order_by` | Current sort column (table cells only) |

### Matching Rules

- **Cell result references first**: `$cell[N].column` references are resolved before other patterns.
- **Selected row references next**: `$cell.selected.column` references are resolved after cell result refs but before dotted variable patterns. The keyword `selected` disambiguates from dotted variable access.
- **Longest name first**: variables are substituted in order of name length (longest first) to prevent partial matches. For example, with variables `$metric` and `$metric_name`, the longer `$metric_name` is substituted first so `$metric` doesn't accidentally match the prefix.
- **Dotted access first**: `$variable.column` references are resolved before simple `$variable` references.
- **SQL escaping**: all substituted values have single quotes escaped (`'` becomes `''`) to prevent SQL injection.

### Examples

```sql
-- Simple variable
SELECT * FROM measures
WHERE name = '$metric'

-- Multi-column variable (combobox returning name + unit columns)
SELECT time, value
FROM measures
WHERE name = '$metric.name'
ORDER BY time

-- Sort variable (table cells)
SELECT process_id, exe, start_time
FROM processes
ORDER BY $order_by
LIMIT 100

-- Cell result reference (upstream cell "game_session" returns a table with process_id column)
SELECT time, level, target, msg
FROM view_instance('log_entries', '$game_session[0].process_id')
ORDER BY time DESC
LIMIT 100

-- Selected row reference (upstream cell "processes" has row selection enabled)
SELECT time, level, target, msg
FROM view_instance('log_entries', '$processes.selected.process_id')
ORDER BY time DESC
LIMIT 100
```

### Row Selection

Table cells support interactive row selection. When enabled (via the **Row Selection** section in the cell editor), a radio-button column appears in the table. Clicking a row selects it, and its values become available to downstream cells via `$cellName.selected.column` macros.

- **Selection mode** is configured in the cell editor: None (default) or Single.
- **Selection state** is ephemeral (like pagination) — it resets when the cell re-executes.
- **Waiting for selection**: when a downstream cell references `$cell.selected.column` but no row is selected yet, the cell shows a "waiting for selection" placeholder instead of executing.
- The Available Variables panel shows `$cellName.selected.column` entries for cells with selection enabled.

### Column Format Overrides

Table and transposed table cells support column format overrides using markdown with row macros:

```markdown
[$row.exe](/process/$row.process_id?from=$from&to=$to)
```

Row macros use `$row.columnName` or `$row["column-name"]` syntax to reference values from the current row. Standard variable macros (`$from`, `$to`, `$variableName`) are also available in format strings.

### Template Functions

Markdown templates (Map detail panel, Markdown cells, and table column overrides) support a small set of function-call expressions that operate on resolved macro values. Function calls are **template-only** — they are not applied inside SQL queries.

#### `format_value(value, unit)`

Adaptive unit formatting. Picks the best display unit for each individual value, matching how the chart cell formats stats.

| Template | Output |
|---|---|
| `format_value(3678630912, 'bytes')` | `3.4 GB` |
| `format_value($metric_avg, $metric.unit)` | `3.4 GB` (when `$metric.unit` is `bytes`) |
| `format_value($cell.selected.duration_ns, 'nanoseconds')` | `4.07 milliseconds` |
| `format_value($row.bytes_used, 'bytes')` | `3.4 GB` (table column override) |

Arguments may be:

- **Macros** — `$variable`, `$variable.column`, `$cell[N].column`, `$cell.selected.column`, `$row.column`, `$row["column"]` — resolved to their raw value before the function runs (so byte counts and large floats keep full precision).
- **String literals** — `'bytes'` or `"bytes"` (either quote style; the *opposite* quote may appear inside without escaping).
- **Numeric literals** — `3678630912`, `-1.5`, etc.

The accepted unit vocabulary is the same set the chart understands — see `lib/units.ts` for canonical names and aliases (`bytes`, `KB`, `MB`, `seconds`, `ms`, `µs`, `bits/s`, `percent`, `degrees`, `boolean`, …).

#### Error behavior

When an argument macro is unresolved (unknown variable, no row selected, missing column), the function call is left in the rendered output as its original source text and a warning is surfaced:

- Map detail panel and Markdown cell: amber-bordered warning banner above the rendered body.
- Table column override: amber warning icon next to the column header, with the warnings listed in its tooltip.

This avoids silent failures while keeping the cell renderable.

#### Limitations (v1)

- No nested function calls — `format_value(round($x), 'bytes')` is not supported.
- No arithmetic or conditionals — `$a + $b` is not a function call.
- No backslash escapes in string literals — switch the outer quote if you need a literal quote.
- SQL queries do **not** support function calls.

## Expression Evaluation

Expression variables evaluate JavaScript expressions in a sandboxed environment to compute derived values.

### Available Bindings

| Binding | Type | Description |
|---------|------|-------------|
| `$from` | string | Time range start (ISO 8601) |
| `$to` | string | Time range end (ISO 8601) |
| `$duration_ms` | number | Time range duration in milliseconds |
| `$innerWidth` | number | Browser viewport width in CSS pixels |
| `$devicePixelRatio` | number | Device pixel ratio (e.g., 2 for Retina displays) |
| `$variableName` | string | Any upstream variable value |

### Available Functions

| Function | Description |
|----------|-------------|
| `snap_interval(ms)` | Snaps a millisecond duration to a human-friendly SQL interval string |
| `Math.*` | All JavaScript `Math` methods and constants (`Math.floor`, `Math.PI`, etc.) |
| `new Date(...)` | Date construction for date arithmetic |

### `snap_interval`

Takes a duration in milliseconds and returns the largest standard interval that fits within it. Standard intervals: `1ms`, `10ms`, `100ms`, `500ms`, `1s`, `5s`, `15s`, `30s`, `1m`, `5m`, `15m`, `30m`, `1h`, `6h`, `1d`, `7d`, `30d`.

This is commonly used to compute time bin sizes for aggregation queries:

```javascript
snap_interval($duration_ms / $innerWidth)
```

With a 24-hour time range (`$duration_ms` = 86,400,000) and a 1920px viewport (`$innerWidth` = 1920), this computes 86400000 / 1920 = 45000ms, which snaps to `30s`.

### Example: Adaptive Time Bins

A common pattern uses an expression variable to compute a time bin interval, then references it in chart queries:

**Expression variable** `bin_interval`:
```javascript
snap_interval($duration_ms / ($innerWidth * $devicePixelRatio))
```

**Chart SQL** using the computed interval:
```sql
SELECT
  date_bin('$bin_interval', time) as time,
  avg(value) as value
FROM measures
WHERE name = '$metric'
GROUP BY 1
ORDER BY 1
```

### Security

Expressions use an allowlist-based sandbox. Only arithmetic operations, allowed functions, and variable bindings are permitted. Access to `window`, `document`, `eval`, `fetch`, and other browser APIs is blocked.

## Variable Scope

A cell can only reference variables and cell results from cells that appear **above it** in the notebook. This is enforced during both substitution and the Available Variables panel display.

- Variables in the main cell list are visible to all cells below.
- Variables inside a horizontal group (HG) are visible to cells below the group.
- A variable cell cannot reference other variable cells at the same level.

The editor panel shows an **Available Variables** panel listing all variables accessible to the currently selected cell, including `$from`, `$to`, upstream user-defined variables with their current values, and upstream cell results with their column schemas. Multi-column variables show both the full object and individual `.column` accessors. Cell results show `$cellName[0].column` entries for each column in the result schema.

## URL Parameter Sync

Variable values are encoded in the URL using **delta-based encoding** — only values that differ from the saved defaults appear as URL parameters.

### How It Works

1. When a notebook is saved, each variable's `defaultValue` becomes the baseline.
2. At runtime, if a variable's value matches its saved default, it is **not** included in the URL.
3. If the value differs, it appears as a URL parameter: `?metric=memory_usage`.
4. When someone opens the URL, parameters override the saved defaults. Missing parameters use the saved defaults.

This keeps URLs clean when using default values while allowing precise state sharing when values are customized.

### Multi-Column Values

Multi-column variable values (from combobox queries returning multiple columns) are serialized with a `mcol:` prefix followed by JSON:

```
?metric=mcol:%7B%22name%22%3A%22cpu%22%2C%22unit%22%3A%22percent%22%7D
```

### Reserved Parameters

The following URL parameter names are reserved and cannot be used as variable names:

- `from` — time range start
- `to` — time range end
- `type` — screen type

Variable names that conflict with reserved parameters are rejected during cell name validation.
