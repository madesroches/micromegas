//! DB-backed regression test for the create-only block write
//! (tasks/1465_create_only_block_write_plan.md): the four (object, row) combinations
//! `insert_block_typed` can land in against a real PG + object store. Requires a live
//! `MICROMEGAS_SQL_CONNECTION_STRING` / `MICROMEGAS_OBJECT_STORE_URI` (see
//! `rust/analytics/tests/thread_spans_ordering_db_test.rs` for the same harness pattern);
//! does not run under a plain `cargo test`.
//!
//! Assertions are on observable state (object bytes, row presence, `payload_size`), not on
//! the `imetric!` counters themselves — see the plan's "Testing Strategy" section for why.

use anyhow::{Context, Result};
use micromegas_ingestion::data_lake_connection::connect_to_data_lake;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_telemetry::block_wire_format::{Block, BlockPayload};
use uuid::Uuid;

/// Builds a minimal `Block` with the given identity and payload bytes. Times are fixed,
/// valid RFC3339 strings — `insert_block_typed` parses them but this test doesn't care about
/// their values.
fn make_block(block_id: Uuid, stream_id: Uuid, process_id: Uuid, objects: Vec<u8>) -> Block {
    Block {
        block_id,
        stream_id,
        process_id,
        begin_time: "2024-01-01T00:00:00Z".to_string(),
        begin_ticks: 0,
        end_time: "2024-01-01T00:00:01Z".to_string(),
        end_ticks: 1,
        payload: BlockPayload {
            dependencies: Vec::new(),
            objects,
        },
        object_offset: 0,
        nb_objects: 1,
    }
}

async fn block_row_count(pool: &sqlx::PgPool, block_id: Uuid) -> Result<i64> {
    let row = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM blocks WHERE block_id = $1;")
        .bind(block_id)
        .fetch_one(pool)
        .await
        .with_context(|| "counting rows in blocks")?;
    Ok(row)
}

async fn block_payload_size(pool: &sqlx::PgPool, block_id: Uuid) -> Result<i64> {
    let row = sqlx::query_scalar::<_, i64>("SELECT payload_size FROM blocks WHERE block_id = $1;")
        .bind(block_id)
        .fetch_one(pool)
        .await
        .with_context(|| "reading payload_size from blocks")?;
    Ok(row)
}

/// Case 1: the same block ingested twice → one object, one row; the object's bytes equal
/// the first write's.
#[ignore]
#[tokio::test]
async fn same_block_twice_yields_one_object_one_row_with_first_writes_bytes() -> Result<()> {
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
    let ingestion = WebIngestionService::new(lake.clone());

    let block_id = Uuid::new_v4();
    let stream_id = Uuid::new_v4();
    let process_id = Uuid::new_v4();
    let obj_path = format!("blobs/{process_id}/{stream_id}/{block_id}");

    let block1 = make_block(block_id, stream_id, process_id, b"first-arrival".to_vec());
    ingestion
        .insert_block_typed(block1)
        .await
        .map_err(|e| anyhow::anyhow!("insert_block_typed (first arrival): {e}"))?;

    // Same block_id, different bytes — same content is the common case, but different bytes
    // is what #1462 mishandled; this exercises the create-only guard directly.
    let block2 = make_block(
        block_id,
        stream_id,
        process_id,
        b"second-arrival-different-bytes".to_vec(),
    );
    ingestion
        .insert_block_typed(block2)
        .await
        .map_err(|e| anyhow::anyhow!("insert_block_typed (second arrival): {e}"))?;

    assert_eq!(
        block_row_count(&lake.db_pool, block_id).await?,
        1,
        "exactly one row for the block_id"
    );
    let bytes = lake.blob_storage.read_blob(&obj_path).await?;
    // The stored envelope is the CBOR encoding of BlockPayload; the first arrival's
    // `objects` bytes must be a substring of it and the second arrival's must not.
    assert!(
        contains_bytes(&bytes, b"first-arrival"),
        "object must still hold the first arrival's payload"
    );
    assert!(
        !contains_bytes(&bytes, b"second-arrival-different-bytes"),
        "a colliding write must never apply its bytes"
    );

    Ok(())
}

/// Case 2: the object is pre-written directly (simulating a crash between PUT and INSERT on
/// a prior attempt), then `insert_block_typed` runs → the row appears (orphan healed), and
/// the pre-written object's bytes are left untouched.
#[ignore]
#[tokio::test]
async fn orphaned_object_is_healed_by_insert_block_typed() -> Result<()> {
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
    let ingestion = WebIngestionService::new(lake.clone());

    let block_id = Uuid::new_v4();
    let stream_id = Uuid::new_v4();
    let process_id = Uuid::new_v4();
    let obj_path = format!("blobs/{process_id}/{stream_id}/{block_id}");

    // Simulate a prior attempt that wrote the object but crashed before the INSERT: write
    // the object directly via `blob_storage`, bypassing `insert_block_typed` entirely.
    let pre_written_payload = micromegas_telemetry::wire_format::encode_cbor(&BlockPayload {
        dependencies: Vec::new(),
        objects: b"orphaned-object".to_vec(),
    })?;
    lake.blob_storage
        .put(&obj_path, bytes::Bytes::from(pre_written_payload))
        .await
        .with_context(|| "pre-writing the orphaned object")?;
    assert_eq!(
        block_row_count(&lake.db_pool, block_id).await?,
        0,
        "no row should exist yet"
    );

    let block = make_block(block_id, stream_id, process_id, b"heal-arrival".to_vec());
    ingestion
        .insert_block_typed(block)
        .await
        .map_err(|e| anyhow::anyhow!("insert_block_typed (healing): {e}"))?;

    assert_eq!(
        block_row_count(&lake.db_pool, block_id).await?,
        1,
        "the orphaned object's row must now exist"
    );
    let bytes = lake.blob_storage.read_blob(&obj_path).await?;
    assert!(
        contains_bytes(&bytes, b"orphaned-object"),
        "the pre-written object's bytes must be left untouched by the healing insert"
    );
    assert!(
        !contains_bytes(&bytes, b"heal-arrival"),
        "the healing insert's own bytes must never overwrite the pre-written object"
    );

    Ok(())
}

