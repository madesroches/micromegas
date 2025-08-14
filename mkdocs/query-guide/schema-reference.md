# Schema Reference

This page provides a complete reference to all views, data types, and field definitions available in Micromegas SQL queries.

## Views Overview

Micromegas organizes telemetry data into several views that can be queried using SQL:

| View | Description | Use Cases |
|------|-------------|-----------|
| [`processes`](#processes) | Process metadata and system information | System overview, process tracking |
| [`streams`](#streams) | Data stream information within processes | Stream debugging, data flow analysis |
| [`blocks`](#blocks) | Core telemetry block metadata | Low-level data inspection |
| [`log_entries`](#log-entries) | Application log messages with levels | Error tracking, debugging, monitoring |
| [`measures`](#measures) | Numeric metrics and performance data | Performance monitoring, alerting |
| [`thread_spans`](#thread-spans) | Synchronous execution spans and timing | Performance profiling, call tracing |
| [`async_events`](#async-events) | Asynchronous event lifecycle tracking | Async operation monitoring |

## Core Views

### `processes`

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

**Example Queries:**
```sql
-- Get all processes from the last day
SELECT process_id, exe, computer, start_time
FROM processes
WHERE start_time >= NOW() - INTERVAL '1 day'
ORDER BY start_time DESC;

-- Find processes by executable name
SELECT process_id, exe, username, computer
FROM processes
WHERE exe LIKE '%analytics%';
```

### `streams`

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

**Example Queries:**
```sql
-- Get streams for a specific process
SELECT stream_id, tags, properties
FROM streams
WHERE process_id = 'my_process_123';

-- Join streams with process information
SELECT s.stream_id, s.tags, p.exe, p.computer
FROM streams s
JOIN processes p ON s.process_id = p.process_id;
```

### `blocks`

Core table containing telemetry block metadata with joined process and stream information.

| Field | Type | Description |
|-------|------|-------------|
| `block_id` | `Utf8` | Unique identifier for the block |
| `stream_id` | `Utf8` | Stream identifier |
| `process_id` | `Utf8` | Process identifier |
| `begin_time` | `Timestamp(Nanosecond)` | Block start time |
| `begin_ticks` | `Int64` | Block start time in ticks |
| `end_time` | `Timestamp(Nanosecond)` | Block end time |
| `end_ticks` | `Int64` | Block end time in ticks |
| `nb_objects` | `Int32` | Number of objects in block |
| `object_offset` | `Int64` | Offset to objects in storage |
| `payload_size` | `Int64` | Size of block payload |
| `insert_time` | `Timestamp(Nanosecond)` | When block was inserted |

**Joined Process Fields:**
| Field | Type | Description |
|-------|------|-------------|
| `processes.start_time` | `Timestamp(Nanosecond)` | Process start time |
| `processes.start_ticks` | `Int64` | Process start ticks |
| `processes.tsc_frequency` | `Int64` | Time stamp counter frequency |
| `processes.exe` | `Utf8` | Executable name |
| `processes.username` | `Utf8` | User who ran the process |
| `processes.realname` | `Utf8` | Real name of the user |
| `processes.computer` | `Utf8` | Computer/hostname |
| `processes.distro` | `Utf8` | Operating system distribution |
| `processes.cpu_brand` | `Utf8` | CPU brand information |

**Example Queries:**
```sql
-- Analyze block sizes and object counts
SELECT 
    process_id,
    AVG(payload_size) as avg_block_size,
    AVG(nb_objects) as avg_objects_per_block,
    COUNT(*) as total_blocks
FROM blocks
WHERE insert_time >= NOW() - INTERVAL '1 hour'
GROUP BY process_id;
```

## Observability Data Views

### `log_entries`

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
| `level` | `Int32` | Log level (see [Log Levels](#log-levels)) |
| `msg` | `Utf8` | Log message |
| `properties` | `List<Struct>` | Log-specific properties |
| `process_properties` | `List<Struct>` | Process-specific properties |

#### Log Levels

Micromegas uses numeric log levels for efficient filtering:

| Level | Name    | Description |
|-------|---------|-------------|
| 1     | Fatal   | Critical errors that cause application termination |
| 2     | Error   | Errors that don't stop execution but need attention |
| 3     | Warn    | Warning conditions that might cause problems |
| 4     | Info    | Informational messages about normal operation |
| 5     | Debug   | Detailed information for debugging |
| 6     | Trace   | Very detailed tracing information |

**Example Queries:**
```sql
-- Get recent error and warning logs
SELECT time, process_id, level, target, msg
FROM log_entries
WHERE level <= 3  -- Fatal, Error, Warn
  AND time >= NOW() - INTERVAL '1 hour'
ORDER BY time DESC;

-- Count logs by level for a specific process
SELECT level, COUNT(*) as count
FROM view_instance('log_entries', 'my_process_123')
WHERE time >= NOW() - INTERVAL '1 day'
GROUP BY level
ORDER BY level;
```

### `measures`

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

**Example Queries:**
```sql
-- Get CPU metrics over time
SELECT time, value, unit
FROM measures
WHERE name = 'cpu_usage'
  AND time >= NOW() - INTERVAL '1 hour'
ORDER BY time;

-- Aggregate memory usage by process
SELECT 
    process_id,
    AVG(value) as avg_memory,
    MAX(value) as peak_memory,
    unit
FROM measures
WHERE name LIKE '%memory%'
  AND time >= NOW() - INTERVAL '1 hour'
GROUP BY process_id, unit;
```

### `thread_spans`

Derived view for analyzing span durations and hierarchies. Access via `view_instance('thread_spans', stream_id)`.

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

**Example Queries:**
```sql
-- Get slowest functions in a stream
SELECT name, AVG(duration) as avg_duration_ns, COUNT(*) as call_count
FROM view_instance('thread_spans', 'stream_123')
WHERE duration > 1000000  -- > 1ms
GROUP BY name
ORDER BY avg_duration_ns DESC
LIMIT 10;

-- Analyze call hierarchy
SELECT depth, name, duration
FROM view_instance('thread_spans', 'stream_123')
WHERE parent = 42  -- specific parent span
ORDER BY begin;
```

### `async_events`

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

**Example Queries:**
```sql
-- Find async operations that took longest
SELECT 
    span_id,
    name,
    MAX(time) - MIN(time) as duration_ns
FROM view_instance('async_events', 'my_process_123')
GROUP BY span_id, name
HAVING COUNT(*) = 2  -- Both begin and end events
ORDER BY duration_ns DESC
LIMIT 10;

-- Track async operation lifecycle
SELECT time, event_type, name, span_id, parent_span_id
FROM view_instance('async_events', 'my_process_123')
WHERE span_id = 12345
ORDER BY time;
```

## Data Types

### Properties

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
-- Access property values using property_get function
SELECT property_get(process_properties, 'thread-name') as thread_name
FROM log_entries
WHERE property_get(process_properties, 'thread-name') IS NOT NULL;
```

### Dictionary Compression

Most string fields use dictionary compression (`Dictionary(Int16, Utf8)`) for storage efficiency:

- Reduces storage space for repeated values
- Improves query performance
- Transparent to SQL queries - use as normal strings

### Timestamps

All time fields use `Timestamp(Nanosecond)` precision:

- Nanosecond resolution for high-precision timing
- UTC timezone assumed
- Compatible with standard SQL time functions

## View Relationships

Views can be joined to combine information:

```sql
-- Join log entries with process information
SELECT l.time, l.level, l.msg, p.exe, p.computer
FROM log_entries l
JOIN processes p ON l.process_id = p.process_id
WHERE l.level <= 2;  -- Fatal and Error only

-- Join measures with stream information
SELECT m.time, m.name, m.value, s.tags
FROM measures m
JOIN streams s ON m.stream_id = s.stream_id
WHERE m.name = 'cpu_usage';
```

## Performance Considerations

### Dictionary Fields

Dictionary-compressed fields are optimized for:
- Equality comparisons (`field = 'value'`)
- IN clauses (`field IN ('val1', 'val2')`)
- LIKE patterns on repeated values

### Time-based Queries

Always use time ranges for optimal performance:
```sql
-- Good - uses time index
WHERE time >= NOW() - INTERVAL '1 hour'

-- Avoid - full table scan
WHERE level <= 3
```

### View Instances

Use `view_instance()` for process-specific queries:
```sql
-- Better performance for single process
SELECT * FROM view_instance('log_entries', 'process_123')

-- Less efficient for single process
SELECT * FROM log_entries WHERE process_id = 'process_123'
```

## Next Steps

- **[Functions Reference](functions-reference.md)** - SQL functions available for queries
- **[Query Patterns](query-patterns.md)** - Common observability query patterns
- **[Performance Guide](performance.md)** - Optimize your queries for best performance
