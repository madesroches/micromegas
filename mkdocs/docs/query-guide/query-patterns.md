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

More patterns coming soon...
