use anyhow::Result;
use micromegas_analytics::{
    async_block_processing::AsyncBlockProcessor,
    async_events_table::{AsyncEventRecord, AsyncEventRecordBuilder, async_events_table_schema},
    lakehouse::{async_events_view::AsyncEventsView, view::View, view_factory::ViewFactory},
    scope::ScopeDesc,
};
use std::sync::Arc;

/// Create a dummy ViewFactory for testing
fn create_test_view_factory() -> Arc<ViewFactory> {
    Arc::new(ViewFactory::new(vec![]))
}

/// Test implementation of AsyncBlockProcessor for testing
struct TestAsyncProcessor {
    events: Vec<TestAsyncEvent>,
}

#[derive(Debug, Clone, PartialEq)]
struct TestAsyncEvent {
    event_type: String,
    scope_name: String,
    scope_filename: String,
    scope_target: String,
    scope_line: u32,
    timestamp: i64,
    span_id: i64,
    parent_span_id: i64,
    block_id: String,
}

impl TestAsyncProcessor {
    fn new() -> Self {
        Self { events: Vec::new() }
    }
}

impl AsyncBlockProcessor for TestAsyncProcessor {
    fn on_begin_async_scope(
        &mut self,
        block_id: &str,
        scope: ScopeDesc,
        ts: i64,
        span_id: i64,
        parent_span_id: i64,
        _depth: u32,
    ) -> Result<bool> {
        self.events.push(TestAsyncEvent {
            event_type: "begin".to_string(),
            scope_name: scope.name.to_string(),
            scope_filename: scope.filename.to_string(),
            scope_target: scope.target.to_string(),
            scope_line: scope.line,
            timestamp: ts,
            span_id,
            parent_span_id,
            block_id: block_id.to_string(),
        });
        Ok(true)
    }

    fn on_end_async_scope(
        &mut self,
        block_id: &str,
        scope: ScopeDesc,
        ts: i64,
        span_id: i64,
        parent_span_id: i64,
        _depth: u32,
    ) -> Result<bool> {
        self.events.push(TestAsyncEvent {
            event_type: "end".to_string(),
            scope_name: scope.name.to_string(),
            scope_filename: scope.filename.to_string(),
            scope_target: scope.target.to_string(),
            scope_line: scope.line,
            timestamp: ts,
            span_id,
            parent_span_id,
            block_id: block_id.to_string(),
        });
        Ok(true)
    }
}

#[test]
fn test_async_events_table_schema() {
    let schema = async_events_table_schema();

    // Verify all expected fields are present in optimized schema
    let field_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();

    assert!(field_names.contains(&"stream_id"));
    assert!(field_names.contains(&"block_id"));
    assert!(field_names.contains(&"time"));
    assert!(field_names.contains(&"event_type"));
    assert!(field_names.contains(&"span_id"));
    assert!(field_names.contains(&"parent_span_id"));
    assert!(field_names.contains(&"name"));
    assert!(field_names.contains(&"filename"));
    assert!(field_names.contains(&"target"));
    assert!(field_names.contains(&"line"));
    assert!(field_names.contains(&"depth"));

    // Verify we have the expected number of fields (optimized for high-frequency data)
    assert_eq!(field_names.len(), 11);
}

#[test]
fn test_async_event_record_builder() {
    let mut builder = AsyncEventRecordBuilder::with_capacity(2);

    // Create optimized async event records (no process fields)
    let record1 = AsyncEventRecord {
        stream_id: Arc::new("stream1".to_string()),
        block_id: Arc::new("block1".to_string()),
        time: 2000000000,
        event_type: Arc::new("begin".to_string()),
        span_id: 1,
        parent_span_id: 0,
        depth: 0,
        name: Arc::new("test_function".to_string()),
        filename: Arc::new("test.rs".to_string()),
        target: Arc::new("test_target".to_string()),
        line: 42,
    };

    let record2 = AsyncEventRecord {
        stream_id: Arc::new("stream1".to_string()),
        block_id: Arc::new("block1".to_string()),
        time: 3000000000,
        event_type: Arc::new("end".to_string()),
        span_id: 1,
        parent_span_id: 0,
        depth: 0,
        name: Arc::new("test_function".to_string()),
        filename: Arc::new("test.rs".to_string()),
        target: Arc::new("test_target".to_string()),
        line: 42,
    };

    // Test appending records
    builder.append(&record1).expect("Failed to append record1");
    builder.append(&record2).expect("Failed to append record2");

    // Test record count
    assert_eq!(builder.len(), 2);
    assert!(!builder.is_empty());

    // Test time range
    let time_range = builder.get_time_range().expect("Should have time range");
    assert_eq!(time_range.begin.timestamp_nanos_opt().unwrap(), 2000000000);
    assert_eq!(time_range.end.timestamp_nanos_opt().unwrap(), 3000000000);

    // Test building the record batch (optimized schema has 11 columns including depth)
    let batch = builder.finish().expect("Failed to build record batch");
    assert_eq!(batch.num_rows(), 2);
    assert_eq!(batch.num_columns(), 11);
}

