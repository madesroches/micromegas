use crate::data_lake_connection::DataLakeConnection;
use crate::sql_migration::execute_migration;
use crate::sql_migration::read_schema_version;
use crate::sql_migration::LATEST_SCHEMA_VERSION;
use anyhow::{bail, Context, Result};
use lgn_blob_storage::{AwsS3BlobStorage, AwsS3Url};
use tracing::prelude::*;
use sqlx::migrate::MigrateDatabase;
use sqlx::Row;
use std::str::FromStr;
use std::sync::Arc;

async fn acquire_lock(connection: &mut sqlx::AnyConnection, name: &str) -> Result<()> {
    let row = sqlx::query("SELECT GET_LOCK(?, -1) as result;")
        .bind(name)
        .fetch_one(&mut *connection)
        .await?;
    let result: i32 = row.get("result");
    if result != 1 {
        bail!("Error acquiring lock");
    }
    Ok(())
}

async fn release_lock(connection: &mut sqlx::AnyConnection, name: &str) -> Result<()> {
    let row = sqlx::query("SELECT RELEASE_LOCK(?) as result;")
        .bind(name)
        .fetch_one(&mut *connection)
        .await?;
    let result: i32 = row.get("result");
    if result != 1 {
        bail!("Error releasing lock");
    }
    Ok(())
}

async fn migrate_db(connection: &mut sqlx::AnyConnection) -> Result<()> {
    let mut current_version = read_schema_version(connection).await;
    info!("current schema: {}", current_version);
    if current_version != LATEST_SCHEMA_VERSION {
        acquire_lock(connection, "migration").await?;
        current_version = read_schema_version(connection).await;
        if LATEST_SCHEMA_VERSION == current_version {
            release_lock(connection, "migration").await?;
            return Ok(());
        }
        if let Err(e) = execute_migration(connection).await {
            error!("Error migrating database: {}", e);
            release_lock(connection, "migration").await?;
            return Err(e);
        }
        current_version = read_schema_version(connection).await;
        release_lock(connection, "migration").await?;
    }
    assert_eq!(current_version, LATEST_SCHEMA_VERSION);
    Ok(())
}

pub async fn connect_to_remote_data_lake(db_uri: &str, s3_url: &str) -> Result<DataLakeConnection> {
    info!("connecting to blob storage");
    let blob_storage = AwsS3BlobStorage::new(AwsS3Url::from_str(s3_url)?).await;
    if !sqlx::Any::database_exists(db_uri)
        .await
        .with_context(|| String::from("Searching for telemetry database"))?
    {
        sqlx::Any::create_database(db_uri)
            .await
            .with_context(|| String::from("Creating telemetry database"))?;
    }
    let pool = sqlx::any::AnyPoolOptions::new()
        .connect(db_uri)
        .await
        .with_context(|| String::from("Connecting to telemetry database"))?;
    let mut connection = pool.acquire().await?;
    migrate_db(&mut connection).await?;
    Ok(DataLakeConnection::new(pool, Arc::new(blob_storage)))
}
