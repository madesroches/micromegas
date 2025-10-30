# Plan: Detailed Async Span Tracing for Slow Queries

## Overview
Add comprehensive async span tracing to capture all significant (≥5ms) operations during slow queries (>1 second) in the FlightSQL server. This will provide granular visibility into where time is spent during complex query execution.

## Current State Analysis
### ✅ What's Working
- **Basic Async Tracing**: 4 hierarchical spans implemented:
  - `flight_sql_execute_query` (top-level)
  - `make_session_context` (session setup)
  - `flight_sql_query_planning` (SQL parsing/planning)  
  - `flight_sql_query_execution` (DataFusion execution)
- **CPU Stream Generation**: FlightSQL server properly generates CPU blocks
- **Telemetry Infrastructure**: Async events appear in `view_instance('async_events', process_id)`

### 🔍 What's Missing
- **DataFusion Internal Operations**: No visibility into DataFusion's query execution phases
- **I/O Operations**: Block fetching, partition reading, object storage access
- **Parquet Processing**: File reading, decompression, column projection
- **Memory Operations**: Buffer allocation, data transformation
- **Network Operations**: Flight data streaming, response encoding

## Target: Comprehensive Operation Visibility

### 1. Lakehouse Query Execution Phases
**Primary Location**: `rust/analytics/src/lakehouse/` module

**Key Execution Components to Instrument**:

#### A. Partition Resolution (`partition_cache.rs`)
- **`fetch()`**: Database query to fetch matching partitions from `lakehouse_partitions` table
- **Partition Metadata Parsing**: Converting blob metadata to `ParquetMetaData` objects
- **Time Range Filtering**: Filtering partitions by `query_range` bounds

#### B. Physical Plan Creation (`partitioned_execution_plan.rs`)
- **`make_partitioned_execution_plan()`**: Building DataFusion execution plan
- **File Group Assembly**: Creating `PartitionedFile` objects from partition metadata
- **Predicate Pushdown**: Converting filters to parquet-level predicates
- **ReaderFactory Setup**: Custom parquet reader with pre-loaded metadata

#### C. View Instance Resolution (`view_instance_table_function.rs`)
- **View Factory Lookups**: Resolving view names to table providers
- **Materialized View Access**: Checking for cached materialized views
- **Schema Resolution**: Building combined schemas from multiple partitions

**Implementation Strategy**:
```rust
// Preferred: Use #[span_fn] proc macro for functions in our crates
#[span_fn]
async fn partition_fetch_operation() -> Result<T> {
    // Proc macro automatically creates spans with function name
    let result = actual_work().await?;
    Ok(result)
}

#[span_fn] 
pub fn process_log_block(block: &Block) -> Result<PartitionRowSet> {
    // Proc macro works for both async and sync functions
    let result = process_block_data(block)?;
    Ok(result)
}

// Manual instrumentation only when proc macro can't be used
async fn complex_operation() -> Result<T> {
    let result = some_operation()
        .instrument(span!("detailed_operation_name"))
        .await?;
    Ok(result)
}
```

### 2. Lakehouse Data Processing Operations

#### A. Block Processing (`*_block_processor.rs`)
- **`LogBlockProcessor::process()`**: Processing log blocks into Arrow format
- **`MetricsBlockProcessor::process()`**: Processing metrics blocks  
- **`AsyncEventsBlockProcessor::process()`**: Processing async span events
- **Block Decompression**: Uncompressing transit-encoded payloads
- **Arrow Record Building**: Converting micromegas events to Arrow records

#### B. Partition Merging (`batch_partition_merger.rs`)
- **`BatchPartitionMerger::merge()`**: Merging multiple partitions into one
- **Batch Time Splitting**: Splitting large time ranges for memory efficiency
- **DataFusion Stream Processing**: Executing queries across partition batches
- **Record Batch Collection**: Collecting and combining results

#### C. Parquet I/O (`reader_factory.rs`)
- **`ReaderFactory::create_reader()`**: Creating custom parquet readers
- **Metadata Lookup**: Finding pre-loaded parquet metadata
- **Object Store Access**: Reading parquet files from cloud storage
- **Column Pruning**: Selecting only needed columns during read