#[test]
fn test_async_events_view_creation() {
    let process_id = uuid::Uuid::new_v4();
    let view = AsyncEventsView::new(&process_id.to_string(), create_test_view_factory())
        .expect("Failed to create view");

    assert_eq!(*view.get_view_set_name(), "async_events");
    assert_eq!(*view.get_view_instance_id(), process_id.to_string());
}

#[test]
fn test_async_events_view_global_rejection() {
    let result = AsyncEventsView::new("global", create_test_view_factory());
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("does not support global view access")
    );
}

#[test]
fn test_async_events_view_invalid_uuid() {
    let result = AsyncEventsView::new("invalid-uuid", create_test_view_factory());
    assert!(result.is_err());
}

#[test]
fn test_async_block_processor_trait() {
    let mut processor = TestAsyncProcessor::new();

    let _scope = ScopeDesc::new(
        Arc::new("test_function".to_string()),
        Arc::new("test.rs".to_string()),
        Arc::new("test_target".to_string()),
        42,
    );

    // Test begin event
    processor
        .on_begin_async_scope(
            "block123",
            ScopeDesc::new(
                Arc::new("test_function".to_string()),
                Arc::new("test.rs".to_string()),
                Arc::new("test_target".to_string()),
                42,
            ),
            1000,
            1,
            0,
            0, // depth
        )
        .expect("Failed to process begin event");

    // Test end event
    processor
        .on_end_async_scope(
            "block123",
            ScopeDesc::new(
                Arc::new("test_function".to_string()),
                Arc::new("test.rs".to_string()),
                Arc::new("test_target".to_string()),
                42,
            ),
            2000,
            1,
            0,
            0, // depth
        )
        .expect("Failed to process end event");

    // Verify events were recorded
    assert_eq!(processor.events.len(), 2);

    let begin_event = &processor.events[0];
    assert_eq!(begin_event.event_type, "begin");
    assert_eq!(begin_event.scope_name, "test_function");
    assert_eq!(begin_event.scope_filename, "test.rs");
    assert_eq!(begin_event.scope_target, "test_target");
    assert_eq!(begin_event.scope_line, 42);
    assert_eq!(begin_event.timestamp, 1000);
    assert_eq!(begin_event.span_id, 1);
    assert_eq!(begin_event.parent_span_id, 0);
    assert_eq!(begin_event.block_id, "block123");

    let end_event = &processor.events[1];
    assert_eq!(end_event.event_type, "end");
    assert_eq!(end_event.span_id, 1);
    assert_eq!(end_event.parent_span_id, 0);
}

#[test]
fn test_empty_record_builder() {
    let builder = AsyncEventRecordBuilder::with_capacity(0);

    assert_eq!(builder.len(), 0);
    assert!(builder.is_empty());
    assert!(builder.get_time_range().is_none());

    let batch = builder.finish().expect("Failed to build empty batch");
    assert_eq!(batch.num_rows(), 0);
    assert_eq!(batch.num_columns(), 11);
}

#[test]
fn test_async_events_view_schema_consistency() {
    let process_id = uuid::Uuid::new_v4();
    let view = AsyncEventsView::new(&process_id.to_string(), create_test_view_factory())
        .expect("Failed to create view");

    let view_schema = view.get_file_schema();
    let table_schema = Arc::new(async_events_table_schema());

    // Schemas should be identical
    assert_eq!(view_schema.fields().len(), table_schema.fields().len());

    for (view_field, table_field) in view_schema
        .fields()
        .iter()
        .zip(table_schema.fields().iter())
    {
        assert_eq!(view_field.name(), table_field.name());
        assert_eq!(view_field.data_type(), table_field.data_type());
        assert_eq!(view_field.is_nullable(), table_field.is_nullable());
    }
}

#[test]
fn test_scope_desc_creation() {
    let scope = ScopeDesc::new(
        Arc::new("function_name".to_string()),
        Arc::new("file.rs".to_string()),
        Arc::new("module::target".to_string()),
        123,
    );

    assert_eq!(*scope.name, "function_name");
    assert_eq!(*scope.filename, "file.rs");
    assert_eq!(*scope.target, "module::target");
    assert_eq!(scope.line, 123);
}

