use super::{
    partition_source_data::PartitionSourceDataBlocks,
    view::{PartitionSpec, View},
    view_factory::ViewMaker,
};
use crate::{
    lakehouse::partition_source_data::PartitionSourceBlock,
    metadata::{block_from_row, find_process, find_stream},
    span_table::get_spans_schema,
    time::ConvertTicks,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::arrow::datatypes::Schema;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::stream_info::StreamInfo;
use micromegas_tracing::{prelude::*, process_info::ProcessInfo};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

const VIEW_SET_NAME: &str = "thread_spans";

pub struct ThreadSpansViewMaker {}

impl ViewMaker for ThreadSpansViewMaker {
    fn make_view(&self, stream_id: &str) -> Result<Arc<dyn View>> {
        Ok(Arc::new(ThreadSpansView::new(stream_id)?))
    }
}

pub struct ThreadSpansView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    stream_id: sqlx::types::Uuid,
}

impl ThreadSpansView {
    pub fn new(view_instance_id: &str) -> Result<Self> {
        if view_instance_id == "global" {
            anyhow::bail!("the global view is not implemented for thread spans");
        }

        Ok(Self {
            view_set_name: Arc::new(String::from(VIEW_SET_NAME)),
            view_instance_id: Arc::new(String::from(view_instance_id)),
            stream_id: Uuid::parse_str(view_instance_id).with_context(|| "Uuid::parse_str")?,
        })
    }
}

async fn get_jit_partitions(
    connection: &mut sqlx::PgConnection,
    relative_begin_ticks: i64,
    relative_end_ticks: i64,
    stream: Arc<StreamInfo>,
    process: Arc<ProcessInfo>,
) -> Result<Vec<PartitionSourceDataBlocks>> {
    // we go though all the blocks before the end of the query to avoid
    // making a fragmented partition list over time
    let rows = sqlx::query(
            "SELECT block_id, stream_id, process_id, begin_time, end_time, begin_ticks, end_ticks, nb_objects, object_offset, payload_size
             FROM blocks
             WHERE stream_id = $1
             AND begin_ticks <= $2
             ORDER BY begin_ticks;",
        )
        .bind(stream.stream_id)
        .bind(relative_end_ticks)
        .fetch_all(&mut *connection)
        .await
        .with_context(|| "listing blocks")?;

    let mut partitions = vec![];
    let mut partition_blocks = vec![];
    let mut partition_nb_objects: i64 = 0;
    let mut last_block_end_ticks: i64 = 0;
    // we could do a smarter search using object_offset
    for r in rows {
        let block = block_from_row(&r)?;
        last_block_end_ticks = block.end_ticks;
        partition_nb_objects += block.nb_objects as i64;
        partition_blocks.push(Arc::new(PartitionSourceBlock {
            block,
            stream: stream.clone(),
            process: process.clone(),
        }));

        if partition_nb_objects > 1024 * 1024 {
            if last_block_end_ticks > relative_begin_ticks {
                partitions.push(PartitionSourceDataBlocks {
                    blocks: partition_blocks,
                    block_ids_hash: partition_nb_objects.to_le_bytes().to_vec(),
                });
            }
            partition_blocks = vec![];
            partition_nb_objects = 0;
        }
    }
    if partition_nb_objects != 0 && last_block_end_ticks > relative_begin_ticks {
        partitions.push(PartitionSourceDataBlocks {
            blocks: partition_blocks,
            block_ids_hash: partition_nb_objects.to_le_bytes().to_vec(),
        });
    }
    Ok(partitions)
}

fn get_event_time_range(
    convert_ticks: &ConvertTicks,
    spec: &PartitionSourceDataBlocks,
) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    if spec.blocks.is_empty() {
        anyhow::bail!("empty partition should not exist");
    }
    let mut min_rel_ticks = spec.blocks[0].block.begin_ticks;
    let mut max_rel_ticks = spec.blocks[0].block.end_ticks;
    for block in &spec.blocks {
        min_rel_ticks = min_rel_ticks.min(block.block.begin_ticks);
        max_rel_ticks = max_rel_ticks.min(block.block.end_ticks);
    }
    Ok((
        convert_ticks.delta_ticks_to_time(min_rel_ticks),
        convert_ticks.delta_ticks_to_time(max_rel_ticks),
    ))
}