/// Case 3: the row is pre-inserted directly with no object present, then
/// `insert_block_typed` runs → the object appears with the second write's bytes, and the
/// pre-existing row is left untouched (`payload_size` unchanged — the INSERT is `ON
/// CONFLICT (block_id) DO NOTHING`).
#[ignore]
#[tokio::test]
async fn row_without_object_gets_object_written_but_row_left_untouched() -> Result<()> {
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
    let ingestion = WebIngestionService::new(lake.clone());

    let block_id = Uuid::new_v4();
    let stream_id = Uuid::new_v4();
    let process_id = Uuid::new_v4();
    let obj_path = format!("blobs/{process_id}/{stream_id}/{block_id}");

    // Pre-insert the row directly, with a sentinel payload_size that would never match a
    // real encoding, so a later overwrite of the row would be obvious.
    let sentinel_payload_size: i64 = 999_999;
    let insert_time = sqlx::types::chrono::Utc::now();
    sqlx::query("INSERT INTO blocks VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11);")
        .bind(block_id)
        .bind(stream_id)
        .bind(process_id)
        .bind(sqlx::types::chrono::Utc::now())
        .bind(0i64)
        .bind(sqlx::types::chrono::Utc::now())
        .bind(1i64)
        .bind(1i32)
        .bind(0i64)
        .bind(sentinel_payload_size)
        .bind(insert_time)
        .execute(&lake.db_pool)
        .await
        .with_context(|| "pre-inserting the row")?;

    let block = make_block(
        block_id,
        stream_id,
        process_id,
        b"object-written-after-row".to_vec(),
    );
    ingestion
        .insert_block_typed(block)
        .await
        .map_err(|e| anyhow::anyhow!("insert_block_typed (row pre-exists): {e}"))?;

    assert_eq!(
        block_row_count(&lake.db_pool, block_id).await?,
        1,
        "still exactly one row"
    );
    assert_eq!(
        block_payload_size(&lake.db_pool, block_id).await?,
        sentinel_payload_size,
        "ON CONFLICT DO NOTHING must leave the pre-existing row's payload_size untouched"
    );
    let bytes = lake.blob_storage.read_blob(&obj_path).await?;
    assert!(
        contains_bytes(&bytes, b"object-written-after-row"),
        "the object must now exist, holding this call's bytes (nothing wrote it before)"
    );

    Ok(())
}

/// Case 4: two blocks differing in one payload byte (and thus in `block_id`, since these are
/// distinct logical blocks) → two objects, two rows.
#[ignore]
#[tokio::test]
async fn distinct_blocks_yield_distinct_objects_and_rows() -> Result<()> {
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
    let ingestion = WebIngestionService::new(lake.clone());

    let stream_id = Uuid::new_v4();
    let process_id = Uuid::new_v4();
    let block_id_a = Uuid::new_v4();
    let block_id_b = Uuid::new_v4();

    let block_a = make_block(block_id_a, stream_id, process_id, b"payload-a".to_vec());
    let block_b = make_block(block_id_b, stream_id, process_id, b"payload-b".to_vec());
    ingestion
        .insert_block_typed(block_a)
        .await
        .map_err(|e| anyhow::anyhow!("insert_block_typed (a): {e}"))?;
    ingestion
        .insert_block_typed(block_b)
        .await
        .map_err(|e| anyhow::anyhow!("insert_block_typed (b): {e}"))?;

    assert_eq!(block_row_count(&lake.db_pool, block_id_a).await?, 1);
    assert_eq!(block_row_count(&lake.db_pool, block_id_b).await?, 1);

    let obj_path_a = format!("blobs/{process_id}/{stream_id}/{block_id_a}");
    let obj_path_b = format!("blobs/{process_id}/{stream_id}/{block_id_b}");
    let bytes_a = lake.blob_storage.read_blob(&obj_path_a).await?;
    let bytes_b = lake.blob_storage.read_blob(&obj_path_b).await?;
    assert!(contains_bytes(&bytes_a, b"payload-a"));
    assert!(contains_bytes(&bytes_b, b"payload-b"));

    Ok(())
}

/// Cheap substring search over the small payload sizes this test uses — avoids pulling in a
/// CBOR decoder just to check which of two candidate byte strings the stored envelope holds.
fn contains_bytes(haystack: &bytes::Bytes, needle: &[u8]) -> bool {
    haystack.as_ref().windows(needle.len()).any(|w| w == needle)
}
