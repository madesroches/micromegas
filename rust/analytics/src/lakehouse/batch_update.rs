use super::view::View;
use anyhow::{Context, Result};
use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use sqlx::Row;
use std::sync::Arc;

// verify_overlapping_partitions returns true to continue and make a new partition,
// returns false to abort (existing partition is up to date or there is a problem)
async fn verify_overlapping_partitions(
    pool: &sqlx::PgPool,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    view_set_name: &str,
    view_instance_id: &str,
    file_schema_hash: &[u8],
    source_data_hash: &[u8],
) -> Result<bool> {
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
        return Ok(true);
    }
    let mut matching_needs_update = false;
    for r in rows {
        let begin: DateTime<Utc> = r.try_get("begin_insert_time")?;
        let end: DateTime<Utc> = r.try_get("end_insert_time")?;
        if begin != begin_insert || end != end_insert {
            info!("found overlapping partition of different size, aborting the update");
            return Ok(false);
        }
        let part_file_schema: Vec<u8> = r.try_get("file_schema_hash")?;
        if part_file_schema != file_schema_hash {
            info!("found matching partition with different file schema");
            matching_needs_update = true;
        }
        let part_source_data: Vec<u8> = r.try_get("source_data_hash")?;
        if part_source_data != source_data_hash {
            info!("found matching partition with different source data");
            matching_needs_update = true;
        }
    }
    info!("partition is up to date {view_set_name} {view_instance_id} {begin_insert} {end_insert}");
    Ok(matching_needs_update)
}

async fn create_or_update_partition(
    lake: Arc<DataLakeConnection>,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    view: Arc<dyn View>,
) -> Result<()> {
    let view_set_name = view.get_view_set_name();
    let partition_spec = view
        .make_partition_spec(&lake.db_pool, begin_insert, end_insert)
        .await
        .with_context(|| "make_partition_spec")?;
    let view_instance_id = view.get_view_instance_id();
    let continue_with_creation = verify_overlapping_partitions(
        &lake.db_pool,
        begin_insert,
        end_insert,
        &view_set_name,
        &view_instance_id,
        &view.get_file_schema_hash(),
        &partition_spec.get_source_data_hash(),
    )
    .await?;

    if !continue_with_creation {
        return Ok(());
    }

    partition_spec
        .write(lake)
        .await
        .with_context(|| "writing partition")?;
    Ok(())
}

pub async fn create_or_update_recent_partitions(
    lake: Arc<DataLakeConnection>,
    view: Arc<dyn View>,
    partition_time_delta: TimeDelta,
    nb_partitions: i32,
) -> Result<()> {
    let now = Utc::now();
    let truncated = now.duration_trunc(partition_time_delta)?;
    let start = truncated - partition_time_delta * nb_partitions;
    for index in 0..nb_partitions {
        let start_partition = start + partition_time_delta * index;
        let end_partition = start + partition_time_delta * (index + 1);
        create_or_update_partition(lake.clone(), start_partition, end_partition, view.clone())
            .await
            .with_context(|| "create_or_update_partition")?;
    }
    Ok(())
}

pub async fn create_or_update_partitions(
    lake: Arc<DataLakeConnection>,
    view: Arc<dyn View>,
    begin_range: DateTime<Utc>,
    end_range: DateTime<Utc>,
    partition_time_delta: TimeDelta,
) -> Result<()> {
    let mut begin_part = begin_range;
    let mut end_part = begin_part + partition_time_delta;
    while end_part <= end_range {
        create_or_update_partition(lake.clone(), begin_part, end_part, view.clone())
            .await
            .with_context(|| "create_or_update_partition")?;
        begin_part = end_part;
        end_part = begin_part + partition_time_delta;
    }
    Ok(())
}
