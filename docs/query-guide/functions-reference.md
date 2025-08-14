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

Micromegas supports all standard DataFusion SQL functions. Here are the most commonly used categories:

### Time Functions

#### `NOW()`
Returns the current timestamp.

```sql
SELECT NOW() as current_time;
```

#### `date_trunc(precision, timestamp)`
Truncates timestamp to specified precision.

**Precisions:** `year`, `month`, `day`, `hour`, `minute`, `second`

```sql
-- Group by hour
SELECT date_trunc('hour', time) as hour, COUNT(*) as log_count
FROM log_entries
GROUP BY date_trunc('hour', time);
```

#### `INTERVAL`
Creates time intervals for date arithmetic.

```sql
-- Last 24 hours
WHERE time >= NOW() - INTERVAL '24 hours'

-- Last week
WHERE time >= NOW() - INTERVAL '7 days'

-- Custom intervals
WHERE time >= NOW() - INTERVAL '30 minutes'
```

### Aggregation Functions

#### `COUNT(*)`
Counts all rows.

```sql
SELECT COUNT(*) as total_logs FROM log_entries;
```

#### `COUNT(column)`
Counts non-null values in a column.

```sql
SELECT COUNT(msg) as non_null_messages FROM log_entries;
```

#### `SUM(column)`
Sums numeric values.

```sql
SELECT SUM(value) as total_memory FROM measures WHERE name = 'memory_usage';
```

#### `AVG(column)`
Calculates average of numeric values.

```sql
SELECT AVG(duration) as avg_duration FROM view_instance('thread_spans', 'process_123');
```

#### `MIN(column)` / `MAX(column)`
Finds minimum and maximum values.

```sql
SELECT MIN(time) as earliest, MAX(time) as latest FROM log_entries;
```

#### `STDDEV(column)`
Calculates standard deviation.

```sql
SELECT STDDEV(value) as memory_variance FROM measures WHERE name = 'memory_usage';
```

### String Functions

#### `LIKE` / `ILIKE`
Pattern matching (ILIKE is case-insensitive).

```sql
-- Case sensitive
SELECT * FROM log_entries WHERE msg LIKE '%error%';

-- Case insensitive
SELECT * FROM log_entries WHERE msg ILIKE '%ERROR%';
```

#### `REGEXP_MATCH(string, pattern)`
Regular expression matching.

```sql
SELECT * FROM log_entries 
WHERE REGEXP_MATCH(msg, '^ERROR: [0-9]+');
```

#### `LENGTH(string)`
Returns string length.

```sql
SELECT msg, LENGTH(msg) as msg_length FROM log_entries;
```

#### `SUBSTRING(string, start, length)`
Extracts substring.

```sql
SELECT SUBSTRING(msg, 1, 50) as short_msg FROM log_entries;
```

### Conditional Functions

#### `CASE WHEN`
Conditional logic.

```sql
SELECT 
    level,
    CASE 
        WHEN level <= 2 THEN 'Critical'
        WHEN level = 3 THEN 'Warning'
        ELSE 'Info'
    END as severity
FROM log_entries;
```

#### `COALESCE(value1, value2, ...)`
Returns first non-null value.

```sql
SELECT COALESCE(property_get(properties, 'thread'), 'unknown') as thread_name
FROM log_entries;
```

### Window Functions

#### `ROW_NUMBER()`
Assigns row numbers within partitions.

```sql
SELECT 
    time, msg,
    ROW_NUMBER() OVER (PARTITION BY process_id ORDER BY time) as row_num
FROM log_entries;
```

#### `RANK()` / `DENSE_RANK()`
Ranks values within partitions.

```sql
SELECT 
    name, duration,
    RANK() OVER (ORDER BY duration DESC) as performance_rank
FROM view_instance('thread_spans', 'process_123');
```

#### `LAG()` / `LEAD()`
Access previous/next row values.

```sql
SELECT 
    time, value,
    LAG(value) OVER (ORDER BY time) as previous_value
FROM measures
WHERE name = 'cpu_usage';
```

### Array Functions

#### `ARRAY_AGG(column)`
Aggregates values into an array.

```sql
SELECT process_id, ARRAY_AGG(DISTINCT target) as targets
FROM log_entries
GROUP BY process_id;
```

#### `UNNEST(array)`
Expands array into rows.

```sql
SELECT UNNEST(['error', 'warn', 'info']) as log_level;
```

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

### Performance Analysis

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

For complete documentation of all standard SQL functions, see the [Apache DataFusion SQL Reference](https://datafusion.apache.org/user-guide/sql/).

## Next Steps

- **[Query Patterns](query-patterns.md)** - Common observability query patterns
- **[Performance Guide](performance.md)** - Optimize your queries for best performance
- **[Schema Reference](schema-reference.md)** - Complete view and field reference
