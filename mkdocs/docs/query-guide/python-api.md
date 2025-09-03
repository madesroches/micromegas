# Python API Reference

The Micromegas Python client provides a simple but powerful interface for querying observability data using SQL. This page covers all client methods, connection options, and advanced features.

## Installation

Install the Micromegas Python client from PyPI:

```bash
pip install micromegas
```

## Basic Usage

### Connection

```python
import micromegas

# Connect to local Micromegas instance
client = micromegas.connect()
```

The `connect()` function connects to the analytics service at `grpc://localhost:50051`.

### Simple Queries

```python
import datetime

# Set up time range
now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(hours=1)
end = now

# Execute query with time range
sql = "SELECT * FROM log_entries LIMIT 10;"
df = client.query(sql, begin, end)
print(df)
```

## Client Methods

### `query(sql, begin=None, end=None)`

Execute a SQL query and return results as a pandas DataFrame.

**Parameters:**

- `sql` (str): SQL query string
- `begin` (datetime or str, optional): **⚡ Recommended** - Start time for partition elimination. Can be a `datetime` object or RFC3339 string (e.g., `"2024-01-01T00:00:00Z"`)
- `end` (datetime or str, optional): **⚡ Recommended** - End time for partition elimination. Can be a `datetime` object or RFC3339 string (e.g., `"2024-01-01T23:59:59Z"`)

**Returns:**

- `pandas.DataFrame`: Query results

**Performance Note:**
Using `begin` and `end` parameters instead of SQL time filters allows the analytics server to eliminate entire partitions before query execution, providing significant performance improvements.

**Example:**
```python
# ✅ EFFICIENT: API time range enables partition elimination
df = client.query("""
    SELECT time, process_id, level, msg
    FROM log_entries
    WHERE level <= 3
    ORDER BY time DESC
    LIMIT 100;
""", begin, end)  # ⭐ Time range in API parameters

# ❌ INEFFICIENT: SQL time filter scans all partitions
df = client.query("""
    SELECT time, process_id, level, msg
    FROM log_entries
    WHERE time >= NOW() - INTERVAL '1 hour'  -- Server scans ALL partitions
      AND level <= 3
    ORDER BY time DESC
    LIMIT 100;
""")  # Missing API time parameters!

# ✅ Using RFC3339 strings for time ranges
df = client.query("""
    SELECT time, process_id, level, msg
    FROM log_entries
    WHERE level <= 3
    ORDER BY time DESC
    LIMIT 100;
""", "2024-01-01T00:00:00Z", "2024-01-01T23:59:59Z")  # ⭐ RFC3339 strings

# ✅ OK: Query without time range (for metadata queries)
processes = client.query("SELECT process_id, exe FROM processes LIMIT 10;")
```

### `query_stream(sql, begin=None, end=None)`

Execute a SQL query and return results as a stream of Apache Arrow RecordBatch objects. Use this for large datasets to avoid memory issues.

**Parameters:**

- `sql` (str): SQL query string  
- `begin` (datetime or str, optional): **⚡ Recommended** - Start time for partition elimination. Can be a `datetime` object or RFC3339 string (e.g., `"2024-01-01T00:00:00Z"`)
- `end` (datetime or str, optional): **⚡ Recommended** - End time for partition elimination. Can be a `datetime` object or RFC3339 string (e.g., `"2024-01-01T23:59:59Z"`)

**Returns:**

- Iterator of `pyarrow.RecordBatch`: Stream of result batches

**Example:**
```python
import pyarrow as pa

# Stream large dataset
sql = """
    SELECT time, process_id, level, target, msg
    FROM log_entries
    WHERE time >= NOW() - INTERVAL '7 days'
    ORDER BY time DESC;
"""

for record_batch in client.query_stream(sql, begin, end):
    # record_batch is a pyarrow.RecordBatch
    print(f"Batch shape: {record_batch.num_rows} x {record_batch.num_columns}")
    print(f"Schema: {record_batch.schema}")
    
    # Convert to pandas for analysis
    df = record_batch.to_pandas()
    
    # Process this batch
    error_logs = df[df['level'] <= 3]
    if not error_logs.empty:
        print(f"Found {len(error_logs)} errors in this batch")
        # Process errors...
    
    # Memory is automatically freed after each batch
```

