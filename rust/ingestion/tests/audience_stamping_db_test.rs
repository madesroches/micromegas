//! DB-backed tests for `insert_process`'s and `insert_stream`'s conflict guards, and the
//! stamp's round-trip through the `audience` column. Requires a live
//! `MICROMEGAS_SQL_CONNECTION_STRING` / `MICROMEGAS_OBJECT_STORE_URI` (see
//! `insert_block_dedup_db_test.rs` for the same harness pattern); does not run under a plain
//! `cargo test`.
//!
//! Pure-function logic (`strip_reserved_properties`, `WriteAudience`) is asserted in
//! `write_audience_tests.rs`, not here. What only a live Postgres can prove is `ON CONFLICT
//! (process_id|stream_id) DO NOTHING` + `rows_affected() == 0` + the follow-up `SELECT` each
//! conflict guard runs, and that the stamp actually reads back out of the `audience` column.

use anyhow::{Context, Result};
use micromegas_ingestion::data_lake_connection::connect_to_data_lake;
use micromegas_ingestion::web_ingestion_service::{IngestionServiceError, WebIngestionService};
use micromegas_ingestion::write_audience::WriteAudience;
use micromegas_telemetry::stream_info::StreamInfo;
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

fn stream_body(stream_id: Uuid, process_id: Uuid) -> Result<bytes::Bytes> {
    let stream_info = StreamInfo {
        stream_id,
        process_id,
        dependencies_metadata: vec![],
        objects_metadata: vec![],
        tags: vec![],
        properties: HashMap::new(),
    };
    Ok(bytes::Bytes::from(encode_cbor(&stream_info)?))
}

async fn read_audience_column(pool: &sqlx::PgPool, process_id: Uuid) -> Result<Option<String>> {
    sqlx::query_scalar("SELECT audience FROM processes WHERE process_id = $1")
        .bind(process_id)
        .fetch_one(pool)
        .await
        .with_context(|| "reading processes.audience")
}

async fn read_stream_audience_column(
    pool: &sqlx::PgPool,
    stream_id: Uuid,
) -> Result<Option<String>> {
    sqlx::query_scalar("SELECT audience FROM streams WHERE stream_id = $1")
        .bind(stream_id)
        .fetch_one(pool)
        .await
        .with_context(|| "reading streams.audience")
}

/// Fabricates a legacy-shaped row by nulling the `audience` column back out after
/// `insert_process` stamped it, reproducing the shape of a row written before schema v8.
async fn nullify_process_audience(pool: &sqlx::PgPool, process_id: Uuid) -> Result<()> {
    sqlx::query("UPDATE processes SET audience = NULL WHERE process_id = $1")
        .bind(process_id)
        .execute(pool)
        .await
        .with_context(|| "nulling processes.audience")?;
    Ok(())
}

/// The stream-side mirror of [`nullify_process_audience`].
async fn nullify_stream_audience(pool: &sqlx::PgPool, stream_id: Uuid) -> Result<()> {
    sqlx::query("UPDATE streams SET audience = NULL WHERE stream_id = $1")
        .bind(stream_id)
        .execute(pool)
        .await
        .with_context(|| "nulling streams.audience")?;
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
        read_audience_column(&lake.db_pool, process_id).await?,
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
        read_audience_column(&lake.db_pool, process_id).await?,
        Some("team-a".to_string())
    );
    Ok(())
}

/// A fabricated legacy row (NULL `audience`) re-registered under a *different* audience than
/// the resolved default is `AudienceConflict`: the guard resolves the NULL column to the
/// deployment default the same way every reader does, and `team-a` disagrees with it.
///
/// The fabricating label (`seed-only`) must differ from the deployment default and from the
/// re-registration labels below, since `insert_process` caches a confirmed-conflict-free
/// audience for 60s and a shared label would hit that cache instead of exercising the check.
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
        .with_context(|| "seeding the row to be nulled")?;
    nullify_process_audience(&lake.db_pool, process_id)
        .await
        .with_context(|| "fabricating the legacy-shaped row")?;
    assert_eq!(
        read_audience_column(&lake.db_pool, process_id).await?,
        None,
        "sanity check: the fabricated row must have a NULL audience column"
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
        read_audience_column(&lake.db_pool, process_id).await?,
        None,
        "a rejected re-registration must never retro-stamp the legacy row"
    );
    Ok(())
}

