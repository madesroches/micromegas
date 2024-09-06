use super::{
    partition_source_data::{PartitionSourceBlock, PartitionSourceDataBlocks},
    view::{PartitionSpec, ViewMetadata},
    write_partition::{write_partition_from_rows, PartitionRowSet},
};
use crate::response_writer::ResponseWriter;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// BlockProcessor transforms a single block of telemetry into a set of rows
#[async_trait]
pub trait BlockProcessor: Send + Sync {
    async fn process(
        &self,
        blob_storage: Arc<BlobStorage>,
        src_block: Arc<PartitionSourceBlock>,
    ) -> Result<Option<PartitionRowSet>>;
}

/// BlockPartitionSpec processes blocks individually and out of order
/// which works fine for measures & log entries
pub struct BlockPartitionSpec {
    pub view_metadata: ViewMetadata,
    pub begin_insert: DateTime<Utc>,
    pub end_insert: DateTime<Utc>,
    pub source_data: PartitionSourceDataBlocks,
    pub block_processor: Arc<dyn BlockProcessor>,
}

#[async_trait]
impl PartitionSpec for BlockPartitionSpec {
    fn get_source_data_hash(&self) -> Vec<u8> {
        self.source_data.block_ids_hash.clone()
    }

    async fn write(
        &self,
        lake: Arc<DataLakeConnection>,
        response_writer: Arc<ResponseWriter>,
    ) -> Result<()> {
        // buffer the whole parquet in memory until https://github.com/apache/arrow-rs/issues/5766 is available in a published version
        // Impl AsyncFileWriter by object_store #5766
        let desc = format!(
            "[{}, {}] {} {}",
            self.view_metadata.view_set_name,
            self.view_metadata.view_instance_id,
            self.begin_insert.to_rfc3339(),
            self.end_insert.to_rfc3339()
        );
        response_writer
            .write_string(&format!("writing {desc}"))
            .await?;

        response_writer
            .write_string(&format!("reading {} blocks", self.source_data.blocks.len()))
            .await?;

        if self.source_data.blocks.is_empty() {
            return Ok(());
        }

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let join_handle = tokio::spawn(write_partition_from_rows(
            lake.clone(),
            self.view_metadata.clone(),
            self.begin_insert,
            self.end_insert,
            self.source_data.block_ids_hash.clone(),
            rx,
            1024 * 1024,
            response_writer.clone(),
        ));

        let mut max_size = self.source_data.blocks[0].block.payload_size as usize;
        for block in &self.source_data.blocks {
            max_size = max_size.max(block.block.payload_size as usize);
        }
        let mut nb_tasks = (100 * 1024 * 1024) / max_size; // try to download up to 100 MB of payloads
        nb_tasks = nb_tasks.clamp(1, 64);

        let mut stream = futures::stream::iter(self.source_data.blocks.clone())
            .map(|src_block| async {
                let block_processor = self.block_processor.clone();
                let blob_storage = lake.blob_storage.clone();
                let handle = tokio::spawn(async move {
                    block_processor
                        .process(blob_storage, src_block)
                        .await
                        .with_context(|| "processing source block")
                });
                handle.await.unwrap()
            })
            .buffer_unordered(nb_tasks);

        while let Some(res_opt_rows) = stream.next().await {
            match res_opt_rows {
                Err(e) => {
                    error!("{e:?}");
                    response_writer.write_string(&format!("{e:?}")).await?;
                }
                Ok(Some(row_set)) => {
                    tx.send(row_set).await?;
                }
                Ok(None) => {
                    debug!("empty block");
                }
            }
        }
        drop(tx);
        join_handle.await??;
        Ok(())
    }
}
