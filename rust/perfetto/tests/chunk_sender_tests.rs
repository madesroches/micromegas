use datafusion::arrow::array::{BinaryArray, Int32Array};
use micromegas_perfetto::chunk_sender::ChunkSender;
use tokio::sync::mpsc;

#[tokio::test]
async fn test_chunk_sender_basic() -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel(10);

    // Create ChunkSender with 100 byte threshold
    let mut sender = ChunkSender::new(tx, 100);

    // Write small amounts of data
    sender.write(b"Hello, ").await?;
    sender.write(b"World!").await?;

    // Should not have sent anything yet (under threshold)
    assert!(rx.try_recv().is_err());

    // Flush should send the chunk
    sender.flush().await?;

    // Should receive one chunk
    let batch = rx.recv().await.unwrap()?;
    assert_eq!(batch.num_rows(), 1);

    // Verify chunk_id is 0
    let chunk_ids = batch
        .column_by_name("chunk_id")
        .unwrap()
        .as_any()
        .downcast_ref::<Int32Array>()
        .unwrap();
    assert_eq!(chunk_ids.value(0), 0);

    // Verify data
    let chunk_data = batch
        .column_by_name("chunk_data")
        .unwrap()
        .as_any()
        .downcast_ref::<BinaryArray>()
        .unwrap();
    assert_eq!(chunk_data.value(0), b"Hello, World!");

    Ok(())
}

#[tokio::test]
async fn test_chunk_sender_auto_flush() -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel(10);

    // Create ChunkSender with 10 byte threshold
    let mut sender = ChunkSender::new(tx, 10);

    // Write data that exceeds threshold
    sender.write(b"This is a long message").await?;

    // Should have auto-flushed
    let batch = rx.recv().await.unwrap()?;
    assert_eq!(batch.num_rows(), 1);

    let chunk_data = batch
        .column_by_name("chunk_data")
        .unwrap()
        .as_any()
        .downcast_ref::<BinaryArray>()
        .unwrap();
    assert_eq!(chunk_data.value(0), b"This is a long message");

    Ok(())
}

#[tokio::test]
async fn test_chunk_sender_multiple_chunks() -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel(10);

    // Create ChunkSender with 5 byte threshold
    let mut sender = ChunkSender::new(tx, 5);

    // Write multiple chunks worth of data
    sender.write(b"123456").await?; // Triggers flush at 6 bytes
    sender.write(b"7890AB").await?; // Triggers flush at 6 bytes
    sender.flush().await?; // Flush any remaining

    // Should receive multiple chunks with incrementing IDs
    let mut chunk_ids = Vec::new();
    while let Ok(batch) = rx.try_recv() {
        let batch = batch?;
        let id_array = batch
            .column_by_name("chunk_id")
            .unwrap()
            .as_any()
            .downcast_ref::<Int32Array>()
            .unwrap();
        chunk_ids.push(id_array.value(0));
    }

    // Should have at least 2 chunks
    assert!(chunk_ids.len() >= 2);
    // IDs should be sequential
    for i in 1..chunk_ids.len() {
        assert_eq!(chunk_ids[i], chunk_ids[i - 1] + 1);
    }

    Ok(())
}
