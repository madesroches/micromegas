# Usage Guide

This guide covers using the Micromegas Grafana plugin to query and visualize your telemetry data.

## Quick Start

The Micromegas plugin makes querying simple with automatic filtering and limiting:

1. **Build your query** using the Query Builder or write SQL
2. **Enable Time Filter checkbox** (enabled by default) to automatically filter results to the dashboard time range
3. **Enable Auto Limit checkbox** (enabled by default) to automatically limit results to the panel's display capacity
4. **Run query** - results are automatically filtered and limited!

No need to write `$__timeFilter()` macros or `LIMIT` clauses in your SQL - the checkboxes handle it automatically.

## Query Builder

The visual query builder helps you construct SQL queries without writing SQL manually.

### Building a Query

1. **Select Table**:
   - Click the table dropdown
   - Choose from available tables (e.g., `log_entries`, `measures`, `thread_spans`)
   - Tables auto-populate from your schema

2. **Select Columns**:
   - Click **+** to add columns
   - Choose from available columns for the selected table
   - Use `*` to select all columns
   - Type custom column names or expressions

3. **Add WHERE Clauses**:
   - Click **+** next to WHERE to add conditions
   - Enter conditions like `level = 2`
   - Multiple conditions combined with AND

4. **Preview Query**:
   - The SQL query is shown at the bottom
   - Click **Edit SQL** to switch to raw SQL mode

5. **Run Query**:
   - Click **Run query** to execute
   - Results appear in the panel

### Time Filter Checkbox

The **Time Filter** checkbox (enabled by default) automatically applies the Grafana dashboard time range to your queries. When enabled:

- The backend receives the time range from the dashboard time picker
- No need to manually add time filters in your SQL
- Works with both Query Builder and Raw SQL modes
- Time range updates automatically when you change the dashboard time picker

**Example**: To query error logs within the dashboard's selected time range:

1. Select table: `log_entries`
2. Select columns: `time`, `msg`, `level`, `exe`
3. Add WHERE clause: `level = 2`
4. Ensure **Time Filter** checkbox is checked (default)
5. Run query

The backend automatically filters results to the dashboard time range - no SQL time filter needed!

**Note**: To disable automatic time filtering (e.g., to query all historical data), uncheck the **Time Filter** checkbox.

### Auto Limit Checkbox

The **Auto Limit** checkbox (enabled by default) automatically limits query results to match the panel's display capacity. When enabled:

- The backend receives the optimal number of data points based on panel width
- Prevents overwhelming Grafana with excessive data
- No need to manually add LIMIT clauses in most cases
- Limits are dynamically adjusted when panel is resized

**Example**: A panel that is 1000 pixels wide might have `maxDataPoints` of 1000, automatically limiting results to 1000 rows.

**When to disable**:

- When you need specific result counts (e.g., "show all errors")
- When using aggregation queries that already return limited results
- When you want to add explicit LIMIT clauses in your SQL

**Note**: The Auto Limit applies to the number of rows returned, while the Time Filter applies to the time range. They work together - Time Filter narrows the time window, and Auto Limit caps the number of results.

## Raw SQL Mode

For advanced queries, switch to raw SQL mode:

1. Click **Edit SQL** button
2. Write your SQL query
3. Ensure **Time Filter** checkbox is checked (default)
4. Click **Run query**

The dashboard time range is automatically applied - no need for time filter macros in your SQL!

### SQL Examples

#### Time-Series Query

```sql
SELECT
  time_bucket('1 minute', time) AS time,
  process_name,
  COUNT(*) as event_count
FROM log_entries
WHERE level = 2
GROUP BY 1, 2
ORDER BY 1
```

Time range is automatically applied when **Time Filter** is checked.

#### Filtering by Process

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

#### Aggregating Metrics

```sql
SELECT
  time_bucket('5 minutes', time) AS time,
  metric_name,
  AVG(value) as avg_value,
  MAX(value) as max_value
FROM measures
WHERE metric_name LIKE 'cpu.%'
GROUP BY 1, 2
ORDER BY 1
```

## Grafana Variables

Use Grafana variables for dynamic queries.

### Dashboard Variables

Create variables in Dashboard Settings → Variables:

**Variable: `process`**
```sql
SELECT DISTINCT exe FROM processes
```

**Use in query**:
```sql
SELECT time, msg
FROM log_entries
WHERE exe = '$process'
```

**Variable: `level`**
```sql
-- Custom values
1 : Fatal
2 : Error
3 : Warn
4 : Info
5 : Debug
```

**Use in query**:
```sql
SELECT time, msg, level
FROM log_entries
WHERE level = $level
```

### Multi-Select Variables

Enable multi-select in variable settings:

**Variable: `processes`** (multi-select enabled)
```sql
SELECT DISTINCT exe FROM processes
```

**Use with IN clause**:
```sql
SELECT time, msg
FROM log_entries
WHERE exe IN ($processes)
```

