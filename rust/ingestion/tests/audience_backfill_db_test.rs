//! DB-backed tests for `backfill_default_audience` (#1482 §0.2) -- the idempotent `UPDATE` that
//! stamps `micromegas.audience` onto legacy `processes` rows at every ingestion-service startup.
//! Requires a live `MICROMEGAS_SQL_CONNECTION_STRING` / `MICROMEGAS_OBJECT_STORE_URI` (see
//! `insert_block_dedup_db_test.rs` for the same harness pattern); does not run under a plain
//! `cargo test`.

use anyhow::{Context, Result};
use micromegas_ingestion::audience_backfill::backfill_default_audience;
use micromegas_ingestion::data_lake_connection::connect_to_data_lake;
use micromegas_ingestion::write_audience::WriteAudience;
use micromegas_telemetry::property::{PROPERTY_AUDIENCE, Property, make_properties};
use uuid::Uuid;

async fn connect() -> Result<micromegas_ingestion::data_lake_connection::DataLakeConnection> {
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    connect_to_data_lake(&connection_string, &object_store_uri).await
}

/// Inserts a bare `processes` row with the given `properties` (`None` ⇒ `NULL` properties column
/// -- the legacy, never-populated shape; `Some` ⇒ an explicit property list).
async fn insert_process_row(
    pool: &sqlx::PgPool,
    process_id: Uuid,
    properties: Option<Vec<Property>>,
) -> Result<()> {
    let now = sqlx::types::chrono::Utc::now();
    sqlx::query(
        "INSERT INTO processes VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) ON CONFLICT (process_id) DO NOTHING;",
    )
    .bind(process_id)
    .bind("exe")
    .bind("username")
    .bind("username")
    .bind("computer")
    .bind("distro")
    .bind("cpu_brand")
    .bind(1_000_000_000_i64)
    .bind(now)
    .bind(0_i64)
    .bind(now)
    .bind(Option::<Uuid>::None)
    .bind(properties)
    .execute(pool)
    .await
    .with_context(|| "inserting processes row")?;
    Ok(())
}

async fn read_audience_property(pool: &sqlx::PgPool, process_id: Uuid) -> Result<Option<String>> {
    let properties: Option<Vec<Property>> =
        sqlx::query_scalar("SELECT properties FROM processes WHERE process_id = $1")
            .bind(process_id)
            .fetch_one(pool)
            .await
            .with_context(|| "reading process properties")?;
    Ok(properties
        .unwrap_or_default()
        .iter()
        .find(|p| p.key_str() == PROPERTY_AUDIENCE)
        .map(|p| p.value_str().to_string()))
}

/// Stamps a never-stamped (`NULL`-properties) row and a `properties = []` row with the
/// configured default, and leaves an already-stamped row untouched; a second run changes
/// nothing (idempotency is the property the startup re-run relies on).
#[ignore]
#[tokio::test]
async fn backfill_stamps_unstamped_rows_and_is_idempotent() -> Result<()> {
    let lake = connect().await?;

    let null_properties_id = Uuid::new_v4();
    insert_process_row(&lake.db_pool, null_properties_id, None).await?;

    let empty_properties_id = Uuid::new_v4();
    insert_process_row(&lake.db_pool, empty_properties_id, Some(vec![])).await?;

    let already_stamped_id = Uuid::new_v4();
    let mut already_stamped_props = std::collections::HashMap::new();
    already_stamped_props.insert(PROPERTY_AUDIENCE.to_string(), "team-a".to_string());
    insert_process_row(
        &lake.db_pool,
        already_stamped_id,
        Some(make_properties(&already_stamped_props)),
    )
    .await?;

    let default_audience = WriteAudience::new("team-default")?;
    backfill_default_audience(&lake.db_pool, &default_audience).await?;

    assert_eq!(
        read_audience_property(&lake.db_pool, null_properties_id).await?,
        Some("team-default".to_string()),
        "a NULL-properties row must be stamped with the configured default"
    );
    assert_eq!(
        read_audience_property(&lake.db_pool, empty_properties_id).await?,
        Some("team-default".to_string()),
        "an empty-properties row must be stamped with the configured default"
    );
    assert_eq!(
        read_audience_property(&lake.db_pool, already_stamped_id).await?,
        Some("team-a".to_string()),
        "an already-stamped row must be left untouched"
    );

    // Idempotency: a second run must change nothing (this is what makes it safe to re-run at
    // every ingestion-service startup).
    backfill_default_audience(&lake.db_pool, &default_audience).await?;
    assert_eq!(
        read_audience_property(&lake.db_pool, null_properties_id).await?,
        Some("team-default".to_string())
    );
    assert_eq!(
        read_audience_property(&lake.db_pool, empty_properties_id).await?,
        Some("team-default".to_string())
    );
    assert_eq!(
        read_audience_property(&lake.db_pool, already_stamped_id).await?,
        Some("team-a".to_string())
    );

    Ok(())
}
