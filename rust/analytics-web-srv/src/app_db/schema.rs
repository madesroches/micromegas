use anyhow::{Context, Result};
use sqlx::Executor;

async fn create_migration_table(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    sqlx::query("CREATE TABLE migration(version INTEGER NOT NULL);")
        .execute(&mut **tr)
        .await
        .with_context(|| "Creating migration table")?;
    sqlx::query("INSERT INTO migration VALUES(1);")
        .execute(&mut **tr)
        .await
        .with_context(|| "Recording the initial schema version")?;
    Ok(())
}

async fn create_screens_table(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    let sql = "
        CREATE TABLE screens(
            name VARCHAR(255) PRIMARY KEY,
            screen_type VARCHAR(50) NOT NULL,
            config JSONB NOT NULL,
            created_by VARCHAR(255),
            updated_by VARCHAR(255),
            created_at TIMESTAMPTZ DEFAULT NOW(),
            updated_at TIMESTAMPTZ DEFAULT NOW()
        );
        CREATE INDEX screens_screen_type ON screens(screen_type);
        CREATE INDEX screens_created_at ON screens(created_at);
    ";
    tr.execute(sql)
        .await
        .with_context(|| "Creating screens table and indices")?;
    Ok(())
}

pub async fn create_data_sources_table(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    let sql = "
        CREATE TABLE data_sources(
            name VARCHAR(255) PRIMARY KEY,
            config JSONB NOT NULL,
            is_default BOOLEAN NOT NULL DEFAULT FALSE,
            created_by VARCHAR(255) NOT NULL,
            updated_by VARCHAR(255) NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
        CREATE UNIQUE INDEX data_sources_one_default
            ON data_sources (is_default) WHERE is_default = TRUE;
    ";
    tr.execute(sql)
        .await
        .with_context(|| "Creating data_sources table and indices")?;
    Ok(())
}

/// Creates the tables for the micromegas_app database.
pub async fn create_tables(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    create_screens_table(tr).await?;
    create_migration_table(tr).await?;
    Ok(())
}
