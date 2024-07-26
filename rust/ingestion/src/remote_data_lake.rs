use crate::data_lake_connection::DataLakeConnection;
use crate::sql_migration::execute_migration;
use crate::sql_migration::read_data_lake_schema_version;
use crate::sql_migration::LATEST_DATA_LAKE_SCHEMA_VERSION;
use anyhow::{Context, Result};
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

pub async fn acquire_lock(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>, key: i64) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut **tr)
        .await?;
    Ok(())
}

pub async fn migrate_db(pool: sqlx::Pool<sqlx::Postgres>) -> Result<()> {
    let mut tr = pool.begin().await?;
    let mut current_version = read_data_lake_schema_version(&mut tr).await;
    drop(tr);
    info!("current data lake schema: {}", current_version);
    if current_version != LATEST_DATA_LAKE_SCHEMA_VERSION {
        let mut tr = pool.begin().await?;
        acquire_lock(&mut tr, 0).await?;
        current_version = read_data_lake_schema_version(&mut pool.begin().await?).await;
        if LATEST_DATA_LAKE_SCHEMA_VERSION == current_version {
            return Ok(());
        }
        if let Err(e) = execute_migration(pool.clone()).await {
            error!("Error migrating database: {}", e);
            return Err(e);
        }
        current_version = read_data_lake_schema_version(&mut tr).await;
    }
    assert_eq!(current_version, LATEST_DATA_LAKE_SCHEMA_VERSION);
    Ok(())
}

pub async fn connect_to_remote_data_lake(
    db_uri: &str,
    object_store_url: &str,
) -> Result<DataLakeConnection> {
    info!("connecting to blob storage");
    let blob_storage = Arc::new(
        BlobStorage::connect(object_store_url).with_context(|| "connecting to blob storage")?,
    );
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(db_uri)
        .await
        .with_context(|| String::from("Connecting to telemetry database"))?;
    migrate_db(pool.clone()).await?;
    Ok(DataLakeConnection::new(pool, blob_storage))
}
