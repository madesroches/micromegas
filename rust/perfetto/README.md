# micromegas-perfetto

A streaming writer for [Perfetto](https://perfetto.dev/) trace files. It encodes thread and async spans as protobuf `TracePacket`s and writes them to any `AsyncWriter` sink, so traces can be produced incrementally without holding the whole file in memory.

Use it to generate traces viewable in [ui.perfetto.dev](https://ui.perfetto.dev/) from your own span data. The crate also bundles the Perfetto protobuf bindings in the `protos` module, for users who build packets by hand.

## Example

```rust
use async_trait::async_trait;
use micromegas_perfetto::{async_writer::AsyncWriter, streaming_writer::PerfettoWriter};
use std::sync::{Arc, Mutex};

// A sink that appends to a shared in-memory buffer.
struct BufferSink(Arc<Mutex<Vec<u8>>>);

#[async_trait]
impl AsyncWriter for BufferSink {
    async fn write(&mut self, buf: &[u8]) -> anyhow::Result<()> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(())
    }

    async fn flush(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let mut writer = PerfettoWriter::new(Box::new(BufferSink(buffer.clone())), "my-process-id");
    writer.emit_process_descriptor("my-app.exe").await?;
    writer.emit_thread_descriptor("thread-1", 1234, "main").await?;
    // begin and end timestamps are in nanoseconds
    writer
        .emit_span(1_000_000, 2_000_000, "my_span", "my_target", "main.rs", 42)
        .await?;
    writer.flush().await?;
    assert!(!buffer.lock().unwrap().is_empty());
    Ok(())
}
```

Save the written bytes to a file and open it in [ui.perfetto.dev](https://ui.perfetto.dev/).

## Documentation

- [API documentation on docs.rs](https://docs.rs/micromegas-perfetto)
- [GitHub Repository](https://github.com/madesroches/micromegas)
- [Micromegas Home Page](https://micromegas.info/)
