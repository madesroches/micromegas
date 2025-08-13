# Async Events View Plan

## Overview
Plan for implementing a view to visualize and analyze async events in the micromegas telemetry system.

## Current Status
✅ **IMPLEMENTATION COMPLETE** - Core functionality has been implemented:
- `async_events_table.rs` - Schema and record builder
- `lakehouse/async_events_view.rs` - View implementation following process-scoped pattern
- `lakehouse/async_events_block_processor.rs` - Block processor for parsing async events
- `thread_block_processor.rs` - Extended with async event parsing functions
- View factory registration - AsyncEventsViewMaker added to default_view_factory

## Remaining Tasks

### 🎯 Next Steps (In Priority Order)

1. **📝 Add Documentation** - Add async_events view documentation to `rust/analytics/src/lakehouse/view_factory.rs`
   - Add schema documentation table following existing pattern
   - Document view instance usage with `view_instance('async_events', process_id)`
   - Add to module comments

2. **🧪 Create Test Suite** - Following patterns from existing tests:
   - Unit tests for `AsyncEventRecordBuilder` in `async_events_table.rs`
   - Integration tests for view creation and block processing
   - Cross-thread async flow validation tests
   - Mock data generation for consistent test scenarios

3. **🐍 Python Integration Tests** - Add to `python/micromegas/tests/`:
   - End-to-end async events querying tests 
   - Cross-thread async execution validation
   - Parent-child span relationship analysis
   - Duration calculation and performance analysis

4. **🔧 Validation** - Ensure implementation works correctly:
   - Format check: `cargo fmt` (required before commit)
   - Build validation: `cargo build` from rust/ directory  
   - Test validation: `cargo test` with async events tests

## Goals
- Provide visibility into async operation lifecycles
- Track async call stacks and execution flow
- Identify performance bottlenecks in async operations
- Correlate async events with their initiating context

## View Instance Keying Options

Two approaches are being considered for keying the async events view:

### Option 1: Use `process_id` (Like LogView/MetricsView)

**Pattern**: Follow `LogView` and `MetricsView` approach
- Accept `process_id` as view_instance_id (or "global" for all processes)
- Aggregate async events from **ALL streams** within the process
- Use `list_process_streams_tagged()` to find all relevant streams

**Advantages**:
- **Cross-thread visibility**: Async operations often span multiple threads within a process (task spawning, thread pool migration)
- **Complete async picture**: Shows the full async execution flow across the entire process
- **Consistency**: Matches pattern used by LogView and MetricsView
- **Global view support**: Can support "global" view across all processes
- **Simpler UX**: Users don't need to know specific stream IDs
- **Task migration**: Captures async tasks that move between threads/streams

**Disadvantages**:
- **More data**: Higher volume of events to process and display
- **Less granular**: Can't focus on a specific thread's async behavior
- **Performance**: Potentially slower queries due to larger data sets

### Option 2: Use `stream_id` (Like ThreadSpansView)

**Pattern**: Follow `ThreadSpansView` approach
- Accept `stream_id` as view_instance_id
- Show async events from **ONE specific stream** only
- Parse as UUID to identify the specific stream

**Advantages**:
- **Granular control**: Precise filtering for debugging specific threads
- **Performance**: Faster queries with less data to process
- **Consistency**: Matches `ThreadSpansView` (async events relate to spans)
- **Thread-local context**: Aligns with per-thread async call stack tracking
- **Focused debugging**: Better for isolating issues in specific execution contexts

**Disadvantages**:
- **Limited visibility**: Misses async operations that span multiple threads
- **Complex UX**: Users need to know and specify stream IDs
- **Fragmented view**: May need multiple queries to understand full async flow
- **Task boundaries**: Can't see async tasks that migrate between streams

### Key Consideration: Async Runtime Behavior

Modern async runtimes (tokio, async-std) commonly:
- Spawn tasks that execute on different threads than where they were created
- Move futures between thread pools for load balancing
- Use work-stealing schedulers that migrate tasks between threads
- Coordinate across multiple streams within the same process

This suggests **process_id** might provide more valuable insights for async debugging and analysis.

## ✅ COMPLETED: Option 1 Implementation (process_id)

**IMPLEMENTED** - Using `process_id` for view instance keying following LogView/MetricsView pattern:
- Process-scoped async event aggregation across all thread streams ✅
- Proper UUID parsing with global view rejection ✅ 
- Integration with JIT partitioning system ✅
- Uses `list_process_streams_tagged()` to find CPU streams ✅

**Implementation Details**:
- Follows ThreadSpansView pattern for UUID parsing but LogView pattern for cross-stream aggregation
- Aggregates data from **all thread streams** within a process (like LogView/MetricsView) 
- Provides complete async execution flow across the entire process

## ✅ COMPLETED Implementation

### ✅ Build Order Completed

All implementation files have been created and integrated:

1. ✅ **`rust/analytics/src/thread_block_processor.rs`** - Async parsing functions added
2. ✅ **`rust/analytics/src/async_events_table.rs`** - Schema and record builder implemented
3. ✅ **`rust/analytics/src/lakehouse/async_events_block_processor.rs`** - Block processor implemented
4. ✅ **`rust/analytics/src/lakehouse/async_events_view.rs`** - View implementation completed
5. ✅ **Integration** - Module declarations and view factory registration completed

---

### 1. thread_block_processor.rs - Async Block Parsing (Lowest Level)

Add to existing `rust/analytics/src/thread_block_processor.rs`:

