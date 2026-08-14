//! Persistence round-trip DB test for `max_sort_key_time`
//! (`tasks/thread_spans_segment_boundary_overlap_plan.md`, Part B): isolates the SQL plumbing --
//! the INSERT's explicit column/bind list and the `SELECT`s that decode it back into a
//! `Partition` -- from the ingestion/parquet/query-engine machinery the end-to-end
//! `thread_spans_ordering_db_test.rs` exercises. In the style of
//! `net_spans_retire_overlap_db_test.rs`: synthetic `lakehouse_partitions` rows written through
//! the production `insert_partition`, with no ingestion, no parquet file, and no query engine.
//!
//! This is deliberately separate from the end-to-end test rather than folded into it: this
//! change's realistic failure modes -- a missed bind in the INSERT, a column absent from one of
//! the four SELECTs, a misordered append in the strictly-positional `list_partitions` schema --
//! are all SQL-shaped and Postgres-specific (there is no testcontainers or embedded-Postgres
//! harness in this repo), so no unit test can reach them. This test pins them directly, in
//! seconds, instead of incidentally behind ~200 lines of ingestion and query machinery.
//!
//! Requires a live `MICROMEGAS_SQL_CONNECTION_STRING` / `MICROMEGAS_OBJECT_STORE_URI`; does not
//! run under a plain `cargo test`.

use anyhow::{Context, Result};
use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use micromegas_analytics::lakehouse::partition::Partition;
use micromegas_analytics::lakehouse::partition_cache::PartitionCache;
use micromegas_analytics::lakehouse::view::ViewMetadata;
use micromegas_analytics::lakehouse::write_partition::{RetireMatch, insert_partition};
use micromegas_analytics::response_writer::ResponseWriter;
use micromegas_analytics::time::TimeRange;
use micromegas_ingestion::data_lake_connection::{DataLakeConnection, connect_to_data_lake};
use std::sync::Arc;

const VIEW_SET_NAME: &str = "thread_spans";

async fn connect() -> Result<DataLakeConnection> {
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    connect_to_data_lake(&connection_string, &object_store_uri).await
}

/// A fresh, random `view_instance_id` per test, so the retire predicate run by `insert_partition`
/// is a no-op against whatever else this persistent dev lake holds and no object-store file is
/// ever referenced (no parquet file backs `file_path` here).
fn instance_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Truncated to microsecond precision: Postgres `timestamptz` columns only store microseconds, so
/// a nanosecond-precision `Utc::now()` would never compare equal to the same value read back out
/// of `lakehouse_partitions`.
fn base_time() -> Result<DateTime<Utc>> {
    Ok((Utc::now() - TimeDelta::minutes(30)).duration_trunc(TimeDelta::microseconds(1))?)
}

/// A non-empty (`num_rows > 0`) partition literal. `Partition::validate` (run on every read)
/// requires `Some(file_path)` and `Some(event_time_range)` for a non-empty partition, and
/// `source_data_hash` must be exactly 8 bytes -- `insert_partition` decodes it via
/// `hash_to_object_count` (`i64::from_le_bytes`).
fn make_partition(
    view_instance_id: &str,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    max_sort_key_time: Option<DateTime<Utc>>,
) -> Partition {
    Partition {
        view_metadata: ViewMetadata {
            view_set_name: Arc::new(VIEW_SET_NAME.to_owned()),
            view_instance_id: Arc::new(view_instance_id.to_owned()),
            file_schema_hash: vec![3],
        },
        insert_time_range: TimeRange::new(begin, end),
        event_time_range: Some(TimeRange::new(begin, end)),
        updated: Utc::now(),
        file_path: Some(format!(
            "test/{view_instance_id}/{}.parquet",
            uuid::Uuid::new_v4()
        )),
        file_size: 1024,
        source_data_hash: 1i64.to_le_bytes().to_vec(),
        num_rows: 10,
        sort_order: None,
        max_sort_key_time,
    }
}

/// `Some(t)` and `None` rows in disjoint view_instance_ids and insert ranges: a `Some(t)` row
/// survives the production `insert_partition` -> `PartitionCache::fetch_overlapping_insert_range`
/// round trip exactly, and a sibling `None` row reads back as `None` (the legacy/no-ordering-view
/// path), not as some accidental zero value.
#[ignore]
#[tokio::test]
async fn max_sort_key_time_survives_insert_and_read_back() -> Result<()> {
    let lake = connect().await?;
    let logger = Arc::new(ResponseWriter::new(None));
    let t0 = base_time()?;

    // Some(t): the value this plan actually cares about persisting.
    let view_instance_some = instance_id();
    let recorded = t0 + TimeDelta::seconds(4);
    let partition_some = make_partition(
        &view_instance_some,
        t0,
        t0 + TimeDelta::seconds(10),
        Some(recorded),
    );
    insert_partition(
        &lake,
        &partition_some,
        RetireMatch::Containment,
        &[],
        logger.clone(),
    )
    .await
    .with_context(|| "inserting Some(max_sort_key_time) partition")?;

    // None: the legacy-fallback path. Disjoint view_instance_id *and* insert range, so neither
    // insert's retire step (or the lakehouse_partitions_no_overlap exclusion constraint, which is
    // scoped by file_schema_hash rather than view_instance_id) removes or rejects the other.
    let view_instance_none = instance_id();
    let partition_none = make_partition(
        &view_instance_none,
        t0 + TimeDelta::hours(1),
        t0 + TimeDelta::hours(1) + TimeDelta::seconds(10),
        None,
    );
    insert_partition(
        &lake,
        &partition_none,
        RetireMatch::Containment,
        &[],
        logger,
    )
    .await
    .with_context(|| "inserting None max_sort_key_time partition")?;

    let insert_range = TimeRange::new(t0 - TimeDelta::minutes(1), t0 + TimeDelta::hours(2));
    let cache = PartitionCache::fetch_overlapping_insert_range(&lake.db_pool, insert_range)
        .await
        .with_context(|| "fetch_overlapping_insert_range")?;

    let found_some = cache
        .partitions
        .iter()
        .find(|p| *p.view_metadata.view_instance_id == view_instance_some)
        .with_context(|| "Some(...) partition not found after read-back")?;
    assert_eq!(
        found_some.max_sort_key_time(),
        Some(recorded),
        "max_sort_key_time must survive the INSERT -> SELECT round trip exactly"
    );

    let found_none = cache
        .partitions
        .iter()
        .find(|p| *p.view_metadata.view_instance_id == view_instance_none)
        .with_context(|| "None partition not found after read-back")?;
    assert_eq!(
        found_none.max_sort_key_time(),
        None,
        "a partition that never recorded max_sort_key_time must read back as None, not some \
         accidental default"
    );

    Ok(())
}
