use datafusion::arrow::array::{BinaryArray, Int32Array};
use datafusion::arrow::record_batch::RecordBatch;
use tokio::sync::mpsc;

use crate::async_writer::AsyncWriter;

/// ChunkSender sends data as RecordBatch chunks through a channel.
/// It accumulates data until reaching a threshold size, then sends it as a chunk.
pub struct ChunkSender {
    chunk_sender: mpsc::Sender<anyhow::Result<RecordBatch>>,
    chunk_id: i32,
    current_chunk: Vec<u8>,
    chunk_threshold: usize,
}

impl ChunkSender {
    /// Creates a new ChunkSender with specified chunk size threshold
    pub fn new(
        chunk_sender: mpsc::Sender<anyhow::Result<RecordBatch>>,
        chunk_threshold: usize,
    ) -> Self {
        Self {
            chunk_sender,
            chunk_id: 0,
            current_chunk: Vec::new(),
            chunk_threshold,
        }
    }

    /// Writes data to the chunk buffer, automatically flushing when threshold is reached
    pub async fn write(&mut self, buf: &[u8]) -> anyhow::Result<()> {
        self.current_chunk.extend_from_slice(buf);

        // If chunk exceeds threshold, flush it
        if self.current_chunk.len() >= self.chunk_threshold {
            self.flush().await?;
        }
        Ok(())
    }

    /// Flushes the current chunk as a RecordBatch to the channel
    pub async fn flush(&mut self) -> anyhow::Result<()> {
        if self.current_chunk.is_empty() {
            return Ok(());
        }

        let chunk_id_array = Int32Array::from(vec![self.chunk_id]);
        let chunk_data_array = BinaryArray::from(vec![self.current_chunk.as_slice()]);

        let batch = RecordBatch::try_from_iter(vec![
            (
                "chunk_id",
                std::sync::Arc::new(chunk_id_array)
                    as std::sync::Arc<dyn datafusion::arrow::array::Array>,
            ),
            (
                "chunk_data",
                std::sync::Arc::new(chunk_data_array)
                    as std::sync::Arc<dyn datafusion::arrow::array::Array>,
            ),
        ])?;

        // Send the batch through the channel
        self.chunk_sender
            .send(Ok(batch))
            .await
            .map_err(|_| anyhow::anyhow!("Channel receiver dropped"))?;

        self.chunk_id += 1;
        self.current_chunk.clear();
        Ok(())
    }
}

/// Implementation of AsyncWriter for ChunkSender
#[async_trait::async_trait]
impl AsyncWriter for ChunkSender {
    async fn write(&mut self, buf: &[u8]) -> anyhow::Result<()> {
        self.write(buf).await
    }

    async fn flush(&mut self) -> anyhow::Result<()> {
        self.flush().await
    }
}
