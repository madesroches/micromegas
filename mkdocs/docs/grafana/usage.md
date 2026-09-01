# Usage Guide

This guide covers using the Micromegas Grafana plugin to query and visualize telemetry data.

## Quick Start

1. Build a query with the Query Builder, or switch to raw SQL.
2. Leave **Time Filter** checked (default) to scope results to the dashboard time range.
3. Leave **Auto Limit** checked (default) to cap results at the panel's display capacity.
4. Run the query.

With both checkboxes enabled, you don't need `$__timeFilter()` macros or `LIMIT` clauses in your SQL.

## Query Builder

1. **Table**: pick from the dropdown; tables auto-populate from your schema.
2. **Columns**: click **+** to add columns, or use `*` for all columns.
3. **WHERE**: click **+** next to WHERE to add conditions (e.g. `level = 2`); multiple conditions combine with AND.
4. The generated SQL is shown at the bottom — click **Edit SQL** to switch to raw SQL mode.
5. Click **Run query**.

## Time Filter and Auto Limit

**Time Filter** (default: on) applies the Grafana dashboard time range to the query on the backend — no manual time filter needed in SQL, and it works in both Query Builder and Raw SQL modes. Uncheck it to query without a time bound (e.g. across all historical data).

**Auto Limit** (default: on) caps the result count to the panel's display width (`maxDataPoints`), adjusting automatically on resize. Disable it when you need an exact row count (e.g. "show all errors"), when using an aggregation query that's already bounded, or when adding your own `LIMIT`.

The two are independent: Time Filter narrows the time window, Auto Limit caps the row count.

## Raw SQL Mode

Click **Edit SQL**, write SQL, and run. With **Time Filter** checked, the dashboard time range is applied automatically — no macro needed.

### Examples

Time-series:
```sql
SELECT
  date_bin('1 minute', time) AS time,
  exe,
  COUNT(*) as event_count
FROM log_entries
WHERE level = 2
GROUP BY 1, 2
ORDER BY 1
```

Filter by process:
```sql
SELECT
  time,
  msg,
  level
FROM log_entries
WHERE exe = 'api-server'
ORDER BY time DESC
LIMIT 100
```

Aggregate metrics:
```sql
SELECT
  date_bin('5 minutes', time) AS time,
  name,
  AVG(value) as avg_value,
  MAX(value) as max_value
FROM measures
WHERE name LIKE 'cpu.%'
GROUP BY 1, 2
ORDER BY 1
```

## Grafana Variables

Define variables in Dashboard Settings → Variables, then reference them in queries.

**Query variable** (`process`):
```sql
SELECT DISTINCT exe FROM processes
```
```sql
SELECT time, msg
FROM log_entries
WHERE exe = '$process'
```

**Custom variable** (`level`):
```
1 : Fatal
2 : Error
3 : Warn
4 : Info
5 : Debug
```
```sql
SELECT time, msg, level
FROM log_entries
WHERE level = $level
```

**Multi-select variable** (`processes`, multi-select enabled), used with `IN`:
```sql
SELECT time, msg
FROM log_entries
WHERE exe IN ($processes)
```

## Query Performance Tips

- **Keep Time Filter enabled.** Disabling it scans the entire table regardless of the dashboard time range.
- **Keep Auto Limit enabled** for most queries; add an explicit `LIMIT` only when you need a specific row count regardless of panel size.
- **Prefer pre-aggregated views over raw data.** Aggregating raw rows scans everything that matches the filter, even when the final result is small; a materialized view scans only the pre-computed rows.

```sql
-- Fast: query the pre-aggregated view
SELECT
  time_bin as time,
  SUM(CASE WHEN level <= 2 THEN count ELSE 0 END) as error_count
FROM log_stats
GROUP BY time_bin
ORDER BY time_bin

-- Slower: aggregate raw data
SELECT
  date_bin('1 minute', time) AS time,
  COUNT(*) as error_count
FROM log_entries
WHERE level <= 2
GROUP BY 1
ORDER BY 1
```

`log_stats` holds log counts pre-aggregated by minute, process, level, and target, updated as new data arrives and partitioned daily — querying it scans orders of magnitude fewer rows than aggregating `log_entries` directly. Use it for log volume analysis and trend monitoring. For other frequently-used aggregations, ask your administrator to create a custom materialized view — see [Admin Guide - Materialized Views](../admin/maintenance.md).

## Manual Time Filter Macros

For cases the **Time Filter** checkbox doesn't cover — multiple time ranges in one query, or wanting the filter visible in the SQL — disable the checkbox and use macros directly:

- **`$__timeFilter(columnName)`** — expands to a `WHERE` range condition:
  ```sql
  SELECT time, msg
  FROM log_entries
  WHERE $__timeFilter(time)
  ```
- **`$__timeFrom()`** — start of the dashboard time range.
- **`$__timeTo()`** — end of the dashboard time range.

## Next Steps

- [Schema Reference](../query-guide/schema-reference.md) - Available tables and columns
- [Query Patterns](../query-guide/query-patterns.md) - More query examples
- [Functions Reference](../query-guide/functions-reference.md) - SQL functions