```rust
/// Helper function to extract async event fields (non-named)
fn on_async_event<F>(obj: &Object, mut fun: F) -> Result<bool>
where
    F: FnMut(Arc<Object>, u64, u64, i64) -> Result<bool>,
{
    let span_id = obj.get::<u64>("span_id")?;
    let parent_span_id = obj.get::<u64>("parent_span_id")?;
    let time = obj.get::<i64>("time")?;
    let span_desc = obj.get::<Arc<Object>>("span_desc")?;
    fun(span_desc, span_id, parent_span_id, time)
}

/// Helper function to extract async named event fields  
fn on_async_named_event<F>(obj: &Object, mut fun: F) -> Result<bool>
where
    F: FnMut(Arc<Object>, Arc<String>, u64, u64, i64) -> Result<bool>,
{
    let span_id = obj.get::<u64>("span_id")?;
    let parent_span_id = obj.get::<u64>("parent_span_id")?; 
    let time = obj.get::<i64>("time")?;
    let span_location = obj.get::<Arc<Object>>("span_location")?;
    let name = obj.get::<Arc<String>>("name")?;
    fun(span_location, name, span_id, parent_span_id, time)
}

/// Trait for processing async event blocks.
pub trait AsyncBlockProcessor {
    fn on_begin_async_scope(&mut self, block_id: &str, scope: ScopeDesc, ts: i64, span_id: i64, parent_span_id: i64) -> Result<bool>;
    fn on_end_async_scope(&mut self, block_id: &str, scope: ScopeDesc, ts: i64, span_id: i64, parent_span_id: i64) -> Result<bool>;
}

/// Parses async span events from a thread event block payload.
#[span_fn]
pub fn parse_async_block_payload<Proc: AsyncBlockProcessor>(
    block_id: &str,
    object_offset: i64,
    payload: &micromegas_telemetry::block_wire_format::BlockPayload,
    stream: &micromegas_telemetry::stream_info::StreamInfo,
    processor: &mut Proc,
) -> Result<bool> {
    parse_block(stream, payload, |val| {
        if let Value::Object(obj) = val {
            match obj.type_name.as_str() {
                "BeginAsyncSpanEvent" => on_async_event(&obj, |span_desc, span_id, parent_span_id, ts| {
                    let name = span_desc.get::<Arc<String>>("name")?;
                    let filename = span_desc.get::<Arc<String>>("file")?;
                    let target = span_desc.get::<Arc<String>>("target")?;
                    let line = span_desc.get::<u32>("line")?;
                    let scope_desc = ScopeDesc::new(name, filename, target, line);
                    processor.on_begin_async_scope(block_id, scope_desc, ts, span_id as i64, parent_span_id as i64)
                })
                .with_context(|| "reading BeginAsyncSpanEvent"),
                "EndAsyncSpanEvent" => on_async_event(&obj, |span_desc, span_id, parent_span_id, ts| {
                    let name = span_desc.get::<Arc<String>>("name")?;
                    let filename = span_desc.get::<Arc<String>>("file")?;
                    let target = span_desc.get::<Arc<String>>("target")?;
                    let line = span_desc.get::<u32>("line")?;
                    let scope_desc = ScopeDesc::new(name, filename, target, line);
                    processor.on_end_async_scope(block_id, scope_desc, ts, span_id as i64, parent_span_id as i64)
                })
                .with_context(|| "reading EndAsyncSpanEvent"),
                "BeginAsyncNamedSpanEvent" => on_async_named_event(&obj, |span_location, name, span_id, parent_span_id, ts| {
                    let filename = span_location.get::<Arc<String>>("file")?;
                    let target = span_location.get::<Arc<String>>("target")?;
                    let line = span_location.get::<u32>("line")?;
                    let scope_desc = ScopeDesc::new(name, filename, target, line);
                    processor.on_begin_async_scope(block_id, scope_desc, ts, span_id as i64, parent_span_id as i64)
                })
                .with_context(|| "reading BeginAsyncNamedSpanEvent"),
                "EndAsyncNamedSpanEvent" => on_async_named_event(&obj, |span_location, name, span_id, parent_span_id, ts| {
                    let filename = span_location.get::<Arc<String>>("file")?;
                    let target = span_location.get::<Arc<String>>("target")?;
                    let line = span_location.get::<u32>("line")?;
                    let scope_desc = ScopeDesc::new(name, filename, target, line);
                    processor.on_end_async_scope(block_id, scope_desc, ts, span_id as i64, parent_span_id as i64)
                })
                .with_context(|| "reading EndAsyncNamedSpanEvent"),
                "BeginThreadSpanEvent"
                | "EndThreadSpanEvent" 
                | "BeginThreadNamedSpanEvent"
                | "EndThreadNamedSpanEvent" => {
                    // Ignore thread span events as they are not relevant for async events view
                    Ok(true)
                }
                event_type => {
                    warn!("unknown event type {}", event_type);
                    Ok(true)
                }
            }
        } else {
            Ok(true) // continue
        }
    })
}
```

---

### 2. async_events_table.rs - Schema and Record Builder (Data Layer)

Create new file `rust/analytics/src/async_events_table.rs`:

```rust
use std::sync::Arc;
use anyhow::{Context, Result};
use chrono::DateTime;
use datafusion::arrow::array::{ArrayBuilder, PrimitiveBuilder, StringDictionaryBuilder};
use datafusion::arrow::datatypes::{DataType, Field, Int16Type, Int64Type, Schema, TimeUnit, TimestampNanosecondType, UInt32Type};
use datafusion::arrow::record_batch::RecordBatch;
use crate::time::TimeRange;

/// Returns the schema for the async_events table.
pub fn get_async_events_schema() -> Schema {
    Schema::new(vec![
        Field::new("span_id", DataType::Int64, false),
        Field::new("parent_span_id", DataType::Int64, true), // nullable for root spans
        Field::new("event_type", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("time", DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())), false),
        Field::new("stream_id", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("block_id", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("name", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("target", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("filename", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("line", DataType::UInt32, false),
    ])
}

/// Data structure for a single async event record
#[derive(Debug)]
pub struct AsyncEventData {
    pub span_id: i64,
    pub parent_span_id: Option<i64>,
    pub event_type: Arc<String>,
    pub timestamp: i64,
    pub stream_id: Arc<String>,
    pub name: Arc<String>,
    pub target: Arc<String>,
    pub filename: Arc<String>,
    pub line: u32,
}

/// A builder for creating a `RecordBatch` of async events.
pub struct AsyncEventRecordBuilder {
    pub span_ids: PrimitiveBuilder<Int64Type>,
    pub parent_span_ids: PrimitiveBuilder<Int64Type>,
    pub event_types: StringDictionaryBuilder<Int16Type>,
    pub timestamps: PrimitiveBuilder<TimestampNanosecondType>,
    pub stream_ids: StringDictionaryBuilder<Int16Type>,
    pub names: StringDictionaryBuilder<Int16Type>,
    pub targets: StringDictionaryBuilder<Int16Type>,
    pub filenames: StringDictionaryBuilder<Int16Type>,
    pub lines: PrimitiveBuilder<UInt32Type>,
    min_time: Option<i64>,
    max_time: Option<i64>,
}

impl AsyncEventRecordBuilder {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            span_ids: PrimitiveBuilder::with_capacity(capacity),
            parent_span_ids: PrimitiveBuilder::with_capacity(capacity),
            event_types: StringDictionaryBuilder::with_capacity(capacity, 4096),
            timestamps: PrimitiveBuilder::with_capacity(capacity),
            stream_ids: StringDictionaryBuilder::with_capacity(capacity, 4096),
            names: StringDictionaryBuilder::with_capacity(capacity, 4096),
            targets: StringDictionaryBuilder::with_capacity(capacity, 4096),
            filenames: StringDictionaryBuilder::with_capacity(capacity, 4096),
            lines: PrimitiveBuilder::with_capacity(capacity),
            min_time: None,
            max_time: None,
        }
    }

    pub fn append_async_event(&mut self, event: AsyncEventData) -> Result<()> {
        self.span_ids.append_value(event.span_id);
        self.parent_span_ids.append_option(event.parent_span_id);
        self.event_types.append_value(event.event_type.as_ref());
        self.timestamps.append_value(event.timestamp);
        self.stream_ids.append_value(event.stream_id.as_ref());
        self.names.append_value(event.name.as_ref());
        self.targets.append_value(event.target.as_ref());
        self.filenames.append_value(event.filename.as_ref());
        self.lines.append_value(event.line);

        // Track time range
        match (self.min_time, self.max_time) {
            (None, None) => {
                self.min_time = Some(event.timestamp);
                self.max_time = Some(event.timestamp);
            }
            (Some(min), Some(max)) => {
                if event.timestamp < min {
                    self.min_time = Some(event.timestamp);
                }
                if event.timestamp > max {
                    self.max_time = Some(event.timestamp);
                }
            }
            _ => unreachable!(),
        }

        Ok(())
    }

    pub fn get_time_range(&self) -> Option<TimeRange> {
        match (self.min_time, self.max_time) {
            (Some(min), Some(max)) => {
                let min_dt = DateTime::from_timestamp_nanos(min);
                let max_dt = DateTime::from_timestamp_nanos(max);
                Some(TimeRange::new(min_dt, max_dt))
            }
            _ => None,
        }
    }

    pub fn finish(mut self) -> Result<RecordBatch> {
        let schema = get_async_events_schema();
        RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(self.span_ids.finish()),
                Arc::new(self.parent_span_ids.finish()),
                Arc::new(self.event_types.finish()),
                Arc::new(self.timestamps.finish()),
                Arc::new(self.stream_ids.finish()),
                Arc::new(self.names.finish()),
                Arc::new(self.targets.finish()),
                Arc::new(self.filenames.finish()),
                Arc::new(self.lines.finish()),
            ],
        ).with_context(|| "RecordBatch::try_new")
    }
}
```

---

### 3. async_events_view.rs - View Implementation (Highest Level)

Create new file `rust/analytics/src/lakehouse/async_events_view.rs`:

```rust

#[async_trait]
impl View for AsyncEventsView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.view_instance_id.clone()
    }

    async fn make_batch_partition_spec(
        &self,
        runtime: Arc<RuntimeEnv>,
        lake: Arc<DataLakeConnection>,
        existing_partitions: Arc<PartitionCache>,
        insert_range: TimeRange,
    ) -> Result<Arc<dyn PartitionSpec>> {
        let process = Arc::new(
            find_process(&lake.db_pool, &self.process_id)
                .await
                .with_context(|| "find_process")?
        );
        let source_data = fetch_partition_source_data(
            runtime,
            lake.clone(),
            existing_partitions,
            insert_range,
            &self.process_id,
        ).await.with_context(|| "fetch_partition_source_data")?;

        Ok(Arc::new(BlockPartitionSpec {
            view_meta: ViewMetadata {
                view_set_name: self.view_set_name.clone(),
                view_instance_id: self.view_instance_id.clone(),
                file_schema_hash: self.get_file_schema_hash(),
            },
            schema: self.get_file_schema(),
            insert_range,
            source_data,
            block_processor: Arc::new(AsyncEventBlockProcessor {}),
        }))
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![1] // Version for async events schema
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(get_async_events_schema())
    }

    async fn jit_update(
        &self,
        runtime: Arc<RuntimeEnv>,
        lake: Arc<DataLakeConnection>, 
        query_range: Option<TimeRange>,
    ) -> Result<()> {
        let process = Arc::new(
            find_process(&lake.db_pool, &self.process_id)
                .await
                .with_context(|| "find_process")?
        );
        let query_range = query_range.unwrap_or_else(|| 
            TimeRange::new(process.start_time, chrono::Utc::now())
        );

        // Find all streams for this process that might contain async events
        // We'll look for streams with "cpu" tag as async events come from thread streams
        let streams = list_process_streams_tagged(&lake.db_pool, process.process_id, "cpu")
            .await
            .with_context(|| "list_process_streams_tagged")?;
        
        let mut all_partitions = vec![];
        let blocks_view = BlocksView::new()?;
        
        for stream in streams {
            let mut partitions = generate_jit_partitions(
                &JitPartitionConfig::default(),
                runtime.clone(),
                lake.clone(),
                &blocks_view,
                &query_range,
                Arc::new(stream),
                process.clone(),
            )
            .await
            .with_context(|| "generate_jit_partitions")?;
            all_partitions.append(&mut partitions);
        }
        
        let view_meta = ViewMetadata {
            view_set_name: self.get_view_set_name(),
            view_instance_id: self.get_view_instance_id(),
            file_schema_hash: self.get_file_schema_hash(),
        };

        for part in all_partitions {
            if !is_jit_partition_up_to_date(&lake.db_pool, view_meta.clone(), &part).await? {
                write_partition_from_blocks(
                    lake.clone(),
                    view_meta.clone(),
                    self.get_file_schema(),
                    part,
                    Arc::new(AsyncEventBlockProcessor {}),
                )
                .await
                .with_context(|| "write_partition_from_blocks")?;
            }
        }
        Ok(())
    }

    fn make_time_filter(&self, begin: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Expr>> {
        Ok(vec![Expr::Between(Between::new(
            col("time").into(),
            false,
            Expr::Literal(datetime_to_scalar(begin), None).into(),
            Expr::Literal(datetime_to_scalar(end), None).into(),
        ))])
    }

    fn get_time_bounds(&self) -> Arc<dyn DataFrameTimeBounds> {
        Arc::new(NamedColumnsTimeBounds::new(
            Arc::new(String::from("time")),
            Arc::new(String::from("time")),
        ))
    }

    fn get_update_group(&self) -> Option<i32> {
        None // No daemon updates for process-specific views
    }
}
```

