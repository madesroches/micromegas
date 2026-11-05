//! Pure, no-DB unit tests for `batch_windows` and `process_batch_sql`: the adaptive batch-width
//! packing over planned buckets and the lean-projection guard.

use chrono::{DateTime, TimeDelta, Utc};
use micromegas_analytics::lakehouse::jit_partitions::{batch_windows, process_batch_sql};
use micromegas_analytics::lakehouse::partition_ladder::PlannedBucket;
use micromegas_analytics::time::TimeRange;

fn base_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
        .unwrap()
        .to_utc()
}

fn bucket(begin: DateTime<Utc>, width: TimeDelta, nb_blocks: i64) -> PlannedBucket {
    PlannedBucket {
        range: TimeRange::new(begin, begin + width),
        rung: width,
        closed: true,
        nb_blocks,
        nb_objects: nb_blocks,
    }
}

/// Planned buckets of one `slice` width at the given bucket indices with the given counts.
fn hourly(t0: DateTime<Utc>, counts: &[(i32, i64)]) -> Vec<PlannedBucket> {
    let slice = TimeDelta::hours(1);
    counts
        .iter()
        .map(|(i, n)| bucket(t0 + slice * *i, slice, *n))
        .collect()
}

fn window_total(w: &TimeRange, buckets: &[PlannedBucket]) -> i64 {
    buckets
        .iter()
        .filter(|b| b.range.begin >= w.begin && b.range.end <= w.end)
        .map(|b| b.nb_blocks)
        .sum()
}

/// Asserts the tiling invariants every `batch_windows` output must uphold: windows are contiguous
/// (no gaps, no overlaps), the first begins at the first bucket and the last ends at the last
/// bucket, and every edge lands on a bucket edge.
fn assert_tiling(windows: &[TimeRange], buckets: &[PlannedBucket]) {
    assert!(!windows.is_empty(), "expected at least one window");
    assert_eq!(windows[0].begin, buckets[0].range.begin);
    assert_eq!(
        windows[windows.len() - 1].end,
        buckets[buckets.len() - 1].range.end
    );
    for pair in windows.windows(2) {
        assert_eq!(pair[0].end, pair[1].begin, "windows must be contiguous");
    }
    for w in windows {
        assert!(w.begin < w.end, "a window must not be empty/inverted");
        assert!(
            buckets.iter().any(|b| b.range.begin == w.begin),
            "window begin must be a bucket edge"
        );
        assert!(
            buckets.iter().any(|b| b.range.end == w.end)
                || buckets.iter().any(|b| b.range.begin == w.end),
            "window end must be a bucket edge"
        );
    }
}

/// Windows tile the planned buckets with no gaps or overlaps across a mix of busy and empty
/// stretches forcing several closes.
#[test]
fn tiles_the_buckets_with_no_gaps_or_overlaps() {
    let t0 = base_time();
    let buckets = hourly(t0, &[(0, 1), (2, 3), (3, 6), (4, 5), (7, 2), (9, 1)]);
    let windows: Vec<TimeRange> = batch_windows(&buckets, 5).collect();
    assert_tiling(&windows, &buckets);
    assert!(windows.len() > 1, "expected more than one batch window");
}

/// Sparse density (few blocks spread over many buckets) collapses to a single window: the running
/// total never reaches `target_rows_per_query`.
#[test]
fn sparse_density_collapses_to_a_single_window() {
    let t0 = base_time();
    let counts: Vec<(i32, i64)> = (0..720).step_by(5).map(|i| (i, 60)).collect();
    let buckets = hourly(t0, &counts);
    let windows: Vec<TimeRange> = batch_windows(&buckets, 250_000).collect();
    assert_tiling(&windows, &buckets);
    assert_eq!(
        windows.len(),
        1,
        "a sparse instance must collapse to one query for the whole range"
    );
}

