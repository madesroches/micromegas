use micromegas_perfetto::{async_writer::AsyncWriter, streaming_writer::PerfettoWriter};
use prost::Message;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

/// Mock AsyncWriter that stores written data in a buffer for testing
struct MockAsyncWriter {
    buffer: Vec<u8>,
    flush_calls: usize,
}

impl MockAsyncWriter {
    fn new() -> Self {
        Self {
            buffer: Vec::new(),
            flush_calls: 0,
        }
    }

    #[allow(dead_code)]
    fn into_buffer(self) -> Vec<u8> {
        self.buffer
    }

    #[allow(dead_code)]
    fn flush_call_count(&self) -> usize {
        self.flush_calls
    }
}

#[async_trait::async_trait]
impl AsyncWriter for MockAsyncWriter {
    async fn write(&mut self, buf: &[u8]) -> anyhow::Result<()> {
        self.buffer.extend_from_slice(buf);
        Ok(())
    }

    async fn flush(&mut self) -> anyhow::Result<()> {
        self.flush_calls += 1;
        Ok(())
    }
}

#[tokio::test]
async fn test_async_streaming_writer_basic_usage() -> anyhow::Result<()> {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let shared_writer = SharedBufferAsyncWriter::new(buffer.clone());
    let mut streaming_writer = PerfettoWriter::new(Box::new(shared_writer), "test_process");

    // Emit process descriptor
    streaming_writer.emit_process_descriptor("test.exe").await?;

    // Emit thread descriptor
    streaming_writer
        .emit_thread_descriptor("thread_1", 1234, "main")
        .await?;

    // Emit a span
    streaming_writer
        .emit_span(1000000, 2000000, "test_span", "test_target", "test.rs", 42)
        .await?;

    streaming_writer.flush().await?;

    // Verify we have written some data
    let written_data = buffer.lock().unwrap();
    assert!(
        !written_data.is_empty(),
        "Buffer should not be empty after writing"
    );

    // Verify the data is a valid protobuf trace
    let trace =
        micromegas_perfetto::protos::Trace::decode(&written_data[..]).expect("decode trace");
    assert!(!trace.packet.is_empty(), "Trace should contain packets");

    // Should have process descriptor + thread descriptor + span begin + span end = 4 packets
    assert_eq!(trace.packet.len(), 4, "Should have 4 packets");

    Ok(())
}

#[tokio::test]
async fn test_async_streaming_writer_packet_framing() -> anyhow::Result<()> {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let shared_writer = SharedBufferAsyncWriter::new(buffer.clone());
    let mut streaming_writer = PerfettoWriter::new(Box::new(shared_writer), "test_process");

    // Emit a simple process descriptor
    streaming_writer.emit_process_descriptor("test.exe").await?;

    streaming_writer.flush().await?;

    // Manually verify the packet framing
    let written_data = buffer.lock().unwrap();
    let mut cursor = Cursor::new(&*written_data);

    // First byte should be the field key (0x0A for field 1, wire type 2)
    let mut key_buf = [0u8; 1];
    std::io::Read::read_exact(&mut cursor, &mut key_buf).expect("read key");
    assert_eq!(key_buf[0] & 0xF8, 0x08, "Field number should be 1");
    assert_eq!(
        key_buf[0] & 0x07,
        0x02,
        "Wire type should be 2 (length-delimited)"
    );

    // Verify we can decode the full trace
    let trace =
        micromegas_perfetto::protos::Trace::decode(&written_data[..]).expect("decode trace");
    assert_eq!(trace.packet.len(), 1, "Should have exactly 1 packet");

    Ok(())
}

/// AsyncWriter implementation that writes to a shared buffer for testing
struct SharedBufferAsyncWriter {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl SharedBufferAsyncWriter {
    fn new(buffer: Arc<Mutex<Vec<u8>>>) -> Self {
        Self { buffer }
    }
}

#[async_trait::async_trait]
impl AsyncWriter for SharedBufferAsyncWriter {
    async fn write(&mut self, buf: &[u8]) -> anyhow::Result<()> {
        self.buffer.lock().unwrap().extend_from_slice(buf);
        Ok(())
    }

