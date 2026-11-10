//! Pure, no-DB unit tests for the bucket ladder: `select_rung` thresholds and `plan_buckets`.

use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use micromegas_analytics::lakehouse::partition_ladder::{
    BucketCount, LADDER, PlannedBucket, finer_rung, plan_buckets, plan_dense_buckets, select_rung,
};
use micromegas_analytics::time::TimeRange;

fn t(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339).unwrap().to_utc()
}

fn base_time() -> DateTime<Utc> {
    t("2024-01-01T00:00:00Z")
}

fn range_of(width: TimeDelta) -> TimeRange {
    let t0 = base_time();
    TimeRange::new(t0, t0 + width)
}

const MINUTE: TimeDelta = LADDER[0];
const FIVE_MINUTES: TimeDelta = LADDER[1];
const HOUR: TimeDelta = LADDER[2];
const SIX_HOURS: TimeDelta = LADDER[3];
const DAY: TimeDelta = LADDER[4];

#[test]
fn select_rung_at_each_threshold() {
    let cases = [
        (TimeDelta::seconds(10), MINUTE),
        (TimeDelta::minutes(1), MINUTE),
        (TimeDelta::minutes(15), MINUTE),
        (TimeDelta::hours(1), MINUTE),
        (TimeDelta::hours(2), FIVE_MINUTES),
        (TimeDelta::hours(6), FIVE_MINUTES),
        (TimeDelta::hours(23), FIVE_MINUTES),
        (TimeDelta::hours(24), HOUR),
        (TimeDelta::days(2), HOUR),
        (TimeDelta::days(5), HOUR),
        (TimeDelta::days(6), SIX_HOURS),
        (TimeDelta::days(7), SIX_HOURS),
        (TimeDelta::days(23), SIX_HOURS),
        (TimeDelta::days(30), DAY),
        (TimeDelta::days(400), DAY),
    ];
    for (width, expected) in cases {
        assert_eq!(select_rung(range_of(width)), expected, "range {width}");
    }
}

#[test]
fn every_rung_divides_the_next_and_finer_rung_walks_down() {
    for pair in LADDER.windows(2) {
        assert_eq!(pair[1].num_seconds() % pair[0].num_seconds(), 0);
        assert_eq!(finer_rung(pair[1]), Some(pair[0]));
    }
    assert_eq!(finer_rung(MINUTE), None);
    assert_eq!(finer_rung(TimeDelta::seconds(7)), None);
}

/// One block of 10 objects in every minute of `range`.
fn dense_minutes(range: TimeRange) -> Vec<BucketCount> {
    let mut counts = vec![];
    let mut minute = range.begin;
    while minute < range.end {
        counts.push((minute, 1, 10));
        minute += MINUTE;
    }
    counts
}

fn plan(
    insert_range: TimeRange,
    rung: TimeDelta,
    now: DateTime<Utc>,
    counts: &[BucketCount],
    max_nb_objects: i64,
) -> Vec<PlannedBucket> {
    plan_buckets(
        insert_range,
        rung,
        MINUTE,
        now,
        TimeDelta::seconds(90),
        counts,
        max_nb_objects,
    )
}

#[test]
fn rolling_24h_range_has_closed_hours_and_an_open_minute_tail() {
    let now = t("2024-01-02T10:30:30Z");
    let insert_range = TimeRange::new(t("2024-01-01T10:00:00Z"), t("2024-01-02T11:00:00Z"));
    let counts = dense_minutes(TimeRange::new(insert_range.begin, now));
    let planned = plan(insert_range, HOUR, now, &counts, i64::MAX);

    let hours: Vec<_> = planned.iter().filter(|b| b.rung == HOUR).collect();
    assert_eq!(hours.len(), 24);
    assert!(hours.iter().all(|b| b.closed));
    for b in planned.iter().filter(|b| !b.closed) {
        assert_eq!(b.rung, MINUTE, "only minute buckets may be open: {b:?}");
    }
    assert!(!planned.last().unwrap().closed);
    assert_eq!(planned.last().unwrap().rung, MINUTE);
    assert!(planned.iter().any(|b| b.rung == FIVE_MINUTES && b.closed));
    // the planned buckets account for every block, once
    assert_eq!(
        planned.iter().map(|b| b.nb_blocks).sum::<i64>(),
        counts.len() as i64
    );
    for pair in planned.windows(2) {
        assert!(pair[0].range.end <= pair[1].range.begin);
    }
}

#[test]
fn a_bucket_over_the_cap_descends_one_level_at_a_time() {
    let now = t("2024-01-02T00:00:00Z");
    let hour = TimeRange::new(t("2024-01-01T10:00:00Z"), t("2024-01-01T11:00:00Z"));
    let counts = dense_minutes(hour); // 600 objects in the hour, 50 per 5 minutes
    let planned = plan(hour, HOUR, now, &counts, 100);
    assert_eq!(planned.len(), 12);
    assert!(planned.iter().all(|b| b.rung == FIVE_MINUTES && b.closed));

    let planned = plan(hour, HOUR, now, &counts, 30);
    assert_eq!(planned.len(), 60);
    assert!(planned.iter().all(|b| b.rung == MINUTE));
}

