# Python API Advanced Guide

This guide covers advanced usage patterns, performance optimization techniques, and specialized features for power users of the Micromegas Python client.

!!! tip "Prerequisites"
    Before reading this guide, familiarize yourself with the [Python API Reference](python-api.md) for basic usage patterns.

## Advanced Connection Patterns

### Authentication and Headers

Configure custom authentication headers for enterprise deployments:

```python
from micromegas.flightsql.client import FlightSQLClient

# Token-based authentication
client = FlightSQLClient(
    "grpc+tls://analytics.company.com:50051",
    headers={
        "authorization": "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
        "x-tenant-id": "production",
        "x-request-id": "trace-12345"
    }
)

# API key authentication  
client = FlightSQLClient(
    "grpc+tls://micromegas.example.com:50051",
    headers={
        "x-api-key": "mm_prod_1234567890abcdef",
        "x-client-version": "python-1.0.0"
    }
)
```

## Schema Discovery and Query Validation

### Advanced Schema Introspection

```python
def analyze_query_schema(client, sql):
    """Analyze query result schema without execution."""
    stmt = client.prepare_statement(sql)
    
    analysis = {
        'column_count': len(stmt.dataset_schema),
        'columns': [],
        'estimated_row_size': 0
    }
    
    for field in stmt.dataset_schema:
        col_info = {
            'name': field.name,
            'type': str(field.type),
            'nullable': field.nullable
        }
        
        # Estimate column size for memory planning
        if 'string' in str(field.type):
            estimated_size = 50  # Average string length
        elif 'int64' in str(field.type):
            estimated_size = 8
        elif 'int32' in str(field.type):
            estimated_size = 4
        elif 'timestamp' in str(field.type):
            estimated_size = 8
        else:
            estimated_size = 16  # Default estimate
            
        col_info['estimated_bytes'] = estimated_size
        analysis['columns'].append(col_info)
        analysis['estimated_row_size'] += estimated_size
    
    return analysis

# Usage
sql = "SELECT time, process_id, level, msg FROM log_entries"
schema_info = analyze_query_schema(client, sql)

print(f"Query will return {schema_info['column_count']} columns")
print(f"Estimated row size: {schema_info['estimated_row_size']} bytes")

for col in schema_info['columns']:
    print(f"  {col['name']}: {col['type']} ({col['estimated_bytes']} bytes)")
```

### Query Validation Pipeline

```python
def validate_query(client, sql, max_columns=50, max_estimated_size=1000):
    """Validate query before execution."""
    try:
        analysis = analyze_query_schema(client, sql)
    except Exception as e:
        return {
            'valid': False,
            'error': f"Schema analysis failed: {e}",
            'recommendations': ["Check SQL syntax and table names"]
        }
    
    recommendations = []
    
    if analysis['column_count'] > max_columns:
        recommendations.append(f"Query returns {analysis['column_count']} columns, consider selecting specific columns")
    
    if analysis['estimated_row_size'] > max_estimated_size:
        recommendations.append(f"Large estimated row size ({analysis['estimated_row_size']} bytes), consider using query_stream()")
    
    # Check for potentially expensive operations
    sql_upper = sql.upper()
    if 'ORDER BY' in sql_upper and 'LIMIT' not in sql_upper:
        recommendations.append("ORDER BY without LIMIT may be expensive, consider adding LIMIT")
    
    if not any(param in sql_upper for param in ['BEGIN', 'END', 'WHERE TIME']):
        recommendations.append("No time filtering detected, consider adding time range parameters")
    
    return {
        'valid': True,
        'analysis': analysis,
        'recommendations': recommendations
    }

# Usage
sql = """
SELECT time, process_id, level, target, msg, properties 
FROM log_entries 
ORDER BY time DESC
"""

validation = validate_query(client, sql)
if validation['valid']:
    print("Query is valid")
    for rec in validation['recommendations']:
        print(f"⚠️  {rec}")
else:
    print(f"❌ Query validation failed: {validation['error']}")
```

## Performance Optimization Patterns

### Intelligent Batching for Large Datasets

