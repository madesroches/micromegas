use super::blocks_view::blocks_file_schema_hash;
use super::partition_cache::PartitionCache;
use crate::arrow_properties::serialize_properties_to_jsonb;
use crate::dfext::{
    binary_column_accessor::binary_column_by_name, string_column_accessor::string_column_by_name,
    typed_column::typed_column_by_name,
};
use crate::metadata::ProcessMetadata;
use crate::properties::utils::extract_properties_from_binary_column;
use crate::time::TimeRange;
use crate::{
    dfext::typed_column::typed_column,
    lakehouse::{blocks_view::blocks_view_schema, query::query_partitions},
};
use anyhow::{Context, Result};
use async_stream::try_stream;
use async_trait::async_trait;
use chrono::DateTime;
use datafusion::functions_aggregate::{count::count_all, expr_fn::sum, min_max::max};
use datafusion::{
    arrow::array::{
        Array, BinaryArray, GenericListArray, Int32Array, Int64Array, StringArray,
        TimestampNanosecondArray,
    },
    execution::runtime_env::RuntimeEnv,
    prelude::*,
};
use futures::{StreamExt, stream::BoxStream};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::{stream_info::StreamInfo, types::block::BlockMetadata};
use std::fmt::Debug;
use std::sync::Arc;
use uuid::Uuid;

/// Represents a single block of source data for a partition.
#[derive(Debug)]
pub struct PartitionSourceBlock {
    pub block: BlockMetadata,
    pub stream: Arc<StreamInfo>,
    pub process: Arc<ProcessMetadata>,
}

/// A trait for providing blocks of source data for partitions.
#[async_trait]
pub trait PartitionBlocksSource: Sync + Send + Debug {
    /// Returns true if there are no blocks in the source.
    fn is_empty(&self) -> bool;
    /// Returns the number of blocks in the source.
    fn get_nb_blocks(&self) -> i64;
    /// Returns the maximum payload size of the blocks in the source.
    fn get_max_payload_size(&self) -> i64;
    /// Returns a hash of the source data.
    fn get_source_data_hash(&self) -> Vec<u8>;
    /// Returns a stream of the source blocks.
    async fn get_blocks_stream(&self) -> BoxStream<'static, Result<Arc<PartitionSourceBlock>>>;
}

/// A `PartitionBlocksSource` implementation that stores blocks in memory.
#[derive(Debug)]
pub struct SourceDataBlocksInMemory {
    pub blocks: Vec<Arc<PartitionSourceBlock>>,
    pub block_ids_hash: Vec<u8>,
}

#[async_trait]
impl PartitionBlocksSource for SourceDataBlocksInMemory {
    fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    fn get_nb_blocks(&self) -> i64 {
        self.blocks.len() as i64
    }

    fn get_max_payload_size(&self) -> i64 {
        let mut max_size = self.blocks[0].block.payload_size;
        for block in &self.blocks {
            max_size = max_size.max(block.block.payload_size);
        }
        max_size
    }

    fn get_source_data_hash(&self) -> Vec<u8> {
        self.block_ids_hash.clone()
    }

    async fn get_blocks_stream(&self) -> BoxStream<'static, Result<Arc<PartitionSourceBlock>>> {
        let stream = futures::stream::iter(self.blocks.clone()).map(Ok);
        stream.boxed()
    }
}

/// A `PartitionBlocksSource` implementation that fetches blocks from a DataFusion DataFrame.
#[derive(Debug)]
pub struct SourceDataBlocks {
    pub blocks_dataframe: DataFrame,
    pub object_count: i64,
    pub block_count: i64,
    pub max_payload: i64,
}

#[async_trait]
impl PartitionBlocksSource for SourceDataBlocks {
    fn is_empty(&self) -> bool {
        self.object_count == 0
    }

    fn get_nb_blocks(&self) -> i64 {
        self.block_count
    }

    fn get_max_payload_size(&self) -> i64 {
        self.max_payload
    }

    fn get_source_data_hash(&self) -> Vec<u8> {
        self.object_count.to_le_bytes().to_vec()
    }

