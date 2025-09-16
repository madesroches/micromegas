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

#### `list_partitions()` 🔧

**Administrative Function** - Lists available data partitions in the lakehouse.

**Syntax:**
```sql
SELECT * FROM list_partitions()
```

**Returns:**

| Column | Type | Description |
|--------|------|-------------|
| view_set_name | Utf8 | Name of the view set |
| view_instance_id | Utf8 | Instance identifier |
| begin_insert_time | Timestamp(Nanosecond) | Partition start time |
| end_insert_time | Timestamp(Nanosecond) | Partition end time |
| min_event_time | Timestamp(Nanosecond) | Earliest event time |
| max_event_time | Timestamp(Nanosecond) | Latest event time |
| updated | Timestamp(Nanosecond) | Last update time |
| file_path | Utf8 | Partition file path |
| file_size | Int64 | File size in bytes |
| file_schema_hash | Binary | Hash of the file schema |
| source_data_hash | Binary | Hash of the source data |

**Example:**
```sql
-- View partition information
SELECT view_set_name, view_instance_id, file_size
FROM list_partitions()
ORDER BY updated DESC;
```

**ℹ️ Administrative Use:** This function provides system-level partition metadata primarily useful for administrators monitoring lakehouse storage and partition management. Regular users querying data typically don't need this information.

#### `retire_partitions(view_set_name, view_instance_id, begin_insert_time, end_insert_time)` 🔧

**Administrative Function** - Retires (removes) data partitions from the lakehouse for a specified time range. Returns a log stream of the operation.

**Syntax:**
```sql
SELECT * FROM retire_partitions(view_set_name, view_instance_id, begin_insert_time, end_insert_time)
```

**Parameters:**
- `view_set_name` (`Utf8`): Name of the view set
- `view_instance_id` (`Utf8`): Instance identifier
- `begin_insert_time` (`Timestamp(Nanosecond)`): Start time for partition retirement
- `end_insert_time` (`Timestamp(Nanosecond)`): End time for partition retirement

**Returns:** Log stream table with operation progress and messages

**Example:**
```sql
-- Retire old partitions for a specific view
SELECT * FROM retire_partitions(
    'log_entries', 
    'global',
    NOW() - INTERVAL '30 days',
    NOW() - INTERVAL '7 days'
);
```

**⚠️ DESTRUCTIVE OPERATION:** This function permanently removes data partitions from the lakehouse, making the contained data inaccessible. Use only for data retention management and with extreme caution in production environments. Ensure proper backups exist before retiring partitions.

#### `materialize_partitions(view_name, begin_insert_time, end_insert_time, partition_delta_seconds)` 🔧

**Administrative Function** - Materializes data partitions for a view over a specified time range. Returns a log stream of the operation.

**Syntax:**
```sql
SELECT * FROM materialize_partitions(view_name, begin_insert_time, end_insert_time, partition_delta_seconds)
```

**Parameters:**
- `view_name` (`Utf8`): Name of the view to materialize
- `begin_insert_time` (`Timestamp(Nanosecond)`): Start time for materialization
- `end_insert_time` (`Timestamp(Nanosecond)`): End time for materialization  
- `partition_delta_seconds` (`Int64`): Partition time delta in seconds

**Returns:** Log stream table with operation progress and messages

**Example:**
```sql
-- Materialize partitions for CPU usage view
SELECT * FROM materialize_partitions(
    'cpu_usage_per_process_per_minute',
    NOW() - INTERVAL '1 day',
    NOW(),
    3600  -- 1 hour partitions
);
```

**⚠️ Administrative Use Only:** This function is intended for system administrators and data engineers managing the lakehouse infrastructure. Regular users querying data should not need to call this function. It triggers background processing to create materialized partitions and can impact system performance.

#### `list_view_sets()` 🔧

**Administrative Function** - Lists all available view sets with their current schema information. Useful for schema discovery and management.

**Syntax:**
```sql
SELECT * FROM list_view_sets()
```

**Returns:**

| Column | Type | Description |
|--------|------|-------------|
| view_set_name | Utf8 | Name of the view set (e.g., 'log_entries', 'measures') |
| current_schema_hash | Binary | Current schema version identifier |
| schema | Utf8 | Full schema as formatted string |
| has_view_maker | Boolean | Whether view set supports process-specific instances |
| global_instance_available | Boolean | Whether a global instance exists |

**Example:**
```sql
-- View all available view sets and their schemas
SELECT view_set_name, current_schema_hash, has_view_maker
FROM list_view_sets()
ORDER BY view_set_name;

-- Check schema for specific view set
SELECT schema
FROM list_view_sets()
WHERE view_set_name = 'log_entries';
```

**ℹ️ Administrative Use:** This function provides schema discovery for administrators managing view compatibility and schema evolution. It shows the current schema versions and capabilities of each view set in the lakehouse.

