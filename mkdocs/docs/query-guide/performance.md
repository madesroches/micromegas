# Performance Guide

Guidelines for writing efficient Micromegas SQL queries and avoiding common performance pitfalls.

## Critical Performance Rules

### 1. Always Use Time Ranges (via Python API)

**⚡ Performance Tip:** Always specify time ranges through the Python API parameters, not in SQL WHERE clauses.

**❌ Inefficient - SQL time filter:**
```python
# Analytics server scans ALL partitions, then filters in SQL
sql = """
    SELECT COUNT(*) FROM log_entries 
    WHERE time >= NOW() - INTERVAL '1 hour';
"""
result = client.query(sql)  # No time range parameters!
```

**✅ Efficient - API time range:**
```python
import datetime

# Analytics server eliminates irrelevant partitions BEFORE query execution
now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(hours=1)
end = now

sql = "SELECT COUNT(*) FROM log_entries;"
result = client.query(sql, begin, end)  # ⭐ Time range in API
```

**Why API time ranges are faster:**

- **Partition Elimination**: Analytics server removes entire partitions from consideration before SQL execution
- **Metadata Optimization**: Uses partition metadata to skip irrelevant data files  
- **Memory Efficiency**: Only loads relevant data into query engine memory
- **Network Efficiency**: Transfers only relevant data over FlightSQL

**Performance Impact:**

- API time range: Query considers only 1-2 partitions
- SQL time filter: Query scans all partitions, then filters millions of rows

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