    async fn flush(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn test_async_streaming_writer_async_track_creation() -> anyhow::Result<()> {
    let mock_writer = MockAsyncWriter::new();
    let mut streaming_writer = PerfettoWriter::new(Box::new(mock_writer), "test_process");

    // Should be able to call emit_async_track_descriptor multiple times idempotently
    streaming_writer.emit_async_track_descriptor().await?;
    streaming_writer.emit_async_track_descriptor().await?;
    streaming_writer.emit_async_track_descriptor().await?;

    Ok(())
}

#[tokio::test]
async fn test_async_streaming_writer_async_span_events() -> anyhow::Result<()> {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let shared_writer = SharedBufferAsyncWriter::new(buffer.clone());
    let mut streaming_writer = PerfettoWriter::new(Box::new(shared_writer), "test_process");

    // Setup required descriptors
    streaming_writer.emit_process_descriptor("test.exe").await?;
    streaming_writer.emit_async_track_descriptor().await?;

    // Emit async span events
    streaming_writer
        .emit_async_span_begin(1000000, "async_task", "async_target", "async.rs", 10)
        .await?;
    streaming_writer
        .emit_async_span_end(2000000, "async_task", "async_target", "async.rs", 10)
        .await?;

    streaming_writer.flush().await?;

    // Verify the trace is valid
    let written_data = buffer.lock().unwrap();
    let trace =
        micromegas_perfetto::protos::Trace::decode(&written_data[..]).expect("decode trace");
    assert_eq!(
        trace.packet.len(),
        4,
        "Should have 4 packets: process + async track + begin + end"
    );

    // Verify the span events have correct timestamps
    let begin_packet = &trace.packet[2];
    let end_packet = &trace.packet[3];

    assert_eq!(begin_packet.timestamp, Some(1000000));
    assert_eq!(end_packet.timestamp, Some(2000000));

    // Both should be track events
    if let Some(micromegas_perfetto::protos::trace_packet::Data::TrackEvent(begin_event)) =
        &begin_packet.data
    {
        assert_eq!(
            begin_event.r#type,
            Some(micromegas_perfetto::protos::track_event::Type::SliceBegin.into())
        );
    } else {
        panic!("Begin packet should contain a TrackEvent");
    }

    if let Some(micromegas_perfetto::protos::trace_packet::Data::TrackEvent(end_event)) =
        &end_packet.data
    {
        assert_eq!(
            end_event.r#type,
            Some(micromegas_perfetto::protos::track_event::Type::SliceEnd.into())
        );
    } else {
        panic!("End packet should contain a TrackEvent");
    }

    Ok(())
}

#[tokio::test]
async fn test_async_streaming_writer_error_handling() -> anyhow::Result<()> {
    // Test with a writer that fails
    let failing_writer = FailingAsyncWriter::new();
    let mut streaming_writer = PerfettoWriter::new(Box::new(failing_writer), "test_process");

    // Should propagate the error
    let result = streaming_writer.emit_process_descriptor("test.exe").await;
    assert!(result.is_err());

    Ok(())
}

/// AsyncWriter implementation that always fails for testing error handling
struct FailingAsyncWriter;

impl FailingAsyncWriter {
    fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl AsyncWriter for FailingAsyncWriter {
    async fn write(&mut self, _buf: &[u8]) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("Mock write failure"))
    }

    async fn flush(&mut self) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("Mock flush failure"))
    }
}