**Proc Macro Instrumentation Pattern**:
```rust
#[span_fn]
async fn create_parquet_reader(path: &str) -> Result<ParquetReader> {
    // Proc macro automatically handles span creation and cleanup
    // Function name becomes span name: "create_parquet_reader"
    let result = reader_creation_logic().await?;
    Ok(result)
}

#[span_fn]
pub fn find_parquet_metadata(filename: &str, domain: &[Partition]) -> Result<Arc<ParquetMetaData>> {
    // Works for sync functions too
    for part in domain {
        if part.file_path == filename {
            return Ok(part.file_metadata.clone());
        }
    }
    anyhow::bail!("file not found {}", filename)
}
```

### 3. Network and Streaming Operations  
**Location**: `rust/public/src/servers/flight_sql_service_impl.rs`

**Operations to Instrument**:
- **Flight Data Encoding**: Converting Arrow to Flight format (if we control this code)
- **Stream Buffering**: Batching records for network efficiency (if we control this code)
- **Response Assembly**: Building flight responses from query results

## Implementation Strategy

### Phase 1: Comprehensive Instrumentation
1. **Focus on Micromegas-Controlled Code**:
   - Custom lakehouse operations in all modules
   - Block processing pipelines
   - Partition management operations
   - View resolution and materialization

2. **Proc Macro First Approach**:
   - Use `#[span_fn]` proc macro for all functions in micromegas crates
   - Proc macro automatically creates spans with function names
   - Fall back to manual instrumentation only when proc macro can't be applied
   - Monitor actual performance data to identify insignificant spans

3. **Span Hierarchy Design**:
   ```
   flight_sql_execute_query (134ms)
   ├── make_session_context (5ms)
   ├── flight_sql_query_planning (0ms)
   └── flight_sql_query_execution (128ms)
       ├── lakehouse_partition_fetch (45ms)
       ├── view_instance_resolution (12ms) 
       ├── partitioned_execution_plan_creation (8ms)
       ├── block_processing_operations (89ms)
       │   ├── log_block_processor_batch_1 (23ms)
       │   ├── metrics_block_processor_batch_2 (31ms)
       │   └── parquet_reader_factory_operations (35ms)
       └── batch_partition_merger (19ms)
   ```

### Phase 2: Performance Analysis and Optimization
1. **Comprehensive Tracing Data Collection**: Instrument all operations initially
2. **Data-Driven Analysis**: Use actual telemetry to identify operation durations
3. **Context Propagation**: Maintain parent-child relationships across async boundaries
4. **Resource Tracking**: Include relevant context (file sizes, row counts, memory usage)

### Phase 3: Span Pruning and Refinement
1. **Remove Insignificant Spans**: Remove instrumentation from consistently fast operations
2. **Optimize High-Frequency Operations**: Reduce instrumentation overhead where needed
3. **Cross-Service Spans**: If queries involve multiple services
4. **Error Context**: Enhanced spans for failed operations

## Technical Implementation Details

### Instrumentation Locations

#### Primary Targets (High Impact)
1. **`rust/analytics/src/lakehouse/partition_cache.rs`**:
   - Add `#[span_fn]` to `QueryPartitionProvider::fetch()` - Database query for partitions
   - Add `#[span_fn]` to `PartitionCache::fetch_overlapping_insert_range()` - Complex partition filtering

2. **`rust/analytics/src/lakehouse/partitioned_execution_plan.rs`**:
   - Add `#[span_fn]` to `make_partitioned_execution_plan()` - DataFusion plan creation

3. **`rust/analytics/src/lakehouse/view_instance_table_function.rs`**:
   - Add `#[span_fn]` to `ViewInstanceTableFunction::call()` - View resolution

4. **`rust/analytics/src/lakehouse/batch_partition_merger.rs`**:
   - Add `#[span_fn]` to `BatchPartitionMerger::merge()` - Multi-partition merging operations
   - Add `#[span_fn]` to time range splitting helper functions

#### Secondary Targets (Medium Impact)  
5. **`rust/analytics/src/lakehouse/*_block_processor.rs`**:
   - Add `#[span_fn]` to `BlockProcessor::process()` implementations for each block type
   - Add `#[span_fn]` to Arrow record building helper functions

6. **`rust/analytics/src/lakehouse/reader_factory.rs`**:
   - Add `#[span_fn]` to `ReaderFactory::create_reader()` - Custom parquet reader creation
   - Add `#[span_fn]` to `find_parquet_metadata()` - Metadata lookup

