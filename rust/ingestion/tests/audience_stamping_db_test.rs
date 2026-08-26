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

/// Fabricates a legacy-shaped row: strips the `micromegas.audience` property back off a process
/// that `insert_process` already stamped, via a direct `UPDATE processes SET properties = ...`.
/// There is no `UPDATE processes` path anywhere in the production codebase -- `insert_process`
/// always stamps now (#1519) -- so this is test-only scaffolding to reproduce the shape a row
/// written before that write-side resolution shipped, or one written by the admin
/// `bulk_ingest`/replication path, still has.
async fn strip_audience_property(pool: &sqlx::PgPool, process_id: Uuid) -> Result<()> {
    let properties: Vec<Property> =
        sqlx::query_scalar("SELECT properties FROM processes WHERE process_id = $1")
            .bind(process_id)
            .fetch_one(pool)
            .await
            .with_context(|| "reading process properties to strip")?;
    let stripped: Vec<Property> = properties
        .into_iter()
        .filter(|p| p.key_str() != PROPERTY_AUDIENCE)
        .collect();
    sqlx::query("UPDATE processes SET properties = $1 WHERE process_id = $2")
        .bind(stripped)
        .bind(process_id)
        .execute(pool)
        .await
        .with_context(|| "stripping micromegas.audience property")?;
    Ok(())
}

/// Re-registering the same `process_id` under the *same* audience is a no-op -- the ordinary
/// retry case.
#[ignore]
#[tokio::test]
async fn same_audience_reregistration_is_ok() -> Result<()> {
    let lake = connect().await?;
    let ingestion = WebIngestionService::new(lake.clone(), WriteAudience::new("public")?);
    let process_id = Uuid::new_v4();
    let audience = WriteAudience::new("team-a")?;

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
    let ingestion = WebIngestionService::new(lake.clone(), WriteAudience::new("public")?);
    let process_id = Uuid::new_v4();
    let audience_a = WriteAudience::new("team-a")?;
    let audience_b = WriteAudience::new("team-b")?;

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

/// A fabricated legacy row -- stamped, then stripped back to a `NULL` `micromegas.audience` via
/// `strip_audience_property`, reproducing the shape of a row written before the write path
/// resolved the default, or one written by the admin replication path -- re-registered under a
/// *different* audience than the resolved default is now `AudienceConflict` (#1519, §7): the
/// guard resolves the legacy row's missing property to the deployment default the same way every
/// reader does, and `team-a` disagrees with it.
///
/// The fabricating label (`seed-only`) must differ from both the deployment default (`public`)
/// and the label each of the two re-registration cases below uses, because `insert_process`
/// caches the audience it just confirmed conflict-free (`remember_process_audience`) and the
/// cache's 60s TTL cannot expire within a test -- a shared label would short-circuit on the
/// cache-hit arm and never reach the resolved comparison this test exists to exercise.
#[ignore]
#[tokio::test]
async fn legacy_unstamped_row_reregistered_under_a_different_audience_is_a_conflict() -> Result<()>
{
    let lake = connect().await?;
    let default_audience = WriteAudience::new("public")?;
    let ingestion = WebIngestionService::new(lake.clone(), default_audience);
    let process_id = Uuid::new_v4();

    ingestion
        .insert_process(process_body(process_id)?, &WriteAudience::new("seed-only")?)
        .await
        .with_context(|| "seeding the row to be stripped")?;
    strip_audience_property(&lake.db_pool, process_id)
        .await
        .with_context(|| "fabricating the legacy-shaped row")?;
    assert_eq!(
        read_audience_property(&lake.db_pool, process_id).await?,
        None,
        "sanity check: the fabricated row must have no micromegas.audience property"
    );

    let result = ingestion
        .insert_process(process_body(process_id)?, &WriteAudience::new("team-a")?)
        .await;
    assert!(
        matches!(result, Err(IngestionServiceError::AudienceConflict { .. })),
        "a legacy row resolves to the deployment default, so a different incoming audience must \
         conflict just like it would against a stamped row -- got {result:?}"
    );

    assert_eq!(
        read_audience_property(&lake.db_pool, process_id).await?,
        None,
        "a rejected re-registration must never retro-stamp the legacy row"
    );
    Ok(())
}

/// The same fabricated legacy row, re-registered under the *deployment default* itself, is `Ok`
/// and leaves the row unstamped (#1519 -- "No retro-stamp, still"): only the comparison changed,
/// not whether a matching re-registration writes anything back.
#[ignore]
#[tokio::test]
async fn legacy_unstamped_row_reregistered_under_the_default_is_ok_and_stays_unstamped()
-> Result<()> {
    let lake = connect().await?;
    let default_audience = WriteAudience::new("public")?;
    let ingestion = WebIngestionService::new(lake.clone(), default_audience.clone());
    let process_id = Uuid::new_v4();

    ingestion
        .insert_process(process_body(process_id)?, &WriteAudience::new("seed-only")?)
        .await
        .with_context(|| "seeding the row to be stripped")?;
    strip_audience_property(&lake.db_pool, process_id)
        .await
        .with_context(|| "fabricating the legacy-shaped row")?;

    ingestion
        .insert_process(process_body(process_id)?, &default_audience)
        .await
        .with_context(|| "re-registration under the resolved default must not fail")?;

    assert_eq!(
        read_audience_property(&lake.db_pool, process_id).await?,
        None,
        "a matching re-registration of a legacy row must still leave it unstamped -- no \
         retro-stamp, ever"
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
    let ingestion = WebIngestionService::new(lake.clone(), WriteAudience::new("public")?);
    let process_id = Uuid::new_v4();
    let audience_a = WriteAudience::new("team-a")?;
    let audience_b = WriteAudience::new("team-b")?;

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