#### `retire_partition_by_file(file_path)` 🔧

**Administrative Function** - Retires a single partition by its exact file path. Provides targeted partition removal for schema evolution and maintenance.

**Syntax:**
```sql
SELECT retire_partition_by_file(file_path) as result
```

**Parameters:**
- `file_path` (`Utf8`): Exact file path of the partition to retire

**Returns:** `Utf8` - Result message indicating success or failure

**Example:**
```sql
-- Retire a specific partition
SELECT retire_partition_by_file('/lakehouse/log_entries/process-123/2024/01/01/partition.parquet') as result;

-- Retire multiple partitions (use with list_partitions())
SELECT retire_partition_by_file(file_path) as result
FROM list_partitions()
WHERE view_set_name = 'log_entries' 
  AND file_schema_hash != '[4]'  -- Retire old schema versions
LIMIT 10;
```

**⚠️ DESTRUCTIVE OPERATION:** This function permanently removes a single data partition from the lakehouse, making the contained data inaccessible. Unlike `retire_partitions()` which operates on time ranges, this function targets exact file paths for precise partition management. Ensure proper backups exist before retiring partitions.

**✅ Safety Note:** This function only affects the specified partition file. It cannot accidentally retire other partitions, making it safer than time-range-based retirement for schema evolution tasks.

### Scalar Functions

#### JSON/JSONB Functions

Micromegas provides functions for working with JSON data stored in binary JSONB format for efficient storage and querying.

##### `jsonb_parse(json_string)`

Parses a JSON string into binary JSONB format.

**Syntax:**
```sql
jsonb_parse(json_string)
```

**Parameters:**
- `json_string` (`Utf8`): JSON string to parse

**Returns:** `Binary` - Parsed JSONB data

**Example:**
```sql
-- Parse JSON string into JSONB
SELECT jsonb_parse('{"name": "web_server", "port": 8080}') as parsed_json
FROM processes;
```

##### `jsonb_get(jsonb, key)`

Extracts a value from a JSONB object by key name.

**Syntax:**
```sql
jsonb_get(jsonb, key)
```

**Parameters:**
- `jsonb` (`Binary`): JSONB object
- `key` (`Utf8`): Key name to extract

**Returns:** `Binary` - JSONB value or NULL if key not found

**Example:**
```sql
-- Extract name field from JSON data
SELECT jsonb_get(jsonb_parse('{"name": "web_server", "port": 8080}'), 'name') as name_value
FROM processes;
```

##### `jsonb_format_json(jsonb)`

Converts a JSONB value back to a human-readable JSON string.

**Syntax:**
```sql
jsonb_format_json(jsonb)
```

**Parameters:**
- `jsonb` (`Binary`): JSONB value to format

**Returns:** `Utf8` - JSON string representation

**Example:**
```sql
-- Format JSONB back to JSON string
SELECT jsonb_format_json(jsonb_parse('{"name": "web_server"}')) as json_string
FROM processes;
```

##### `jsonb_as_string(jsonb)`

Casts a JSONB value to a string.

**Syntax:**
```sql
jsonb_as_string(jsonb)
```

**Parameters:**
- `jsonb` (`Binary`): JSONB value to convert

**Returns:** `Utf8` - String value or NULL if not a string

**Example:**
```sql
-- Extract string value from JSONB
SELECT jsonb_as_string(jsonb_get(jsonb_parse('{"service": "web_server"}'), 'service')) as service_name
FROM processes;
```

##### `jsonb_as_f64(jsonb)`

Casts a JSONB value to a 64-bit float.

**Syntax:**
```sql
jsonb_as_f64(jsonb)
```

**Parameters:**
- `jsonb` (`Binary`): JSONB value to convert

**Returns:** `Float64` - Numeric value or NULL if not a number

**Example:**
```sql
-- Extract numeric value from JSONB
SELECT jsonb_as_f64(jsonb_get(jsonb_parse('{"cpu_usage": 75.5}'), 'cpu_usage')) as cpu_usage
FROM processes;
```

##### `jsonb_as_i64(jsonb)`

Casts a JSONB value to a 64-bit integer.

**Syntax:**
```sql
jsonb_as_i64(jsonb)
```

**Parameters:**
- `jsonb` (`Binary`): JSONB value to convert

**Returns:** `Int64` - Integer value or NULL if not an integer

**Example:**
```sql
-- Extract integer value from JSONB
SELECT jsonb_as_i64(jsonb_get(jsonb_parse('{"port": 8080}'), 'port')) as port_number
FROM processes;
```

#### Data Access Functions

##### `get_payload(process_id, stream_id, block_id)`

Retrieves the raw binary payload of a telemetry block from data lake storage.

**Syntax:**
```sql
get_payload(process_id, stream_id, block_id)
```