7. **`rust/analytics/src/lakehouse/materialized_view.rs`**:
   - Add `#[span_fn]` to materialized view lookup and cache operations

### Performance Considerations
- **Proc Macro Efficiency**: `#[span_fn]` has minimal overhead compared to manual instrumentation
- **Automatic Span Naming**: Function names become span names, no need for custom naming
- **Existing Infrastructure**: Leverages existing micromegas tracing infrastructure
- **Data-Driven Optimization**: Use actual telemetry data to guide span removal decisions
- **Iterative Refinement**: Remove `#[span_fn]` annotations from functions that consistently show minimal duration

### Validation Strategy
1. **Slow Query Testing**: Execute queries with >1 second duration
2. **Comprehensive Span Verification**: Confirm all instrumented operations appear in async_events
3. **Performance Impact Measurement**: Measure overhead of comprehensive instrumentation
4. **Hierarchy Validation**: Verify parent-child span relationships
5. **Data Collection**: Gather duration statistics for all instrumented operations

## Expected Outcomes

### For Slow Queries (>1 second)
- **Complete Operation Breakdown**: All instrumented operations visible in traces
- **Root Cause Analysis**: Identify specific bottlenecks (I/O vs CPU vs memory)
- **Optimization Targets**: Clear data on where time is spent
- **Span Duration Analytics**: Data-driven insights on which spans to keep or remove

### Example Slow Query Trace
```
🌟 flight_sql_execute_query (2,847ms)
├── 📄 make_session_context (12ms)
├── 📄 flight_sql_query_planning (3ms)
└── 📁 flight_sql_query_execution (2,832ms)
    ├── 📄 lakehouse_partition_cache_fetch (156ms)
    ├── 📄 view_instance_table_function_call (23ms)
    ├── 📁 block_processing_pipeline (2,489ms)
    │   ├── 📄 log_block_processor_process_batch_1 (234ms)
    │   ├── 📄 metrics_block_processor_process_batch_2 (187ms)
    │   ├── 📄 async_events_block_processor_process (291ms)
    │   ├── 📄 parquet_reader_factory_create_reader (456ms)
    │   └── 📄 partition_source_data_processing (721ms)
    ├── 📄 batch_partition_merger_merge (89ms)
    └── 📄 materialized_view_lookup (75ms)
```

### Performance Monitoring
- **All Queries**: Comprehensive span data for analysis
- **Slow Queries**: Complete visibility into execution phases
- **Fast Queries**: Data collection to identify consistently fast operations for span removal

## Implementation Priority
1. **Phase 1**: Lakehouse partition and execution operations (highest impact)
2. **Phase 2**: Block processing and data transformation operations
3. **Phase 3**: I/O and storage operations
4. **Phase 4**: Analysis and span pruning based on collected data

## Development Workflow

### Service Management
**Start Services** (from project root):
```bash
python3 local_test_env/ai_scripts/start_services.py
```
- Starts PostgreSQL, telemetry-ingestion-srv (port 9000), flight-sql-srv (port 50051), and telemetry-admin
- Services run in background with logs in `/tmp/`
- PIDs saved to `/tmp/micromegas_pids.txt`

**Stop Services**:
```bash
python3 local_test_env/ai_scripts/stop_services.py
```
- Stops all services and cleans up log files

**Monitor Service Logs**:
```bash
# Ingestion server logs
tail -f /tmp/ingestion.log

# Analytics server (FlightSQL) logs  
tail -f /tmp/analytics.log

# Admin daemon logs
tail -f /tmp/admin.log
```

### Testing and Validation

**Run Python Queries** (from project root):
```bash
cd python/micromegas && source test_venv/bin/activate

# Query CPU blocks to verify async spans are working
python3 -c "
import micromegas
client = micromegas.connect()

# Check for FlightSQL CPU blocks
sql = \"SELECT COUNT(*) as cpu_block_count FROM blocks WHERE array_has(\\\"streams.tags\\\", 'cpu') AND \\\"processes.exe\\\" LIKE '%flight-sql-srv%';\"
result = client.query(sql)
print(f'FlightSQL CPU blocks: {result.iloc[0][\"cpu_block_count\"]}')

# Check async events for latest FlightSQL process
sql = \"SELECT process_id FROM blocks WHERE \\\"processes.exe\\\" LIKE '%flight-sql-srv%' ORDER BY insert_time DESC LIMIT 1;\"
processes = client.query(sql)
if len(processes) > 0:
    process_id = processes.iloc[0]['process_id']
    sql = f\"SELECT COUNT(*) as event_count FROM view_instance('async_events', '{process_id}');\"
    events = client.query(sql)
    print(f'Async events: {events.iloc[0][\"event_count\"]}')
"
```

