use super::partition_cache::PartitionCache;
use crate::{
    dfext::typed_column::typed_column_by_name,
    lakehouse::{blocks_view::blocks_view_schema, query::query_partitions},
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, BinaryArray, GenericListArray, Int32Array, Int64Array, StringArray,
    StructArray, TimestampNanosecondArray,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::property::Property;
use micromegas_telemetry::{stream_info::StreamInfo, types::block::BlockMetadata};
use micromegas_tracing::prelude::*;
use std::sync::Arc;
use uuid::Uuid;

pub struct PartitionSourceBlock {
    pub block: BlockMetadata,
    pub stream: Arc<StreamInfo>,
    pub process: Arc<ProcessInfo>,
}

pub struct PartitionSourceDataBlocks {
    pub blocks: Vec<Arc<PartitionSourceBlock>>,
    pub block_ids_hash: Vec<u8>,
}

pub fn hash_to_object_count(hash: &[u8]) -> Result<i64> {
    Ok(i64::from_le_bytes(
        hash.try_into().with_context(|| "hash_to_object_count")?,
    ))
}

fn read_property_list(value: ArrayRef) -> Result<Vec<Property>> {
    let properties: &StructArray = value.as_struct();
    let (key_index, _key_field) = properties
        .fields()
        .find("key")
        .with_context(|| "getting key field")?;
    let (value_index, _value_field) = properties
        .fields()
        .find("value")
        .with_context(|| "getting value field")?;

    let mut properties_vec = vec![];
    for i in 0..properties.len() {
        let key = properties.column(key_index).as_string::<i32>().value(i);
        let value = properties.column(value_index).as_string::<i32>().value(i);
        properties_vec.push(Property::new(Arc::new(key.into()), Arc::new(value.into())));
    }
    Ok(properties_vec)
}

pub async fn fetch_partition_source_data(
    lake: Arc<DataLakeConnection>,
    existing_partitions: Arc<PartitionCache>,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    source_stream_tag: &str,
) -> Result<PartitionSourceDataBlocks> {
    let begin_rfc = begin_insert.to_rfc3339();
    let end_rfc = end_insert.to_rfc3339();
    let sql = format!(
        r#"
          SELECT block_id, stream_id, process_id, begin_time, begin_ticks, end_time, end_ticks, nb_objects,
              object_offset, payload_size, insert_time as block_insert_time,
              "streams.dependencies_metadata", "streams.objects_metadata", "streams.tags", "streams.properties",
              "processes.start_time", "processes.start_ticks", "processes.tsc_frequency", "processes.exe",
              "processes.username", "processes.realname", "processes.computer", "processes.distro", "processes.cpu_brand",
              "processes.parent_process_id", "processes.properties"
          FROM source
          WHERE array_has( "streams.tags", '{source_stream_tag}' )
          AND insert_time >= '{begin_rfc}'
          AND insert_time < '{end_rfc}'
          ORDER BY insert_time, block_id
          ;"#
    );
    let mut block_ids_hash: i64 = 0;
    let mut partition_src_blocks = vec![];
    let block_partitions = existing_partitions
        .filter("blocks", "global", begin_insert, end_insert)
        .partitions;
    let df = query_partitions(
        lake.clone(),
        Arc::new(blocks_view_schema()),
        block_partitions,
        &sql,
    )
    .await
    .with_context(|| "blocks query")?;
    let record_batches = df.collect().await?;
    for b in record_batches {
        let block_id_column: &StringArray = typed_column_by_name(&b, "block_id")?;
        let stream_id_column: &StringArray = typed_column_by_name(&b, "stream_id")?;
        let process_id_column: &StringArray = typed_column_by_name(&b, "process_id")?;
        let begin_time_column: &TimestampNanosecondArray = typed_column_by_name(&b, "begin_time")?;
        let begin_ticks_column: &Int64Array = typed_column_by_name(&b, "begin_ticks")?;
        let end_time_column: &TimestampNanosecondArray = typed_column_by_name(&b, "end_time")?;
        let end_ticks_column: &Int64Array = typed_column_by_name(&b, "end_ticks")?;
        let nb_objects_column: &Int32Array = typed_column_by_name(&b, "nb_objects")?;
        let object_offset_column: &Int64Array = typed_column_by_name(&b, "object_offset")?;
        let payload_size_column: &Int64Array = typed_column_by_name(&b, "payload_size")?;
        let block_insert_time_column: &TimestampNanosecondArray =
            typed_column_by_name(&b, "block_insert_time")?;
        let dependencies_metadata_column: &BinaryArray =
            typed_column_by_name(&b, "streams.dependencies_metadata")?;
        let objects_metadata_column: &BinaryArray =
            typed_column_by_name(&b, "streams.objects_metadata")?;
        let stream_tags_column: &GenericListArray<i32> = typed_column_by_name(&b, "streams.tags")?;
        let stream_properties_column: &GenericListArray<i32> =
            typed_column_by_name(&b, "streams.properties")?;

        let process_start_time_column: &TimestampNanosecondArray =
            typed_column_by_name(&b, "processes.start_time")?;
        let process_start_ticks_column: &Int64Array =
            typed_column_by_name(&b, "processes.start_ticks")?;
        let process_tsc_freq_column: &Int64Array =
            typed_column_by_name(&b, "processes.tsc_frequency")?;
        let process_exe_column: &StringArray = typed_column_by_name(&b, "processes.exe")?;
        let process_username_column: &StringArray = typed_column_by_name(&b, "processes.username")?;
        let process_realname_column: &StringArray = typed_column_by_name(&b, "processes.realname")?;
        let process_computer_column: &StringArray = typed_column_by_name(&b, "processes.computer")?;
        let process_distro_column: &StringArray = typed_column_by_name(&b, "processes.distro")?;
        let process_cpu_column: &StringArray = typed_column_by_name(&b, "processes.cpu_brand")?;
        let process_parent_column: &StringArray =
            typed_column_by_name(&b, "processes.parent_process_id")?;
        let process_properties_column: &GenericListArray<i32> =
            typed_column_by_name(&b, "processes.properties")?;

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

            let stream_properties = read_property_list(stream_properties_column.value(ir))?;
            let stream = StreamInfo {
                process_id,
                stream_id,
                dependencies_metadata: ciborium::from_reader(dependencies_metadata)
                    .with_context(|| "decoding dependencies_metadata")?,
                objects_metadata: ciborium::from_reader(objects_metadata)
                    .with_context(|| "decoding objects_metadata")?,
                tags: stream_tags,
                properties: micromegas_telemetry::property::into_hashmap(stream_properties),
            };
            let process_properties = read_property_list(process_properties_column.value(ir))?;
            let parent_value = process_parent_column.value(ir);
            let parent_process_id = if parent_value.is_empty() {
                None
            } else {
                Some(Uuid::parse_str(parent_value).with_context(|| "parsing parent process_id")?)
            };
            let process = ProcessInfo {
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
                properties: micromegas_telemetry::property::into_hashmap(process_properties),
            };
            block_ids_hash += block.nb_objects as i64;
            partition_src_blocks.push(Arc::new(PartitionSourceBlock {
                block,
                stream: stream.into(),
                process: process.into(),
            }));
        }
    }
    Ok(PartitionSourceDataBlocks {
        blocks: partition_src_blocks,
        block_ids_hash: block_ids_hash.to_le_bytes().to_vec(),
    })
}