**Parameters:**
- `process_id` (`Utf8`): Process identifier
- `stream_id` (`Utf8`): Stream identifier  
- `block_id` (`Utf8`): Block identifier

**Returns:** `Binary` - Raw block payload data

**Example:**
```sql
-- Get raw payload data for specific blocks
SELECT process_id, stream_id, block_id, get_payload(process_id, stream_id, block_id) as payload
FROM blocks
WHERE insert_time >= NOW() - INTERVAL '1 hour'
LIMIT 10;
```

**Note:** This is an async function that fetches data from object storage. Use sparingly in queries as it can impact performance.

#### Property Functions

Micromegas provides specialized functions for working with property data, including efficient dictionary encoding for memory optimization.

##### `property_get(properties, key)`

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

##### `properties_length(properties)`

Returns the number of properties in a list, supporting both regular and dictionary-encoded formats.

**Syntax:**
```sql
properties_length(properties)
```

**Parameters:**
- `properties` (`List<Struct>` or `Dictionary<Int32, List<Struct>>`): Properties in either format

**Returns:** `Int32` - Number of properties

**Examples:**
```sql
-- Works with regular properties
SELECT properties_length(properties) as prop_count
FROM measures;

-- Works with dictionary-encoded properties
SELECT properties_length(properties_to_dict(properties)) as prop_count
FROM measures;
```

**Note:** This function transparently handles both array and dictionary representations, providing better performance than using `array_length(properties_to_array(...))` for dictionary-encoded data.

##### `properties_to_dict(properties)`

Converts a properties list to a dictionary-encoded array for memory efficiency.

**Syntax:**
```sql
properties_to_dict(properties)
```

**Parameters:**
- `properties` (`List<Struct<key: Utf8, value: Utf8>>`): Properties list to encode

**Returns:** `Dictionary<Int32, List<Struct<key: Utf8, value: Utf8>>>` - Dictionary-encoded properties

**Examples:**
```sql
-- Convert properties to dictionary encoding for memory efficiency
SELECT properties_to_dict(properties) as dict_props
FROM measures;

-- Use with other functions via properties_to_array
SELECT array_length(properties_to_array(properties_to_dict(properties))) as prop_count
FROM measures;
```

**Note:** Dictionary encoding can reduce memory usage by 50-80% for datasets with repeated property patterns.

##### `properties_to_array(dict_properties)`

Converts dictionary-encoded properties back to a regular array for compatibility with standard functions.

**Syntax:**
```sql
properties_to_array(dict_properties)
```

**Parameters:**
- `dict_properties` (`Dictionary<Int32, List<Struct>>`): Dictionary-encoded properties

**Returns:** `List<Struct<key: Utf8, value: Utf8>>` - Regular properties array

**Examples:**
```sql
-- Convert dictionary-encoded properties back to array
SELECT properties_to_array(properties_to_dict(properties)) as props
FROM measures;

-- Use with array functions
SELECT array_length(properties_to_array(properties_to_dict(properties))) as count
FROM measures;
```

#### Histogram Functions

Micromegas provides a comprehensive set of functions for creating and analyzing histograms, enabling efficient statistical analysis of large datasets.

##### `make_histogram(start, end, bins, values)`

Creates histogram data from numeric values with specified range and bin count.

**Syntax:**
```sql
make_histogram(start, end, bins, values)
```

**Parameters:**
- `start` (`Float64`): Histogram minimum value
- `end` (`Float64`): Histogram maximum value  
- `bins` (`Int64`): Number of histogram bins
- `values` (`Float64`): Column of numeric values to histogram

**Returns:** Histogram structure with buckets and counts

**Example:**
```sql
-- Create histogram of response times (0-50ms, 20 bins)
SELECT make_histogram(0.0, 50.0, 20, CAST(duration AS FLOAT64) / 1000000.0) as duration_histogram
FROM view_instance('thread_spans', 'web_server_123')
WHERE name = 'handle_request';
```

##### `sum_histograms(histogram_column)`

Aggregates multiple histograms by summing their bins.

**Syntax:**
```sql
sum_histograms(histogram_column)
```

**Parameters:**
- `histogram_column` (Histogram): Column containing histogram values

**Returns:** Combined histogram with summed bins

**Example:**
```sql
-- Combine histograms across processes
SELECT sum_histograms(duration_histogram) as combined_histogram
FROM cpu_usage_per_process_per_minute
WHERE time_bin >= NOW() - INTERVAL '1 hour';
```

##### `quantile_from_histogram(histogram, quantile)`

Estimates a quantile value from a histogram.

**Syntax:**
```sql
quantile_from_histogram(histogram, quantile)
```

**Parameters:**
- `histogram` (Histogram): Histogram to analyze
- `quantile` (`Float64`): Quantile to estimate (0.0 to 1.0)

