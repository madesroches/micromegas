# Quick Start

Get up and running with Micromegas SQL queries in minutes. This guide shows you the essential patterns for querying your observability data.

## Basic Connection

All Micromegas queries start by connecting to the analytics service:

```python
import datetime
import micromegas

# Connect to Micromegas analytics service
client = micromegas.connect()
```

The `connect()` function automatically discovers your local Micromegas instance or connects to a configured remote endpoint.

## Your First Query

Let's query recent log entries to see what data is available:

```python
# Set up time range for queries
now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(hours=1)
end = now

# Query recent log entries
sql = """
    SELECT time, process_id, level, target, msg
    FROM log_entries
    WHERE level <= 4
    ORDER BY time DESC
    LIMIT 10;
"""

# Execute the query
logs = client.query(sql, begin, end)
print(logs)
print(f"Result type: {type(logs)}")  # pandas.DataFrame
```

**Key points:**

- **⚡ Important**: Always specify time range via API parameters (`begin`, `end`) for best performance
- Results are returned as pandas DataFrames
- `level <= 4` filters to show errors and warnings (see [log levels](#log-levels))
- Use API time parameters instead of SQL time filters for partition elimination

## Understanding Return Types

All queries return **[pandas DataFrames](https://pandas.pydata.org/docs/reference/api/pandas.DataFrame.html)**:

```python
# Query returns a pandas DataFrame
result = client.query("SELECT process_id, exe FROM processes LIMIT 5;")

# Access DataFrame properties
print(f"Shape: {result.shape}")
print(f"Columns: {result.columns.tolist()}")
print(f"Data types:\n{result.dtypes}")

# Use pandas operations
filtered = result[result['exe'].str.contains('analytics')]
print(filtered.head())
```

This makes it easy to work with results using the entire pandas ecosystem for analysis, visualization, and data processing.

## Essential Query Patterns

### 1. Process Information

Get an overview of processes sending telemetry:

```python
processes = client.query("""
    SELECT process_id, exe, computer, start_time
    FROM processes
    ORDER BY start_time DESC
    LIMIT 10;
""")
print(processes)
```

### 2. Recent Log Entries

Query logs with error filtering:

```python
error_logs = client.query("""
    SELECT time, process_id, level, target, msg
    FROM log_entries
    WHERE level <= 3  -- Fatal, Error, Warn
    ORDER BY time DESC
    LIMIT 50;
""", begin, end)
print(error_logs)
```

### 3. Performance Metrics

Query numeric measurements:

```python
metrics = client.query("""
    SELECT time, process_id, name, value, unit
    FROM measures
    WHERE name LIKE '%cpu%'
    ORDER BY time DESC
    LIMIT 20;
""", begin, end)
print(metrics)
```

### 4. Process-Specific Data

Use view instances for better performance when focusing on specific processes:

```python
process_id = "your_process_id_here"  # Replace with actual process ID

process_logs = client.query(f"""
    SELECT time, level, target, msg
    FROM view_instance('log_entries', '{process_id}')
    WHERE level <= 3
    ORDER BY time DESC
    LIMIT 20;
""", begin, end)
print(process_logs)
```

## Log Levels

Micromegas uses numeric log levels for efficient filtering:

| Level | Name    | Description |
|-------|---------|-------------|
| 1     | Fatal   | Critical errors that cause application termination |
| 2     | Error   | Errors that don't stop execution but need attention |
| 3     | Warn    | Warning conditions that might cause problems |
| 4     | Info    | Informational messages about normal operation |
| 5     | Debug   | Detailed information for debugging |
| 6     | Trace   | Very detailed tracing information |

**Common filters:**

- `level <= 2` - Only fatal and error messages
- `level <= 3` - Fatal, error, and warning messages
- `level <= 4` - All messages except debug and trace

## Time Range Best Practices

### Always Use Time Ranges

```python
# ✅ Good - efficient and memory-safe
df = client.query(sql, begin_time, end_time)

# ❌ Avoid - can be slow and memory-intensive
df = client.query(sql)  # Queries ALL data
```

### Common Time Ranges

```python
now = datetime.datetime.now(datetime.timezone.utc)

# Last hour
begin = now - datetime.timedelta(hours=1)

# Last day
begin = now - datetime.timedelta(days=1)

# Last week
begin = now - datetime.timedelta(weeks=1)

# Custom range
begin = datetime.datetime(2024, 1, 1, tzinfo=datetime.timezone.utc)
end = datetime.datetime(2024, 1, 2, tzinfo=datetime.timezone.utc)
```

## Safe Queries Without Time Ranges

Some queries are safe to run without time ranges because they operate on small metadata tables:

```python
# Process information (typically small dataset)
processes = client.query("SELECT process_id, exe FROM processes LIMIT 10;")

# Stream metadata
streams = client.query("SELECT stream_id, process_id FROM streams LIMIT 10;")

# Count queries (use with caution on large datasets)
count = client.query("SELECT COUNT(*) FROM log_entries;")
```

!!! warning "Performance Impact"
    Avoid querying `log_entries`, `measures`, `thread_spans`, or `async_events` without time ranges on production systems with large datasets.

## Next Steps

Now that you can run basic queries:

1. **[Explore the Python API](python-api.md)** - Learn about streaming and advanced features
2. **[Review the Schema](schema-reference.md)** - Understand all available fields and data types
3. **[Try Query Patterns](query-patterns.md)** - Common observability query patterns
4. **[Optimize Performance](performance.md)** - Learn to write efficient queries

## Quick Reference

### Essential Views
- `processes` - Process metadata
- `log_entries` - Application logs
- `measures` - Numeric metrics
- `thread_spans` - Execution timing
- `async_events` - Async operation tracking

### Key Functions
- `view_instance('view_name', 'process_id')` - Process-scoped views
- `property_get(properties, 'key')` - Extract property values
- `make_histogram(values, bins)` - Create histograms

### Time Functions
- `NOW()` - Current timestamp
- `INTERVAL '1 hour'` - Time duration
- `date_trunc('hour', time)` - Truncate to time boundary
