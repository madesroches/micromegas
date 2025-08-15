# Query Guide Overview

Micromegas provides a powerful SQL interface for querying observability data including logs, metrics, spans, and traces. **Micromegas SQL is an extension of [Apache DataFusion SQL](https://datafusion.apache.org/user-guide/sql/)** - you can use all standard DataFusion SQL features plus Micromegas-specific functions and views optimized for observability workloads.

## Key Concepts

### SQL Engine
Micromegas uses **Apache DataFusion** as its SQL engine, which means you get:

- Full SQL standard compliance
- Advanced query optimization
- Vectorized execution engine
- Columnar data processing with Apache Arrow

### Data Architecture
- **Raw data** stored in object storage (S3/GCS) in Parquet format
- **Metadata** stored in PostgreSQL for fast lookups
- **Views** provide logical organization of telemetry data
- **On-demand ETL** processes data only when queried

## Available Interfaces

### Python API
The primary interface for querying Micromegas data programmatically. All queries return **[pandas DataFrames](https://pandas.pydata.org/docs/reference/api/pandas.DataFrame.html)**, making it easy to work with results using the pandas ecosystem:

```python
import micromegas
client = micromegas.connect()
df = client.query("SELECT * FROM log_entries LIMIT 10;")
```

### Grafana Plugin
Use the same SQL capabilities in Grafana dashboards through the [Micromegas Grafana plugin](https://github.com/madesroches/micromegas-grafana).

## Data Views

Micromegas organizes telemetry data into several queryable views:

| View | Description |
|------|-------------|
| `processes` | Process metadata and system information |
| `streams` | Data stream information within processes |
| `log_entries` | Application log messages with levels and context |
| `measures` | Numeric metrics and performance measurements |
| `thread_spans` | Synchronous execution spans and timing |
| `async_events` | Asynchronous event lifecycle tracking |

## Query Capabilities

### Standard SQL Features
- SELECT, FROM, WHERE, ORDER BY, GROUP BY
- JOINs between views
- Aggregation functions (COUNT, SUM, AVG, etc.)
- Window functions and CTEs
- Complex filtering and sorting

### Observability Extensions
- Time-range filtering for performance
- Process-scoped view instances
- Histogram generation functions
- Log level filtering and analysis
- Span relationship queries

### Performance Features
- Query streaming for large datasets
- Predicate pushdown to storage layer
- Automatic view materialization
- Memory-efficient processing

## Getting Started

1. **[Quick Start](quick-start.md)** - Basic queries to get you started
2. **[Python API](python-api.md)** - Complete API reference and examples
3. **[Schema Reference](schema-reference.md)** - Detailed view and field documentation
4. **[Functions Reference](functions-reference.md)** - Available SQL functions
5. **[Query Patterns](query-patterns.md)** - Common observability query patterns
6. **[Performance Guide](performance.md)** - Optimize your queries for best performance
7. **[Advanced Features](advanced-features.md)** - View materialization and custom views

## Best Practices

### Always Use Time Ranges
For performance and memory efficiency, always specify time ranges in your queries:

```python
# Good - uses time range
df = client.query(sql, begin_time, end_time)

# Avoid - queries all data
df = client.query(sql)  # Can be slow and memory-intensive
```

### Start Simple
Begin with basic queries and add complexity incrementally:

```sql
-- Start with this
SELECT * FROM log_entries LIMIT 10;

-- Then add filtering
SELECT * FROM log_entries WHERE level <= 3 LIMIT 10;

-- Then add time range
SELECT * FROM log_entries 
WHERE level <= 3 AND time >= NOW() - INTERVAL '1 hour'
LIMIT 10;
```

### Use Process-Scoped Views
For better performance when analyzing specific processes:

```sql
-- Instead of filtering the global view
SELECT * FROM log_entries WHERE process_id = 'my_process';

-- Use a process-scoped view instance
SELECT * FROM view_instance('log_entries', 'my_process');
```

Ready to start querying? Head to the [Quick Start](quick-start.md) guide!
