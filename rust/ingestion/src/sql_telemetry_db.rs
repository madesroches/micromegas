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

async fn create_property_type(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    let sql = "CREATE TYPE micromegas_property as (key TEXT, value TEXT);";
    tr.execute(sql)
        .await
        .with_context(|| String::from("Creating property type"))?;
    Ok(())
}

async fn create_processes_table(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    let sql = "
         CREATE TABLE processes(
                  process_id UUID, 
                  exe VARCHAR(255), 
                  username VARCHAR(255), 
                  realname VARCHAR(255), 
                  computer VARCHAR(255), 
                  distro VARCHAR(255), 
                  cpu_brand VARCHAR(255), 
                  tsc_frequency BIGINT,
                  start_time TIMESTAMPTZ,
                  start_ticks BIGINT,
                  insert_time TIMESTAMPTZ,
                  parent_process_id UUID,
                  properties micromegas_property[]
                  );
         CREATE INDEX process_id on processes(process_id);
         CREATE INDEX parent_process_id on processes(parent_process_id);
         CREATE INDEX process_start_time on processes(start_time);";
    tr.execute(sql)
        .await
        .with_context(|| String::from("Creating table processes and its indices"))?;
    Ok(())
}

async fn create_streams_table(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    let sql = "
         CREATE TABLE streams(
                  stream_id UUID, 
                  process_id UUID, 
                  dependencies_metadata BYTEA,
                  objects_metadata BYTEA,
                  tags TEXT[],
                  properties micromegas_property[],
                  insert_time TIMESTAMPTZ
                  );
         CREATE INDEX stream_id on streams(stream_id);
         CREATE INDEX stream_process_id on streams(process_id);
         CREATE INDEX stream_insert_time on streams(insert_time);";
    tr.execute(sql)
        .await
        .with_context(|| String::from("Creating table streams and its indices"))?;
    Ok(())
}

async fn create_blocks_table(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    // begin_ticks and end_ticks are relative to the start of the process
    let sql = "
         CREATE TABLE blocks(
                  block_id UUID, 
                  stream_id UUID, 
                  process_id UUID, 
                  begin_time TIMESTAMPTZ,
                  begin_ticks BIGINT,
                  end_time TIMESTAMPTZ,
                  end_ticks BIGINT,
                  nb_objects INT,
                  object_offset BIGINT,
                  payload_size BIGINT
                  );
         CREATE INDEX block_id on blocks(block_id);
         CREATE INDEX block_stream_id on blocks(stream_id);";
    tr.execute(sql)
        .await
        .with_context(|| String::from("Creating table blocks and its indices"))?;
    Ok(())
}

/// Creates the tables for the telemetry database.
pub async fn create_tables(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    create_property_type(tr).await?;
    create_processes_table(tr).await?;
    create_streams_table(tr).await?;
    create_blocks_table(tr).await?;
    create_migration_table(tr).await?;
    Ok(())
}
