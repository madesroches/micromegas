//! Pure, no-DB unit tests for `promotion_candidates`.

use chrono::{DateTime, TimeDelta, Utc};
use micromegas_analytics::lakehouse::jit_partitions::{
    Coverage, JitBucket, PartitionFreshnessRow, RangeConvention,
};
use micromegas_analytics::lakehouse::jit_promotion::promotion_candidates;
use micromegas_analytics::lakehouse::partition_ladder::PlannedBucket;
use micromegas_analytics::time::TimeRange;

const HOUR: TimeDelta = TimeDelta::seconds(3600);
const FIVE_MINUTES: TimeDelta = TimeDelta::seconds(300);
const SCHEMA: [u8; 1] = [7];

fn hour_start() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2024-01-01T10:00:00Z")
        .unwrap()
        .to_utc()
}

fn bucket(rung: TimeDelta, closed: bool) -> JitBucket {
    let begin = hour_start();
    JitBucket {
        planned: PlannedBucket {
            range: TimeRange::new(begin, begin + HOUR),
            rung,
            closed,
            nb_blocks: 60,
            nb_objects: 600,
        },
        specs: vec![],
    }
}

fn minute_rows(nb: i32, count: i64, schema: &[u8]) -> Vec<PartitionFreshnessRow> {
    (0..nb)
        .map(|i| PartitionFreshnessRow {
            begin_insert_time: hour_start() + TimeDelta::minutes(i as i64) + TimeDelta::seconds(1),
            end_insert_time: hour_start() + TimeDelta::minutes(i as i64) + TimeDelta::seconds(30),
            file_schema_hash: schema.to_vec(),
            source_data_hash: count.to_le_bytes().to_vec(),
        })
        .collect()
}

fn select(
    bucket: &JitBucket,
    verdict: Coverage,
    rows: &[PartitionFreshnessRow],
    max_nb_objects: i64,
) -> usize {
    promotion_candidates(
        std::slice::from_ref(bucket),
        &[verdict],
        rows,
        HOUR,
        max_nb_objects,
    )
    .len()
}

#[test]
fn accepts_sixty_minute_rows_inside_a_closed_hour() {
    let rows = minute_rows(60, 10, &SCHEMA);
    let targets = promotion_candidates(
        &[bucket(HOUR, true)],
        &[Coverage::Covered],
        &rows,
        HOUR,
        1_000,
    );
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].inputs.len(), 60);
    assert_eq!(targets[0].bucket.begin, hour_start());
}

#[test]
fn rejects_open_buckets() {
    let rows = minute_rows(60, 10, &SCHEMA);
    assert_eq!(
        select(&bucket(HOUR, false), Coverage::Covered, &rows, 1_000),
        0
    );
}

#[test]
fn rejects_descended_buckets() {
    let rows = minute_rows(5, 10, &SCHEMA);
    assert_eq!(
        select(&bucket(FIVE_MINUTES, true), Coverage::Covered, &rows, 1_000),
        0
    );
}

#[test]
fn rejects_single_row_buckets() {
    let rows = minute_rows(1, 10, &SCHEMA);
    assert_eq!(
        select(&bucket(HOUR, true), Coverage::Covered, &rows, 1_000),
        0
    );
}

#[test]
fn rejects_sums_over_the_cap() {
    let rows = minute_rows(60, 10, &SCHEMA);
    assert_eq!(
        select(&bucket(HOUR, true), Coverage::Covered, &rows, 599),
        0
    );
    assert_eq!(
        select(&bucket(HOUR, true), Coverage::Covered, &rows, 600),
        1
    );
}

#[test]
fn rejects_mixed_schema_hashes() {
    let mut rows = minute_rows(60, 10, &SCHEMA);
    rows[10].file_schema_hash = vec![8];
    assert_eq!(
        select(&bucket(HOUR, true), Coverage::Covered, &rows, 1_000),
        0
    );
}

#[test]
fn rejects_buckets_that_are_not_covered() {
    let rows = minute_rows(60, 10, &SCHEMA);
    let stale = Coverage::Stale {
        retire_match: micromegas_analytics::lakehouse::write_partition::RetireMatch::Containment,
    };
    assert_eq!(select(&bucket(HOUR, true), stale, &rows, 1_000), 0);
}

#[test]
fn aligned_convention_treats_a_row_ending_at_the_bucket_end_as_inside() {
    let bucket = TimeRange::new(hour_start(), hour_start() + HOUR);
    let row = |begin: DateTime<Utc>, end: DateTime<Utc>| PartitionFreshnessRow {
        begin_insert_time: begin,
        end_insert_time: end,
        file_schema_hash: SCHEMA.to_vec(),
        source_data_hash: 1_i64.to_le_bytes().to_vec(),
    };
    let whole = row(bucket.begin, bucket.end);
    assert!(RangeConvention::Aligned.row_inside(&whole, bucket));
    assert!(!RangeConvention::BlockHull.row_inside(&whole, bucket));

    // Touching neighbors do not intersect under `Aligned`, they do under `BlockHull`'s inclusive end.
    let before = row(bucket.begin - HOUR, bucket.begin);
    let after = row(bucket.end, bucket.end + HOUR);
    assert!(!RangeConvention::Aligned.row_intersects(&before, bucket));
    assert!(!RangeConvention::Aligned.row_intersects(&after, bucket));
    assert!(RangeConvention::BlockHull.row_intersects(&before, bucket));

    let straddling = row(bucket.end - FIVE_MINUTES, bucket.end + FIVE_MINUTES);
    assert!(RangeConvention::Aligned.row_intersects(&straddling, bucket));
    assert!(!RangeConvention::Aligned.row_inside(&straddling, bucket));
}
