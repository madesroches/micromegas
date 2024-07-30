use super::{
    partition::{write_partition, Partition},
    partition_source_data::{PartitionSourceBlock, PartitionSourceDataBlocks},
    view::PartitionSpec,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::BufMut;
use chrono::{DateTime, Utc};
use datafusion::{
    arrow::{array::RecordBatch, datatypes::Schema},
    parquet::{
        arrow::ArrowWriter,
        basic::Compression,
        file::properties::{WriterProperties, WriterVersion},
    },
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

pub struct PartitionRowSet {
    pub min_time_row: i64,
    pub max_time_row: i64,
    pub rows: RecordBatch,
}

#[async_trait]
pub trait BlockProcessor: Send + Sync {
    async fn process(
        &self,
        blob_storage: Arc<BlobStorage>,
        src_block: &PartitionSourceBlock,
    ) -> Result<Option<PartitionRowSet>>;
}

pub struct BlockPartitionSpec {
    pub table_set_name: Arc<String>,
    pub table_instance_id: Arc<String>,
    pub begin_insert: DateTime<Utc>,
    pub end_insert: DateTime<Utc>,
    pub file_schema: Arc<Schema>,
    pub file_schema_hash: Vec<u8>,
    pub source_data: PartitionSourceDataBlocks,
    pub block_processor: Arc<dyn BlockProcessor>,
}

#[async_trait]
impl PartitionSpec for BlockPartitionSpec {
    fn get_source_data_hash(&self) -> Vec<u8> {
        self.source_data.block_ids_hash.clone()
    }

    async fn write(&self, lake: Arc<DataLakeConnection>) -> Result<()> {
        // buffer the whole parquet in memory until https://github.com/apache/arrow-rs/issues/5766 is done
        // Impl AsyncFileWriter by object_store #5766
        let mut buffer_writer = bytes::BytesMut::with_capacity(1024 * 1024).writer();
        let props = WriterProperties::builder()
            .set_writer_version(WriterVersion::PARQUET_2_0)
            .set_compression(Compression::LZ4_RAW)
            .build();
        let mut arrow_writer =
            ArrowWriter::try_new(&mut buffer_writer, self.file_schema.clone(), Some(props))?;

        let mut min_time = None;
        let mut max_time = None;
        for src_block in &self.source_data.blocks {
            match self
                .block_processor
                .process(lake.blob_storage.clone(), src_block)
                .await
                .with_context(|| "processing source block")
            {
                Err(e) => {
                    error!("{e:?}");
                }
                Ok(Some(row_set)) => {
                    min_time = Some(
                        min_time
                            .unwrap_or(row_set.min_time_row)
                            .min(row_set.min_time_row),
                    );
                    max_time = Some(
                        max_time
                            .unwrap_or(row_set.max_time_row)
                            .max(row_set.max_time_row),
                    );
                    arrow_writer.write(&row_set.rows)?;
                }
                Ok(None) => {
                    debug!("empty block");
                }
            }
        }
        arrow_writer.close()?;

        let file_id = uuid::Uuid::new_v4();
        let file_path = format!(
            "views/{}/{}/{}/{}_{file_id}.parquet",
            *self.table_set_name,
            *self.table_instance_id,
            self.begin_insert.format("%Y-%m-%d"),
            self.begin_insert.format("%H-%M-%S")
        );
        if min_time.is_none() || max_time.is_none() {
            info!("no data for {file_path} partition, not writing the object");
            // should we check that there is no stale partition left behind?
            return Ok(());
        }
        let buffer: bytes::Bytes = buffer_writer.into_inner().into();
        write_partition(
            &lake,
            &Partition {
                table_set_name: self.table_set_name.to_string(),
                table_instance_id: self.table_instance_id.to_string(),
                begin_insert_time: self.begin_insert,
                end_insert_time: self.end_insert,
                min_event_time: min_time.map(DateTime::<Utc>::from_timestamp_nanos).unwrap(),
                max_event_time: max_time.map(DateTime::<Utc>::from_timestamp_nanos).unwrap(),
                updated: sqlx::types::chrono::Utc::now(),
                file_path,
                file_size: buffer.len() as i64,
                file_schema_hash: self.file_schema_hash.clone(),
                source_data_hash: self.source_data.block_ids_hash.clone(),
            },
            buffer,
        )
        .await?;
        Ok(())
    }
}