#### 3. AsyncEventBlockProcessor
```rust
/// A `BlockProcessor` implementation for processing async event blocks.
#[derive(Debug)]
pub struct AsyncEventBlockProcessor {}

#[async_trait]
impl BlockProcessor for AsyncEventBlockProcessor {
    async fn process(
        &self,
        blob_storage: Arc<BlobStorage>,
        src_block: Arc<PartitionSourceBlock>,
    ) -> Result<Option<PartitionRowSet>> {
        let convert_ticks = make_time_converter_from_block_meta(&src_block.process, &src_block.block)?;
        let nb_events = src_block.block.nb_objects;
        let mut record_builder = AsyncEventRecordBuilder::with_capacity(nb_events as usize);

        let mut processor = AsyncEventProcessor {
            record_builder,
            convert_ticks: Arc::new(convert_ticks),
            stream_id: Arc::new(src_block.stream.stream_id.to_string()),
        };

        let payload = fetch_block_payload(
            blob_storage,
            src_block.process.process_id,
            src_block.stream.stream_id,
            src_block.block.block_id,
        ).await?;

        let block_id_str = src_block.block.block_id
            .hyphenated()
            .encode_lower(&mut sqlx::types::uuid::Uuid::encode_buffer())
            .to_owned();

        parse_async_block_payload(
            &block_id_str,
            src_block.block.object_offset,
            &payload,
            &src_block.stream,
            &mut processor,
        )?;

        if let Some(time_range) = processor.record_builder.get_time_range() {
            let record_batch = processor.record_builder.finish()?;
            Ok(Some(PartitionRowSet {
                rows_time_range: time_range,
                rows: record_batch,
            }))
        } else {
            Ok(None)
        }
    }
}

/// Internal processor for handling async events during parsing
struct AsyncEventProcessor {
    record_builder: AsyncEventRecordBuilder,
    convert_ticks: Arc<ConvertTicks>,
    stream_id: Arc<String>,
}

impl AsyncBlockProcessor for AsyncEventProcessor {
    fn on_begin_async_scope(
        &mut self,
        scope: ScopeDesc,
        ts: i64,
        span_id: i64,
        parent_span_id: i64,
    ) -> Result<bool> {
        let time_ns = self.convert_ticks.delta_ticks_to_ns(ts);
        self.record_builder.append_async_event(AsyncEventData {
            span_id,
            parent_span_id: if parent_span_id == 0 { None } else { Some(parent_span_id) },
            event_type: Arc::new(String::from("begin")),
            timestamp: time_ns,
            stream_id: self.stream_id.clone(),
            name: scope.name.clone(),
            target: scope.target.clone(),
            filename: scope.filename.clone(),
            line: scope.line,
        })?;
        Ok(true)
    }

    fn on_end_async_scope(
        &mut self,
        scope: ScopeDesc,
        ts: i64,
        span_id: i64,
        parent_span_id: i64,
    ) -> Result<bool> {
        let time_ns = self.convert_ticks.delta_ticks_to_ns(ts);
        self.record_builder.append_async_event(AsyncEventData {
            span_id,
            parent_span_id: if parent_span_id == 0 { None } else { Some(parent_span_id) },
            event_type: Arc::new(String::from("end")),
            timestamp: time_ns,
            stream_id: self.stream_id.clone(),
            name: scope.name.clone(),
            target: scope.target.clone(),
            filename: scope.filename.clone(),
            line: scope.line,
        })?;
        Ok(true)
    }
}

/// Data structure for a single async event record
#[derive(Debug)]
struct AsyncEventData {
    span_id: i64,
    parent_span_id: Option<i64>,
    event_type: Arc<String>,
    timestamp: i64,
    stream_id: Arc<String>,
    name: Arc<String>,
    target: Arc<String>,
    filename: Arc<String>,
    line: u32,
}
```

#### 4. AsyncEventRecordBuilder
```rust
/// A builder for creating a `RecordBatch` of async events.
pub struct AsyncEventRecordBuilder {
    pub span_ids: PrimitiveBuilder<Int64Type>,
    pub parent_span_ids: PrimitiveBuilder<Int64Type>,
    pub event_types: StringDictionaryBuilder<Int16Type>, // "begin" | "end"
    pub timestamps: PrimitiveBuilder<TimestampNanosecondType>,
    pub stream_ids: StringDictionaryBuilder<Int16Type>, // stream_id as string
    pub names: StringDictionaryBuilder<Int16Type>,
    pub targets: StringDictionaryBuilder<Int16Type>,
    pub filenames: StringDictionaryBuilder<Int16Type>,
    pub lines: PrimitiveBuilder<UInt32Type>,
    min_time: Option<i64>,
    max_time: Option<i64>,
}

impl AsyncEventRecordBuilder {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            span_ids: PrimitiveBuilder::with_capacity(capacity),
            parent_span_ids: PrimitiveBuilder::with_capacity(capacity),
            event_types: StringDictionaryBuilder::with_capacity(capacity, 4096),
            timestamps: PrimitiveBuilder::with_capacity(capacity),
            stream_ids: StringDictionaryBuilder::with_capacity(capacity, 4096),
            names: StringDictionaryBuilder::with_capacity(capacity, 4096),
            targets: StringDictionaryBuilder::with_capacity(capacity, 4096),
            filenames: StringDictionaryBuilder::with_capacity(capacity, 4096),
            lines: PrimitiveBuilder::with_capacity(capacity),
            min_time: None,
            max_time: None,
        }
    }

    pub fn append_async_event(&mut self, event: AsyncEventData) -> Result<()> {
        self.span_ids.append_value(event.span_id);
        self.parent_span_ids.append_option(event.parent_span_id);
        self.event_types.append_value(event.event_type.as_ref());
        self.timestamps.append_value(event.timestamp);
        self.stream_ids.append_value(event.stream_id.as_ref());
        self.names.append_value(event.name.as_ref());
        self.targets.append_value(event.target.as_ref());
        self.filenames.append_value(event.filename.as_ref());
        self.lines.append_value(event.line);

        // Track time range
        match (self.min_time, self.max_time) {
            (None, None) => {
                self.min_time = Some(event.timestamp);
                self.max_time = Some(event.timestamp);
            }
            (Some(min), Some(max)) => {
                if event.timestamp < min {
                    self.min_time = Some(event.timestamp);
                }
                if event.timestamp > max {
                    self.max_time = Some(event.timestamp);
                }
            }
            _ => unreachable!(),
        }

        Ok(())
    }

    pub fn get_time_range(&self) -> Option<TimeRange> {
        match (self.min_time, self.max_time) {
            (Some(min), Some(max)) => {
                let min_dt = DateTime::from_timestamp_nanos(min);
                let max_dt = DateTime::from_timestamp_nanos(max);
                Some(TimeRange::new(min_dt, max_dt))
            }
            _ => None,
        }
    }

    pub fn finish(mut self) -> Result<RecordBatch> {
        let schema = get_async_events_schema();
        RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(self.span_ids.finish()),
                Arc::new(self.parent_span_ids.finish()),
                Arc::new(self.event_types.finish()),
                Arc::new(self.timestamps.finish()),
                Arc::new(self.stream_ids.finish()),
                Arc::new(self.names.finish()),
                Arc::new(self.targets.finish()),
                Arc::new(self.filenames.finish()),
                Arc::new(self.lines.finish()),
            ],
        ).with_context(|| "RecordBatch::try_new")
    }
}
```

