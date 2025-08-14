# Performance Guide

Guidelines for writing efficient Micromegas SQL queries and avoiding common performance pitfalls.

## Critical Performance Rules

### 1. Always Use Time Ranges

**❌ Avoid:**
```sql
-- Queries entire dataset - can be slow and memory-intensive
SELECT COUNT(*) FROM log_entries;
```

**✅ Good:**
```sql
-- Queries specific time window - fast and memory-efficient
SELECT COUNT(*) FROM log_entries
WHERE time >= NOW() - INTERVAL '1 hour';
```

### 2. Use Process-Scoped Views

**❌ Less Efficient:**
```sql
-- Scans all data then filters
SELECT * FROM log_entries WHERE process_id = 'my_process';
```

**✅ More Efficient:**
```sql
-- Uses optimized process partition
SELECT * FROM view_instance('log_entries', 'my_process');
```

## Query Optimization

### Predicate Pushdown
Micromegas automatically pushes filters down to the storage layer when possible:

```sql
-- These filters are pushed to Parquet reader for efficiency
WHERE time >= NOW() - INTERVAL '1 day'
  AND level <= 3
  AND process_id = 'my_process'
```

### Memory Considerations

**Use LIMIT for exploration:**
```sql
-- Good for testing queries
SELECT * FROM log_entries LIMIT 1000;
```

**Use streaming for large results:**
```python
# Python API for large datasets
for batch in client.query_stream(sql, begin, end):
    process_batch(batch.to_pandas())
```

More performance guidance coming soon...