async fn is_partition_up_to_date(
    connection: &mut sqlx::PgConnection,
    convert_ticks: &ConvertTicks,
    view_set_name: &str,
    view_instance_id: &str,
    file_schema_hash: &[u8],
    spec: &PartitionSourceDataBlocks,
) -> Result<bool> {
    let (min_event_time, max_event_time) =
        get_event_time_range(convert_ticks, spec).with_context(|| "get_event_time_range")?;
    let desc = format!(
        "[{}, {}] {view_set_name} {view_instance_id}",
        min_event_time.to_rfc3339(),
        max_event_time.to_rfc3339()
    );

    let rows = sqlx::query(
        "SELECT begin_insert_time, end_insert_time, file_schema_hash, source_data_hash
         FROM lakehouse_partitions
         WHERE view_set_name = $1
         AND view_instance_id = $2
         AND min_event_time < $3
         AND max_event_time > $4
         ;",
    )
    .bind(view_set_name)
    .bind(view_instance_id)
    .bind(max_event_time)
    .bind(min_event_time)
    .fetch_all(connection)
    .await
    .with_context(|| "fetching matching partitions")?;
    if rows.len() != 1 {
        info!("{desc}: found {} partitions", rows.len());
        return Ok(false);
    }
    let r = &rows[0];
    let part_file_schema: Vec<u8> = r.try_get("file_schema_hash")?;
    if part_file_schema != file_schema_hash {
        info!("{desc}: found matching partition with different file schema");
        return Ok(false);
    }
    let part_source_data: Vec<u8> = r.try_get("source_data_hash")?;
    if part_source_data != spec.block_ids_hash {
        info!("{desc}: existing partition do not match source data: creating a new partition");
        return Ok(false);
    }
    info!("{desc}: partition up to date");
    Ok(true)
}

async fn update_partition(
    connection: &mut sqlx::PgConnection,
    convert_ticks: &ConvertTicks,
    view_set_name: &str,
    view_instance_id: &str,
    file_schema_hash: &[u8],
    spec: &PartitionSourceDataBlocks,
) -> Result<()> {
    if is_partition_up_to_date(
        connection,
        convert_ticks,
        view_set_name,
        view_instance_id,
        file_schema_hash,
        spec,
    )
    .await?
    {
        return Ok(());
    }

    Ok(())
}

#[async_trait]
impl View for ThreadSpansView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.view_instance_id.clone()
    }

    async fn make_partition_spec(
        &self,
        _pool: &sqlx::PgPool,
        _begin_insert: DateTime<Utc>,
        _end_insert: DateTime<Utc>,
    ) -> Result<Arc<dyn PartitionSpec>> {
        anyhow::bail!("not implemented")
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![0]
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(get_spans_schema())
    }

    async fn jit_update(
        &self,
        lake: Arc<DataLakeConnection>,
        begin_query: DateTime<Utc>,
        end_query: DateTime<Utc>,
    ) -> Result<()> {
        let mut connection = lake.db_pool.acquire().await?;
        let stream = Arc::new(
            find_stream(&mut connection, self.stream_id)
                .await
                .with_context(|| "find_stream")?,
        );
        let process = Arc::new(
            find_process(&mut connection, &stream.process_id)
                .await
                .with_context(|| "find_process")?,
        );
        let convert_ticks = ConvertTicks::new(&process);
        let relative_begin_ticks = convert_ticks.to_ticks(begin_query - process.start_time);
        let relative_end_ticks = convert_ticks.to_ticks(end_query - process.start_time);
        let partitions = get_jit_partitions(
            &mut connection,
            relative_begin_ticks,
            relative_end_ticks,
            stream.clone(),
            process.clone(),
        )
        .await
        .with_context(|| "get_jit_partitions")?;

        for part in &partitions {
            update_partition(
                &mut connection,
                &convert_ticks,
                &self.view_set_name,
                &self.view_instance_id,
                &self.get_file_schema_hash(),
                part,
            )
            .await
            .with_context(|| "update_partition")?;
        }

        // let row = sqlx::query(
        //     "SELECT sum(nb_objects) as nb_objects
        //      FROM blocks
        //      WHERE stream_id = $1
        //      AND begin_ticks <= $2
        //      AND end_ticks >= $3;",
        // )
        // .bind(self.stream_id)
        // .bind(relative_end_ticks)
        // .bind(relative_begin_ticks)
        // .fetch_one(&mut *connection)
        // .await
        // .with_context(|| "counting objects in range")?;
        // let nb_objects: i64 = row.try_get("nb_objects")?;
        anyhow::bail!("not implemented");
    }
}