**Test Slow Query for Span Analysis**:
```bash
# Execute a complex query to generate slow spans
python3 -c "
import micromegas
client = micromegas.connect()

# Complex query that should generate multiple spans
sql = \"\"\"
SELECT processes.exe, COUNT(*) as block_count, AVG(nb_objects) as avg_objects
FROM blocks 
WHERE insert_time >= NOW() - INTERVAL '1 hour'
GROUP BY processes.exe
ORDER BY block_count DESC;
\"\"\"
result = client.query(sql)
print('Query executed - check async_events for new spans')
print(result)
"
```

**Analyze Async Span Call Trees**:
```bash
# Generate call tree visualization
python3 -c "
import micromegas
import pandas as pd

client = micromegas.connect()

# Get latest FlightSQL process
sql = \"SELECT process_id FROM blocks WHERE \\\"processes.exe\\\" LIKE '%flight-sql-srv%' ORDER BY insert_time DESC LIMIT 1;\"
processes = client.query(sql)
process_id = processes.iloc[0]['process_id']

# Get async spans with durations
sql = f\"\"\"
SELECT 
    begin_events.span_id,
    begin_events.parent_span_id,
    begin_events.name,
    begin_events.target,
    begin_events.time as begin_time,
    end_events.time as end_time,
    CAST((end_events.time - begin_events.time) AS BIGINT) / 1000000 as duration_ms
FROM 
    (SELECT * FROM view_instance('async_events', '{process_id}') WHERE event_type = 'begin') begin_events
LEFT JOIN 
    (SELECT * FROM view_instance('async_events', '{process_id}') WHERE event_type = 'end') end_events
ON begin_events.span_id = end_events.span_id
ORDER BY begin_events.time DESC
LIMIT 20;
\"\"\"

spans = client.query(sql)
print(f'Found {len(spans)} async spans for analysis')
print(spans[['name', 'duration_ms', 'target']])
"
```

### Development Cycle

1. **Add Instrumentation**: Add `#[span_fn]` to target functions
2. **Rebuild**: `cd rust && cargo build`
3. **Restart Services**: `python3 local_test_env/ai_scripts/stop_services.py && python3 local_test_env/ai_scripts/start_services.py`
4. **Test Queries**: Run slow queries using Python client
5. **Analyze Results**: Query `async_events` to see new spans
6. **Iterate**: Remove spans that prove insignificant, add more where needed

## TODO: Comprehensive Test Suite

Design and implement a comprehensive test suite that exercises all instrumented code paths to ensure complete performance visibility:

1. **Validate Async Trait Instrumentation**:
   - Verify that `#[span_fn]` works correctly on async trait methods
   - Test that spans are properly created for trait implementations like `BlockProcessor::process`
   - Ensure parent-child span relationships work across trait boundaries
   - Validate that async trait methods in `QueryPartitionProvider` generate correct spans

2. **BlockProcessor Implementations**:
   - Test `LogBlockProcessor::process` with various log block scenarios
   - Test `MetricsBlockProcessor::process` with metrics data
   - Test `AsyncEventsBlockProcessor::process` with async event streams
   
3. **Partition Operations**:
   - Test `BatchPartitionMerger::execute_merge_query` with different merge scenarios
   - Exercise all partition cache paths including edge cases
   
4. **Query Execution Paths**:
   - Test queries that trigger `MaterializedView::scan`
   - Test queries using `ViewInstanceTableFunction::call`
   - Exercise `make_session_context` with various session configurations
   
5. **Data Lake Operations**:
   - Test scenarios that exercise reader factory operations
   - Test partitioned execution plan creation
   
6. **Performance Scenarios**:
   - Create queries that generate deep call stacks
   - Test concurrent query execution
   - Test cache miss scenarios
   - Test large dataset processing

The test suite should generate comprehensive async event data covering all instrumented functions to validate performance characteristics and identify optimization opportunities.

This plan will provide comprehensive observability for all queries initially, then use actual performance data to optimize instrumentation by removing spans that don't provide significant value.