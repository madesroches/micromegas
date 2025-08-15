# Advanced Features

Advanced Micromegas features including view materialization, custom views, and system administration.

## View Materialization

Micromegas uses a lakehouse architecture with on-demand view materialization for optimal performance.

### JIT View Processing
- Raw data stored in object storage (S3/GCS)
- Views materialized on-demand when queried
- Automatic caching for frequently accessed data

### Global Views vs View Instances

Micromegas provides two ways to access telemetry data:

#### Global Views (Implicit)
When you query views directly by name, you're using global views that span all processes:

```sql
-- Global view - queries data from ALL processes
SELECT * FROM log_entries WHERE level <= 2;
SELECT * FROM measures WHERE name = 'cpu_usage';
```

Global views are convenient for:
- Exploring data across the entire system
- Cross-process analysis and correlation
- Getting started without knowing specific process IDs

#### View Instances (Explicit)
Use the `view_instance()` function to create process-scoped views for better performance:

```sql
-- View instance - queries data from ONE specific process
SELECT * FROM view_instance('log_entries', 'my_process_123') WHERE level <= 2;
SELECT * FROM view_instance('measures', 'my_process_123') WHERE name = 'cpu_usage';
```

View instances are optimal for:
- Analyzing specific processes or streams
- Better query performance (fewer partitions to scan)
- Production systems with large amounts of data

**Performance Impact:**
- Global views: May scan many partitions across all processes
- View instances: Only scan partitions for the specified process/stream

## Architecture Benefits

### Datalake → Lakehouse → Query
- **Datalake (S3)**: Custom binary format, cheap storage, fast writes
- **Lakehouse (Parquet)**: Columnar format, fast analytics, industry standard
- **Query Engine (DataFusion)**: SQL engine optimized for analytical workloads

### Tail Sampling Support
- Heavy data streams remain unprocessed until queried
- Cheap to store in S3, cheap to delete unused data
- Use low-frequency streams (logs, metrics) to decide sampling of high-frequency streams (spans)

More advanced features documentation coming soon...
