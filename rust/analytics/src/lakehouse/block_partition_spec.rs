use super::{
    partition_source_data::{PartitionBlocksSource, PartitionSourceBlock},
    view::{PartitionSpec, ViewMetadata},
    write_partition::{PartitionRowSet, write_partition_from_rows},
};
use crate::{response_writer::Logger, time::TimeRange};
use anyhow::{Context, Result};
use async_trait::async_trait;
use datafusion::arrow::datatypes::Schema;
use futures::StreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_tracing::prelude::*;
use std::fmt::Debug;
use std::sync::Arc;

/// BlockProcessor transforms a single block of telemetry into a set of rows
#[async_trait]
pub trait BlockProcessor: Send + Sync + Debug {
    /// Processes a single block of telemetry.
    async fn process(
        &self,
        blob_storage: Arc<BlobStorage>,
        src_block: Arc<PartitionSourceBlock>,
    ) -> Result<Option<PartitionRowSet>>;
}

/// BlockPartitionSpec processes blocks individually and out of order
/// which works fine for measures & log entries
#[derive(Debug)]
pub struct BlockPartitionSpec {
    pub view_metadata: ViewMetadata,
    pub schema: Arc<Schema>,
    pub insert_range: TimeRange,
    pub source_data: Arc<dyn PartitionBlocksSource>,
    pub block_processor: Arc<dyn BlockProcessor>,
}

#[async_trait]
impl PartitionSpec for BlockPartitionSpec {
    fn is_empty(&self) -> bool {
        self.source_data.is_empty()
    }

    fn get_source_data_hash(&self) -> Vec<u8> {
        self.source_data.get_source_data_hash()
    }

    async fn write(&self, lake: Arc<DataLakeConnection>, logger: Arc<dyn Logger>) -> Result<()> {
        let desc = format!(
            "[{}, {}] {} {}",
            self.view_metadata.view_set_name,
            self.view_metadata.view_instance_id,
            self.insert_range.begin.to_rfc3339(),
            self.insert_range.end.to_rfc3339()
        );
        logger.write_log_entry(format!("writing {desc}")).await?;

        logger
            .write_log_entry(format!(
                "reading {} blocks",
                self.source_data.get_nb_blocks()
            ))
            .await?;

        if self.source_data.is_empty() {
            return Ok(());
        }

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let join_handle = tokio::spawn(write_partition_from_rows(
            lake.clone(),
            self.view_metadata.clone(),
            self.schema.clone(),
            self.insert_range,
            self.source_data.get_source_data_hash(),
            rx,
            logger.clone(),
        ));

        let max_size = self.source_data.get_max_payload_size() as usize;
        let mut nb_tasks = (100 * 1024 * 1024) / max_size; // try to download up to 100 MB of payloads
        nb_tasks = nb_tasks.clamp(1, 64);

        let mut stream = self
            .source_data
            .get_blocks_stream()
            .await
            .map(|src_block_res| async {
                let src_block = src_block_res.with_context(|| "get_blocks_stream")?;
                let block_processor = self.block_processor.clone();
                let blob_storage = lake.blob_storage.clone();
                let handle = tokio::spawn(async move {
                    block_processor
                        .process(blob_storage, src_block)
                        .await
                        .with_context(|| "processing source block")
                });
                handle.await.with_context(|| "handle.await")?
            })
            .buffer_unordered(nb_tasks);

        while let Some(res_opt_rows) = stream.next().await {
            match res_opt_rows {
                Err(e) => {
                    error!("{e:?}");
                    logger.write_log_entry(format!("{e:?}")).await?;
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
