use anyhow::{Context, Result};
use sqlx::Executor;

async fn create_migration_table(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    sqlx::query("CREATE table migration(version integer);")
        .execute(&mut **tr)
        .await
        .with_context(|| String::from("Creating table migration"))?;
    sqlx::query("INSERT INTO migration VALUES(1);")
        .execute(&mut **tr)
        .await
        .with_context(|| String::from("Recording the initial schema version"))?;
    Ok(())
}

async fn create_processes_table(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    let sql = "
         CREATE TABLE processes(
                  process_id VARCHAR(36), 
                  exe VARCHAR(255), 
                  username VARCHAR(255), 
                  realname VARCHAR(255), 
                  computer VARCHAR(255), 
                  distro VARCHAR(255), 
                  cpu_brand VARCHAR(255), 
                  tsc_frequency BIGINT,
                  start_time VARCHAR(255),
                  start_ticks BIGINT,
                  insert_time VARCHAR(255),
                  parent_process_id VARCHAR(36));
         CREATE INDEX process_id on processes(process_id);
         CREATE INDEX parent_process_id on processes(parent_process_id);
         CREATE INDEX process_insert_time on processes(insert_time);";
    tr.execute(sql)
        .await
        .with_context(|| String::from("Creating table processes and its indices"))?;
    Ok(())
}

async fn create_streams_table(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    // storing tags as text is simplistic - we should move to a tags table if we
    // keep the telemetry metadata in a SQL db
    let sql = "
         CREATE TABLE streams(
                  stream_id VARCHAR(36), 
                  process_id VARCHAR(36), 
                  dependencies_metadata BYTEA,
                  objects_metadata BYTEA,
                  tags TEXT,
                  properties TEXT
                  );
         CREATE INDEX stream_id on streams(stream_id);
         CREATE INDEX stream_process_id on streams(process_id);";
    tr.execute(sql)
        .await
        .with_context(|| String::from("Creating table streams and its indices"))?;
    Ok(())
}

async fn create_blocks_table(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    let sql = "
         CREATE TABLE blocks(
                  block_id VARCHAR(36), 
                  stream_id VARCHAR(36), 
                  process_id VARCHAR(36), 
                  begin_time VARCHAR(255),
                  begin_ticks BIGINT,
                  end_time VARCHAR(255),
                  end_ticks BIGINT,
                  nb_objects INT
                  );
         CREATE INDEX block_id on blocks(block_id);
         CREATE INDEX block_stream_id on blocks(stream_id);";
    tr.execute(sql)
        .await
        .with_context(|| String::from("Creating table blocks and its indices"))?;
    Ok(())
}

pub async fn create_tables(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    create_processes_table(tr).await?;
    create_streams_table(tr).await?;
    create_blocks_table(tr).await?;
    create_migration_table(tr).await?;
    Ok(())
}
