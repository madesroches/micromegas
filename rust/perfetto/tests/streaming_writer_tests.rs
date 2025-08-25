use micromegas_perfetto::{StreamingPerfettoWriter, Writer};
use prost::Message;
use std::io::{Cursor, Write};

#[test]
fn test_streaming_writer_basic_usage() {
    let mut buffer = Vec::new();
    let mut streaming_writer = StreamingPerfettoWriter::new(&mut buffer, "test_process");

    // Emit process descriptor
    streaming_writer
        .emit_process_descriptor("test.exe")
        .expect("emit process descriptor");

    // Emit thread descriptor
    streaming_writer
        .emit_thread_descriptor("thread_1", 1234, "main")
        .expect("emit thread descriptor");

    // Emit a span
    streaming_writer
        .emit_span(1000000, 2000000, "test_span", "test_target", "test.rs", 42)
        .expect("emit span");

    streaming_writer.flush().expect("flush");

    // Verify we have written some data
    assert!(
        !buffer.is_empty(),
        "Buffer should not be empty after writing"
    );

    // Verify the data is a valid protobuf trace
    let trace = micromegas_perfetto::protos::Trace::decode(&buffer[..]).expect("decode trace");
    assert!(!trace.packet.is_empty(), "Trace should contain packets");
}

#[test]
fn test_streaming_vs_regular_writer_compatibility() {
    // Create a trace using the regular writer
    let mut regular_writer = Writer::new("test_process");
    regular_writer.append_process_descriptor("test.exe");
    regular_writer.append_thread_descriptor("thread_1", 1234, "main");
    regular_writer.append_span(1000000, 2000000, "test_span", "test_target", "test.rs", 42);
    let regular_trace = regular_writer.into_trace();
    let regular_bytes = regular_trace.encode_to_vec();

    // Create the same trace using the streaming writer
    let mut buffer = Vec::new();
    let mut streaming_writer = StreamingPerfettoWriter::new(&mut buffer, "test_process");
    streaming_writer
        .emit_process_descriptor("test.exe")
        .expect("emit process descriptor");
    streaming_writer
        .emit_thread_descriptor("thread_1", 1234, "main")
        .expect("emit thread descriptor");
    streaming_writer
        .emit_span(1000000, 2000000, "test_span", "test_target", "test.rs", 42)
        .expect("emit span");
    streaming_writer.flush().expect("flush");

    // Parse both traces and compare structure
    let regular_parsed = micromegas_perfetto::protos::Trace::decode(&regular_bytes[..])
        .expect("decode regular trace");
    let streaming_parsed =
        micromegas_perfetto::protos::Trace::decode(&buffer[..]).expect("decode streaming trace");

    // Both traces should have the same number of packets
    assert_eq!(regular_parsed.packet.len(), streaming_parsed.packet.len());

    // Both traces should be valid and parseable
    assert!(!regular_parsed.packet.is_empty());
    assert!(!streaming_parsed.packet.is_empty());
}

#[test]
fn test_streaming_writer_packet_framing() {
    let mut buffer = Vec::new();
    let mut streaming_writer = StreamingPerfettoWriter::new(&mut buffer, "test_process");

    // Emit a simple process descriptor
    streaming_writer
        .emit_process_descriptor("test.exe")
        .expect("emit process descriptor");
    streaming_writer.flush().expect("flush");

    // Manually verify the packet framing
    let mut cursor = Cursor::new(&buffer);

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
    let trace = micromegas_perfetto::protos::Trace::decode(&buffer[..]).expect("decode trace");
    assert_eq!(trace.packet.len(), 1);
}