#[test]
fn test_async_events_high_frequency_performance() {
    // Test with high-frequency async events to validate performance optimization
    let mut builder = AsyncEventRecordBuilder::with_capacity(10000);

    // Generate many records to test performance
    for i in 0..1000 {
        let record = AsyncEventRecord {
            stream_id: Arc::new(format!("stream_{}", i % 10)),
            block_id: Arc::new(format!("block_{}", i / 100)),
            time: 1000000000 + i,
            event_type: Arc::new(if i % 2 == 0 {
                "begin".to_string()
            } else {
                "end".to_string()
            }),
            span_id: (i / 2) as i64,
            parent_span_id: if i > 0 { (i / 4) as i64 } else { 0 },
            depth: (i % 5) as u32, // Test different depth levels
            name: Arc::new(format!("async_fn_{}", i % 5)),
            filename: Arc::new(format!("src/lib_{}.rs", i % 3)),
            target: Arc::new(format!("module_{}", i % 7)),
            line: (i % 1000) as u32 + 1,
        };
        builder
            .append(&record)
            .expect(&format!("Failed to append record {}", i));
    }

    // Verify all records were added
    assert_eq!(builder.len(), 1000);
    assert!(!builder.is_empty());

    // Test time range calculation
    let time_range = builder.get_time_range().expect("Should have time range");
    assert_eq!(time_range.begin.timestamp_nanos_opt().unwrap(), 1000000000);
    assert_eq!(time_range.end.timestamp_nanos_opt().unwrap(), 1000000999);

    // Test batch creation with many records
    let batch = builder.finish().expect("Failed to build large batch");
    assert_eq!(batch.num_rows(), 1000);
    assert_eq!(batch.num_columns(), 11);
}

#[test]
fn test_async_events_cross_stream_scenarios() {
    // Test scenarios where async operations span multiple streams (threads)
    let mut builder = AsyncEventRecordBuilder::with_capacity(6);

    // Simulate async task that moves between threads
    let records = vec![
        // Task starts on stream 1
        AsyncEventRecord {
            stream_id: Arc::new("stream_001".to_string()),
            block_id: Arc::new("block_a".to_string()),
            time: 1000,
            event_type: Arc::new("begin".to_string()),
            span_id: 100,
            parent_span_id: 0,
            depth: 0,
            name: Arc::new("async_task".to_string()),
            filename: Arc::new("worker.rs".to_string()),
            target: Arc::new("worker".to_string()),
            line: 42,
        },
        // Subtask starts on stream 2 (work stealing)
        AsyncEventRecord {
            stream_id: Arc::new("stream_002".to_string()),
            block_id: Arc::new("block_b".to_string()),
            time: 1100,
            event_type: Arc::new("begin".to_string()),
            span_id: 101,
            parent_span_id: 100,
            depth: 1,
            name: Arc::new("subtask".to_string()),
            filename: Arc::new("worker.rs".to_string()),
            target: Arc::new("worker".to_string()),
            line: 55,
        },
        // Subtask ends on stream 2
        AsyncEventRecord {
            stream_id: Arc::new("stream_002".to_string()),
            block_id: Arc::new("block_c".to_string()),
            time: 1200,
            event_type: Arc::new("end".to_string()),
            span_id: 101,
            parent_span_id: 100,
            depth: 1,
            name: Arc::new("subtask".to_string()),
            filename: Arc::new("worker.rs".to_string()),
            target: Arc::new("worker".to_string()),
            line: 55,
        },
        // Main task continues on stream 1
        AsyncEventRecord {
            stream_id: Arc::new("stream_001".to_string()),
            block_id: Arc::new("block_d".to_string()),
            time: 1300,
            event_type: Arc::new("end".to_string()),
            span_id: 100,
            parent_span_id: 0,
            depth: 0,
            name: Arc::new("async_task".to_string()),
            filename: Arc::new("worker.rs".to_string()),
            target: Arc::new("worker".to_string()),
            line: 42,
        },
    ];

    for record in &records {
        builder.append(record).expect("Failed to append record");
    }

    let batch = builder
        .finish()
        .expect("Failed to build cross-stream batch");
    assert_eq!(batch.num_rows(), 4);

    // This demonstrates how cross-stream async flows are captured
    // in the process-scoped view, which is critical for async debugging
}

#[test]
fn test_async_events_view_maker_integration() {
    use micromegas_analytics::lakehouse::{
        async_events_view::AsyncEventsViewMaker, view_factory::ViewMaker,
    };

    let maker = AsyncEventsViewMaker::new(create_test_view_factory());
    let process_id = uuid::Uuid::new_v4();

    // Test valid process ID
    let view = maker
        .make_view(&process_id.to_string())
        .expect("Failed to create view with valid process ID");
    assert_eq!(*view.get_view_set_name(), "async_events");
    assert_eq!(*view.get_view_instance_id(), process_id.to_string());

    // Test global rejection
    let global_result = maker.make_view("global");
    assert!(global_result.is_err());

    // Test invalid UUID rejection
    let invalid_result = maker.make_view("not-a-uuid");
    assert!(invalid_result.is_err());
}
