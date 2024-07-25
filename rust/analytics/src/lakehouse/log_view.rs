use super::{
    partition::{write_partition, Partition},
    partition_source_data::{
        fetch_partition_source_data, PartitionSourceBlock, PartitionSourceData,
    },
    view::{PartitionSpec, View},
};
use crate::{
    log_entries_table::{log_table_schema, LogEntriesRecordBuilder},
    log_entry::for_each_log_entry_in_block,
    time::ConvertTicks,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::BufMut;
use chrono::{DateTime, Utc};
use datafusion::{
    arrow::array::RecordBatch,
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

const TABLE_SET_NAME: &str = "log_entries";
const TABLE_INSTANCE_ID: &str = "global";

pub struct LogView {
    table_set_name: Arc<String>,
    table_instance_id: Arc<String>,
}

impl LogView {
    pub fn new() -> Self {
        Self {
            table_set_name: Arc::new(String::from(TABLE_SET_NAME)),
            table_instance_id: Arc::new(String::from(TABLE_INSTANCE_ID)),
        }
    }
}

impl Default for LogView {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl View for LogView {
    fn get_table_set_name(&self) -> Arc<String> {
        self.table_set_name.clone()
    }

    fn get_table_instance_id(&self) -> Arc<String> {
        self.table_instance_id.clone()
    }

    async fn make_partition_spec(
        &self,
        pool: &sqlx::PgPool,
        begin_insert: DateTime<Utc>,
        end_insert: DateTime<Utc>,
    ) -> Result<Arc<dyn PartitionSpec>> {
        let source_data = fetch_partition_source_data(pool, begin_insert, end_insert, "log")
            .await
            .with_context(|| "fetch_partition_source_data")?;
        Ok(Arc::new(LogPartitionSpec {
            begin_insert,
            end_insert,
            source_data,
        }))
    }
}

// Log partition spec
struct LogPartitionSpec {
    pub begin_insert: DateTime<Utc>,
    pub end_insert: DateTime<Utc>,
    pub source_data: PartitionSourceData,
}

#[async_trait]
impl PartitionSpec for LogPartitionSpec {
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
        let schema = Arc::new(log_table_schema());
        let mut arrow_writer = ArrowWriter::try_new(&mut buffer_writer, schema, Some(props))?;

        let mut min_time = None;
        let mut max_time = None;
        for src_block in &self.source_data.blocks {
            if let Some(row_set) =
                fetch_log_block_row_set(lake.blob_storage.clone(), src_block).await?
            {
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
        }
        arrow_writer.close()?;

        let file_id = uuid::Uuid::new_v4();
        let file_path = format!(
            "views/{}/{}/minutes/{}/{file_id}.parquet",
            TABLE_SET_NAME,
            TABLE_INSTANCE_ID,
            self.begin_insert.format("%Y-%m-%d-%H-%M-%S")
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
                table_set_name: TABLE_SET_NAME.to_string(),
                table_instance_id: TABLE_INSTANCE_ID.to_string(),
                begin_insert_time: self.begin_insert,
                end_insert_time: self.end_insert,
                min_event_time: min_time.map(DateTime::<Utc>::from_timestamp_nanos).unwrap(),
                max_event_time: max_time.map(DateTime::<Utc>::from_timestamp_nanos).unwrap(),
                updated: sqlx::types::chrono::Utc::now(),
                file_path,
                file_size: buffer.len() as i64,
                file_schema_hash: vec![0],
                source_data_hash: self.source_data.block_ids_hash.clone(),
            },
            buffer,
        )
        .await?;
        Ok(())
    }
}

pub struct PartitionRowSet {
    pub min_time_row: i64,
    pub max_time_row: i64,
    pub rows: RecordBatch,
}

async fn fetch_log_block_row_set(
    blob_storage: Arc<BlobStorage>,
    src_block: &PartitionSourceBlock,
) -> Result<Option<PartitionRowSet>> {
    let convert_ticks = ConvertTicks::from_meta_data(
        src_block.process_start_ticks,
        src_block
            .process_start_time
            .timestamp_nanos_opt()
            .unwrap_or_default(),
        src_block.process_tsc_frequency,
    );
    let nb_log_entries = src_block.block.nb_objects;
    let mut record_builder = LogEntriesRecordBuilder::with_capacity(nb_log_entries as usize);

    for_each_log_entry_in_block(
        blob_storage,
        &convert_ticks,
        &src_block.stream,
        &src_block.block,
        |log_entry| {
            record_builder.append(&log_entry)?;
            Ok(true) // continue
        },
    )
    .await
    .with_context(|| "for_each_log_entry_in_block")?;

    if let Some(time_range) = record_builder.get_time_range() {
        let record_batch = record_builder.finish()?;
        Ok(Some(PartitionRowSet {
            min_time_row: time_range.0,
            max_time_row: time_range.1,
            rows: record_batch,
        }))
    } else {
        Ok(None)
    }
}