    async fn get_blocks_stream(&self) -> BoxStream<'static, Result<Arc<PartitionSourceBlock>>> {
        let df = self.blocks_dataframe.clone();
        Box::pin(try_stream! {
            let mut stream = df.execute_stream().await?;
            while let Some(res) = stream.next().await {
                let b = res.with_context(|| "fetching blocks query results")?;
                let block_id_column = string_column_by_name(&b, "block_id")?;
                let stream_id_column = string_column_by_name(&b, "stream_id")?;
                let process_id_column = string_column_by_name(&b, "process_id")?;
                let begin_time_column: &TimestampNanosecondArray = typed_column_by_name(&b, "begin_time")?;
                let begin_ticks_column: &Int64Array = typed_column_by_name(&b, "begin_ticks")?;
                let end_time_column: &TimestampNanosecondArray = typed_column_by_name(&b, "end_time")?;
                let end_ticks_column: &Int64Array = typed_column_by_name(&b, "end_ticks")?;
                let nb_objects_column: &Int32Array = typed_column_by_name(&b, "nb_objects")?;
                let object_offset_column: &Int64Array = typed_column_by_name(&b, "object_offset")?;
                let payload_size_column: &Int64Array = typed_column_by_name(&b, "payload_size")?;
                let block_insert_time_column: &TimestampNanosecondArray =
                    typed_column_by_name(&b, "insert_time")?;
                let dependencies_metadata_column: &BinaryArray =
                    typed_column_by_name(&b, "streams.dependencies_metadata")?;
                let objects_metadata_column: &BinaryArray =
                    typed_column_by_name(&b, "streams.objects_metadata")?;
                let stream_tags_column: &GenericListArray<i32> = typed_column_by_name(&b, "streams.tags")?;
                let stream_properties_accessor = binary_column_by_name(&b, "streams.properties")?;

                let process_start_time_column: &TimestampNanosecondArray =
                    typed_column_by_name(&b, "processes.start_time")?;
                let process_start_ticks_column: &Int64Array =
                    typed_column_by_name(&b, "processes.start_ticks")?;
                let process_tsc_freq_column: &Int64Array =
                    typed_column_by_name(&b, "processes.tsc_frequency")?;
                let process_exe_column = string_column_by_name(&b, "processes.exe")?;
                let process_username_column = string_column_by_name(&b, "processes.username")?;
                let process_realname_column = string_column_by_name(&b, "processes.realname")?;
                let process_computer_column = string_column_by_name(&b, "processes.computer")?;
                let process_distro_column = string_column_by_name(&b, "processes.distro")?;
                let process_cpu_column = string_column_by_name(&b, "processes.cpu_brand")?;
                let process_parent_column = string_column_by_name(&b, "processes.parent_process_id")?;
                let process_properties_accessor = binary_column_by_name(&b, "processes.properties")?;
                for ir in 0..b.num_rows() {
                    let block_insert_time = block_insert_time_column.value(ir);
                    let stream_id = Uuid::parse_str(stream_id_column.value(ir))?;
                    let process_id = Uuid::parse_str(process_id_column.value(ir))?;
                    let block = BlockMetadata {
                        block_id: Uuid::parse_str(block_id_column.value(ir))?,
                        stream_id,
                        process_id,
                        begin_time: DateTime::from_timestamp_nanos(begin_time_column.value(ir)),
                        end_time: DateTime::from_timestamp_nanos(end_time_column.value(ir)),
                        begin_ticks: begin_ticks_column.value(ir),
                        end_ticks: end_ticks_column.value(ir),
                        nb_objects: nb_objects_column.value(ir),
                        payload_size: payload_size_column.value(ir),
                        object_offset: object_offset_column.value(ir),
                        insert_time: DateTime::from_timestamp_nanos(block_insert_time),
                    };

                    let dependencies_metadata = dependencies_metadata_column.value(ir);
                    let objects_metadata = objects_metadata_column.value(ir);
                    let stream_tags = stream_tags_column
                        .value(ir)
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .with_context(|| "casting stream_tags")?
                        .iter()
                        .map(|item| String::from(item.unwrap_or_default()))
                        .collect();

                    let stream_properties = extract_properties_from_binary_column(stream_properties_accessor.as_ref(), ir)?;
                    let stream = StreamInfo {
                        process_id,
                        stream_id,
                        dependencies_metadata: ciborium::from_reader(dependencies_metadata)
                            .with_context(|| "decoding dependencies_metadata")?,
                        objects_metadata: ciborium::from_reader(objects_metadata)
                            .with_context(|| "decoding objects_metadata")?,
                        tags: stream_tags,
                        properties: stream_properties,
                    };
                    let process_properties = extract_properties_from_binary_column(process_properties_accessor.as_ref(), ir)?;
                    let parent_value = process_parent_column.value(ir);
                    let parent_process_id = if parent_value.is_empty() {
                        None
                    } else {
                        Some(Uuid::parse_str(parent_value).with_context(|| "parsing parent process_id")?)
                    };

                    // Pre-serialize properties to JSONB for ProcessMetadata
                    let properties_jsonb = serialize_properties_to_jsonb(&process_properties)
                        .with_context(|| "serializing properties to JSONB")?;

                    let process = ProcessMetadata {
                        process_id,
                        exe: process_exe_column.value(ir).into(),
                        username: process_username_column.value(ir).into(),
                        realname: process_realname_column.value(ir).into(),
                        computer: process_computer_column.value(ir).into(),
                        distro: process_distro_column.value(ir).into(),
                        cpu_brand: process_cpu_column.value(ir).into(),
                        tsc_frequency: process_tsc_freq_column.value(ir),
                        start_time: DateTime::from_timestamp_nanos(process_start_time_column.value(ir)),
                        start_ticks: process_start_ticks_column.value(ir),
                        parent_process_id,
                        properties: Arc::new(properties_jsonb),
                    };
                    yield Arc::new(PartitionSourceBlock {
                        block,
                        stream: stream.into(),
                        process: Arc::new(process),
                    });
                }
            }

        })
    }
}

