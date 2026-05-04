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
use std::collections::HashMap;
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

/// Map from `streams.format` to the processor that handles that wire format.
/// Views register one entry per format they understand (e.g. log entries register
/// both `"micromegas-transit"` and `"otlp/v1/logs"`).
pub type BlockProcessorMap = HashMap<&'static str, Arc<dyn BlockProcessor>>;

/// BlockPartitionSpec processes blocks individually and out of order
/// which works fine for measures & log entries.
///
/// Per-block dispatch keys on `PartitionSourceBlock::format` so a single view can
/// materialize blocks coming from heterogeneous wire formats (native CBOR + OTLP).
/// Unknown formats are warned and skipped.
#[derive(Debug)]
pub struct BlockPartitionSpec {
    pub view_metadata: ViewMetadata,
    pub schema: Arc<Schema>,
    pub insert_range: TimeRange,
    pub source_data: Arc<dyn PartitionBlocksSource>,
    pub block_processors: Arc<BlockProcessorMap>,
}

#[async_trait]
impl PartitionSpec for BlockPartitionSpec {
    fn is_empty(&self) -> bool {
        self.source_data.is_empty()
    }

    fn get_source_data_hash(&self) -> Vec<u8> {
        self.source_data.get_source_data_hash()
    }

    #[span_fn]
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

        // Allow empty source data - write_partition_from_rows will create
        // an empty partition record if no data is sent through the channel
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let join_handle = spawn_with_context(write_partition_from_rows(
            lake.clone(),
            self.view_metadata.clone(),
            self.schema.clone(),
            self.insert_range,
            self.source_data.get_source_data_hash(),
            rx,
            logger.clone(),
        ));

        // If source data is empty, just close the channel to create an empty partition
        if self.source_data.is_empty() {
            drop(tx);
            join_handle.await??;
            return Ok(());
        }

        let max_size = self.source_data.get_max_payload_size() as usize;
        let mut nb_tasks = (100 * 1024 * 1024) / max_size; // try to download up to 100 MB of payloads
        nb_tasks = nb_tasks.clamp(1, 64);

        let mut stream = self
            .source_data
            .get_blocks_stream()
            .await
            .map(|src_block_res| async {
                let src_block = src_block_res.with_context(|| "get_blocks_stream")?;
                // Per-block dispatch on `streams.format`. A view that doesn't register
                // a processor for some format silently skips matching blocks instead of
                // erroring — keeps the partition build moving when an unknown format
                // shows up alongside known ones.
                let Some(block_processor) = self
                    .block_processors
                    .get(src_block.format.as_str())
                    .cloned()
                else {
                    warn!(
                        "no block processor for format={} (view={}/{}); skipping block_id={}",
                        src_block.format,
                        self.view_metadata.view_set_name,
                        self.view_metadata.view_instance_id,
                        src_block.block.block_id
                    );
                    return Ok::<Option<PartitionRowSet>, anyhow::Error>(None);
                };
                let blob_storage = lake.blob_storage.clone();
                let handle = spawn_with_context(async move {
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
