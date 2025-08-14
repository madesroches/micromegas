# How to Query Micromegas Data

## Overview

Micromegas provides a powerful SQL interface for querying observability data including logs, metrics, spans, and traces. **Micromegas SQL is an extension of [Apache DataFusion SQL](https://datafusion.apache.org/user-guide/sql/)** - you can use all standard DataFusion SQL features plus Micromegas-specific functions and views optimized for observability workloads.

## Quick Start

### Python API

```python
import datetime
import micromegas

# Connect to Micromegas analytics service
client = micromegas.connect()

# Set up time range for queries
now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(hours=1)
end = now

# Query recent log entries
# Returns: pandas DataFrame with columns matching the SELECT fields
sql = """
    SELECT time, process_id, level, target, msg
    FROM log_entries
    WHERE level <= 4
    ORDER BY time DESC
    LIMIT 10;
"""
logs = client.query(sql, begin, end)  # Returns pandas.DataFrame
print(logs)
print(f"Result type: {type(logs)}")  # <class 'pandas.core.frame.DataFrame'>

# Query logs from specific process using view instance
sql = """
    SELECT time, level, target, msg
    FROM view_instance('log_entries', '{process_id}')
    WHERE level <= 3
    ORDER BY time DESC
    LIMIT 20;
""".format(process_id="your_process_id")
process_logs = client.query(sql, begin, end)  # Returns pandas.DataFrame
print(process_logs)
```

### Query without time range (uses all available data)

⚠️ **Performance Warning**: Queries without time ranges scan all available data, which can cause:
- **Long query times** - Processing months or years of data
- **High memory usage** - Query engine loads large datasets into memory
- **Potential instability** - Memory exhaustion may cause query failures or system instability

Always use time ranges for production queries. Only omit time ranges for:
- Small datasets during development
- System metadata queries (processes, streams)
- Quick counts with LIMIT clauses

```python
# Count total log entries (use with caution on large datasets)
rows = client.query("SELECT COUNT(*) FROM log_entries;")
print(rows)

# Get process information (safe - typically small dataset)
processes = client.query("SELECT process_id, exe FROM processes LIMIT 10;")
print(processes)
```

### Return Types

All queries return **[pandas DataFrames](https://pandas.pydata.org/docs/reference/api/pandas.DataFrame.html)**, making it easy to work with the results using the pandas ecosystem:

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

For more information on working with DataFrames, see the [pandas documentation](https://pandas.pydata.org/docs/).

### Query Streaming

For large result sets, Micromegas supports query streaming to handle data efficiently:

```python
import datetime
import micromegas
import pyarrow as pa

client = micromegas.connect()

# Set up time range for large dataset
now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(days=7)  # Week of data
end = now

# Stream query results to process large datasets
sql = """
    SELECT time, process_id, level, target, msg
    FROM log_entries
    WHERE time >= NOW() - INTERVAL '7 days'
    ORDER BY time DESC;
"""

# Use query_stream() to get record batches
for record_batch in client.query_stream(sql, begin, end):
    # record_batch is a pyarrow.RecordBatch
    print(f"RecordBatch schema: {record_batch.schema}")
    print(f"Number of rows: {record_batch.num_rows}")
    print(f"Number of columns: {record_batch.num_columns}")
    
    # Convert RecordBatch to pandas DataFrame for processing
    df = record_batch.to_pandas()
    print(f"Processing DataFrame with {len(df)} rows")
    
    # Process the DataFrame (e.g., filter, aggregate, save to file)
    error_logs = df[df['level'] <= 3]
    if not error_logs.empty:
        print(f"Found {len(error_logs)} error logs in this batch")
        # Process error logs...
    
    # Memory is freed after each batch is processed
```

**Record Batch Data Type:**
- Streaming returns [pyarrow.RecordBatch](https://arrow.apache.org/docs/python/generated/pyarrow.RecordBatch.html) objects
- RecordBatch is Apache Arrow's columnar data structure for efficient data transfer
- Use `record_batch.to_pandas()` to convert to pandas DataFrame for analysis
- RecordBatch provides schema information and efficient memory layout
- **Conversion to pandas is zero-copy** - uses the same underlying buffers for maximum efficiency

**FlightSQL Benefits:**
- Query streaming leverages Apache Arrow FlightSQL for high-performance data transfer
- Columnar data format makes transfers **orders of magnitude more efficient** than JSON
- Binary protocol eliminates serialization/deserialization overhead
- Native compression and vectorized operations for optimal throughput

**Accessing record batches:**
- Use `client.query_stream(sql, begin, end)` instead of `client.query()`
- Returns an iterator of pyarrow RecordBatch objects
- Each batch contains a subset of the total results
- Convert to pandas with `.to_pandas()` method when needed
- Process each batch individually to keep memory usage low

**Benefits of streaming:**
- Start processing results before server completes the full query
- Reduced perceived latency for large datasets
- Better resource utilization on both client and server

**Memory considerations:**
- Results are processed in chunks, allowing datasets larger than available RAM
- Each chunk must fit in memory, but not the entire result set
- For extremely large datasets, consider using time-based partitioning or LIMIT clauses

### Grafana Plugin

The same SQL capabilities are available through the [Grafana plugin](https://github.com/madesroches/micromegas-grafana) for creating dashboards and visualizations. Simply use the Micromegas data source and write SQL queries in the query editor.

## Table of Contents

### [Schema Reference](#schema-reference)
- [Views](#views)
- [Data Types](#data-types)
- [View Relationships](#view-relationships)

### [Functions Reference](#functions-reference)
- [Observability Functions](#observability-functions)
- [Time-based Functions](#time-based-functions)
- [Aggregation Functions](#aggregation-functions)
- [DataFusion Functions](#datafusion-functions)

### [Query Patterns](#query-patterns)
- [Common Queries](#common-queries)
- [Performance Tips](#performance-tips)
- [JOIN Strategies](#join-strategies)
- [Troubleshooting](#troubleshooting)

### [Advanced Features](#advanced-features)
- [View Materialization](#view-materialization)
- [Custom Views](#custom-views)

---

## Schema Reference

### Views

Micromegas organizes telemetry data into several views that can be queried using SQL:

#### `processes`
Contains metadata about processes that have sent telemetry data.

| Field | Type | Description |
|-------|------|-------------|
| `process_id` | `Dictionary(Int16, Utf8)` | Unique identifier for the process |
| `exe` | `Dictionary(Int16, Utf8)` | Executable name |
| `username` | `Dictionary(Int16, Utf8)` | User who ran the process |
| `realname` | `Dictionary(Int16, Utf8)` | Real name of the user |
| `computer` | `Dictionary(Int16, Utf8)` | Computer/hostname |
| `distro` | `Dictionary(Int16, Utf8)` | Operating system distribution |
| `cpu_brand` | `Dictionary(Int16, Utf8)` | CPU brand information |
| `tsc_frequency` | `UInt64` | Time stamp counter frequency |
| `start_time` | `Timestamp(Nanosecond)` | Process start time |
| `start_ticks` | `UInt64` | Process start time in ticks |
| `insert_time` | `Timestamp(Nanosecond)` | When the process data was first inserted |
| `parent_process_id` | `Dictionary(Int16, Utf8)` | Parent process identifier |
| `properties` | `Map` | Additional process metadata |
| `last_update_time` | `Timestamp(Nanosecond)` | When the process data was last updated |

#### `streams`
Contains information about data streams within processes.

| Field | Type | Description |
|-------|------|-------------|
| `stream_id` | `Dictionary(Int16, Utf8)` | Unique identifier for the stream |
| `process_id` | `Dictionary(Int16, Utf8)` | Reference to the parent process |
| `dependencies_metadata` | Various | Stream dependency metadata |
| `objects_metadata` | Various | Stream object metadata |
| `tags` | Various | Stream tags |
| `properties` | Various | Stream properties |
| `insert_time` | `Timestamp(Nanosecond)` | When the stream data was first inserted |
| `last_update_time` | `Timestamp(Nanosecond)` | When the stream data was last updated |

#### `async_events`
Asynchronous span events for tracking async operations.

| Field | Type | Description |
|-------|------|-------------|
| `stream_id` | `Dictionary(Int16, Utf8)` | Thread stream identifier |
| `block_id` | `Dictionary(Int16, Utf8)` | Block identifier |
| `time` | `Timestamp(Nanosecond)` | Event timestamp |
| `event_type` | `Dictionary(Int16, Utf8)` | "begin" or "end" |
| `span_id` | `Int64` | Async span identifier |
| `parent_span_id` | `Int64` | Parent span identifier |
| `name` | `Dictionary(Int16, Utf8)` | Span name (function) |
| `filename` | `Dictionary(Int16, Utf8)` | Source file |
| `target` | `Dictionary(Int16, Utf8)` | Module/target |
| `line` | `UInt32` | Line number |

#### `thread_spans` 
Derived view for analyzing span durations and hierarchies (accessed via `view_instance('thread_spans', stream_id)`).

| Field | Type | Description |
|-------|------|-------------|
| `id` | `Int64` | Span identifier |
| `parent` | `Int64` | Parent span identifier |
| `depth` | `UInt32` | Nesting depth in call tree |
| `hash` | `UInt32` | Span hash for deduplication |
| `begin` | `Timestamp(Nanosecond)` | Span start time |
| `end` | `Timestamp(Nanosecond)` | Span end time |
| `duration` | `Int64` | Span duration in nanoseconds |
| `name` | `Dictionary(Int16, Utf8)` | Span name (function) |
| `target` | `Dictionary(Int16, Utf8)` | Module/target |
| `filename` | `Dictionary(Int16, Utf8)` | Source file |
| `line` | `UInt32` | Line number |

#### `measures` (metrics)
Numerical measurements and counters.

| Field | Type | Description |
|-------|------|-------------|
| `process_id` | `Dictionary(Int16, Utf8)` | Process identifier |
| `stream_id` | `Dictionary(Int16, Utf8)` | Stream identifier |
| `block_id` | `Dictionary(Int16, Utf8)` | Block identifier |
| `insert_time` | `Timestamp(Nanosecond)` | Block insertion time |
| `exe` | `Dictionary(Int16, Utf8)` | Executable name |
| `username` | `Dictionary(Int16, Utf8)` | User who ran the process |
| `computer` | `Dictionary(Int16, Utf8)` | Computer/hostname |
| `time` | `Timestamp(Nanosecond)` | Measurement timestamp |
| `target` | `Dictionary(Int16, Utf8)` | Module/target |
| `name` | `Dictionary(Int16, Utf8)` | Metric name |
| `unit` | `Dictionary(Int16, Utf8)` | Measurement unit |
| `value` | `Float64` | Metric value |
| `properties` | `List<Struct>` | Metric-specific properties |
| `process_properties` | `List<Struct>` | Process-specific properties |

#### `log_entries`
Text-based log entries with levels and structured data.

| Field | Type | Description |
|-------|------|-------------|
| `process_id` | `Dictionary(Int16, Utf8)` | Process identifier |
| `stream_id` | `Dictionary(Int16, Utf8)` | Stream identifier |
| `block_id` | `Dictionary(Int16, Utf8)` | Block identifier |
| `insert_time` | `Timestamp(Nanosecond)` | Block insertion time |
| `exe` | `Dictionary(Int16, Utf8)` | Executable name |
| `username` | `Dictionary(Int16, Utf8)` | User who ran the process |
| `computer` | `Dictionary(Int16, Utf8)` | Computer/hostname |
| `time` | `Timestamp(Nanosecond)` | Log entry timestamp |
| `target` | `Dictionary(Int16, Utf8)` | Module/target |
| `level` | `Int32` | Log level (lower = more severe) |
| `msg` | `Utf8` | Log message |
| `properties` | `List<Struct>` | Log-specific properties |
| `process_properties` | `List<Struct>` | Process-specific properties |

### Data Types

Micromegas uses custom structured data types for observability-specific data:

#### Properties
Key-value pairs stored as `List<Struct>` with the following structure:
```sql
-- Properties structure
List<Struct<
    key: Utf8,
    value: Utf8
>>
```

**Common properties fields:**
- `properties` - Event-specific metadata (log properties, metric properties)
- `process_properties` - Process-wide metadata shared across all events from a process

**Querying properties:**
```sql
-- Access property values (functions may vary by implementation)
SELECT property_get("process_properties", 'thread-name') as thread_name
FROM log_entries
```

#### Histograms
Statistical distributions stored as structured data for performance metrics:
```sql
-- Histogram structure (implementation-specific)
Struct<
    min: Float64,
    max: Float64,
    buckets: List<Struct<
        upper_bound: Float64,
        count: Int64
    >>
>
```

### View Relationships

Views can be joined to get complete information:

```sql
-- Join streams with processes to get process info
SELECT ae.*, p.exe, p.username, p.computer 
FROM view_instance('async_events', 'process_id') ae
JOIN streams s ON ae.stream_id = s.stream_id  
JOIN processes p ON s.process_id = p.process_id
```

---

## Functions Reference

### Table Functions

#### `view_instance`

Creates a view instance for a specific process or stream.

```sql
view_instance(view_name, identifier)
```

**Arguments:**
- `view_name`: `Utf8` - Name of the view ('async_events', 'log_entries', 'measures', 'thread_spans', etc.)
- `identifier`: `Utf8` - Process ID (for most views) or Stream ID (for thread_spans)

**Returns:** Schema depends on the view type (see Views section)

**Example:**
```sql
SELECT * FROM view_instance('async_events', 'my_process_123')
WHERE time >= NOW() - INTERVAL '1 hour'
```

#### `list_partitions`

Lists available partitions in the data lake.

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
| `file_schema_hash` | `Binary` | Schema hash |
| `source_data_hash` | `Binary` | Source data hash |

#### `retire_partitions`

Administrative function for retiring old partitions.

```sql
SELECT * FROM retire_partitions()
```

**Returns:**
| Column | Type | Description |
|--------|------|-------------|
| `time` | `Timestamp(Nanosecond)` | Log entry timestamp |
| `msg` | `Utf8` | Log message |

#### `materialize_partitions`

Administrative function for materializing view partitions.

```sql
SELECT * FROM materialize_partitions()
```

**Returns:**
| Column | Type | Description |
|--------|------|-------------|
| `time` | `Timestamp(Nanosecond)` | Log entry timestamp |
| `msg` | `Utf8` | Log message |

### Scalar Functions

#### `property_get`

Extracts a value from a properties structure.

```sql
property_get(properties_column, 'key')
```

**Arguments:**
- `properties_column`: `List<Struct>` - Column containing properties (e.g., `properties`, `process_properties`)
- `key`: `Utf8` - String key to extract

**Returns:** `Utf8` - The value associated with the key, or NULL if not found

**Example:**
```sql
SELECT property_get(process_properties, 'thread-name') as thread_name
FROM log_entries
```

#### `get_payload`

Retrieves payload data from storage (async function).

```sql
get_payload(payload_reference)
```

**Arguments:**
- `payload_reference`: `Utf8` - Reference to payload data in storage

**Returns:** `Binary` - The payload data

### Histogram Functions

#### `quantile_from_histogram`

Calculates quantiles from histogram data.

```sql
quantile_from_histogram(histogram, quantile)
```

**Arguments:**
- `histogram`: `Struct` - Histogram data structure
- `quantile`: `Float64` - Quantile value between 0.0 and 1.0

**Returns:** `Float64` - The calculated quantile value

**Example:**
```sql
SELECT quantile_from_histogram(duration_histogram, 0.95) as p95_duration
FROM performance_metrics
```

#### `variance_from_histogram`

Calculates variance from histogram data.

```sql
variance_from_histogram(histogram)
```

**Arguments:**
- `histogram`: `Struct` - Histogram data structure

**Returns:** `Float64` - The calculated variance

#### `count_from_histogram`

Extracts total count from histogram data.

```sql
count_from_histogram(histogram)
```

**Arguments:**
- `histogram`: `Struct` - Histogram data structure

**Returns:** `Int64` - The total count of observations

#### `sum_from_histogram`

Calculates sum from histogram data.

```sql
sum_from_histogram(histogram)
```

**Arguments:**
- `histogram`: `Struct` - Histogram data structure

**Returns:** `Float64` - The sum of all observations

### Aggregate Functions

#### `make_histogram`

Creates histograms from numeric data.

```sql
make_histogram(start, end, nb_bins, values)
```

**Arguments:**
- `start`: `Float64` - Start value of histogram range
- `end`: `Float64` - End value of histogram range  
- `nb_bins`: `Int64` - Number of histogram bins
- `values`: `Float64` - Numeric values to create histogram from

**Returns:** `Struct` - Histogram data structure

**Example:**
```sql
SELECT target, make_histogram(0.0, 1000000.0, 100, duration) as duration_histogram
FROM view_instance('thread_spans', 'stream_123')
GROUP BY target
```

#### `sum_histograms`

Aggregates multiple histograms into a single histogram.

```sql
sum_histograms(histogram_column)
```

**Arguments:**
- `histogram_column`: `Struct` - Column containing histogram data structures

**Returns:** `Struct` - Combined histogram data structure

**Example:**
```sql
SELECT sum_histograms(duration_histogram) as combined_histogram
FROM performance_data
GROUP BY service_name
```

### JSON Functions

Micromegas provides JSON support through integration with the [jsonb crate](https://docs.rs/jsonb/latest/jsonb/), offering efficient binary JSON storage and manipulation.

#### `jsonb_parse`

Parses JSON strings into JSONB format.

```sql
jsonb_parse(json_string)
```

**Arguments:**
- `json_string`: `Utf8` - JSON string to parse

**Returns:** `Binary` - JSONB binary data

#### `jsonb_format_json`

Formats JSONB data as JSON string.

```sql
jsonb_format_json(jsonb_data)
```

**Arguments:**
- `jsonb_data`: `Binary` - JSONB binary data

**Returns:** `Utf8` - Formatted JSON string

#### `jsonb_get`

Extracts values from JSONB data.

```sql
jsonb_get(jsonb_data, 'key')
```

**Arguments:**
- `jsonb_data`: `Binary` - JSONB binary data
- `key`: `Utf8` - Key to extract from JSONB

**Returns:** `Binary` - JSONB value for the specified key

#### `jsonb_as_string`

Converts JSONB value to string.

```sql
jsonb_as_string(jsonb_value)
```

**Arguments:**
- `jsonb_value`: `Binary` - JSONB value to convert

**Returns:** `Utf8` - String representation

#### `jsonb_as_f64`

Converts JSONB value to float64.

```sql
jsonb_as_f64(jsonb_value)
```

**Arguments:**
- `jsonb_value`: `Binary` - JSONB value to convert

**Returns:** `Float64` - Numeric value

#### `jsonb_as_i64`

Converts JSONB value to int64.

```sql
jsonb_as_i64(jsonb_value)
```

**Arguments:**
- `jsonb_value`: `Binary` - JSONB value to convert

**Returns:** `Int64` - Integer value

### DataFusion Functions

Micromegas supports the complete DataFusion SQL function library:

- **[Scalar Functions](https://datafusion.apache.org/user-guide/sql/scalar_functions.html)** - Math, string, date/time operations
- **[Aggregate Functions](https://datafusion.apache.org/user-guide/sql/aggregate_functions.html)** - COUNT, SUM, AVG, etc.
- **[Window Functions](https://datafusion.apache.org/user-guide/sql/window_functions.html)** - ROW_NUMBER, RANK, LAG/LEAD
- **[Array Functions](https://datafusion.apache.org/user-guide/sql/scalar_functions.html#array-functions)** - Array manipulation

---

## Query Patterns

### Common Queries

#### Basic Log Analysis
```python
# Get recent error logs
sql = """
    SELECT time, process_id, level, target, msg
    FROM log_entries
    WHERE level <= 3
    AND time >= NOW() - INTERVAL '1 hour'
    ORDER BY time DESC
    LIMIT 50;
"""
errors = client.query(sql, begin, end)
print(errors)
```

#### Log Filtering by Application
```python
# Find logs from specific executable
sql = """
    SELECT time, level, target, msg
    FROM log_entries
    WHERE exe LIKE '%analytics%'
    ORDER BY time DESC
    LIMIT 20;
"""
app_logs = client.query(sql, begin, end)
```

#### Process Discovery
```python
# Get processes and their basic info
sql = """
    SELECT process_id, exe, start_time
    FROM processes
    ORDER BY start_time DESC
    LIMIT 10;
"""
processes = client.query(sql)
print(processes)
```

#### Advanced: Finding Slow Operations
```sql
-- Find streams with CPU profiling data first
SELECT stream_id, property_get("streams.properties", 'thread-name') as thread_name
FROM blocks
WHERE array_has("streams.tags", 'cpu')
GROUP BY stream_id, thread_name
LIMIT 5;
```

```python
# Then query spans for specific stream
sql = """
    SELECT target, name, duration, begin, end
    FROM view_instance('thread_spans', '{stream_id}')
    ORDER BY duration DESC
    LIMIT 10;
""".format(stream_id=stream_id)
spans = client.query(sql, begin, end)
```

#### Advanced: Async Event Analysis
```python
# Query async events for specific process
sql = """
    SELECT stream_id, time, event_type, span_id, name, target
    FROM view_instance('async_events', '{process_id}')
    ORDER BY time
    LIMIT 10;
""".format(process_id=process_id)
events = client.query(sql, begin, end)
```

### Performance Tips

1. **Use time filters early**: Always filter on time ranges to limit data scanned
2. **Leverage dictionary compression**: String comparisons are efficient due to dictionary encoding
3. **Use view_instance()**: Scope queries to specific processes when possible
4. **Consider materialized views**: For frequently-run queries, use pre-materialized views

### JOIN Strategies

#### Basic Log Joins
```python
# Get logs with process information (simple join)
sql = """
    SELECT l.time, l.level, l.target, l.msg, p.exe
    FROM log_entries l
    JOIN processes p ON l.process_id = p.process_id
    WHERE l.level <= 3
    ORDER BY l.time DESC
    LIMIT 20;
"""
logs_with_process = client.query(sql, begin, end)
```

#### Advanced: Process Context Joins
```python
# Get async events with process information
sql = """
    SELECT ae.time, ae.name, ae.event_type, p.exe, p.process_id
    FROM view_instance('async_events', '{process_id}') ae
    JOIN processes p ON ae.process_id = p.process_id
    ORDER BY ae.time
    LIMIT 10;
""".format(process_id=process_id)
events_with_process = client.query(sql, begin, end)
```

#### Advanced: Stream Relationships
```python
# Join blocks data with stream properties
sql = """
    SELECT stream_id, 
           property_get("streams.properties", 'thread-name') as thread_name,
           property_get("streams.properties", 'thread-id') as thread_id,
           nb_objects
    FROM blocks
    WHERE process_id = '{process_id}'
    AND array_has("streams.tags", 'cpu')
    GROUP BY stream_id, thread_name, thread_id, nb_objects
    ORDER BY nb_objects DESC;
""".format(process_id=process_id)
stream_info = client.query(sql)
```

### Troubleshooting

#### Query Performance Issues
- Check if time filters are applied early in the query
- Verify view_instance() is used with appropriate process scoping
- Consider if the query can benefit from materialized views

#### Data Not Found
- Verify the process_id exists in the processes table
- Check time ranges match when data was actually collected
- Ensure view names are correct ('thread_events', 'async_events', etc.)

---

## Advanced Features

### View Materialization

Micromegas uses two types of view materialization:

#### Global Views
Maintained by the maintenance service for commonly accessed data patterns. These are automatically refreshed and provide fast access to frequently queried aggregations.

#### JIT View Instances
Created on-demand when you call `view_instance()`. These are materialized just-in-time for your specific query and cached temporarily for performance.

#### Using Materialized Views
```python
# This creates a JIT materialized view for the specified process
import datetime

# Set up time range
now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(days=1)
end = now

# Query view instance with time range
sql = """
    SELECT stream_id, time, event_type, span_id, name
    FROM view_instance('async_events', '{process_id}')
    WHERE time BETWEEN '{begin}' AND '{end}'
    ORDER BY time
    LIMIT 100;
""".format(
    process_id=process_id,
    begin=begin.isoformat(),
    end=end.isoformat()
)
events = client.query(sql, begin, end)
```

### Custom Views

Advanced users can create custom views by extending the view factory system. This requires Rust development and is documented in the contributor guide.

---

## Getting Help

- **DataFusion SQL Reference**: https://datafusion.apache.org/user-guide/sql/
- **Micromegas Documentation**: See `doc/` directory
- **Issues and Support**: GitHub Issues

For questions about specific Micromegas SQL extensions or observability use cases, please refer to the project's issue tracker or documentation.