/// Dense density yields multiple windows, each within `target_rows_per_query`.
#[test]
fn dense_density_yields_multiple_windows() {
    let t0 = base_time();
    let counts: Vec<(i32, i64)> = (0..20).map(|i| (i, 4_000)).collect();
    let buckets = hourly(t0, &counts);
    let target = 10_000;
    let windows: Vec<TimeRange> = batch_windows(&buckets, target).collect();
    assert_tiling(&windows, &buckets);
    assert!(windows.len() > 1);
    for w in &windows {
        let total = window_total(w, &buckets);
        assert!(total >= 1 && total <= target, "window {w:?} totals {total}");
    }
}

/// A burst in a handful of buckets amid many near-empty ones is not diluted by its neighbors.
#[test]
fn a_burst_is_not_diluted_by_surrounding_empty_buckets() {
    let t0 = base_time();
    let buckets = hourly(t0, &[(0, 1), (4, 8_000), (5, 8_000), (9, 1)]);
    let target = 10_000;
    let windows: Vec<TimeRange> = batch_windows(&buckets, target).collect();
    assert_tiling(&windows, &buckets);
    for w in &windows {
        let total = window_total(w, &buckets);
        assert!(total <= target, "window {w:?} totals {total}");
    }
    assert!(windows.len() >= 2, "got {windows:?}");
}

/// A single bucket whose own count already exceeds `target_rows_per_query` still forms one batch
/// on its own (the loop never splits a bucket).
#[test]
fn a_single_oversized_bucket_still_forms_its_own_window() {
    let t0 = base_time();
    let buckets = hourly(t0, &[(0, 1_000_000), (1, 5)]);
    let windows: Vec<TimeRange> = batch_windows(&buckets, 1_000).collect();
    assert_tiling(&windows, &buckets);
    assert_eq!(windows[0], buckets[0].range);
}

/// No planned buckets yields no window.
#[test]
fn no_buckets_yields_no_window() {
    assert_eq!(batch_windows(&[], 250_000).count(), 0);
}

/// A single bucket yields one window, regardless of its count.
#[test]
fn single_bucket_yields_one_window() {
    let t0 = base_time();
    let buckets = hourly(t0, &[(0, 42)]);
    let windows: Vec<TimeRange> = batch_windows(&buckets, 10).collect();
    assert_eq!(windows, vec![buckets[0].range]);
}

/// Buckets of different widths (a closed hour followed by minutes) pack by count, with window
/// edges on bucket edges.
#[test]
fn mixed_width_buckets_pack_by_count() {
    let t0 = base_time();
    let minute = TimeDelta::minutes(1);
    let mut buckets = vec![bucket(t0, TimeDelta::hours(1), 6)];
    for i in 0..4 {
        buckets.push(bucket(t0 + TimeDelta::hours(1) + minute * i, minute, 3));
    }
    let windows: Vec<TimeRange> = batch_windows(&buckets, 10).collect();
    assert_tiling(&windows, &buckets);
    // 6 + 3 fits; the next 3 would reach 12; the remaining three minutes total 9.
    assert_eq!(windows.len(), 2, "got {windows:?}");
    assert_eq!(windows[0].begin, t0);
    assert_eq!(windows[0].end, t0 + TimeDelta::hours(1) + minute);
    assert_eq!(windows[1].end, t0 + TimeDelta::hours(1) + minute * 4);
}

/// Projection guard (the exact regression the module's rustdoc warns about): `process_batch_sql`'s
/// `SELECT` list must not contain any `streams.`-prefixed column. The `WHERE` clause still may
/// (filtering needs no projection).
#[test]
fn process_batch_sql_projects_no_stream_level_column() {
    let t0 = base_time();
    let range = TimeRange::new(t0, t0 + TimeDelta::hours(1));
    let process_id = uuid::Uuid::new_v4();
    let sql = process_batch_sql(&process_id, "cpu", &range);

    let select_list = sql
        .split("FROM")
        .next()
        .expect("SQL must have a SELECT clause")
        .to_ascii_lowercase();
    assert!(
        !select_list.contains("streams."),
        "process_batch_sql's SELECT list must not project any stream-level column, got: {sql}"
    );
    // The WHERE clause is allowed (and expected) to filter on a stream-level column.
    assert!(
        sql.to_ascii_lowercase().contains(r#""streams.tags""#),
        "process_batch_sql must still filter on streams.tags in the WHERE clause, got: {sql}"
    );
}
