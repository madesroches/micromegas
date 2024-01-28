use anyhow::{Context, Result};
use lgn_blob_storage::LocalBlobStorage;
use sqlx::migrate::MigrateDatabase;
use std::{path::PathBuf, sync::Arc};

use crate::{data_lake_connection::DataLakeConnection, sql_migration::execute_migration};

pub async fn connect_to_local_data_lake(path: PathBuf) -> Result<DataLakeConnection> {
    let blocks_folder = path.join("blobs");
    let blob_storage = LocalBlobStorage::new(blocks_folder).await?;
    let db_path = path.join("telemetry.db3");
    let db_uri = format!("sqlite://{}", db_path.to_str().unwrap().replace('\\', "/"));
    if !sqlx::Any::database_exists(&db_uri)
        .await
        .with_context(|| String::from("Searching for telemetry database"))?
    {
        sqlx::Any::create_database(&db_uri)
            .await
            .with_context(|| String::from("Creating telemetry database"))?;
    }
    let pool = sqlx::any::AnyPoolOptions::new()
        .connect(&db_uri)
        .await
        .with_context(|| String::from("Connecting to telemetry database"))?;
    let mut connection = pool.acquire().await?;
    execute_migration(&mut connection).await?;
    Ok(DataLakeConnection::new(pool, Arc::new(blob_storage)))
}