#### 5. Schema Function
```rust
/// Returns the schema for the async_events table.
pub fn get_async_events_schema() -> Schema {
    Schema::new(vec![
        Field::new("span_id", DataType::Int64, false),
        Field::new("parent_span_id", DataType::Int64, true), // nullable for root spans
        Field::new("event_type", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("time", DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())), false),
        Field::new("stream_id", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("block_id", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("name", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("target", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("filename", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("line", DataType::UInt32, false),
    ])
}
```

### Integration with Existing Code

#### Module Structure
Add new file: `rust/analytics/src/lakehouse/async_events_view.rs`

Add module declaration to `rust/analytics/src/lakehouse/mod.rs`:
```rust
pub mod async_events_view;
```

Add public exports to `rust/analytics/src/lib.rs`:
```rust
pub use lakehouse::async_events_view::{AsyncEventsView, AsyncEventsViewMaker, AsyncEventBlockProcessor};
```

#### ViewFactory Registration
Add to `default_view_factory()` in `view_factory.rs`:
```rust
let async_events_view_maker = Arc::new(AsyncEventsViewMaker {});
factory.add_view_set(String::from("async_events"), async_events_view_maker);
```

#### ViewFactory Documentation Update
Add to the view_factory.rs module documentation:
```rust
//! ## async_events
//!
//! | field        | type                        | description                                               |
//! |------------- |-----------------------------|-----------------------------------------------------------|
//! |span_id       |Int64                        | unique async span identifier                              |
//! |parent_span_id|Int64 (nullable)             | span id of the parent async span                          |
//! |event_type    |Dictionary(Int16, Utf8)      | type of event: "begin" or "end"                           |
//! |time          |UTC Timestamp (nanoseconds)  | time when the async event occurred                        |
//! |stream_id     |Dictionary(Int16, Utf8)      | identifier of the thread stream that emitted the event    |
//! |name          |Dictionary(Int16, Utf8)      | name of the async span, usually a function name           |
//! |target        |Dictionary(Int16, Utf8)      | category or module name                                   |
//! |filename      |Dictionary(Int16, Utf8)      | name or path of the source file where the span is coded   |
//! |line          |UInt32                       | line number in the file where the span can be found       |
//!
//! ### async_events view instances
//!
//! There is no 'global' instance in the 'async_events' view set, there is therefore no implicit async_events table available.
//! Users can call the table function `view_instance('async_events', process_id)` to query the async events in all thread streams associated with the specified process_id.
//! Process-specific views are materialized just-in-time and can provide good query performance.
```

#### New Async Block Parser Function
Create a new function `parse_async_block_payload()` for async events:
- Handle `BeginAsyncSpanEvent`, `EndAsyncSpanEvent`, `BeginAsyncNamedSpanEvent`, `EndAsyncNamedSpanEvent`
- Keep `parse_thread_block_payload()` focused on sync thread events only
- Extract async span data (span_id, parent_span_id, thread context)
- Use similar pattern but with async-specific processor trait