## Working with Results

### pandas DataFrames

All `query()` results are pandas DataFrames, giving you access to the full pandas ecosystem:

```python
# Basic DataFrame operations
result = client.query("SELECT process_id, exe, start_time FROM processes;")

# Inspect the data
print(f"Shape: {result.shape}")
print(f"Columns: {result.columns.tolist()}")
print(f"Data types:\n{result.dtypes}")

# Filter and analyze
recent = result[result['start_time'] > datetime.datetime.now() - datetime.timedelta(days=1)]
print(f"Recent processes: {len(recent)}")

# Group and aggregate
by_exe = result.groupby('exe').size().sort_values(ascending=False)
print("Processes by executable:")
print(by_exe.head())
```

### pyarrow RecordBatch

Streaming queries return Apache Arrow RecordBatch objects:

```python
for batch in client.query_stream(sql, begin, end):
    # RecordBatch properties
    print(f"Rows: {batch.num_rows}")
    print(f"Columns: {batch.num_columns}")
    print(f"Schema: {batch.schema}")
    
    # Access individual columns
    time_column = batch.column('time')
    level_column = batch.column('level')
    
    # Convert to pandas (zero-copy operation)
    df = batch.to_pandas()
    
    # Convert to other formats
    table = batch.to_pylist()  # List of dictionaries
    numpy_dict = batch.to_pydict()  # Dictionary of numpy arrays
```

## Connection Configuration

### `FlightSQLClient(uri, headers=None)`

For advanced connection scenarios, use the `FlightSQLClient` class directly:

```python
from micromegas.flightsql.client import FlightSQLClient

# Connect to remote server with authentication
client = FlightSQLClient(
    "grpc+tls://remote-server:50051",
    headers={"authorization": "Bearer your-token"}
)

# Connect to local server (equivalent to micromegas.connect())
client = FlightSQLClient("grpc://localhost:50051")
```

**Parameters:**
- `uri` (str): FlightSQL server URI. Use `grpc://` for unencrypted or `grpc+tls://` for TLS connections
- `headers` (dict, optional): Custom headers for authentication or metadata

## Schema Discovery

### `prepare_statement(sql)`

Get query schema information without executing the query:

```python
# Prepare statement to discover schema
stmt = client.prepare_statement(
    "SELECT time, level, msg FROM log_entries WHERE level <= 3"
)

# Inspect the schema
print("Query result schema:")
for field in stmt.dataset_schema:
    print(f"  {field.name}: {field.type}")

# Output:
#   time: timestamp[ns]
#   level: int32  
#   msg: string

# The query is also available
print(f"Query: {stmt.query}")
```

### `prepared_statement_stream(statement)`

Execute a prepared statement (mainly useful after schema inspection):

```python
# Execute the prepared statement
for batch in client.prepared_statement_stream(stmt):
    df = batch.to_pandas()
    print(f"Received {len(df)} rows")
```

**Note:** Prepared statements are primarily for schema discovery. Execution offers no performance benefit over `query_stream()`.

## Process and Stream Discovery

### `find_process(process_id)`

Find detailed information about a specific process:

```python
# Find process by ID
process_info = client.find_process('550e8400-e29b-41d4-a716-446655440000')

if not process_info.empty:
    print(f"Process: {process_info['exe'].iloc[0]}")
    print(f"Started: {process_info['start_time'].iloc[0]}")
    print(f"Computer: {process_info['computer'].iloc[0]}")
else:
    print("Process not found")
```

### `query_streams(begin, end, limit, process_id=None, tag_filter=None)`

Query event streams with filtering:

```python
# Query all streams from the last hour
end = datetime.datetime.now(datetime.timezone.utc)
begin = end - datetime.timedelta(hours=1)
streams = client.query_streams(begin, end, limit=100)

# Filter by process
process_streams = client.query_streams(
    begin, end, 
    limit=50,
    process_id='550e8400-e29b-41d4-a716-446655440000'
)

# Filter by stream tag
log_streams = client.query_streams(
    begin, end,
    limit=20, 
    tag_filter='log'
)

print(f"Found {len(streams)} total streams")
print(f"Stream types: {streams['stream_type'].value_counts()}")
```