Time filtering is automatically applied when the **Time Filter** checkbox is enabled.

## Query Performance Tips

### Use Time Filters

Always enable the **Time Filter** checkbox to limit data scanned:

- ✅ **Good**: Time Filter checkbox enabled (default)
  - Automatically limits query to dashboard time range
  - Reduces data scanned and improves performance

- ❌ **Bad**: Time Filter checkbox disabled
  - Scans entire table regardless of dashboard time range
  - Can be slow for large datasets

### Limit Result Size

The **Auto Limit** checkbox (enabled by default) automatically limits results based on panel width. For custom limits, you can:

**Option 1: Use Auto Limit (Recommended)**
- Enable **Auto Limit** checkbox
- Let Grafana automatically determine optimal limit
- No SQL changes needed

**Option 2: Explicit LIMIT clause**
- Disable **Auto Limit** checkbox
- Add your own LIMIT to the SQL:

```sql
SELECT * FROM log_entries
ORDER BY time DESC
LIMIT 1000  -- Custom limit
```

**Best Practice**: Keep **Auto Limit** enabled for most queries. Only use explicit LIMIT when you need a specific number of results regardless of panel size.

### Use Pre-Aggregated Views

For best performance, use pre-aggregated materialized views instead of aggregating raw data:

```sql
-- ✅ Best: Query pre-aggregated view
-- Fast - minimal data scanning
SELECT
  time_bin as time,
  SUM(CASE WHEN level <= 2 THEN count ELSE 0 END) as error_count
FROM log_stats
GROUP BY time_bin
ORDER BY time_bin

-- ⚠️ Acceptable: Aggregate raw data
-- Slow - scans all matching rows
SELECT
  time_bucket('1 minute', time) AS time,
  COUNT(*) as error_count
FROM log_entries
WHERE level <= 2
GROUP BY 1
ORDER BY 1

-- ❌ Bad: Raw data without aggregation
-- Very slow - scans and transfers large result set
SELECT time, msg FROM log_entries WHERE level <= 2
```

**Available Pre-Aggregated Views:**

- **`log_stats`** - Pre-aggregated log counts by minute, process, level, and target
  - Much faster than aggregating `log_entries`
  - Updated automatically as new data arrives
  - Daily partitioned for efficient storage

**Why Pre-Aggregated Views Are Faster:**

The bottleneck in queries is **data scanning**, not data transfer. Aggregating raw data requires scanning millions of rows from object storage, even when the final result is small. Pre-aggregated views store the computed results, so queries scan far fewer rows.

**Example**: Counting errors over 24 hours

- `log_entries`: Scan 100 million log rows → aggregate → return 1,440 data points (1 per minute)
- `log_stats`: Scan 1,440 pre-aggregated rows → return 1,440 data points

Both return the same amount of data, but `log_stats` is 100,000x faster because it scans 100,000x less data.

**Recommendation**: Use `log_stats` for log volume analysis and trend monitoring. For other frequently-used aggregation queries, ask your administrator to create custom materialized views. See [Admin Guide - Materialized Views](../admin/authentication.md) for setup details.

## Advanced: Manual Time Filter Macros

For advanced use cases where you need explicit control over time filtering in your SQL, you can disable the **Time Filter** checkbox and use macros directly in your queries.

### When to Use Macros Instead of the Checkbox

- **Complex time logic**: When you need multiple different time ranges in a single query
- **Explicit SQL requirements**: When you need the time filter visible in the SQL for documentation
- **Custom time ranges**: When you need time ranges different from the dashboard picker

### Available Time Macros

**`$__timeFilter(columnName)`** - Adds a time range condition:
```sql
SELECT time, msg
FROM log_entries
WHERE $__timeFilter(time)
```

Expands to:
```sql
WHERE time >= '2025-10-30 10:00:00' AND time <= '2025-10-30 11:00:00'
```

**`$__timeFrom()`** - Start of time range:
```sql
SELECT COUNT(*) FROM log_entries
WHERE time >= $__timeFrom()
```

**`$__timeTo()`** - End of time range:
```sql
SELECT COUNT(*) FROM log_entries
WHERE time <= $__timeTo()
```

### Checkbox vs. Macros Comparison

| Feature | Time Filter Checkbox | Manual Macros |
|---------|---------------------|---------------|
| Ease of use | ✅ Simple, automatic | ❌ Requires SQL knowledge |
| Default behavior | ✅ Enabled by default | ❌ Must write manually |
| SQL visibility | ❌ Hidden in metadata | ✅ Visible in SQL |
| Multiple time ranges | ❌ Single range | ✅ Multiple ranges possible |
| **Recommended for** | **Most users** | **Advanced users** |

## Next Steps

- [Schema Reference](../query-guide/schema-reference.md) - Available tables and columns
- [Query Patterns](../query-guide/query-patterns.md) - More query examples
- [Functions Reference](../query-guide/functions-reference.md) - SQL functions
