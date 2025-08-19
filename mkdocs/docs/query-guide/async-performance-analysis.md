# Async Performance Analysis Guide

This guide provides comprehensive patterns and examples for analyzing asynchronous operation performance using the `async_events` view with depth tracking.

## Understanding Async Event Depth

The `depth` field in `async_events` represents the nesting level in the async call hierarchy:

- **Depth 0**: Top-level async operations (entry points)
- **Depth 1**: First-level nested async operations
- **Depth 2+**: Deeper nested async operations

This enables hierarchical performance analysis similar to synchronous call stack profiling.

## Core Analysis Patterns

### 1. Top-Level Performance Overview

Start with top-level operations (depth = 0) to identify primary performance bottlenecks:

```sql
-- Top-level async operations with performance metrics
SELECT
    name,
    COUNT(*) as operation_count,
    AVG(duration_ms) as avg_duration,
    MIN(duration_ms) as min_duration,
    MAX(duration_ms) as max_duration,
    STDDEV(duration_ms) as duration_stddev
FROM (
    SELECT
        begin_events.name,
        CAST((end_events.time - begin_events.time) AS BIGINT) / 1000000 as duration_ms
    FROM
        (SELECT * FROM view_instance('async_events', 'process_id')
         WHERE event_type = 'begin' AND depth = 0) begin_events
    LEFT JOIN
        (SELECT * FROM view_instance('async_events', 'process_id')
         WHERE event_type = 'end') end_events
        ON begin_events.span_id = end_events.span_id
    WHERE end_events.span_id IS NOT NULL
)
GROUP BY name
ORDER BY avg_duration DESC;
```

### 2. Depth-Based Performance Comparison

Compare performance characteristics across different call depths:

```sql
-- Performance metrics by async call depth
SELECT
    depth,
    COUNT(*) as span_count,
    AVG(duration_ms) as avg_duration,
    PERCENTILE(duration_ms, 0.5) as median_duration,
    PERCENTILE(duration_ms, 0.95) as p95_duration,
    PERCENTILE(duration_ms, 0.99) as p99_duration
FROM (
    SELECT
        begin_events.depth,
        CAST((end_events.time - begin_events.time) AS BIGINT) / 1000000 as duration_ms
    FROM
        (SELECT * FROM view_instance('async_events', 'process_id') WHERE event_type = 'begin') begin_events
    LEFT JOIN
        (SELECT * FROM view_instance('async_events', 'process_id') WHERE event_type = 'end') end_events
        ON begin_events.span_id = end_events.span_id
    WHERE end_events.span_id IS NOT NULL
)
GROUP BY depth
ORDER BY depth;
```

### 3. Parent-Child Performance Analysis

Analyze how async operations delegate work to nested operations:

```sql
-- Parent-child async operation performance relationships
SELECT
    parent.name as parent_operation,
    parent.depth as parent_depth,
    child.name as child_operation,
    child.depth as child_depth,
    COUNT(*) as relationship_count,
    AVG(parent_duration_ms) as avg_parent_duration,
    AVG(child_duration_ms) as avg_child_duration,
    AVG(parent_duration_ms) - AVG(child_duration_ms) as avg_overhead_ms
FROM (
    SELECT
        p.name, p.depth, p.span_id,
        c.name as child_name, c.depth as child_depth, c.span_id as child_span_id,
        CAST((p_end.time - p_begin.time) AS BIGINT) / 1000000 as parent_duration_ms,
        CAST((c_end.time - c_begin.time) AS BIGINT) / 1000000 as child_duration_ms
    FROM view_instance('async_events', 'process_id') p
    JOIN view_instance('async_events', 'process_id') c ON p.span_id = c.parent_span_id
    JOIN view_instance('async_events', 'process_id') p_begin ON p.span_id = p_begin.span_id AND p_begin.event_type = 'begin'
    JOIN view_instance('async_events', 'process_id') p_end ON p.span_id = p_end.span_id AND p_end.event_type = 'end'
    JOIN view_instance('async_events', 'process_id') c_begin ON c.span_id = c_begin.span_id AND c_begin.event_type = 'begin'
    JOIN view_instance('async_events', 'process_id') c_end ON c.span_id = c_end.span_id AND c_end.event_type = 'end'
    WHERE p.event_type = 'begin' AND c.event_type = 'begin'
) as relationships(name, depth, span_id, child_name, child_depth, child_span_id, parent_duration_ms, child_duration_ms)
GROUP BY parent_operation, parent_depth, child_operation, child_depth
HAVING COUNT(*) > 5  -- Focus on significant relationships
ORDER BY relationship_count DESC, avg_overhead_ms DESC;
```

## Advanced Analysis Techniques

### 4. Async Concurrency Analysis