### `query_blocks(begin, end, limit, stream_id)`

Query data blocks within a stream (for low-level inspection):

```python
# First find a stream
streams = client.query_streams(begin, end, limit=1)
if not streams.empty:
    stream_id = streams['stream_id'].iloc[0]
    
    # Query blocks in that stream
    blocks = client.query_blocks(begin, end, 100, stream_id)
    print(f"Found {len(blocks)} blocks")
    print(f"Total events: {blocks['nb_events'].sum()}")
    print(f"Total size: {blocks['payload_size'].sum()} bytes")
```

### `query_spans(begin, end, limit, stream_id)`

Query execution spans for performance analysis:

```python
# Query spans for detailed performance analysis
spans = client.query_spans(begin, end, 1000, stream_id)

# Find slowest operations
slow_spans = spans.nlargest(10, 'duration')
print("Slowest operations:")
for _, span in slow_spans.iterrows():
    duration_ms = span['duration'] / 1000000  # Convert nanoseconds to milliseconds
    print(f"  {span['name']}: {duration_ms:.2f}ms")

# Analyze span hierarchy
root_spans = spans[spans['parent_span_id'].isna()]
print(f"Found {len(root_spans)} root operations")
```

## Data Management

### `bulk_ingest(table_name, df)`

Bulk ingest metadata for replication or administrative tasks:

```python
import pandas as pd

# Example: Replicate process metadata
processes_df = pd.DataFrame({
    'process_id': ['550e8400-e29b-41d4-a716-446655440000'],
    'exe': ['/usr/bin/myapp'],
    'username': ['user'],
    'realname': ['User Name'],
    'computer': ['hostname'],
    'distro': ['Ubuntu 22.04'],
    'cpu_brand': ['Intel Core i7'],
    'tsc_frequency': [2400000000],
    'start_time': [datetime.datetime.now(datetime.timezone.utc)],
    'start_ticks': [1234567890],
    'insert_time': [datetime.datetime.now(datetime.timezone.utc)],
    'parent_process_id': [''],
    'properties': [[]]
})

# Ingest process metadata
result = client.bulk_ingest('processes', processes_df)
if result:
    print(f"Ingested {result.record_count} process records")
```

**Supported tables:** `processes`, `streams`, `blocks`, `payloads`

**Note:** This method is for metadata replication and administrative tasks. Use the telemetry ingestion service HTTP API for normal data ingestion.

### `materialize_partitions(view_set_name, begin, end, partition_delta_seconds)`

Create materialized partitions for performance optimization:

```python
# Materialize hourly partitions for the last 24 hours
end = datetime.datetime.now(datetime.timezone.utc)
begin = end - datetime.timedelta(days=1)

client.materialize_partitions(
    'log_entries',
    begin,
    end,
    3600  # 1-hour partitions
)
# Prints progress messages for each materialized partition
```

### `retire_partitions(view_set_name, view_instance_id, begin, end)`

Remove materialized partitions to free up storage:

```python
# Retire old partitions
client.retire_partitions(
    'log_entries',
    'process-123-456', 
    begin,
    end
)
# Prints status messages as partitions are retired
```

**Warning:** This operation cannot be undone. Retired partitions must be re-materialized if needed.

## Time Utilities

### `format_datetime(value)` and `parse_time_delta(user_string)`

Utility functions for time handling:

```python
from micromegas.time import format_datetime, parse_time_delta

# Format datetime for queries
dt = datetime.datetime.now(datetime.timezone.utc)
formatted = format_datetime(dt)
print(formatted)  # "2024-01-01T12:00:00+00:00"

# Parse human-readable time deltas
one_hour = parse_time_delta('1h')
thirty_minutes = parse_time_delta('30m') 
seven_days = parse_time_delta('7d')

# Use in calculations
recent_time = datetime.datetime.now(datetime.timezone.utc) - parse_time_delta('2h')
```

**Supported units:** `m` (minutes), `h` (hours), `d` (days)

## Advanced Features

### Query Streaming Benefits

Use `query_stream()` for large datasets to:

- **Reduce memory usage**: Process data in chunks instead of loading everything
- **Improve responsiveness**: Start processing before the query completes
- **Handle large results**: Query datasets larger than available RAM