```rust
/// Helper function to extract async event fields (non-named)
fn on_async_event<F>(obj: &Object, mut fun: F) -> Result<bool>
where
    F: FnMut(Arc<Object>, u64, u64, i64) -> Result<bool>,
{
    let span_id = obj.get::<u64>("span_id")?;
    let parent_span_id = obj.get::<u64>("parent_span_id")?;
    let time = obj.get::<i64>("time")?;
    let span_desc = obj.get::<Arc<Object>>("span_desc")?;
    fun(span_desc, span_id, parent_span_id, time)
}

/// Helper function to extract async named event fields  
fn on_async_named_event<F>(obj: &Object, mut fun: F) -> Result<bool>
where
    F: FnMut(Arc<Object>, Arc<String>, u64, u64, i64) -> Result<bool>,
{
    let span_id = obj.get::<u64>("span_id")?;
    let parent_span_id = obj.get::<u64>("parent_span_id")?; 
    let time = obj.get::<i64>("time")?;
    let span_location = obj.get::<Arc<Object>>("span_location")?;
    let name = obj.get::<Arc<String>>("name")?;
    fun(span_location, name, span_id, parent_span_id, time)
}

/// Parses async span events from a thread event block payload.
#[span_fn]
pub fn parse_async_block_payload<Proc: AsyncBlockProcessor>(
    block_id: &str,
    object_offset: i64,
    payload: &micromegas_telemetry::block_wire_format::BlockPayload,
    stream: &micromegas_telemetry::stream_info::StreamInfo,
    processor: &mut Proc,
) -> Result<bool> {
    parse_block(stream, payload, |val| {
        if let Value::Object(obj) = val {
            match obj.type_name.as_str() {
                "BeginAsyncSpanEvent" => on_async_event(&obj, |span_desc, span_id, parent_span_id, ts| {
                    let name = span_desc.get::<Arc<String>>("name")?;
                    let filename = span_desc.get::<Arc<String>>("file")?;
                    let target = span_desc.get::<Arc<String>>("target")?;
                    let line = span_desc.get::<u32>("line")?;
                    let scope_desc = ScopeDesc::new(name, filename, target, line);
                    processor.on_begin_async_scope(block_id, scope_desc, ts, span_id as i64, parent_span_id as i64)
                })
                .with_context(|| "reading BeginAsyncSpanEvent"),
                "EndAsyncSpanEvent" => on_async_event(&obj, |span_desc, span_id, parent_span_id, ts| {
                    let name = span_desc.get::<Arc<String>>("name")?;
                    let filename = span_desc.get::<Arc<String>>("file")?;
                    let target = span_desc.get::<Arc<String>>("target")?;
                    let line = span_desc.get::<u32>("line")?;
                    let scope_desc = ScopeDesc::new(name, filename, target, line);
                    processor.on_end_async_scope(block_id, scope_desc, ts, span_id as i64, parent_span_id as i64)
                })
                .with_context(|| "reading EndAsyncSpanEvent"),
                "BeginAsyncNamedSpanEvent" => on_async_named_event(&obj, |span_location, name, span_id, parent_span_id, ts| {
                    let filename = span_location.get::<Arc<String>>("file")?;
                    let target = span_location.get::<Arc<String>>("target")?;
                    let line = span_location.get::<u32>("line")?;
                    let scope_desc = ScopeDesc::new(name, filename, target, line);
                    processor.on_begin_async_scope(block_id, scope_desc, ts, span_id as i64, parent_span_id as i64)
                })
                .with_context(|| "reading BeginAsyncNamedSpanEvent"),
                "EndAsyncNamedSpanEvent" => on_async_named_event(&obj, |span_location, name, span_id, parent_span_id, ts| {
                    let filename = span_location.get::<Arc<String>>("file")?;
                    let target = span_location.get::<Arc<String>>("target")?;
                    let line = span_location.get::<u32>("line")?;
                    let scope_desc = ScopeDesc::new(name, filename, target, line);
                    processor.on_end_async_scope(block_id, scope_desc, ts, span_id as i64, parent_span_id as i64)
                })
                .with_context(|| "reading EndAsyncNamedSpanEvent"),
                "BeginThreadSpanEvent"
                | "EndThreadSpanEvent" 
                | "BeginThreadNamedSpanEvent"
                | "EndThreadNamedSpanEvent" => {
                    // Ignore thread span events as they are not relevant for async events view
                    Ok(true)
                }
                event_type => {
                    warn!("unknown event type {}", event_type);
                    Ok(true)
                }
            }
        } else {
            Ok(true) // continue
        }
    })
}

/// Trait for processing async event blocks.
pub trait AsyncBlockProcessor {
    fn on_begin_async_scope(&mut self, block_id: &str, scope: ScopeDesc, ts: i64, span_id: i64, parent_span_id: i64) -> Result<bool>;
    fn on_end_async_scope(&mut self, block_id: &str, scope: ScopeDesc, ts: i64, span_id: i64, parent_span_id: i64) -> Result<bool>;
}
```

### Data Flow

1. **Event Collection**: Async span events from all thread streams in a process
2. **Processing**: Use `list_process_streams_tagged()` to find all relevant streams
3. **Aggregation**: Collect events across streams using JIT partitions approach
4. **Schema**: Raw async events with thread context, not aggregated spans like ThreadSpansView

### Key Differences from ThreadSpansView

- **Scope**: Process-wide instead of single stream
- **Data**: Raw async events instead of constructed call tree spans  
- **Thread Context**: Include thread_id/stream_id to show cross-thread async flow
- **Event Types**: Separate begin/end events to show async lifecycle

## Unit Testing Strategy

Following patterns from existing async and analytics tests, the testing approach will include:

### 1. Async Event Generation Tests
Based on `async_span_tests.rs` patterns:

```rust
#[test]
#[serial]
fn test_async_events_view_manual_instrumentation() {
    let sink = Arc::new(InMemorySink::new());
    init_in_mem_tracing(sink.clone());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("async-events-test")
        .with_tracing_callbacks()
        .build()
        .expect("failed to build tokio runtime");

    // Create async operations across multiple threads
    runtime.block_on(async {
        let handles = (0..3).map(|i| {
            tokio::spawn(async move {
                test_async_function(i).await;
                flush_thread_buffer();
            })
        }).collect::<Vec<_>>();
        
        for handle in handles {
            handle.await.expect("Task failed");
        }
    });

    drop(runtime);
    shutdown_dispatch();

    // Validate async events were captured
    let state = sink.state.lock().expect("Failed to lock sink state");
    let total_events: usize = state.thread_blocks.iter()
        .map(|block| count_async_events(block))
        .sum();
    
    assert!(total_events > 0, "Expected async events to be captured");
    unsafe { force_uninit() };
}
```

### 2. Block Payload Parsing Tests  
Based on `log_tests.rs` and `span_tests.rs` patterns:

```rust
#[test]
#[serial]
fn test_parse_async_block_payload() {
    let sink = Arc::new(InMemorySink::new());
    init_in_mem_tracing(sink.clone());
    
    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let mut stream = ThreadStream::new(1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    // Add mock async span events
    stream.get_events_mut().push(BeginAsyncSpanEvent { /* ... */ });
    stream.get_events_mut().push(EndAsyncSpanEvent { /* ... */ });
    
    let mut block = stream.replace_block(Arc::new(ThreadBlock::new(1024, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info).unwrap();
    
    // Test parsing with AsyncEventBlockProcessor
    let mut processor = MockAsyncEventBlockProcessor::new();
    let stream_info = make_stream_info(&stream);
    let received_block: micromegas_telemetry::block_wire_format::Block = 
        ciborium::from_reader(&encoded[..]).unwrap();
        
    parse_async_block_payload("test_block", 0, &received_block.payload, &stream_info, &mut processor)
        .unwrap();
        
    assert_eq!(processor.begin_count, 1);
    assert_eq!(processor.end_count, 1);
    
    shutdown_dispatch();
    unsafe { force_uninit() };
}
```

### 3. View Integration Tests
Based on analytics test patterns:

```rust
#[tokio::test]
async fn test_async_events_view_creation() {
    let process_id = uuid::Uuid::new_v4();
    let view = AsyncEventsView::new(&process_id.to_string()).unwrap();
    
    assert_eq!(*view.get_view_set_name(), "async_events");
    assert_eq!(*view.get_view_instance_id(), process_id.to_string());
}

#[tokio::test] 
async fn test_async_events_view_maker() {
    let maker = AsyncEventsViewMaker {};
    let process_id = uuid::Uuid::new_v4();
    let view = maker.make_view(&process_id.to_string()).unwrap();
    
    assert_eq!(*view.get_view_set_name(), "async_events");
}
```