Identify periods of high async concurrency:

```sql
-- Concurrent async operations over time
SELECT
    time_bucket,
    MAX(concurrent_operations) as peak_concurrency,
    AVG(concurrent_operations) as avg_concurrency
FROM (
    SELECT
        date_trunc('minute', time) as time_bucket,
        COUNT(*) as concurrent_operations
    FROM view_instance('async_events', 'process_id')
    WHERE event_type = 'begin'
    GROUP BY date_trunc('minute', time)
)
GROUP BY time_bucket
ORDER BY time_bucket;
```

### 5. Deep Nesting Detection

Find problematic deep async call chains:

```sql
-- Operations with excessive async nesting depth
SELECT
    name,
    depth,
    COUNT(*) as occurrence_count,
    AVG(duration_ms) as avg_duration
FROM (
    SELECT
        begin_events.name,
        begin_events.depth,
        CAST((end_events.time - begin_events.time) AS BIGINT) / 1000000 as duration_ms
    FROM
        (SELECT * FROM view_instance('async_events', 'process_id') WHERE event_type = 'begin') begin_events
    LEFT JOIN
        (SELECT * FROM view_instance('async_events', 'process_id') WHERE event_type = 'end') end_events
        ON begin_events.span_id = end_events.span_id
    WHERE end_events.span_id IS NOT NULL
)
WHERE depth >= 3  -- Focus on deep nesting
GROUP BY name, depth
ORDER BY depth DESC, occurrence_count DESC;
```

### 6. Async Operation Hotspots

Identify the most frequently called async operations by depth:

```sql
-- Async operation frequency by depth level
SELECT
    depth,
    name,
    COUNT(*) as call_count,
    AVG(duration_ms) as avg_duration,
    COUNT(*) * AVG(duration_ms) as total_time_spent
FROM (
    SELECT
        begin_events.name,
        begin_events.depth,
        CAST((end_events.time - begin_events.time) AS BIGINT) / 1000000 as duration_ms
    FROM
        (SELECT * FROM view_instance('async_events', 'process_id') WHERE event_type = 'begin') begin_events
    LEFT JOIN
        (SELECT * FROM view_instance('async_events', 'process_id') WHERE event_type = 'end') end_events
        ON begin_events.span_id = end_events.span_id
    WHERE end_events.span_id IS NOT NULL
)
GROUP BY depth, name
ORDER BY total_time_spent DESC;
```

## Performance Optimization Strategies

### Focus Areas Based on Depth Analysis

1. **Depth 0 Optimization**: Target top-level operations for maximum impact
2. **High-Frequency Operations**: Optimize operations with high call counts
3. **Deep Nesting Reduction**: Flatten async call hierarchies where possible
4. **Concurrency Tuning**: Balance async concurrency with resource usage

### Query Performance Tips

1. **Always use time ranges** through Python API parameters
2. **Filter by depth early** to reduce data processing
3. **Use process-scoped views** (`view_instance`) for efficiency
4. **Combine with other views** (logs, measures) for context

### Example: Comprehensive Async Performance Dashboard

```sql
-- Multi-dimensional async performance summary
WITH async_durations AS (
    SELECT
        begin_events.name,
        begin_events.depth,
        begin_events.time as start_time,
        CAST((end_events.time - begin_events.time) AS BIGINT) / 1000000 as duration_ms
    FROM
        (SELECT * FROM view_instance('async_events', 'process_id') WHERE event_type = 'begin') begin_events
    LEFT JOIN
        (SELECT * FROM view_instance('async_events', 'process_id') WHERE event_type = 'end') end_events
        ON begin_events.span_id = end_events.span_id
    WHERE end_events.span_id IS NOT NULL
),
depth_summary AS (
    SELECT
        depth,
        COUNT(*) as operation_count,
        AVG(duration_ms) as avg_duration,
        PERCENTILE(duration_ms, 0.95) as p95_duration
    FROM async_durations
    GROUP BY depth
),
top_operations AS (
    SELECT
        name,
        COUNT(*) as call_count,
        AVG(duration_ms) as avg_duration
    FROM async_durations
    WHERE depth = 0  -- Top-level only
    GROUP BY name
    ORDER BY avg_duration DESC
    LIMIT 5
)
SELECT
    'Depth Summary' as analysis_type,
    CAST(depth AS VARCHAR) as name,
    operation_count as count,
    avg_duration,
    p95_duration as p95
FROM depth_summary
UNION ALL
SELECT
    'Top Operations' as analysis_type,
    name,
    call_count as count,
    avg_duration,
    NULL as p95
FROM top_operations
ORDER BY analysis_type, avg_duration DESC;
```

This comprehensive approach enables effective async performance analysis and optimization based on call hierarchy depth information.