```python
import time
from datetime import datetime, timedelta, timezone

class OptimizedQueryExecutor:
    """Execute large queries with intelligent batching and progress tracking."""
    
    def __init__(self, client, batch_size_mb=100):
        self.client = client
        self.batch_size_mb = batch_size_mb
        self.stats = {
            'batches_processed': 0,
            'rows_processed': 0,
            'total_time': 0,
            'avg_batch_time': 0
        }
    
    def execute_large_query(self, sql, begin, end, processor_func=None):
        """Execute query with automatic batching based on result size."""
        start_time = time.time()
        total_rows = 0
        
        # First, estimate result size
        count_sql = f"SELECT COUNT(*) as row_count FROM ({sql}) as subquery"
        try:
            count_df = self.client.query(count_sql, begin, end)
            estimated_rows = count_df['row_count'].iloc[0]
            print(f"📊 Estimated {estimated_rows:,} rows")
        except:
            estimated_rows = None
            print("⚠️  Could not estimate row count")
        
        # Execute with streaming for large results
        batch_count = 0
        for batch in self.client.query_stream(sql, begin, end):
            batch_start = time.time()
            df = batch.to_pandas()
            batch_rows = len(df)
            
            # Process batch
            if processor_func:
                processor_func(df, batch_count)
            
            # Update statistics
            batch_count += 1
            total_rows += batch_rows
            batch_time = time.time() - batch_start
            
            self.stats['batches_processed'] = batch_count
            self.stats['rows_processed'] = total_rows
            self.stats['avg_batch_time'] = (self.stats['avg_batch_time'] * (batch_count - 1) + batch_time) / batch_count
            
            # Progress reporting
            if estimated_rows:
                progress = (total_rows / estimated_rows) * 100
                print(f"🔄 Batch {batch_count}: {batch_rows:,} rows ({progress:.1f}% complete)")
            else:
                print(f"🔄 Batch {batch_count}: {batch_rows:,} rows ({total_rows:,} total)")
        
        self.stats['total_time'] = time.time() - start_time
        
        print(f"✅ Completed: {total_rows:,} rows in {self.stats['total_time']:.2f}s")
        print(f"   Average: {total_rows/self.stats['total_time']:.0f} rows/sec")
        
        return self.stats

# Usage example
def process_error_logs(df, batch_num):
    """Process each batch of error logs."""
    errors = df[df['level'] <= 2]  # Error and critical levels
    if not errors.empty:
        print(f"  Found {len(errors)} errors in batch {batch_num}")
        # Could save to file, send alerts, etc.

executor = OptimizedQueryExecutor(client)
end = datetime.now(timezone.utc)
begin = end - timedelta(days=7)

stats = executor.execute_large_query(
    "SELECT time, level, target, msg FROM log_entries WHERE level <= 3",
    begin, end,
    processor_func=process_error_logs
)
```

## Advanced Data Management

### Bulk Retirement of Non-Global Partitions

For advanced partition management, you can retire all non-global partitions in batch. This is useful for cleaning up process-specific materialized views:

```python
def retire_non_global_partitions(client):
    """Retire all non-global partitions across all view sets."""
    
    # Find all non-global partition instances
    sql = """
    SELECT view_set_name, view_instance_id, 
           min(begin_insert_time) as begin, 
           max(end_insert_time) as end
    FROM list_partitions()
    WHERE view_instance_id != 'global'
    GROUP BY view_set_name, view_instance_id
    """
    
    print("🔍 Finding non-global partitions...")
    df_instances = client.query(sql)
    
    if df_instances.empty:
        print("ℹ️  No non-global partitions found")
        return
    
    print(f"📊 Found {len(df_instances)} partition instances to retire")
    
    # Retire each partition instance
    retired_count = 0
    for _, row in df_instances.iterrows():
        view_set = row["view_set_name"]
        instance_id = row["view_instance_id"]
        begin_time = row["begin"]
        end_time = row["end"]
        
        print(f"🗑️  Retiring {view_set}/{instance_id} ({begin_time} to {end_time})")
        
        try:
            client.retire_partitions(view_set, instance_id, begin_time, end_time)
            retired_count += 1
        except Exception as e:
            print(f"⚠️  Failed to retire {view_set}/{instance_id}: {e}")
    
    print(f"✅ Successfully retired {retired_count}/{len(df_instances)} partition instances")

# Usage
retire_non_global_partitions(client)
```

### Process-Specific Partition Management

For more targeted partition management, you can retire partitions for specific processes:

```python
def retire_process_partitions(client, process_id, retention_days=30):
    """Retire partitions for a specific process older than retention period."""
    
    now = datetime.now(datetime.timezone.utc)
    cutoff_time = now - timedelta(days=retention_days)
    
    # Align to day boundaries for cleaner partition management
    begin = datetime.combine(cutoff_time - timedelta(days=7), datetime.time(), tzinfo=cutoff_time.tzinfo)
    end = datetime.combine(cutoff_time, datetime.time(), tzinfo=cutoff_time.tzinfo)
    
    print(f"🧹 Retiring partitions for process {process_id}")
    print(f"📅 Time range: {begin} to {end}")
    
    # Common view sets that have process-specific partitions
    view_sets = ['async_events', 'thread_spans', 'log_entries', 'measures']
    
    for view_set in view_sets:
        try:
            print(f"  Retiring {view_set} partitions...")
            client.retire_partitions(view_set, process_id, begin, end)
        except Exception as e:
            print(f"  ⚠️  Failed to retire {view_set}: {e}")

# Usage - retire partitions for specific process older than 30 days
retire_process_partitions(client, 'd35a4b40-7407-40cd-a1cf-c619124ee529', retention_days=30)
```

## Next Steps

- **[Schema Reference](schema-reference.md)** - Understand table structures and relationships
- **[Functions Reference](functions-reference.md)** - Available SQL functions and operators  
- **[Query Patterns](query-patterns.md)** - Common observability query patterns
- **[Performance Guide](performance.md)** - Query optimization techniques

For complex integration scenarios or custom tooling, consider the patterns in this guide as starting points for your specific use case.