### 4. Record Builder Tests

```rust
#[test]
fn test_async_event_record_builder() {
    let mut builder = AsyncEventRecordBuilder::with_capacity(10);
    
    // Add test async event data
    builder.append_async_event(AsyncEventData {
        span_id: 1,
        parent_span_id: Some(0), 
        event_type: "begin",
        timestamp: 1000000,
        stream_id: "stream-1",
        name: "async_function",
        target: "my_module",
        filename: "test.rs",
        line: 42,
    }).unwrap();
    
    let batch = builder.finish().unwrap();
    assert_eq!(batch.num_rows(), 1);
    
    // Verify schema matches expected
    assert_eq!(batch.schema(), &get_async_events_schema());
}
```

### 5. Cross-Thread Async Flow Tests

```rust
#[test]
#[serial]
fn test_cross_thread_async_tracking() {
    let sink = Arc::new(InMemorySink::new());
    init_in_mem_tracing(sink.clone());
    
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
        
    runtime.block_on(async {
        // Spawn task that migrates across threads
        let handle = tokio::spawn(async {
            for i in 0..5 {
                tokio::task::yield_now().await; // Force potential thread migration
                async_instrumented_work(i).await;
            }
        });
        handle.await.unwrap();
    });
    
    // Validate that async events show cross-thread execution
    let events = extract_async_events_from_sink(&sink);
    let unique_streams: std::collections::HashSet<_> = events.iter()
        .map(|e| &e.stream_id)
        .collect();
        
    // Should have events from multiple threads (streams) due to work stealing
    assert!(unique_streams.len() > 1, "Expected cross-thread async execution");
}
```

### 6. Test Organization

Create new test file: `rust/analytics/tests/async_events_view_tests.rs`

**Test Dependencies**:
- `serial_test::serial` - For tests that need exclusive access to global tracing state
- `tokio-test` - For async test utilities  
- Mock data generators for consistent test scenarios
- Test database setup utilities from existing analytics tests

**Sequential Test Requirements**:
Tests that use `init_in_mem_tracing()`, `shutdown_dispatch()`, or `force_uninit()` must be marked with `#[serial]` because they modify global tracing state:
- ✅ `test_async_events_view_manual_instrumentation`
- ✅ `test_parse_async_block_payload` 
- ✅ `test_cross_thread_async_tracking`
- ❌ `test_async_event_record_builder` (pure data structure test)
- ❌ `test_async_events_view_creation` (no tracing setup)
- ❌ `test_async_events_view_maker` (no tracing setup)

**Test Coverage Areas**:
- ✅ Async event generation and capture
- ✅ Block payload parsing for async events
- ✅ View creation and factory patterns
- ✅ Record builder functionality  
- ✅ End-to-end view materialization
- ✅ Cross-thread async tracking
- ✅ Error handling and edge cases
- ✅ Performance with large async workloads

## Python Integration Tests

Following patterns from existing Python tests in `python/micromegas/tests/`, create end-to-end integration tests:

### New Test File: `python/micromegas/tests/test_async_events.py`

```python
from .test_utils import *

def test_async_events_query():
    """Test basic async events view querying"""
    # Get a process from the generator binary (which produces async spans)
    sql = """
    SELECT processes.process_id 
    FROM processes
    WHERE exe LIKE '%generator%' OR exe LIKE '%telemetry-generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)
    assert len(processes) > 0, "No generator processes found - run telemetry generator first"
    process_id = processes.iloc[0]["process_id"]
    
    # Query async events for this process
    sql = """
    SELECT span_id, parent_span_id, event_type, time, stream_id, name, target
    FROM view_instance('async_events', '{process_id}')
    ORDER BY time
    LIMIT 10;
    """.format(process_id=process_id)
    
    async_events = client.query(sql, begin, end)
    print(async_events)
    assert len(async_events) > 0, "Expected async events in results"

def test_async_events_cross_thread():
    """Test that async events show cross-thread execution"""
    sql = """
    SELECT processes.process_id 
    FROM processes
    WHERE exe LIKE '%generator%' OR exe LIKE '%telemetry-generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)
    assert len(processes) > 0, "No generator processes found - run telemetry generator first"
    process_id = processes.iloc[0]["process_id"]
    
    # Query for async events across different streams
    sql = """
    SELECT DISTINCT stream_id, COUNT(*) as event_count
    FROM view_instance('async_events', '{process_id}')
    WHERE event_type = 'begin'
    GROUP BY stream_id
    ORDER BY event_count DESC;
    """.format(process_id=process_id)
    
    stream_summary = client.query(sql, begin, end)
    print("Async events per stream:")
    print(stream_summary)
    
    # Should have events from multiple streams
    assert len(stream_summary) > 0, "Expected async events"

def test_async_events_span_relationships():
    """Test parent-child relationships in async spans"""
    sql = """
    SELECT processes.process_id 
    FROM processes
    WHERE exe LIKE '%generator%' OR exe LIKE '%telemetry-generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)
    assert len(processes) > 0, "No generator processes found - run telemetry generator first"
    process_id = processes.iloc[0]["process_id"]
    
    # Query for parent-child async span relationships
    sql = """
    SELECT parent.name as parent_name, child.name as child_name, 
           parent.span_id as parent_id, child.parent_span_id
    FROM view_instance('async_events', '{process_id}') parent
    JOIN view_instance('async_events', '{process_id}') child 
         ON parent.span_id = child.parent_span_id
    WHERE parent.event_type = 'begin' AND child.event_type = 'begin'
    LIMIT 10;
    """.format(process_id=process_id)
    
    relationships = client.query(sql, begin, end)
    print("Parent-child async span relationships:")
    print(relationships)

def test_async_events_duration_analysis():
    """Test analyzing async operation durations"""
    sql = """
    SELECT processes.process_id 
    FROM processes
    WHERE exe LIKE '%generator%' OR exe LIKE '%telemetry-generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)
    assert len(processes) > 0, "No generator processes found - run telemetry generator first"
    process_id = processes.iloc[0]["process_id"]
    
    # Calculate durations by matching begin/end events
    sql = """
    SELECT begin_events.name, begin_events.stream_id,
           (end_events.time - begin_events.time) / 1000000 as duration_ms
    FROM 
        (SELECT * FROM view_instance('async_events', '{process_id}') 
         WHERE event_type = 'begin') begin_events
    JOIN 
        (SELECT * FROM view_instance('async_events', '{process_id}') 
         WHERE event_type = 'end') end_events
    ON begin_events.span_id = end_events.span_id
    ORDER BY duration_ms DESC
    LIMIT 10;
    """.format(process_id=process_id)
    
    durations = client.query(sql, begin, end)
    print("Longest async operations:")
    print(durations)
```

