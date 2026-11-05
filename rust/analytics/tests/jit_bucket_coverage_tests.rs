//! Pure, no-DB unit tests for `resolve_bucket_coverage` and `decide_conflict_retry`.

use chrono::{DateTime, TimeDelta, Utc};
use micromegas_analytics::lakehouse::jit_partitions::{
    ConflictRetry, Coverage, JitBucket, PartitionFreshnessRow, decide_conflict_retry,
    resolve_bucket_coverage,
};
use micromegas_analytics::lakehouse::partition_ladder::PlannedBucket;
use micromegas_analytics::lakehouse::partition_source_data::{
    PartitionSourceBlock, SourceDataBlocksInMemory,
};
use micromegas_analytics::lakehouse::view::ViewMetadata;
use micromegas_analytics::lakehouse::write_partition::RetireMatch;
use micromegas_analytics::metadata::{ProcessMetadata, StreamMetadata};
use micromegas_analytics::time::TimeRange;
use micromegas_telemetry::types::block::BlockMetadata;
use std::sync::Arc;

const MINUTE: TimeDelta = TimeDelta::seconds(60);
const STALE_CONTAINMENT: Coverage = Coverage::Stale {
    retire_match: RetireMatch::Containment,
};
const STALE_OVERLAP: Coverage = Coverage::Stale {
    retire_match: RetireMatch::Overlap,
};

fn at(hms: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&format!("2024-01-01T{hms}Z"))
        .unwrap()
        .to_utc()
}

fn view_meta() -> ViewMetadata {
    ViewMetadata {
        view_set_name: Arc::new("test_view".to_owned()),
        view_instance_id: Arc::new("test_instance".to_owned()),
        file_schema_hash: vec![1, 2, 3],
    }
}

fn make_block(insert_time: DateTime<Utc>, nb_objects: i32) -> Arc<PartitionSourceBlock> {
    let process_id = uuid::Uuid::new_v4();
    let stream_id = uuid::Uuid::new_v4();
    let block = BlockMetadata {
        block_id: uuid::Uuid::new_v4(),
        stream_id,
        process_id,
        begin_time: insert_time,
        end_time: insert_time,
        begin_ticks: 0,
        end_ticks: 100,
        nb_objects,
        payload_size: 0,
        object_offset: 0,
        insert_time,
    };
    let stream = Arc::new(StreamMetadata {
        process_id,
        stream_id,
        dependencies_metadata: vec![],
        objects_metadata: vec![],
        tags: vec![],
        properties: Arc::new(vec![]),
    });
    let process = Arc::new(ProcessMetadata {
        process_id,
        exe: "test".to_owned(),
        username: "test".to_owned(),
        realname: "test".to_owned(),
        computer: "test".to_owned(),
        distro: "test".to_owned(),
        cpu_brand: "test".to_owned(),
        tsc_frequency: 1_000_000_000,
        start_time: insert_time,
        start_ticks: 0,
        parent_process_id: None,
        properties: Arc::new(vec![]),
        audience: Arc::from("public"),
    });
    Arc::new(PartitionSourceBlock {
        block,
        stream,
        process,
        format: "test".to_owned(),
    })
}

/// A minute bucket holding one spec of two blocks (at `first` and `last`) totalling `total`
/// objects.
fn minute_bucket(minute: &str, first: &str, last: &str, total: i32) -> JitBucket {
    let begin = at(minute);
    JitBucket {
        planned: PlannedBucket {
            range: TimeRange::new(begin, begin + MINUTE),
            rung: MINUTE,
            closed: true,
            nb_blocks: 2,
            nb_objects: total as i64,
        },
        specs: vec![SourceDataBlocksInMemory {
            blocks: vec![
                make_block(at(first), total / 2),
                make_block(at(last), total - total / 2),
            ],
            block_ids_hash: (total as i64).to_le_bytes().to_vec(),
        }],
    }
}

fn row(begin: &str, end: &str, count: i64) -> PartitionFreshnessRow {
    row_with_schema(begin, end, count, vec![1, 2, 3])
}

fn row_with_schema(begin: &str, end: &str, count: i64, schema: Vec<u8>) -> PartitionFreshnessRow {
    PartitionFreshnessRow {
        begin_insert_time: at(begin),
        end_insert_time: at(end),
        file_schema_hash: schema,
        source_data_hash: count.to_le_bytes().to_vec(),
    }
}

fn resolve(buckets: &[JitBucket], rows: &[PartitionFreshnessRow]) -> Vec<Coverage> {
    resolve_bucket_coverage(buckets, &view_meta(), rows).unwrap()
}

#[test]
fn inner_rows_summing_exactly_are_covered() {
    let bucket = minute_bucket("10:00:00", "10:00:05", "10:00:30", 10);
    let rows = [row("10:00:05", "10:00:30", 10)];
    assert_eq!(resolve(&[bucket], &rows), vec![Coverage::Covered]);
}

#[test]
fn inner_rows_summing_below_the_requirement_are_stale() {
    let bucket = minute_bucket("10:00:00", "10:00:05", "10:00:30", 10);
    let rows = [row("10:00:05", "10:00:30", 9)];
    assert_eq!(resolve(&[bucket], &rows), vec![STALE_CONTAINMENT]);
}