/// The same fabricated legacy row, re-registered under the *deployment default* itself, is `Ok`
/// and leaves the row unstamped: a matching re-registration never writes anything back.
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
        .with_context(|| "seeding the row to be nulled")?;
    nullify_process_audience(&lake.db_pool, process_id)
        .await
        .with_context(|| "fabricating the legacy-shaped row")?;

    ingestion
        .insert_process(process_body(process_id)?, &default_audience)
        .await
        .with_context(|| "re-registration under the resolved default must not fail")?;

    assert_eq!(
        read_audience_column(&lake.db_pool, process_id).await?,
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
        read_audience_column(&lake.db_pool, process_id).await?,
        Some("team-a".to_string())
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// check_stream_audience_conflict -- mirrors the three `insert_process` cases above, via
// `insert_stream`.
// ---------------------------------------------------------------------------

/// Re-registering the same `stream_id` under the *same* audience is a no-op.
#[ignore]
#[tokio::test]
async fn same_audience_stream_reregistration_is_ok() -> Result<()> {
    let lake = connect().await?;
    let ingestion = WebIngestionService::new(lake.clone(), WriteAudience::new("public")?);
    let process_id = Uuid::new_v4();
    let stream_id = Uuid::new_v4();
    let audience = WriteAudience::new("team-a")?;

    ingestion
        .insert_stream(stream_body(stream_id, process_id)?, &audience)
        .await
        .with_context(|| "first insert_stream")?;
    ingestion
        .insert_stream(stream_body(stream_id, process_id)?, &audience)
        .await
        .with_context(|| "re-registration under the same audience must succeed")?;

    assert_eq!(
        read_stream_audience_column(&lake.db_pool, stream_id).await?,
        Some("team-a".to_string())
    );
    Ok(())
}

/// Re-registering an existing `stream_id` under a *different* audience is
/// `IngestionServiceError::StreamAudienceConflict` -- without this guard, a re-pointed
/// credential's later blocks on the same stream would be silently excluded by the
/// audience-mismatch predicate, with no signal at write time.
#[ignore]
#[tokio::test]
async fn different_audience_stream_reregistration_is_a_conflict() -> Result<()> {
    let lake = connect().await?;
    let ingestion = WebIngestionService::new(lake.clone(), WriteAudience::new("public")?);
    let process_id = Uuid::new_v4();
    let stream_id = Uuid::new_v4();
    let audience_a = WriteAudience::new("team-a")?;
    let audience_b = WriteAudience::new("team-b")?;

    ingestion
        .insert_stream(stream_body(stream_id, process_id)?, &audience_a)
        .await
        .with_context(|| "first insert_stream")?;

    let result = ingestion
        .insert_stream(stream_body(stream_id, process_id)?, &audience_b)
        .await;
    assert!(
        matches!(
            result,
            Err(IngestionServiceError::StreamAudienceConflict { .. })
        ),
        "expected StreamAudienceConflict, got {result:?}"
    );

    assert_eq!(
        read_stream_audience_column(&lake.db_pool, stream_id).await?,
        Some("team-a".to_string()),
        "a rejected re-registration must never retro-stamp"
    );
    Ok(())
}

/// A fabricated legacy stream row -- stamped, then nulled back via `nullify_stream_audience` --
/// re-registered under a *different* audience than the resolved default is a conflict, the same
/// way the process-side legacy case is.
#[ignore]
#[tokio::test]
async fn legacy_unstamped_stream_reregistered_under_a_different_audience_is_a_conflict()
-> Result<()> {
    let lake = connect().await?;
    let default_audience = WriteAudience::new("public")?;
    let ingestion = WebIngestionService::new(lake.clone(), default_audience);
    let process_id = Uuid::new_v4();
    let stream_id = Uuid::new_v4();

    ingestion
        .insert_stream(
            stream_body(stream_id, process_id)?,
            &WriteAudience::new("seed-only")?,
        )
        .await
        .with_context(|| "seeding the row to be nulled")?;
    nullify_stream_audience(&lake.db_pool, stream_id)
        .await
        .with_context(|| "fabricating the legacy-shaped row")?;
    assert_eq!(
        read_stream_audience_column(&lake.db_pool, stream_id).await?,
        None,
        "sanity check: the fabricated row must have a NULL audience column"
    );

    let result = ingestion
        .insert_stream(
            stream_body(stream_id, process_id)?,
            &WriteAudience::new("team-a")?,
        )
        .await;
    assert!(
        matches!(
            result,
            Err(IngestionServiceError::StreamAudienceConflict { .. })
        ),
        "a legacy row resolves to the deployment default, so a different incoming audience must \
         conflict just like it would against a stamped row -- got {result:?}"
    );

    assert_eq!(
        read_stream_audience_column(&lake.db_pool, stream_id).await?,
        None,
        "a rejected re-registration must never retro-stamp the legacy row"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Stamp round-trip: `insert_stream`/`insert_block_typed` under a given audience land rows whose
// `audience` column reads back exactly that value.
// ---------------------------------------------------------------------------

#[ignore]
#[tokio::test]
async fn insert_stream_stamps_the_audience_column() -> Result<()> {
    let lake = connect().await?;
    let ingestion = WebIngestionService::new(lake.clone(), WriteAudience::new("public")?);
    let process_id = Uuid::new_v4();
    let stream_id = Uuid::new_v4();
    let audience = WriteAudience::new("alpha")?;

    ingestion
        .insert_stream(stream_body(stream_id, process_id)?, &audience)
        .await
        .with_context(|| "insert_stream")?;

    assert_eq!(
        read_stream_audience_column(&lake.db_pool, stream_id).await?,
        Some("alpha".to_string())
    );
    Ok(())
}

#[ignore]
#[tokio::test]
async fn insert_block_typed_stamps_the_audience_column() -> Result<()> {
    use micromegas_telemetry::block_wire_format::{Block, BlockPayload};

    let lake = connect().await?;
    let ingestion = WebIngestionService::new(lake.clone(), WriteAudience::new("public")?);
    let process_id = Uuid::new_v4();
    let stream_id = Uuid::new_v4();
    let block_id = Uuid::new_v4();
    let audience = WriteAudience::new("alpha")?;

    let block = Block {
        block_id,
        stream_id,
        process_id,
        begin_time: "2024-01-01T00:00:00Z".to_string(),
        begin_ticks: 0,
        end_time: "2024-01-01T00:00:01Z".to_string(),
        end_ticks: 0,
        payload: BlockPayload {
            dependencies: vec![],
            objects: vec![],
        },
        nb_objects: 0,
        object_offset: 0,
    };

    ingestion
        .insert_block_typed(block, &audience)
        .await
        .with_context(|| "insert_block_typed")?;

    let stamped: Option<String> =
        sqlx::query_scalar("SELECT audience FROM blocks WHERE block_id = $1")
            .bind(block_id)
            .fetch_one(&lake.db_pool)
            .await
            .with_context(|| "reading blocks.audience")?;
    assert_eq!(stamped, Some("alpha".to_string()));
    Ok(())
}