### Integration Test Coverage:
- ✅ Basic async events view querying 
- ✅ Cross-thread async execution validation
- ✅ Parent-child span relationship analysis
- ✅ Duration calculation and performance analysis
- ✅ Integration with existing micromegas Python client
- ✅ Real data validation using view_instance() function

## Plan Review: Inconsistencies and Missing Items

### Inconsistencies Found

1. **Global View Support Error**: The AsyncEventsView should reject "global" view_instance_id like ThreadSpansView does, not support it like LogView. Should return UUID parsing error for non-UUID strings.
   - **✅ ADDRESSED**: Confirmed - AsyncEventsView should follow ThreadSpansView pattern exactly, rejecting global views and requiring explicit `view_instance('async_events', process_id)` calls.

2. **Schema Mismatch**: The plan shows `thread_id` as a string field, but actual async span events don't contain explicit thread information - the thread context comes from which stream/thread emitted the event.
   - **✅ ADDRESSED**: Confirmed - `thread_id` should be renamed to `stream_id` and is available from the block's metadata (which thread/stream emitted the async event).

3. **Missing Parent Span ID Context**: The actual `BeginAsyncSpanEvent`/`EndAsyncSpanEvent` structures include `parent_span_id` fields, but the plan doesn't show how these will be parsed from the event objects.
   - **✅ ADDRESSED**: Confirmed - all event fields including `parent_span_id` will be accessible to the processor for extraction.

4. **Event Parsing Function Design**: The plan suggests creating `parse_async_block_payload()` but `parse_thread_block_payload()` already handles async events (ignores them). Should extend existing function rather than duplicate.
   - **✅ ADDRESSED**: Confirmed - `parse_async_block_payload` will be a separate function that ignores thread events (like `parse_thread_block_payload` ignores async events). This maintains separation of concerns.

### Missing Items Found

1. **Missing Import Statements**: Plan doesn't specify imports needed for new structs (Arc, Result, ViewMaker trait, etc.)
   - **✅ ADDRESSED**: Confirmed - compiling will surface import errors that can be fixed incrementally during implementation.

2. **Missing Error Handling**: No mention of UUID parsing error handling in `AsyncEventsView::new()`
   - **✅ ADDRESSED**: Confirmed - will rely on anyhow for error reporting by default, following existing codebase patterns.

3. **Missing Integration Points**: 
   - No mention of adding async events to view_factory.rs documentation
   - Missing module declarations (`mod async_events_view;`)
   - No pub use statements for exposing new types
   - **✅ ADDRESSED**: 
     - All documentation should be updated during implementation
     - Module declarations will be added to the plan
     - Most structs should be public by default since we're building an extensible library

4. **Missing Async Event Parsing Logic**: Plan shows trait signatures but not actual parsing logic for extracting span_id, parent_span_id from event objects
   - **✅ ADDRESSED**: Will add detailed parsing logic implementation to the plan showing how to extract all fields from async event objects.

5. **Missing Test Dependencies**: Plan doesn't mention test utilities needed (`make_process_info`, `make_stream_info` functions)
   - **✅ ADDRESSED**: Will handle test dependencies as needed during implementation - any utility we need we may add along the way.

6. **Missing View Implementation**: Plan shows struct but not actual `View` trait implementation with `get_schema()`, `get_record_batches()`, etc.
   - **✅ ADDRESSED**: Will add complete View trait implementation following LogView pattern for process-scoped aggregation.

7. **Missing JIT Partition Logic**: No mention of how view will use JIT partitioning system like other views
   - **✅ ADDRESSED**: Added complete JIT partition logic in View implementation using generate_jit_partitions() and write_partition_from_blocks().

8. **Missing Stream Filtering**: Plan doesn't address filtering streams that contain async events vs other stream types
   - **✅ ADDRESSED**: Uses `list_process_streams_tagged(&lake.db_pool, process.process_id, "cpu")` to find thread streams that contain async events.

### Corrections Completed

- ✅ Follow ThreadSpansView pattern exactly for UUID-only parsing - Added in AsyncEventsView::new()
- ✅ Create separate `parse_async_block_payload` function with proper separation of concerns
- ✅ Add complete View trait implementation following LogView pattern for process-scoped aggregation  
- ✅ Include module structure and integration points with public exports
- ✅ Show detailed async event object parsing logic with helper functions
- ✅ Address stream filtering using "cpu" tag and complete JIT partition integration
- ✅ Add AsyncEventBlockProcessor implementing BlockProcessor trait
- ✅ Add complete AsyncEventRecordBuilder with time tracking and Arrow integration

## Future Enhancements

### Async Event Context Support

After the initial implementation is complete and working, consider adding support for tagging async events with context information:

#### Context as String Key-Value Pairs

**Motivation**: Async operations often carry contextual information that would be valuable for analysis:
- Request IDs for tracing distributed operations
- User IDs for user-specific performance analysis  
- Feature flags or experiment identifiers
- Custom application-specific metadata

#### Implementation Approach

When implementing context support in the future:
- Extend async span events to include optional context field
- Update parsing logic to extract context from event objects  
- Maintain backward compatibility with existing events
- Design schema to support efficient querying of context dimensions

#### Benefits

- **Enhanced Analytics**: Query async events by context dimensions
- **Distributed Tracing**: Connect async operations across service boundaries
- **Performance Analysis**: Correlate performance with business context
- **Debugging**: Add runtime context for better error investigation

#### Example Use Cases

- **User-specific Performance**: Track async operation performance per user
- **Feature Flag Analysis**: Correlate async behavior with feature rollouts  
- **Request Tracing**: Connect async operations across service boundaries
- **A/B Testing**: Analyze async performance across experiment variants
- **Custom Metadata**: Add application-specific context for debugging

#### Implementation Notes

- **Performance**: Context extraction should be optional to avoid overhead when not needed
- **Schema Evolution**: Design for backward compatibility as context fields evolve
- **Memory Usage**: Consider impact of storing variable-length context data
- **Querying**: Plan for efficient access patterns for context dimensions

This enhancement would significantly increase the analytical value of async events data while maintaining the simplicity of the core implementation.