#[tokio::test]
#[should_panic(expected = "Must call emit_async_track_descriptor")]
async fn test_async_streaming_writer_async_span_without_track() {
    let mock_writer = MockAsyncWriter::new();
    let mut streaming_writer = PerfettoWriter::new(Box::new(mock_writer), "test_process");

    // Should panic when trying to emit async span without creating async track first
    streaming_writer
        .emit_async_span_begin(1000000, "async_task", "async_target", "async.rs", 10)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_async_track_creation_idempotent() -> anyhow::Result<()> {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let shared_writer = SharedBufferAsyncWriter::new(buffer.clone());
    let mut streaming_writer = PerfettoWriter::new(Box::new(shared_writer), "test_process");

    // Multiple calls should not create multiple tracks
    streaming_writer.emit_async_track_descriptor().await?;
    streaming_writer.emit_async_track_descriptor().await?;
    streaming_writer.emit_async_track_descriptor().await?;

    streaming_writer.flush().await?;

    // Should only have one async track descriptor packet (no duplicates)
    let written_data = buffer.lock().unwrap();
    let trace =
        micromegas_perfetto::protos::Trace::decode(&written_data[..]).expect("decode trace");
    assert_eq!(
        trace.packet.len(),
        1,
        "Should have exactly 1 async track descriptor packet"
    );

    Ok(())
}

#[tokio::test]
async fn test_async_streaming_writer_interning() -> anyhow::Result<()> {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let shared_writer = SharedBufferAsyncWriter::new(buffer.clone());
    let mut streaming_writer = PerfettoWriter::new(Box::new(shared_writer), "test_process");

    // Setup descriptors
    streaming_writer.emit_process_descriptor("test.exe").await?;
    streaming_writer
        .emit_thread_descriptor("thread_1", 1234, "main")
        .await?;

    // Emit multiple spans with the same name and category - should be interned
    streaming_writer
        .emit_span(
            1000000,
            2000000,
            "repeated_name",
            "repeated_category",
            "test.rs",
            42,
        )
        .await?;
    streaming_writer
        .emit_span(
            3000000,
            4000000,
            "repeated_name",
            "repeated_category",
            "test.rs",
            42,
        )
        .await?;
    streaming_writer
        .emit_span(
            5000000,
            6000000,
            "repeated_name",
            "repeated_category",
            "test.rs",
            42,
        )
        .await?;

    streaming_writer.flush().await?;

    let written_data = buffer.lock().unwrap();
    let trace =
        micromegas_perfetto::protos::Trace::decode(&written_data[..]).expect("decode trace");

    // Find a packet with interned data
    let interned_packet = trace
        .packet
        .iter()
        .find(|packet| packet.interned_data.is_some())
        .expect("Should have at least one packet with interned data");

    let interned_data = interned_packet.interned_data.as_ref().unwrap();

    // Should have interned the repeated name and category
    assert!(
        !interned_data.event_names.is_empty(),
        "Should have interned event names"
    );
    assert!(
        !interned_data.event_categories.is_empty(),
        "Should have interned event categories"
    );

    // Should have exactly one entry for each repeated string
    assert_eq!(
        interned_data.event_names.len(),
        1,
        "Should have exactly one interned name"
    );
    assert_eq!(
        interned_data.event_categories.len(),
        1,
        "Should have exactly one interned category"
    );

    Ok(())
}

#[tokio::test]
async fn test_async_streaming_writer_memory_usage() -> anyhow::Result<()> {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let shared_writer = SharedBufferAsyncWriter::new(buffer.clone());
    let mut streaming_writer = PerfettoWriter::new(Box::new(shared_writer), "test_process");

    // Setup descriptors
    streaming_writer.emit_process_descriptor("test.exe").await?;
    streaming_writer
        .emit_thread_descriptor("thread_1", 1234, "main")
        .await?;

    // Emit many spans to test memory usage doesn't grow unbounded
    for i in 0..1000 {
        let start_time = i * 1000;
        let end_time = start_time + 500;
        streaming_writer
            .emit_span(
                start_time,
                end_time,
                &format!("span_{}", i),
                "test_category",
                "test.rs",
                42,
            )
            .await?;
    }

    streaming_writer.flush().await?;

    let written_data = buffer.lock().unwrap();
    let trace =
        micromegas_perfetto::protos::Trace::decode(&written_data[..]).expect("decode trace");

    // Should have process + thread + 2000 span events (begin/end for each of 1000 spans) = 2002
    assert_eq!(trace.packet.len(), 2002, "Should have all emitted packets");

    // Verify the trace is valid and spans have correct timing
    let first_span_begin = &trace.packet[2]; // First span begin event
    let last_span_end = &trace.packet[trace.packet.len() - 1]; // Last span end event

    assert_eq!(first_span_begin.timestamp, Some(0));
    assert_eq!(last_span_end.timestamp, Some(999500)); // 999 * 1000 + 500

    Ok(())
}

#[tokio::test]
async fn test_async_streaming_writer_into_inner() -> anyhow::Result<()> {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let shared_writer = SharedBufferAsyncWriter::new(buffer.clone());
    let mut streaming_writer = PerfettoWriter::new(Box::new(shared_writer), "test_process");

    // Write some data
    streaming_writer.emit_process_descriptor("test.exe").await?;
    streaming_writer.flush().await?;

    // Extract the writer
    let _inner_writer = streaming_writer.into_inner();

    // Verify we wrote some data to the shared buffer
    let written_data = buffer.lock().unwrap();
    assert!(!written_data.is_empty(), "Should have written data");

    Ok(())
}