```python
# Example: Process week of data in batches
total_errors = 0
total_rows = 0

for batch in client.query_stream("""
    SELECT level, msg FROM log_entries 
    WHERE time >= NOW() - INTERVAL '7 days'
""", begin, end):
    df = batch.to_pandas()
    errors_in_batch = len(df[df['level'] <= 2])
    
    total_errors += errors_in_batch
    total_rows += len(df)
    
    print(f"Batch: {len(df)} rows, {errors_in_batch} errors")

print(f"Total: {total_rows} rows, {total_errors} errors")
```

### FlightSQL Protocol Benefits

Micromegas uses Apache Arrow FlightSQL for optimal performance:

- **Columnar data transfer**: Orders of magnitude faster than JSON
- **Binary protocol**: No serialization/deserialization overhead  
- **Native compression**: Efficient network utilization
- **Vectorized operations**: Optimized for analytical workloads
- **Zero-copy operations**: Direct memory mapping from network buffers

### Connection Configuration

```python
# Connect to local server (default)
client = micromegas.connect()

# Connect to a custom endpoint using FlightSQLClient directly
from micromegas.flightsql.client import FlightSQLClient
client = FlightSQLClient("grpc://remote-server:50051")
```

## Error Handling

```python
try:
    df = client.query("SELECT * FROM log_entries;", begin, end)
except Exception as e:
    print(f"Query failed: {e}")
    
# Check for empty results
if df.empty:
    print("No data found for this time range")
else:
    print(f"Found {len(df)} rows")
```

## Performance Tips

### Use Time Ranges

Always specify time ranges for better performance:

```python
# ✅ Good - efficient
df = client.query(sql, begin, end)

# ❌ Avoid - can be slow
df = client.query(sql)
```

### Streaming for Large Results

Use streaming for queries that might return large datasets:

```python
# If you expect > 100MB of results, use streaming
if expected_result_size_mb > 100:
    for batch in client.query_stream(sql, begin, end):
        process_batch(batch.to_pandas())
else:
    df = client.query(sql, begin, end)
    process_dataframe(df)
```

### Limit Result Size

Add LIMIT clauses for exploratory queries:

```python
# Good for exploration
df = client.query("SELECT * FROM log_entries LIMIT 1000;", begin, end)

# Then remove limit for production queries
df = client.query("SELECT * FROM log_entries WHERE level <= 2;", begin, end)
```

## Integration Examples

### Jupyter Notebooks

```python
import matplotlib.pyplot as plt
import seaborn as sns

# Query data
df = client.query("""
    SELECT time, name, value 
    FROM measures 
    WHERE name = 'cpu_usage'
""", begin, end)

# Plot time series
plt.figure(figsize=(12, 6))
plt.plot(df['time'], df['value'])
plt.title('CPU Usage Over Time')
plt.xlabel('Time')
plt.ylabel('CPU Usage %')
plt.show()
```

### Data Pipeline

```python
import pandas as pd

def extract_metrics(process_id, hours=24):
    """Extract metrics for a specific process."""
    end = datetime.datetime.now(datetime.timezone.utc)
    begin = end - datetime.timedelta(hours=hours)
    
    sql = f"""
        SELECT time, name, value, unit
        FROM view_instance('measures', '{process_id}')
        ORDER BY time;
    """
    
    return client.query(sql, begin, end)

def analyze_performance(df):
    """Analyze performance metrics."""
    metrics = {}
    for name in df['name'].unique():
        data = df[df['name'] == name]['value']
        metrics[name] = {
            'mean': data.mean(),
            'max': data.max(),
            'min': data.min(),
            'std': data.std()
        }
    return metrics

# Use in pipeline
process_metrics = extract_metrics('my-service-123')
performance_summary = analyze_performance(process_metrics)
print(performance_summary)
```

## Next Steps

- **[Python API Advanced](python-api-advanced.md)** - Advanced patterns, performance optimization, and specialized tooling
- **[Schema Reference](schema-reference.md)** - Understand available views and fields
- **[Functions Reference](functions-reference.md)** - Learn about SQL functions
- **[Query Patterns](query-patterns.md)** - Common observability query patterns
- **[Performance Guide](performance.md)** - Optimize your queries
