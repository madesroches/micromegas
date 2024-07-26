use anyhow::{Context, Result};
use chrono::{TimeDelta, Utc};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use sqlx::Row;

pub struct Partition {
    pub table_set_name: String,
    pub table_instance_id: String,
    pub begin_insert_time: chrono::DateTime<chrono::Utc>,
    pub end_insert_time: chrono::DateTime<chrono::Utc>,
    pub min_event_time: chrono::DateTime<chrono::Utc>,
    pub max_event_time: chrono::DateTime<chrono::Utc>,
    pub updated: chrono::DateTime<chrono::Utc>,
    pub file_path: String,
    pub file_size: i64,
    pub file_schema_hash: Vec<u8>,
    pub source_data_hash: Vec<u8>,
}

pub async fn write_partition(
    lake: &DataLakeConnection,
    partition_metadata: &Partition,
    contents: bytes::Bytes,
) -> Result<()> {
    lake.blob_storage
        .put(&partition_metadata.file_path, contents)
        .await
        .with_context(|| "writing partition to object storage")?;
    let mut tr = lake.db_pool.begin().await?;
    sqlx::query(
        "INSERT INTO lakehouse_partitions VALUES($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11);",
    )
    .bind(&partition_metadata.table_set_name)
    .bind(&partition_metadata.table_instance_id)
    .bind(partition_metadata.begin_insert_time)
    .bind(partition_metadata.end_insert_time)
    .bind(partition_metadata.min_event_time)
    .bind(partition_metadata.max_event_time)
    .bind(partition_metadata.updated)
    .bind(&partition_metadata.file_path)
    .bind(partition_metadata.file_size)
    .bind(&partition_metadata.file_schema_hash)
    .bind(&partition_metadata.source_data_hash)
    .execute(&mut *tr)
    .await
    .with_context(|| "inserting into lakehouse_partitions")?;

    let old_partitions = sqlx::query(
        "SELECT file_path, file_size
         FROM lakehouse_partitions
         WHERE table_set_name = $1
         AND table_instance_id = $2
         AND begin_insert_time = $3
         AND end_insert_time = $4
         AND source_data_hash != $5
         ;",
    )
    .bind(&partition_metadata.table_set_name)
    .bind(&partition_metadata.table_instance_id)
    .bind(partition_metadata.begin_insert_time)
    .bind(partition_metadata.end_insert_time)
    .bind(&partition_metadata.source_data_hash)
    .fetch_all(&mut *tr)
    .await
    .with_context(|| "listing old partitions")?;
    for old_part in old_partitions {
        let file_path: String = old_part.try_get("file_path")?;
        let file_size: i64 = old_part.try_get("file_size")?;
        let expiration = Utc::now() + TimeDelta::try_hours(1).with_context(|| "making one hour")?;
        info!("adding out of date partition {file_path} to temporary files to be deleted");
        sqlx::query("INSERT INTO temporary_files VALUES ($1, $2, $3);")
            .bind(file_path)
            .bind(file_size)
            .bind(expiration)
            .execute(&mut *tr)
            .await
            .with_context(|| "adding old partition to temporary files to be deleted")?;
    }

    sqlx::query(
        "DELETE from lakehouse_partitions
         WHERE table_set_name = $1
         AND table_instance_id = $2
         AND begin_insert_time = $3
         AND end_insert_time = $4
         AND source_data_hash != $5
         ;",
    )
    .bind(&partition_metadata.table_set_name)
    .bind(&partition_metadata.table_instance_id)
    .bind(partition_metadata.begin_insert_time)
    .bind(partition_metadata.end_insert_time)
    .bind(&partition_metadata.source_data_hash)
    .execute(&mut *tr)
    .await
    .with_context(|| "deleting out of date partitions")?;

    tr.commit().await.with_context(|| "commit")?;
    Ok(())
}
