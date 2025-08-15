# Functions Reference

This page provides a complete reference to all SQL functions available in Micromegas queries, including both standard DataFusion functions and Micromegas-specific extensions.

## Micromegas Extensions

### Table Functions

Table functions return tables that can be used in FROM clauses.

#### `view_instance(view_name, identifier)`

Creates a process or stream-scoped view instance for better performance.

**Syntax:**
```sql
view_instance(view_name, identifier)
```

**Parameters:**
- `view_name` (`Utf8`): Name of the view ('log_entries', 'measures', 'thread_spans', 'async_events')
- `identifier` (`Utf8`): Process ID (for most views) or Stream ID (for thread_spans)

**Returns:** Schema depends on the view type (see [Schema Reference](schema-reference.md))

**Examples:**
```sql
-- Get logs for a specific process
SELECT time, level, msg
FROM view_instance('log_entries', 'my_process_123')
WHERE level <= 3;

-- Get spans for a specific stream
SELECT name, duration
FROM view_instance('thread_spans', 'stream_456')
WHERE duration > 1000000;  -- > 1ms
```

#### `list_partitions()`

Lists available data partitions in the lakehouse.

**Syntax:**
```sql
SELECT * FROM list_partitions()
```

**Returns:**
| Column | Type | Description |
|--------|------|-------------|
| `view_set_name` | `Utf8` | Name of the view set |
| `view_instance_id` | `Utf8` | Instance identifier |
| `begin_insert_time` | `Timestamp(Nanosecond)` | Partition start time |
| `end_insert_time` | `Timestamp(Nanosecond)` | Partition end time |
| `min_event_time` | `Timestamp(Nanosecond)` | Earliest event time |
| `max_event_time` | `Timestamp(Nanosecond)` | Latest event time |
| `updated` | `Timestamp(Nanosecond)` | Last update time |
| `file_path` | `Utf8` | Partition file path |
| `file_size` | `Int64` | File size in bytes |

**Example:**
```sql
-- View partition information
SELECT view_set_name, view_instance_id, file_size
FROM list_partitions()
ORDER BY updated DESC;
```

### Scalar Functions

#### `property_get(properties, key)`

Extracts a value from a properties map.

**Syntax:**
```sql
property_get(properties, key)
```

**Parameters:**
- `properties` (`List<Struct>`): Properties map field
- `key` (`Utf8`): Property key to extract

**Returns:** `Utf8` - Property value or NULL if not found

**Examples:**
```sql
-- Get thread name from process properties
SELECT time, msg, property_get(process_properties, 'thread-name') as thread
FROM log_entries
WHERE property_get(process_properties, 'thread-name') IS NOT NULL;

-- Filter by custom property
SELECT time, name, value
FROM measures
WHERE property_get(properties, 'source') = 'system_monitor';
```

#### `make_histogram(values, bins)`

Creates histogram data from numeric values.

**Syntax:**
```sql
make_histogram(values, bins)
```

**Parameters:**
- `values` (`Float64`): Column of numeric values
- `bins` (`Int32`): Number of histogram bins

**Returns:** Histogram structure with buckets and counts

**Example:**
```sql
-- Create histogram of response times
SELECT make_histogram(duration, 20) as duration_histogram
FROM view_instance('thread_spans', 'web_server_123')
WHERE name = 'handle_request';
```

## Standard SQL Functions

Micromegas supports all standard DataFusion SQL functions including math, string, date/time, conditional, and array functions. For a complete list with examples, see the [DataFusion Scalar Functions documentation](https://datafusion.apache.org/user-guide/sql/scalar_functions.html).
## Advanced Query Patterns

### Histogram Analysis

```sql
-- Create performance histogram
SELECT make_histogram(duration / 1000000.0, 10) as response_time_ms_histogram
FROM view_instance('thread_spans', 'web_server')
WHERE name = 'handle_request'
  AND duration > 1000000;  -- > 1ms
```

### Property Extraction and Filtering

```sql
-- Find logs with specific thread names
SELECT time, level, msg, property_get(process_properties, 'thread-name') as thread
FROM log_entries
WHERE property_get(process_properties, 'thread-name') LIKE '%worker%'
ORDER BY time DESC;
```

### Time-based Aggregation

```sql
-- Hourly error counts
SELECT 
    date_trunc('hour', time) as hour,
    COUNT(*) as error_count
FROM log_entries
WHERE level <= 2  -- Fatal and Error
  AND time >= NOW() - INTERVAL '24 hours'
GROUP BY date_trunc('hour', time)
ORDER BY hour;
```

### Performance Trace Analysis

```sql
-- Top 10 slowest functions with statistics
SELECT 
    name,
    COUNT(*) as call_count,
    AVG(duration) / 1000000.0 as avg_ms,
    MAX(duration) / 1000000.0 as max_ms,
    STDDEV(duration) / 1000000.0 as stddev_ms
FROM view_instance('thread_spans', 'my_process')
WHERE duration > 100000  -- > 0.1ms
GROUP BY name
ORDER BY avg_ms DESC
LIMIT 10;
```

## DataFusion Reference

Micromegas supports all standard DataFusion SQL syntax, functions, and operators. For complete documentation including functions, operators, data types, and SQL syntax, see the [Apache DataFusion SQL Reference](https://datafusion.apache.org/user-guide/sql/).

## Next Steps

- **[Query Patterns](query-patterns.md)** - Common observability query patterns
- **[Performance Guide](performance.md)** - Optimize your queries for best performance
- **[Schema Reference](schema-reference.md)** - Complete view and field reference
