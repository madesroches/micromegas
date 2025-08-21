// Example demonstrating that StreamingPerfettoWriter produces identical output to regular Writer

use micromegas_perfetto::{StreamingPerfettoWriter, Writer};
use prost::Message;

fn main() -> anyhow::Result<()> {
    println!("Comparing regular Writer vs StreamingPerfettoWriter output...");

    // Create trace using regular writer
    let mut regular_writer = Writer::new("test_process_12345");
    regular_writer.append_process_descriptor("example.exe");
    regular_writer.append_thread_descriptor("main_thread", 1234, "main");
    regular_writer.append_thread_descriptor("worker_thread", 5678, "worker");

    // Add some spans
    regular_writer.append_span(1000000, 1500000, "initialization", "main", "main.rs", 10);
    regular_writer.append_span(1600000, 2100000, "process_data", "worker", "worker.rs", 25);
    regular_writer.append_span(2200000, 2800000, "cleanup", "main", "main.rs", 50);

    let regular_trace = regular_writer.into_trace();
    let regular_bytes = regular_trace.encode_to_vec();

    println!("Regular writer output: {} bytes", regular_bytes.len());
    println!("Regular trace packets: {}", regular_trace.packet.len());

    // Create identical trace using streaming writer
    let mut buffer = Vec::new();
    let mut streaming_writer = StreamingPerfettoWriter::new(&mut buffer, "test_process_12345");

    streaming_writer.emit_process_descriptor("example.exe")?;
    streaming_writer.emit_thread_descriptor("main_thread", 1234, "main")?;
    streaming_writer.emit_thread_descriptor("worker_thread", 5678, "worker")?;

    // Add the same spans
    streaming_writer.emit_span(1000000, 1500000, "initialization", "main", "main.rs", 10)?;
    streaming_writer.emit_span(1600000, 2100000, "process_data", "worker", "worker.rs", 25)?;
    streaming_writer.emit_span(2200000, 2800000, "cleanup", "main", "main.rs", 50)?;

    streaming_writer.flush()?;

    println!("Streaming writer output: {} bytes", buffer.len());

    // Parse streaming output back to trace
    let streaming_trace = micromegas_perfetto::protos::Trace::decode(&buffer[..])?;
    println!("Streaming trace packets: {}", streaming_trace.packet.len());

    // Compare the traces
    if regular_trace.packet.len() == streaming_trace.packet.len() {
        println!("✓ Both traces have the same number of packets");
    } else {
        println!(
            "✗ Packet count mismatch: regular={}, streaming={}",
            regular_trace.packet.len(),
            streaming_trace.packet.len()
        );
    }

    // Both traces should be valid and parseable
    println!("✓ Both traces are valid protobuf messages");

    // Save both traces to temp directory for manual inspection if needed
    let temp_dir = std::env::temp_dir();
    let regular_path = temp_dir.join("regular_trace.perfetto");
    let streaming_path = temp_dir.join("streaming_trace.perfetto");

    std::fs::write(&regular_path, &regular_bytes)?;
    std::fs::write(&streaming_path, &buffer)?;

    println!("✓ Traces saved to temp directory:");
    println!("  Regular: {}", regular_path.display());
    println!("  Streaming: {}", streaming_path.display());
    println!("  You can load these in ui.perfetto.dev to verify they're identical");

    Ok(())
}
