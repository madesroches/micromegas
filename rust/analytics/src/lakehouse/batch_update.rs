use crate::response_writer::ResponseWriter;

use super::{
    merge::create_merged_partition, partition_source_data::hash_to_object_count, view::View,
};
use anyhow::{Context, Result};
use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use sqlx::Row;
use std::sync::Arc;

pub enum PartitionCreationStrategy {
    CreateFromSource,
    MergeExisting,
    Abort,
}

// verify_overlapping_partitions returns true to continue and make a new partition,
// returns false to abort (existing partition is up to date or there is a problem)
#[allow(clippy::too_many_arguments)]
async fn verify_overlapping_partitions(
    pool: &sqlx::PgPool,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    view_set_name: &str,
    view_instance_id: &str,
    file_schema_hash: &[u8],
    source_data_hash: &[u8],
    writer: Arc<ResponseWriter>,
) -> Result<PartitionCreationStrategy> {
    let desc = format!(
        "[{}, {}] {view_set_name} {view_instance_id}",
        begin_insert.to_rfc3339(),
        end_insert.to_rfc3339()
    );
    if source_data_hash.len() != std::mem::size_of::<i64>() {
        anyhow::bail!("Source data hash should be a i64");
    }
    let nb_source_events = hash_to_object_count(source_data_hash)?;
    let rows = sqlx::query(
        "SELECT begin_insert_time, end_insert_time, file_schema_hash, source_data_hash
         FROM lakehouse_partitions
         WHERE view_set_name = $1
         AND view_instance_id = $2
         AND begin_insert_time < $3
         AND end_insert_time > $4
         ;",
    )
    .bind(view_set_name)
    .bind(view_instance_id)
    .bind(end_insert)
    .bind(begin_insert)
    .fetch_all(pool)
    .await
    .with_context(|| "fetching matching partitions")?;
    if rows.is_empty() {
        writer
            .write_string(&format!("{desc}: matching partitions not found"))
            .await?;
        return Ok(PartitionCreationStrategy::CreateFromSource);
    }
    let mut existing_source_hash: i64 = 0;
    let nb_existing_partitions = rows.len();
    for r in rows {
        let begin: DateTime<Utc> = r.try_get("begin_insert_time")?;
        let end: DateTime<Utc> = r.try_get("end_insert_time")?;
        if begin < begin_insert || end > end_insert {
            writer
                .write_string(&format!(
                    "{desc}: found overlapping partition [{}, {}], aborting the update",
                    begin.to_rfc3339(),
                    end.to_rfc3339()
                ))
                .await?;
            return Ok(PartitionCreationStrategy::Abort);
        }
        let part_file_schema: Vec<u8> = r.try_get("file_schema_hash")?;
        if part_file_schema != file_schema_hash {
            writer
                .write_string(&format!(
                    "{desc}: found matching partition with different file schema"
                ))
                .await?;
            return Ok(PartitionCreationStrategy::CreateFromSource);
        }
        let part_source_data: Vec<u8> = r.try_get("source_data_hash")?;
        if part_source_data.len() == std::mem::size_of::<i64>() {
            existing_source_hash += hash_to_object_count(&part_source_data)?
        } else {
            // old hash that does not represent the number of events
            writer
                .write_string(&format!(
                    "{desc}: found partition with incompatible source hash: recreate"
                ))
                .await?;
            return Ok(PartitionCreationStrategy::CreateFromSource);
        }
    }

    if nb_source_events != existing_source_hash {
        writer
            .write_string(&format!(
                "{desc}: existing partitions do not match source data: creating a new partition"
            ))
            .await?;
        return Ok(PartitionCreationStrategy::CreateFromSource);
    }

    if nb_existing_partitions > 1 {
        writer
            .write_string(&format!("{desc}: merging existing partitions"))
            .await?;
        return Ok(PartitionCreationStrategy::MergeExisting);
    }

    writer
        .write_string(&format!("{desc}: already up to date"))
        .await?;
    Ok(PartitionCreationStrategy::Abort)
}

async fn materialize_partition(
    lake: Arc<DataLakeConnection>,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    view: Arc<dyn View>,
    writer: Arc<ResponseWriter>,
) -> Result<()> {
    let view_set_name = view.get_view_set_name();
    let partition_spec = view
        .make_batch_partition_spec(&lake.db_pool, begin_insert, end_insert)
        .await
        .with_context(|| "make_partition_spec")?;
    let view_instance_id = view.get_view_instance_id();
    let strategy = verify_overlapping_partitions(
        &lake.db_pool,
        begin_insert,
        end_insert,
        &view_set_name,
        &view_instance_id,
        &view.get_file_schema_hash(),
        &partition_spec.get_source_data_hash(),
        writer.clone(),
    )
    .await?;

    match strategy {
        PartitionCreationStrategy::CreateFromSource => {
            partition_spec
                .write(lake, writer)
                .await
                .with_context(|| "writing partition")?;
        }
        PartitionCreationStrategy::MergeExisting => {
            create_merged_partition(lake, view, begin_insert, end_insert, writer).await?;
        }
        PartitionCreationStrategy::Abort => {}
    }

    Ok(())
}

pub async fn materialize_recent_partitions(
    lake: Arc<DataLakeConnection>,
    view: Arc<dyn View>,
    partition_time_delta: TimeDelta,
    nb_partitions: i32,
    writer: Arc<ResponseWriter>,
) -> Result<()> {
    let now = Utc::now();
    let truncated = now.duration_trunc(partition_time_delta)?;
    let start = truncated - partition_time_delta * nb_partitions;
    for index in 0..nb_partitions {
        let start_partition = start + partition_time_delta * index;
        let end_partition = start + partition_time_delta * (index + 1);
        materialize_partition(
            lake.clone(),
            start_partition,
            end_partition,
            view.clone(),
            writer.clone(),
        )
        .await
        .with_context(|| "create_or_update_partition")?;
    }
    Ok(())
}

pub async fn materialize_partition_range(
    lake: Arc<DataLakeConnection>,
    view: Arc<dyn View>,
    begin_range: DateTime<Utc>,
    end_range: DateTime<Utc>,
    partition_time_delta: TimeDelta,
    writer: Arc<ResponseWriter>,
) -> Result<()> {
    let mut begin_part = begin_range;
    let mut end_part = begin_part + partition_time_delta;
    while end_part <= end_range {
        materialize_partition(
            lake.clone(),
            begin_part,
            end_part,
            view.clone(),
            writer.clone(),
        )
        .await
        .with_context(|| "materialize_partition")?;
        begin_part = end_part;
        end_part = begin_part + partition_time_delta;
    }
    Ok(())
}
