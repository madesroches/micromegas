# Advanced Features

## View Materialization

Micromegas uses a lakehouse architecture with on-demand view materialization: raw data lives in object storage (S3/GCS) and views are materialized when queried, with automatic caching for frequently accessed data.

### Global Views vs View Instances

#### Global Views (Implicit)
Querying a view directly by name uses a global view that spans all processes:

```sql
-- Global view - queries data from ALL processes
SELECT * FROM log_entries WHERE level <= 2;
SELECT * FROM measures WHERE name = 'cpu_usage';
```

Global views are convenient for exploring data across the whole system and for cross-process analysis, without needing specific process IDs.

#### View Instances (Explicit)
The `view_instance()` function creates a process- or stream-scoped view for better performance:

```sql
-- View instance - queries data from ONE specific process
SELECT * FROM view_instance('log_entries', 'my_process_123') WHERE level <= 2;
SELECT * FROM view_instance('measures', 'my_process_123') WHERE name = 'cpu_usage';
```

View instances only scan partitions for the specified process/stream, so they're faster than filtering a global view — use them when analyzing specific processes or streams, especially on production systems with large amounts of data.

## Architecture

- **Datalake (S3)**: custom binary format, cheap storage, fast writes
- **Lakehouse (Parquet)**: columnar format, fast analytics, industry standard
- **Query Engine (DataFusion)**: SQL engine optimized for analytical workloads

Heavy data streams remain unprocessed until queried — cheap to store in S3, cheap to delete unused data. Low-frequency streams (logs, metrics) can be used to decide sampling of high-frequency streams (spans).
