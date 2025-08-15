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

# Connect to local Micromegas instance (default)
client = micromegas.connect()

# Connect to remote instance
client = micromegas.connect(endpoint="http://your-server:8080")
```

The `connect()` function automatically discovers your Micromegas server or uses the provided endpoint URL.

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
# Default connection (auto-discovery)
client = micromegas.connect()

# Explicit endpoint
client = micromegas.connect(endpoint="http://analytics.mycompany.com:8080")

# Connection with timeout
client = micromegas.connect(
    endpoint="http://remote-server:8080",
    timeout=30.0
)
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
        WHERE time >= '{begin.isoformat()}'
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

- **[Schema Reference](schema-reference.md)** - Understand available views and fields
- **[Functions Reference](functions-reference.md)** - Learn about SQL functions
- **[Query Patterns](query-patterns.md)** - Common observability query patterns
- **[Performance Guide](performance.md)** - Optimize your queries
