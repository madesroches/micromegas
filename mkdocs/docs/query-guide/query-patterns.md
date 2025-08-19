# Query Patterns

Common patterns and examples for querying observability data with Micromegas SQL.

## Error Tracking and Debugging

### Recent Errors
```sql
-- Get all errors from the last hour
SELECT time, process_id, target, msg
FROM log_entries
WHERE level <= 2  -- Fatal and Error
  AND time >= NOW() - INTERVAL '1 hour'
ORDER BY time DESC;
```

### Error Trends
```sql
-- Hourly error counts for trend analysis
SELECT
    date_trunc('hour', time) as hour,
    COUNT(*) as error_count
FROM log_entries
WHERE level <= 2
  AND time >= NOW() - INTERVAL '24 hours'
GROUP BY date_trunc('hour', time)
ORDER BY hour;
```

## Performance Monitoring

### Slow Operations
```sql
-- Find slowest function calls
SELECT
    name,
    AVG(duration) / 1000000.0 as avg_ms,
    MAX(duration) / 1000000.0 as max_ms,
    COUNT(*) as call_count
FROM view_instance('thread_spans', 'my_process')
WHERE duration > 10000000  -- > 10ms
GROUP BY name
ORDER BY avg_ms DESC
LIMIT 10;
```

### Resource Usage
```sql
-- CPU and memory trends
SELECT
    date_trunc('minute', time) as minute,
    name,
    AVG(value) as avg_value,
    unit
FROM measures
WHERE name IN ('cpu_usage', 'memory_usage')
  AND time >= NOW() - INTERVAL '1 hour'
GROUP BY minute, name, unit
ORDER BY minute, name;
```

## Async Performance Analysis

### Top-Level Async Operations
```sql
-- Find slowest top-level async operations
SELECT
    name,
    AVG(duration_ms) as avg_duration,
    MAX(duration_ms) as max_duration,
    COUNT(*) as operation_count
FROM (
    SELECT
        begin_events.name,
        CAST((end_events.time - begin_events.time) AS BIGINT) / 1000000 as duration_ms
    FROM
        (SELECT * FROM view_instance('async_events', 'my_process') WHERE event_type = 'begin' AND depth = 0) begin_events
    LEFT JOIN
        (SELECT * FROM view_instance('async_events', 'my_process') WHERE event_type = 'end') end_events
        ON begin_events.span_id = end_events.span_id
    WHERE end_events.span_id IS NOT NULL
)
GROUP BY name
ORDER BY avg_duration DESC
LIMIT 10;
```

### Nested Async Operations
```sql
-- Find operations that spawn many async children
SELECT
    parent_name,
    parent_depth,
    COUNT(*) as child_count,
    AVG(child_duration_ms) as avg_child_duration
FROM (
    SELECT
        parent.name as parent_name,
        parent.depth as parent_depth,
        CAST((child_end.time - child_begin.time) AS BIGINT) / 1000000 as child_duration_ms
    FROM view_instance('async_events', 'my_process') parent
    JOIN view_instance('async_events', 'my_process') child_begin
         ON parent.span_id = child_begin.parent_span_id AND child_begin.event_type = 'begin'
    JOIN view_instance('async_events', 'my_process') child_end
         ON child_begin.span_id = child_end.span_id AND child_end.event_type = 'end'
    WHERE parent.event_type = 'begin'
)
GROUP BY parent_name, parent_depth
HAVING COUNT(*) > 5  -- Operations with many children
ORDER BY child_count DESC;
```

### Async Operation Timeline
```sql
-- Timeline view of async operations with depth hierarchy
SELECT
    time,
    event_type,
    name,
    depth,
    span_id,
    parent_span_id,
    REPEAT('  ', depth) || name as indented_name  -- Visual hierarchy
FROM view_instance('async_events', 'my_process')
WHERE time >= NOW() - INTERVAL '10 minutes'
ORDER BY time;
```