**Returns:** `Float64` - Estimated quantile value

**Examples:**
```sql
-- Get median (50th percentile) response time
SELECT quantile_from_histogram(duration_histogram, 0.5) as median_duration
FROM performance_histograms;

-- Get 95th percentile response time
SELECT quantile_from_histogram(duration_histogram, 0.95) as p95_duration
FROM performance_histograms;
```

##### `variance_from_histogram(histogram)`

Calculates variance from histogram data.

**Syntax:**
```sql
variance_from_histogram(histogram)
```

**Parameters:**
- `histogram` (Histogram): Histogram to analyze

**Returns:** `Float64` - Variance of the histogram data

**Example:**
```sql
-- Calculate response time variance
SELECT variance_from_histogram(duration_histogram) as duration_variance
FROM performance_histograms;
```

##### `count_from_histogram(histogram)`

Extracts the total count of values from a histogram.

**Syntax:**
```sql
count_from_histogram(histogram)
```

**Parameters:**
- `histogram` (Histogram): Histogram to analyze

**Returns:** `UInt64` - Total number of values in the histogram

**Example:**
```sql
-- Get total sample count from histogram
SELECT count_from_histogram(duration_histogram) as total_samples
FROM performance_histograms;
```

##### `sum_from_histogram(histogram)`

Extracts the sum of all values from a histogram.

**Syntax:**
```sql
sum_from_histogram(histogram)
```

**Parameters:**
- `histogram` (Histogram): Histogram to analyze

**Returns:** `Float64` - Sum of all values in the histogram

**Example:**
```sql
-- Get total duration from histogram
SELECT sum_from_histogram(duration_histogram) as total_duration
FROM performance_histograms;
```

## Standard SQL Functions

Micromegas supports all standard DataFusion SQL functions including math, string, date/time, conditional, and array functions. For a complete list with examples, see the [DataFusion Scalar Functions documentation](https://datafusion.apache.org/user-guide/sql/scalar_functions.html).
## Advanced Query Patterns

### Histogram Analysis

```sql
-- Create performance histogram (0-100ms, 10 bins)
SELECT make_histogram(0.0, 100.0, 10, duration / 1000000.0) as response_time_ms_histogram
FROM view_instance('thread_spans', 'web_server')
WHERE name = 'handle_request'
  AND duration > 1000000;  -- > 1ms
```

```sql
-- Analyze histogram statistics
SELECT 
    quantile_from_histogram(response_time_histogram, 0.5) as median_ms,
    quantile_from_histogram(response_time_histogram, 0.95) as p95_ms,
    quantile_from_histogram(response_time_histogram, 0.99) as p99_ms,
    variance_from_histogram(response_time_histogram) as variance,
    count_from_histogram(response_time_histogram) as sample_count,
    sum_from_histogram(response_time_histogram) as total_time_ms
FROM performance_histograms
WHERE time_bin >= NOW() - INTERVAL '1 hour';
```

```sql
-- Aggregate histograms across multiple processes
SELECT 
    time_bin,
    sum_histograms(cpu_usage_histo) as combined_cpu_histogram,
    quantile_from_histogram(sum_histograms(cpu_usage_histo), 0.95) as p95_cpu
FROM cpu_usage_per_process_per_minute
WHERE time_bin >= NOW() - INTERVAL '1 day'
GROUP BY time_bin
ORDER BY time_bin;
```

### Property Extraction and Filtering

```sql
-- Find logs with specific thread names
SELECT time, level, msg, property_get(process_properties, 'thread-name') as thread
FROM log_entries
WHERE property_get(process_properties, 'thread-name') LIKE '%worker%'
ORDER BY time DESC;
```

### JSON Data Processing

```sql
-- Parse and extract configuration from JSON logs
SELECT 
    time,
    msg,
    jsonb_as_string(jsonb_get(jsonb_parse(msg), 'service')) as service_name,
    jsonb_as_i64(jsonb_get(jsonb_parse(msg), 'port')) as port,
    jsonb_as_f64(jsonb_get(jsonb_parse(msg), 'cpu_limit')) as cpu_limit
FROM log_entries
WHERE msg LIKE '%{%'  -- Contains JSON
  AND jsonb_parse(msg) IS NOT NULL
ORDER BY time DESC;
```

```sql
-- Aggregate metrics from JSON payloads
SELECT 
    jsonb_as_string(jsonb_get(jsonb_parse(msg), 'service')) as service,
    COUNT(*) as event_count,
    AVG(jsonb_as_f64(jsonb_get(jsonb_parse(msg), 'response_time'))) as avg_response_ms
FROM log_entries
WHERE msg LIKE '%response_time%'
  AND jsonb_parse(msg) IS NOT NULL
GROUP BY service
ORDER BY avg_response_ms DESC;
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