#[test]
fn an_over_cap_minute_is_emitted_as_is() {
    let now = t("2024-01-02T00:00:00Z");
    let minute = TimeRange::new(t("2024-01-01T10:00:00Z"), t("2024-01-01T10:01:00Z"));
    let counts = vec![(minute.begin, 3, 500)];
    let planned = plan_buckets(
        TimeRange::new(minute.begin, minute.begin + HOUR),
        HOUR,
        MINUTE,
        now,
        TimeDelta::seconds(90),
        &counts,
        100,
    );
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].range, minute);
    assert_eq!(planned[0].rung, MINUTE);
    assert!(planned[0].closed);
}

#[test]
fn empty_buckets_are_skipped() {
    let now = t("2024-01-02T00:00:00Z");
    let insert_range = TimeRange::new(t("2024-01-01T00:00:00Z"), t("2024-01-01T06:00:00Z"));
    let counts = vec![(t("2024-01-01T03:20:00Z"), 2, 7)];
    let planned = plan(insert_range, HOUR, now, &counts, 100);
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].range.begin, t("2024-01-01T03:00:00Z"));
    assert_eq!(planned[0].rung, HOUR);
    assert_eq!((planned[0].nb_blocks, planned[0].nb_objects), (2, 7));
}

#[test]
fn a_single_bucket_range_with_skipped_counts_is_non_empty() {
    let now = t("2024-01-02T00:00:00Z");
    let minute = TimeRange::new(t("2024-01-01T10:00:00Z"), t("2024-01-01T10:01:00Z"));
    let planned = plan(minute, MINUTE, now, &[], 100);
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].range, minute);
}

#[test]
fn buckets_nest_inside_every_coarser_rung() {
    let now = t("2024-01-02T10:30:30Z");
    let insert_range = TimeRange::new(t("2024-01-01T10:00:00Z"), t("2024-01-02T11:00:00Z"));
    let counts = dense_minutes(TimeRange::new(insert_range.begin, now));
    let planned = plan(insert_range, HOUR, now, &counts, 300);
    assert!(!planned.is_empty());
    for b in &planned {
        for rung in LADDER.iter().filter(|r| **r >= b.rung) {
            let begin = b.range.begin.duration_trunc(*rung).unwrap();
            assert!(
                b.range.end <= begin + *rung,
                "{b:?} straddles a {rung} boundary"
            );
        }
    }
}

#[test]
fn fixed_sizing_reproduces_hourly_buckets() {
    let insert_range = TimeRange::new(t("2024-01-01T00:00:00Z"), t("2024-01-01T05:00:00Z"));
    let counts = vec![
        (t("2024-01-01T00:00:00Z"), 1, 5),
        (t("2024-01-01T02:00:00Z"), 4, 1_000_000_000),
        (t("2024-01-01T04:00:00Z"), 2, 6),
    ];
    let planned = plan_buckets(
        insert_range,
        HOUR,
        HOUR,
        DateTime::<Utc>::MAX_UTC,
        TimeDelta::zero(),
        &counts,
        100,
    );
    let begins: Vec<_> = planned.iter().map(|b| b.range.begin).collect();
    assert_eq!(
        begins,
        vec![
            t("2024-01-01T00:00:00Z"),
            t("2024-01-01T02:00:00Z"),
            t("2024-01-01T04:00:00Z")
        ]
    );
    assert!(planned.iter().all(|b| b.rung == HOUR && b.closed));
}

#[test]
fn dense_plan_emits_closed_buckets_at_the_rung_and_descends_only_the_open_edge() {
    let now = t("2024-01-02T10:30:30Z");
    let end_of_minute = t("2024-01-02T10:31:00Z");
    let insert_range = TimeRange::new(t("2024-01-01T00:00:00Z"), end_of_minute);
    let planned = plan_dense_buckets(insert_range, SIX_HOURS, MINUTE, now, TimeDelta::seconds(90));

    let closed: Vec<_> = planned.iter().filter(|b| b.closed).collect();
    assert!(closed.iter().any(|b| b.rung == SIX_HOURS));
    for b in planned.iter().filter(|b| !b.closed) {
        assert_eq!(b.rung, MINUTE, "only minute buckets may be open: {b:?}");
        assert!(b.range.begin >= t("2024-01-02T10:00:00Z"));
    }
    // Jan 1's four 6h buckets and Jan 2's 00:00-06:00 are closed; the 06:00-12:00 one is open
    // and descends.
    assert_eq!(planned.iter().filter(|b| b.rung == SIX_HOURS).count(), 5);
    for pair in planned.windows(2) {
        assert!(pair[0].range.end <= pair[1].range.begin);
    }
}

#[test]
fn dense_plan_never_begins_a_bucket_at_or_after_the_end() {
    let now = t("2024-01-02T10:30:30Z");
    let end_of_minute = t("2024-01-02T10:31:00Z");
    let insert_range = TimeRange::new(t("2024-01-02T06:00:00Z"), end_of_minute);
    let planned = plan_dense_buckets(insert_range, SIX_HOURS, MINUTE, now, TimeDelta::seconds(90));
    assert!(!planned.is_empty());
    assert!(planned.iter().all(|b| b.range.begin < end_of_minute));
    assert_eq!(planned.last().unwrap().range.end, end_of_minute);
}

#[test]
fn dense_plan_of_a_future_range_is_empty() {
    let now = t("2024-01-02T10:30:30Z");
    let insert_range = TimeRange::new(t("2024-01-02T12:00:00Z"), t("2024-01-02T10:31:00Z"));
    assert!(
        plan_dense_buckets(insert_range, MINUTE, MINUTE, now, TimeDelta::seconds(90)).is_empty()
    );
}