#[test]
fn inner_rows_outliving_deleted_blocks_are_covered() {
    let bucket = minute_bucket("10:00:00", "10:00:05", "10:00:30", 10);
    let rows = [row("10:00:05", "10:00:30", 11)];
    assert_eq!(resolve(&[bucket], &rows), vec![Coverage::Covered]);
}

#[test]
fn no_rows_means_stale() {
    let bucket = minute_bucket("10:00:00", "10:00:05", "10:00:30", 10);
    assert_eq!(resolve(&[bucket], &[]), vec![STALE_CONTAINMENT]);
}

#[test]
fn one_coarse_row_containing_the_hull_is_covered() {
    let bucket = minute_bucket("10:05:00", "10:05:10", "10:05:50", 10);
    let rows = [row("10:00:03", "10:20:00", 100)];
    assert_eq!(resolve(&[bucket], &rows), vec![Coverage::Covered]);
}

#[test]
fn a_coarse_row_with_too_few_objects_is_stale() {
    let bucket = minute_bucket("10:05:00", "10:05:10", "10:05:50", 10);
    let rows = [row("10:00:03", "10:20:00", 9)];
    assert_eq!(resolve(&[bucket], &rows), vec![STALE_OVERLAP]);
}

#[test]
fn a_coarse_row_ending_mid_bucket_is_stale_with_overlap() {
    let bucket = minute_bucket("10:05:00", "10:05:10", "10:05:50", 10);
    let rows = [row("10:00:03", "10:05:30", 100)];
    assert_eq!(resolve(&[bucket], &rows), vec![STALE_OVERLAP]);
}

#[test]
fn a_schema_hash_mismatch_is_stale() {
    let bucket = minute_bucket("10:00:00", "10:00:05", "10:00:30", 10);
    let rows = [row_with_schema("10:00:05", "10:00:30", 10, vec![9])];
    assert_eq!(resolve(&[bucket], &rows), vec![STALE_CONTAINMENT]);

    let bucket = minute_bucket("10:05:00", "10:05:10", "10:05:50", 10);
    let rows = [row_with_schema("10:00:03", "10:20:00", 100, vec![9])];
    assert_eq!(resolve(&[bucket], &rows), vec![STALE_OVERLAP]);
}

/// A legacy partition written while its hour was open covers the earlier minutes until a later
/// bucket's `Overlap` write retires it; every bucket it covered must then be rewritten, with
/// `Overlap` because the raw candidate set still holds the straddling row.
#[test]
fn an_overlap_retire_cascades_to_buckets_that_relied_on_the_row() {
    let early = minute_bucket("10:00:00", "10:00:10", "10:00:50", 10);
    let late = minute_bucket("10:06:00", "10:06:00", "10:06:30", 4);
    let rows = [row("10:00:03", "10:06:00", 100)];
    assert_eq!(
        resolve(&[early, late], &rows),
        vec![STALE_OVERLAP, STALE_OVERLAP]
    );
}

#[test]
fn a_bucket_beside_a_straddling_row_gets_overlap_not_containment() {
    // The straddler is outer to the second bucket only.
    let first = minute_bucket("10:00:00", "10:00:10", "10:00:50", 10);
    let second = minute_bucket("10:01:00", "10:01:10", "10:01:50", 6);
    let rows = [
        row("10:00:10", "10:00:50", 10),
        row("10:00:55", "10:01:20", 3),
    ];
    let verdicts = resolve(&[first, second], &rows);
    assert_eq!(verdicts[1], STALE_OVERLAP);
}

#[test]
fn buckets_with_independent_rows_do_not_cascade() {
    let a = minute_bucket("10:00:00", "10:00:05", "10:00:30", 10);
    let b = minute_bucket("10:01:00", "10:01:05", "10:01:30", 8);
    let rows = [
        row("10:00:05", "10:00:30", 10),
        row("10:01:05", "10:01:30", 7),
    ];
    assert_eq!(
        resolve(&[a, b], &rows),
        vec![Coverage::Covered, STALE_CONTAINMENT]
    );
}

#[test]
fn conflict_retry_drops_a_bucket_a_concurrent_writer_covers() {
    let bucket = minute_bucket("10:05:00", "10:05:10", "10:05:50", 10);
    let refetched = [row("10:00:03", "10:20:00", 100)];
    assert_eq!(
        decide_conflict_retry(&bucket, &view_meta(), &refetched).unwrap(),
        ConflictRetry::Drop
    );
}

#[test]
fn conflict_retry_rederives_the_retire_mode_from_the_refetched_rows() {
    let bucket = minute_bucket("10:05:00", "10:05:10", "10:05:50", 10);
    // a concurrent writer committed a straddling row that does not cover the bucket
    let straddler = [row("10:04:00", "10:05:30", 100)];
    assert_eq!(
        decide_conflict_retry(&bucket, &view_meta(), &straddler).unwrap(),
        ConflictRetry::Retry {
            retire_match: RetireMatch::Overlap
        }
    );
    // only a short inner row: contained, so Containment suffices
    let short = [row("10:05:10", "10:05:20", 4)];
    assert_eq!(
        decide_conflict_retry(&bucket, &view_meta(), &short).unwrap(),
        ConflictRetry::Retry {
            retire_match: RetireMatch::Containment
        }
    );
}