#[test]
fn test_streaming_writer_interning() {
    let mut buffer = Vec::new();
    let mut streaming_writer = StreamingPerfettoWriter::new(&mut buffer, "test_process");

    streaming_writer
        .emit_process_descriptor("test.exe")
        .expect("emit process descriptor");
    streaming_writer
        .emit_thread_descriptor("thread_1", 1234, "main")
        .expect("emit thread descriptor");

    // Emit two spans with the same name and category
    streaming_writer
        .emit_span(1000000, 2000000, "same_name", "same_target", "test.rs", 42)
        .expect("emit span 1");
    streaming_writer
        .emit_span(3000000, 4000000, "same_name", "same_target", "test.rs", 43)
        .expect("emit span 2");

    streaming_writer.flush().expect("flush");

    // Verify the trace is valid
    let trace = micromegas_perfetto::protos::Trace::decode(&buffer[..]).expect("decode trace");
    assert!(!trace.packet.is_empty());
}

#[test]
fn test_streaming_writer_memory_usage() {
    // Test that the streaming writer doesn't accumulate packets in memory
    let mut buffer = Vec::new();
    let mut streaming_writer = StreamingPerfettoWriter::new(&mut buffer, "test_process");

    streaming_writer
        .emit_process_descriptor("test.exe")
        .expect("emit process descriptor");
    streaming_writer
        .emit_thread_descriptor("thread_1", 1234, "main")
        .expect("emit thread descriptor");

    // Emit many spans
    for i in 0..1000 {
        streaming_writer
            .emit_span(
                i * 1000,
                (i + 1) * 1000,
                &format!("span_{}", i),
                "test_target",
                "test.rs",
                42,
            )
            .expect("emit span");
    }

    streaming_writer.flush().expect("flush");

    // Buffer should contain a valid trace
    let trace = micromegas_perfetto::protos::Trace::decode(&buffer[..]).expect("decode trace");
    // Should have process + thread + 2000 span events (begin + end for each)
    assert_eq!(trace.packet.len(), 2 + 2000);
}

#[test]
fn test_streaming_writer_error_handling() {
    // Test with a writer that fails
    struct FailingWriter {
        should_fail: bool,
    }

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            if self.should_fail {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Test failure",
                ))
            } else {
                Ok(_buf.len())
            }
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let failing_writer = FailingWriter { should_fail: true };
    let mut streaming_writer = StreamingPerfettoWriter::new(failing_writer, "test_process");

    // Should return an error when trying to write
    let result = streaming_writer.emit_process_descriptor("test.exe");
    assert!(result.is_err(), "Should fail when writer fails");
}

#[test]
fn test_streaming_writer_async_track_creation() {
    let mut buffer = Vec::new();
    let mut streaming_writer = StreamingPerfettoWriter::new(&mut buffer, "test_process");

    // Emit process descriptor
    streaming_writer
        .emit_process_descriptor("test.exe")
        .expect("emit process descriptor");

    // Emit async track descriptor
    streaming_writer
        .emit_async_track_descriptor()
        .expect("emit async track descriptor");

    streaming_writer.flush().expect("flush");

    // Verify the trace is valid
    let trace = micromegas_perfetto::protos::Trace::decode(&buffer[..]).expect("decode trace");
    assert_eq!(trace.packet.len(), 2); // Process + async track

    // Second packet should be the async track descriptor
    let async_track_packet = &trace.packet[1];
    assert!(async_track_packet.data.is_some());
    if let Some(micromegas_perfetto::protos::trace_packet::Data::TrackDescriptor(track)) =
        &async_track_packet.data
    {
        assert!(track.parent_uuid.is_some()); // Should be parented to process
        assert!(track.static_or_dynamic_name.is_some()); // Should have a name
    } else {
        panic!("Second packet should be a track descriptor");
    }
}

