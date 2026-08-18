//! DB-backed tests for `insert_process`'s conflict guard (AbAC Stage 5, #1373, §6) and the
//! stamp's round-trip through `sqlx`. Requires a live `MICROMEGAS_SQL_CONNECTION_STRING` /
//! `MICROMEGAS_OBJECT_STORE_URI` (see `insert_block_dedup_db_test.rs` for the same harness
//! pattern); does not run under a plain `cargo test`.
//!
//! Scoped deliberately: everything that is a pure function of its inputs
//! (`finalize_process_properties`, `WriteAudience`) is asserted in `write_audience_tests.rs`
//! and is not re-asserted against a database here. What only a live Postgres can prove is `ON
//! CONFLICT (process_id) DO NOTHING` + `rows_affected() == 0` + the follow-up `SELECT` the
//! conflict guard runs, and that the stamped property actually reads back out of the
//! `processes.properties` column.

use anyhow::{Context, Result};
use micromegas_ingestion::data_lake_connection::connect_to_data_lake;
use micromegas_ingestion::web_ingestion_service::{IngestionServiceError, WebIngestionService};
use micromegas_ingestion::write_audience::WriteAudience;
use micromegas_telemetry::property::{PROPERTY_AUDIENCE, Property};
use micromegas_telemetry::wire_format::encode_cbor;
use micromegas_tracing::dispatch::make_process_info;
use std::collections::HashMap;
use uuid::Uuid;

async fn connect() -> Result<micromegas_ingestion::data_lake_connection::DataLakeConnection> {
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    connect_to_data_lake(&connection_string, &object_store_uri).await
}

fn process_body(process_id: Uuid) -> Result<bytes::Bytes> {
    let process_info = make_process_info(process_id, None, HashMap::new());
    Ok(bytes::Bytes::from(encode_cbor(&process_info)?))
}

async fn read_audience_property(pool: &sqlx::PgPool, process_id: Uuid) -> Result<Option<String>> {
    let properties: Vec<Property> =
        sqlx::query_scalar("SELECT properties FROM processes WHERE process_id = $1")
            .bind(process_id)
            .fetch_one(pool)
            .await
            .with_context(|| "reading process properties")?;
    Ok(properties
        .iter()
        .find(|p| p.key_str() == PROPERTY_AUDIENCE)
        .map(|p| p.value_str().to_string()))
}

/// Re-registering the same `process_id` under the *same* audience is a no-op -- the ordinary
/// retry case.
#[ignore]
#[tokio::test]
async fn same_audience_reregistration_is_ok() -> Result<()> {
    let lake = connect().await?;
    let ingestion = WebIngestionService::new(lake.clone());
    let process_id = Uuid::new_v4();
    let audience = WriteAudience::new(Some("team-a"))?;

    ingestion
        .insert_process(process_body(process_id)?, &audience)
        .await
        .with_context(|| "first insert_process")?;
    ingestion
        .insert_process(process_body(process_id)?, &audience)
        .await
        .with_context(|| "re-registration under the same audience must succeed")?;

    assert_eq!(
        read_audience_property(&lake.db_pool, process_id).await?,
        Some("team-a".to_string())
    );
    Ok(())
}

/// Re-registering an existing `process_id` under a *different* audience is
/// `IngestionServiceError::AudienceConflict` -- the invariant that makes Stage 2's
/// `MAX(audience)` per-process resolution sound.
#[ignore]
#[tokio::test]
async fn different_audience_reregistration_is_a_conflict() -> Result<()> {
    let lake = connect().await?;
    let ingestion = WebIngestionService::new(lake.clone());
    let process_id = Uuid::new_v4();
    let audience_a = WriteAudience::new(Some("team-a"))?;
    let audience_b = WriteAudience::new(Some("team-b"))?;

    ingestion
        .insert_process(process_body(process_id)?, &audience_a)
        .await
        .with_context(|| "first insert_process")?;

    let result = ingestion
        .insert_process(process_body(process_id)?, &audience_b)
        .await;
    assert!(
        matches!(result, Err(IngestionServiceError::AudienceConflict { .. })),
        "expected AudienceConflict, got {result:?}"
    );

    // The row must keep its original audience -- a rejected conflicting re-registration must
    // never retro-stamp.
    assert_eq!(
        read_audience_property(&lake.db_pool, process_id).await?,
        Some("team-a".to_string())
    );
    Ok(())
}

/// An existing `NULL` (never-stamped) row, re-registered with `Some` audience, is a no-op --
/// the process must not be lost, but it must also not be retro-stamped.
#[ignore]
#[tokio::test]
async fn existing_null_audience_reregistration_is_ok_and_stays_unstamped() -> Result<()> {
    let lake = connect().await?;
    let ingestion = WebIngestionService::new(lake.clone());
    let process_id = Uuid::new_v4();

    ingestion
        .insert_process(process_body(process_id)?, &WriteAudience::none())
        .await
        .with_context(|| "first insert_process, unstamped")?;

    let audience = WriteAudience::new(Some("team-a"))?;
    ingestion
        .insert_process(process_body(process_id)?, &audience)
        .await
        .with_context(|| "re-registration of an unstamped process must not fail")?;

    assert_eq!(
        read_audience_property(&lake.db_pool, process_id).await?,
        None,
        "an existing NULL audience must never be retro-stamped by a later re-registration"
    );
    Ok(())
}

/// Cross-path squatting guard (§6, AbAC Stage 5, #1373): a `process_id` registered via the
/// native `insert_process` path under one audience must reject a later `register_otel_process`
/// re-registration of that *same* `process_id` under a *different* audience. This is the
/// headline confidentiality fix on this branch -- without it, a credential could pre-register
/// (via `insert_process`) the exact `process_id` a victim audience's OTLP producer would later
/// derive, and `register_otel_process`'s `ON CONFLICT DO NOTHING` would silently let the
/// victim's stream/blocks land on the squatter's row.
#[ignore]
#[tokio::test]
async fn otel_reregistration_conflicts_with_native_registration() -> Result<()> {
    let lake = connect().await?;
    let ingestion = WebIngestionService::new(lake.clone());
    let process_id = Uuid::new_v4();
    let audience_a = WriteAudience::new(Some("team-a"))?;
    let audience_b = WriteAudience::new(Some("team-b"))?;

    ingestion
        .insert_process(process_body(process_id)?, &audience_a)
        .await
        .with_context(|| "first insert_process (native path)")?;

    let result = ingestion
        .register_otel_process(
            process_id,
            "exe".to_string(),
            "username".to_string(),
            "computer".to_string(),
            "distro".to_string(),
            "cpu_brand".to_string(),
            1_000_000_000,
            sqlx::types::chrono::Utc::now(),
            0,
            vec![],
            &audience_b,
        )
        .await;
    match result {
        Err(IngestionServiceError::AudienceConflict {
            process_id: conflicting_id,
            existing,
            incoming,
        }) => {
            assert_eq!(conflicting_id, process_id);
            assert_eq!(existing, "team-a");
            assert_eq!(incoming, "team-b");
        }
        other => panic!("expected AudienceConflict, got {other:?}"),
    }

    // The row must keep its original, natively-stamped audience -- a rejected conflicting
    // OTLP re-registration must never retro-stamp.
    assert_eq!(
        read_audience_property(&lake.db_pool, process_id).await?,
        Some("team-a".to_string())
    );
    Ok(())
}
