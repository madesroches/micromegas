# Log Stats View Implementation Plan

## Feature Overview
Create a new SQL-based view (`log_stats`) that aggregates log entries by process, minute, level, and target. This will provide efficient querying of log statistics over time periods, grouped by key dimensions.

## Current State Analysis

### Existing Infrastructure
- **SqlBatchView**: Framework for materialized SQL-based views with incremental updates
- **Example Implementation**: `sql_view_test.rs` contains `log_entries_per_process_per_minute` view
  - Aggregates by process_id and 1-minute time bins
  - Counts log levels (fatal, error, warn, info, debug, trace)
  - Lacks target field aggregation

### Log Entry Schema
Available fields from `log_entries` view:
- `process_id`: Dictionary(Int16, Utf8) - Process identifier
- `time`: Timestamp(Nanosecond) - Event timestamp
- `insert_time`: Timestamp(Nanosecond) - Server insertion time  
- `level`: Int32 - Log level (1=Fatal through 6=Trace)
- `target`: Dictionary(Int16, Utf8) - Module/category name
- `msg`: Utf8 - Log message
- Additional metadata: exe, username, computer, stream_id, block_id

## Proposed Design

### View Name
`log_stats`

### Schema Definition
```rust
Field::new("time_bin", DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())), true),
Field::new("process_id", DataType::Dictionary(DataType::Int16.into(), DataType::Utf8.into()), false),
Field::new("level", DataType::Int32, false),
Field::new("target", DataType::Dictionary(DataType::Int16.into(), DataType::Utf8.into()), false),
Field::new("count", DataType::Int64, false),
```

### SQL Queries

#### Count Source Query
```sql
SELECT count(*) as count
FROM log_entries
WHERE insert_time >= '{begin}'
AND insert_time < '{end}';
```

#### Transform Query
```sql
SELECT date_bin('1 minute', time) as time_bin,
       process_id,
       level,
       target,
       count(*) as count
FROM log_entries
WHERE insert_time >= '{begin}'
AND insert_time < '{end}'
GROUP BY process_id, level, target, time_bin;
```

#### Merge Partitions Query
```sql
SELECT time_bin,
       process_id,
       level,
       target,
       sum(count) as count
FROM {source}
GROUP BY process_id, level, target, time_bin;
```

## Implementation Plan

### Phase 1: Core View Implementation
1. Create new module `log_stats_view.rs` in `rust/analytics/src/lakehouse/`
2. Implement `make_log_stats_view()` function
3. Define schema with all required fields
4. Implement the three SQL queries (count, transform, merge)
5. Configure SqlBatchView with appropriate parameters:
   - Source partition delta: 1 day
   - Merge partition delta: 1 day
   - Update group: 3000 (controls materialization order - blocks=1000, core views (log_entries)=2000, derived views (log_stats)=3000. This ensures log_entries is materialized before log_stats)

### Phase 2: Integration
1. Add view to `view_factory.rs` default views
2. Register in view factory initialization
3. Ensure proper view naming and instance ID

### Phase 3: Testing
1. Create unit test in `tests/log_stats_view_test.rs`
2. Test materialization with sample data
3. Validate aggregation accuracy against raw queries
4. Test partition merging behavior
5. Performance benchmarking with large datasets

### Phase 4: Query Optimization
1. Consider custom merger if needed (like LogSummaryMerger example)
2. Optimize for common query patterns:
   - Filter by specific process
   - Filter by level threshold (e.g., errors and above)
   - Filter by target prefix matching

## Use Cases

### Primary Queries Enabled
1. **Error Rate Analysis**
   ```sql
   SELECT time_bin, process_id, target, count
   FROM log_stats
   WHERE level <= 2  -- Fatal and Error only
   ORDER BY count DESC;
   ```

2. **Target-Specific Monitoring**
   ```sql
   SELECT time_bin, level, sum(count) as total
   FROM log_stats
   WHERE target LIKE 'micromegas_tracing%'
   GROUP BY time_bin, level;
   ```

3. **Process Health Dashboard**
   ```sql
   SELECT process_id,
          sum(CASE WHEN level <= 2 THEN count ELSE 0 END) as errors,
          sum(CASE WHEN level = 3 THEN count ELSE 0 END) as warnings,
          sum(count) as total
   FROM log_stats
   WHERE time_bin >= now() - interval '1 hour'
   GROUP BY process_id;
   ```

## Benefits
- **Performance**: Pre-aggregated data reduces query time from seconds to milliseconds
- **Storage**: Compact representation of high-volume log data
- **Flexibility**: Granular grouping allows various analytical queries
- **Simplicity**: Clean schema focused on essential statistics

## Future Enhancements
1. Add percentile calculations for log message lengths
2. Include error classification based on message patterns
3. Support for custom time bins (5 min, 1 hour)
4. Integration with alerting system for anomaly detection
5. Add more statistical aggregations (stddev, variance)

## Configuration
- Time bin: 1 minute (configurable)
- Partition size: 1 day
- Retention: Follow global retention policy
- Update frequency: Incremental as new data arrives

## Dependencies
- Existing log_entries view (update group 2000)
- SqlBatchView framework
- DataFusion query engine
- PostgreSQL metadata store
- Object storage for materialized partitions

## Update Group Hierarchy
The system uses update groups to control materialization order:
- **1000**: `blocks` - Foundation view with raw block metadata
- **2000**: Core views (`log_entries`, `measures`, `processes`, `streams`)
- **3000**: `log_stats` - Derived view depending on log_entries
- **4000**: Test and complex derived views

Views with lower IDs materialize first, ensuring dependencies are met. Views with the same ID can run concurrently.
