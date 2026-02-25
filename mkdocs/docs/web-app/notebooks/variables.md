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
| `$variableName` | Replaced with the variable's value |
| `$variableName.column` | Replaced with a specific column from a multi-column variable |
| `$begin` | Start of the current time range (ISO 8601 timestamp) |
| `$end` | End of the current time range (ISO 8601 timestamp) |
| `$order_by` | Current sort column (table cells only) |

### Matching Rules

- **Longest name first**: variables are substituted in order of name length (longest first) to prevent partial matches. For example, with variables `$metric` and `$metric_name`, the longer `$metric_name` is substituted first so `$metric` doesn't accidentally match the prefix.
- **Dotted access first**: `$variable.column` references are resolved before simple `$variable` references.
- **SQL escaping**: all substituted values have single quotes escaped (`'` becomes `''`) to prevent SQL injection.

### Examples

```sql
-- Simple variable
SELECT * FROM measures
WHERE name = '$metric'
  AND time >= '$begin' AND time < '$end'

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
```

### Column Format Overrides

Table and transposed table cells support column format overrides using markdown with row macros:

```markdown
[$row.exe](/process/$row.process_id?from=$begin&to=$end)
```

Row macros use `$row.columnName` or `$row["column-name"]` syntax to reference values from the current row. Standard variable macros (`$begin`, `$end`, `$variableName`) are also available in format strings.

## Expression Evaluation

Expression variables evaluate JavaScript expressions in a sandboxed environment to compute derived values.

### Available Bindings

| Binding | Type | Description |
|---------|------|-------------|
| `$begin` | string | Time range start (ISO 8601) |
| `$end` | string | Time range end (ISO 8601) |
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
  AND time >= '$begin' AND time < '$end'
GROUP BY 1
ORDER BY 1
```

### Security

Expressions use an allowlist-based sandbox. Only arithmetic operations, allowed functions, and variable bindings are permitted. Access to `window`, `document`, `eval`, `fetch`, and other browser APIs is blocked.

## Variable Scope

A cell can only reference variables from cells that appear **above it** in the notebook. This is enforced during both substitution and the Available Variables panel display.

- Variables in the main cell list are visible to all cells below.
- Variables inside a horizontal group (HG) are visible to cells below the group.
- A variable cell cannot reference other variable cells at the same level.

The editor panel shows an **Available Variables** panel listing all variables accessible to the currently selected cell, including `$begin`, `$end`, and any upstream user-defined variables with their current values. Multi-column variables show both the full object and individual `.column` accessors.

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
