use super::view::View;
use anyhow::{Context, Result};
use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::{error, info};
use sqlx::Row;
use std::sync::Arc;

async fn count_equal_partitions(
    pool: &sqlx::PgPool,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    table_set_name: &str,
    table_instance_id: &str,
    block_ids_hash: &[u8],
) -> Result<i64> {
    let count: i64 = sqlx::query(
        "SELECT count(*) as count
         FROM lakehouse_partitions
         WHERE table_set_name = $1
         AND table_instance_id = $2
         AND begin_insert_time = $3
         AND end_insert_time = $4
         AND source_data_hash = $5
         ;",
    )
    .bind(table_set_name)
    .bind(table_instance_id)
    .bind(begin_insert)
    .bind(end_insert)
    .bind(block_ids_hash)
    .fetch_one(pool)
    .await
    .with_context(|| "counting matching partitions")?
    .try_get("count")?;
    Ok(count)
}

async fn create_or_update_partition(
    lake: Arc<DataLakeConnection>,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    view: Arc<dyn View>,
) -> Result<()> {
    let table_set_name = view.get_table_set_name();
    let partition_spec = view
        .make_partition_spec(&lake.db_pool, begin_insert, end_insert)
        .await
        .with_context(|| "make_partition_spec")?;
    let table_instance_id = view.get_table_instance_id();
    let nb_equal_partitions = count_equal_partitions(
        &lake.db_pool,
        begin_insert,
        end_insert,
        &table_set_name,
        &table_instance_id,
        &partition_spec.get_source_data_hash(),
    )
    .await?;

    if nb_equal_partitions == 1 {
        info!("partition up to date, no need to replace it");
        return Ok(());
    }
    if nb_equal_partitions > 1 {
        error!("too many partitions for the same time range");
        return Ok(()); // continue with the rest of the process, the error has been logged
    }

    partition_spec
        .write(lake)
        .await
        .with_context(|| "writing partition")?;
    Ok(())
}

pub async fn create_or_update_minute_partitions(
    lake: Arc<DataLakeConnection>,
    view: Arc<dyn View>,
) -> Result<()> {
    let now = Utc::now();
    let one_minute = TimeDelta::try_minutes(1).with_context(|| "making a minute")?;
    let truncated = now.duration_trunc(one_minute)?;
    let nb_minute_partitions: i32 = 15;
    let start = truncated
        - TimeDelta::try_minutes(nb_minute_partitions as i64)
            .with_context(|| "making time delta")?;
    for index in 0..nb_minute_partitions {
        let start_partition = start + one_minute * index;
        let end_partition = start + one_minute * (index + 1);
        create_or_update_partition(lake.clone(), start_partition, end_partition, view.clone())
            .await
            .with_context(|| "create_or_update_partition")?;
    }
    Ok(())
}