/// Converts a hash (expected to be an i64 as bytes) to an object count.
pub fn hash_to_object_count(hash: &[u8]) -> Result<i64> {
    Ok(i64::from_le_bytes(
        hash.try_into().with_context(|| "hash_to_object_count")?,
    ))
}

/// Fetches partition source data from the data lake.
pub async fn fetch_partition_source_data(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    existing_partitions: Arc<PartitionCache>,
    insert_range: TimeRange,
    source_stream_tag: &str,
) -> Result<SourceDataBlocks> {
    let begin_rfc = insert_range.begin.to_rfc3339();
    let end_rfc = insert_range.end.to_rfc3339();
    let sql = format!(
        r#"
          SELECT block_id, stream_id, process_id, begin_time, begin_ticks, end_time, end_ticks, nb_objects,
              object_offset, payload_size, insert_time,
              "streams.dependencies_metadata", "streams.objects_metadata", "streams.tags", "streams.properties",
              "processes.start_time", "processes.start_ticks", "processes.tsc_frequency", "processes.exe",
              "processes.username", "processes.realname", "processes.computer", "processes.distro", "processes.cpu_brand",
              "processes.parent_process_id", "processes.properties"
          FROM source
          WHERE array_has( "streams.tags", '{source_stream_tag}' )
          AND insert_time >= '{begin_rfc}'
          AND insert_time < '{end_rfc}'
          ;"#
    );
    let block_partitions = existing_partitions
        .filter("blocks", "global", &blocks_file_schema_hash(), insert_range)
        .partitions;
    let df = query_partitions(
        runtime,
        lake.clone(),
        Arc::new(blocks_view_schema()),
        Arc::new(block_partitions),
        &sql,
    )
    .await
    .with_context(|| "blocks query")?;
    let blocks_stats_df = df.clone().aggregate(
        vec![],
        vec![
            sum(col("nb_objects")),
            count_all(),
            max(col("payload_size")),
        ],
    )?;
    let blocks_stats_rbs = blocks_stats_df.collect().await?;
    if blocks_stats_rbs.len() != 1 {
        anyhow::bail!("nb_objects_rbs has size {}", blocks_stats_rbs.len());
    }
    if blocks_stats_rbs[0].num_rows() != 1 {
        anyhow::bail!(
            "nb_objects_rbs[0] has size {}",
            blocks_stats_rbs[0].num_rows()
        );
    }
    let sub_nb_objects_column: &Int64Array = typed_column(&blocks_stats_rbs[0], 0)?;
    let object_count = sub_nb_objects_column.value(0);
    let block_count_column: &Int64Array = typed_column(&blocks_stats_rbs[0], 1)?;
    let block_count = block_count_column.value(0);
    let max_payload_size_column: &Int64Array = typed_column(&blocks_stats_rbs[0], 2)?;
    let max_payload = max_payload_size_column.value(0);

    Ok(SourceDataBlocks {
        blocks_dataframe: df,
        object_count,
        block_count,
        max_payload,
    })
}