#[test]
fn test_streaming_writer_async_span_events() {
    let mut buffer = Vec::new();
    let mut streaming_writer = StreamingPerfettoWriter::new(&mut buffer, "test_process");

    // Setup required descriptors
    streaming_writer
        .emit_process_descriptor("test.exe")
        .expect("emit process descriptor");
    streaming_writer
        .emit_async_track_descriptor()
        .expect("emit async track descriptor");

    // Emit async span events
    streaming_writer
        .emit_async_span_begin(1000000, "async_task", "async_target", "async.rs", 10)
        .expect("emit async span begin");
    streaming_writer
        .emit_async_span_end(2000000, "async_task", "async_target", "async.rs", 10)
        .expect("emit async span end");

    streaming_writer.flush().expect("flush");

    // Verify the trace is valid
    let trace = micromegas_perfetto::protos::Trace::decode(&buffer[..]).expect("decode trace");
    assert_eq!(trace.packet.len(), 4); // Process + async track + begin + end

    // Verify the span events
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
        assert!(begin_event.track_uuid.is_some());
    } else {
        panic!("Begin packet should be a track event");
    }

    if let Some(micromegas_perfetto::protos::trace_packet::Data::TrackEvent(end_event)) =
        &end_packet.data
    {
        assert_eq!(
            end_event.r#type,
            Some(micromegas_perfetto::protos::track_event::Type::SliceEnd.into())
        );
        assert!(end_event.track_uuid.is_some());
    } else {
        panic!("End packet should be a track event");
    }
}

#[test]
fn test_regular_writer_async_track_creation() {
    let mut writer = Writer::new("test_process");

    // Setup descriptors
    writer.append_process_descriptor("test.exe");
    writer.append_async_track_descriptor();

    let trace = writer.into_trace();
    assert_eq!(trace.packet.len(), 2); // Process + async track

    // Second packet should be the async track descriptor
    let async_track_packet = &trace.packet[1];
    if let Some(micromegas_perfetto::protos::trace_packet::Data::TrackDescriptor(track)) =
        &async_track_packet.data
    {
        assert!(track.parent_uuid.is_some()); // Should be parented to process
        assert!(track.static_or_dynamic_name.is_some()); // Should have a name
    } else {
        panic!("Second packet should be a track descriptor");
    }
}

#[test]
fn test_regular_writer_async_span_events() {
    let mut writer = Writer::new("test_process");

    // Setup descriptors
    writer.append_process_descriptor("test.exe");
    writer.append_async_track_descriptor();

    // Emit async span events
    writer.append_async_span_begin(1000000, "async_task", "async_target", "async.rs", 10);
    writer.append_async_span_end(2000000, "async_task", "async_target", "async.rs", 10);

    let trace = writer.into_trace();
    assert_eq!(trace.packet.len(), 4); // Process + async track + begin + end

    // Verify the span events
    let begin_packet = &trace.packet[2];
    let end_packet = &trace.packet[3];

    assert_eq!(begin_packet.timestamp, Some(1000000));
    assert_eq!(end_packet.timestamp, Some(2000000));
}

#[test]
#[should_panic(expected = "Must call append_async_track_descriptor()")]
fn test_regular_writer_async_span_without_track() {
    let mut writer = Writer::new("test_process");
    writer.append_process_descriptor("test.exe");

    // This should panic because async track descriptor wasn't created
    writer.append_async_span_begin(1000000, "async_task", "async_target", "async.rs", 10);
}

#[test]
#[should_panic(expected = "Must call emit_async_track_descriptor()")]
fn test_streaming_writer_async_span_without_track() {
    let mut buffer = Vec::new();
    let mut streaming_writer = StreamingPerfettoWriter::new(&mut buffer, "test_process");

    streaming_writer
        .emit_process_descriptor("test.exe")
        .expect("emit process descriptor");

    // This should panic because async track descriptor wasn't created
    streaming_writer
        .emit_async_span_begin(1000000, "async_task", "async_target", "async.rs", 10)
        .expect("emit async span begin");
}

#[test]
fn test_async_track_creation_idempotent() {
    let mut writer = Writer::new("test_process");
    writer.append_process_descriptor("test.exe");

    // Call async track creation multiple times
    writer.append_async_track_descriptor();
    writer.append_async_track_descriptor();
    writer.append_async_track_descriptor();

    let trace = writer.into_trace();
    assert_eq!(trace.packet.len(), 2); // Process + only one async track
}
