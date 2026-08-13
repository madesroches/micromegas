//! Pure, no-DB unit tests for `batch_windows` and `process_batch_sql`
//! (tasks/jit_batched_block_queries_plan.md, Testing Strategy): the adaptive batch-width packing
//! and the lean-projection guard.

use chrono::{DateTime, TimeDelta, Utc};
use micromegas_analytics::lakehouse::jit_partitions::{batch_windows, process_batch_sql};
use micromegas_analytics::time::TimeRange;

fn base_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
        .unwrap()
        .to_utc()
}

/// Asserts the tiling invariants every `batch_windows` output must uphold, regardless of density:
/// the first window begins at `insert_range.begin`, the last ends at `insert_range.end`, windows
/// are contiguous (no gaps, no overlaps), and every edge lands on a `slice`-aligned bucket
/// boundary relative to `insert_range.begin`.
fn assert_tiling(windows: &[TimeRange], insert_range: TimeRange, slice: TimeDelta) {
    assert!(
        !windows.is_empty(),
        "batch_windows must return at least one window"
    );
    assert_eq!(
        windows[0].begin, insert_range.begin,
        "the first window must begin at insert_range.begin"
    );
    assert_eq!(
        windows[windows.len() - 1].end,
        insert_range.end,
        "the last window must end exactly at insert_range.end"
    );
    for pair in windows.windows(2) {
        assert_eq!(
            pair[0].end, pair[1].begin,
            "windows must be contiguous: no gaps, no overlaps"
        );
    }
    for w in windows {
        assert!(w.begin < w.end, "a window must not be empty/inverted");
        let begin_offset = (w.begin - insert_range.begin).num_seconds();
        let end_offset = (w.end - insert_range.begin).num_seconds();
        let slice_secs = slice.num_seconds();
        assert_eq!(
            begin_offset % slice_secs,
            0,
            "window begin must be bucket-aligned"
        );
        assert_eq!(
            end_offset % slice_secs,
            0,
            "window end must be bucket-aligned"
        );
    }
}

/// `batch_windows` tiles a slice-aligned range with no gaps or overlaps, edges on bucket
/// boundaries, last window ending exactly at the range end -- across a mix of empty and non-empty
/// buckets forcing several closes.
#[test]
fn tiles_the_range_with_no_gaps_or_overlaps() {
    let t0 = base_time();
    let slice = TimeDelta::hours(1);
    let insert_range = TimeRange::new(t0, t0 + slice * 10);
    // Buckets 0, 2, 3, 4, 7, 9 are non-empty; everything else implicitly 0.
    let bucket_counts = vec![
        (t0, 1),
        (t0 + slice * 2, 3),
        (t0 + slice * 3, 6),
        (t0 + slice * 4, 5),
        (t0 + slice * 7, 2),
        (t0 + slice * 9, 1),
    ];
    let windows: Vec<TimeRange> = batch_windows(insert_range, slice, &bucket_counts, 5).collect();
    assert_tiling(&windows, insert_range, slice);
    // Sanity: more than one window, since some individual buckets/pairs exceed the target of 5.
    assert!(windows.len() > 1, "expected more than one batch window");
}

/// Sparse density (few blocks spread over many buckets) collapses to a single window: the running
/// total never reaches `target_rows_per_query`.
#[test]
fn sparse_density_collapses_to_a_single_window() {
    let t0 = base_time();
    let slice = TimeDelta::hours(1);
    let insert_range = TimeRange::new(t0, t0 + slice * 30 * 24); // 30 days of hourly buckets
    // ~43k blocks spread over 720 buckets averages ~60/bucket -- model that sparsely: one block
    // every few buckets.
    let bucket_counts: Vec<(DateTime<Utc>, i64)> = (0..720i64)
        .step_by(5)
        .map(|i| (t0 + slice * i as i32, 60))
        .collect();
    let windows: Vec<TimeRange> =
        batch_windows(insert_range, slice, &bucket_counts, 250_000).collect();
    assert_tiling(&windows, insert_range, slice);
    assert_eq!(
        windows.len(),
        1,
        "a sparse instance must collapse to one query for the whole range"
    );
}

/// Dense density yields multiple windows, each covering at least one bucket, and (aside from the
/// single-oversized-bucket residual case) each window's own row total stays within
/// `target_rows_per_query`.
#[test]
fn dense_density_yields_multiple_windows() {
    let t0 = base_time();
    let slice = TimeDelta::hours(1);
    let insert_range = TimeRange::new(t0, t0 + slice * 20);
    // Every bucket busy at 4k blocks/bucket; target 10k -> packs ~2 buckets/window.
    let bucket_counts: Vec<(DateTime<Utc>, i64)> =
        (0..20i64).map(|i| (t0 + slice * i as i32, 4_000)).collect();
    let target = 10_000;
    let windows: Vec<TimeRange> =
        batch_windows(insert_range, slice, &bucket_counts, target).collect();
    assert_tiling(&windows, insert_range, slice);
    assert!(
        windows.len() > 1,
        "a dense, evenly busy instance must split into more than one window"
    );
    for w in &windows {
        let nb_buckets = (w.end - w.begin).num_seconds() / slice.num_seconds();
        assert!(
            nb_buckets >= 1,
            "every window must cover at least one bucket"
        );
        let total: i64 = bucket_counts
            .iter()
            .filter(|(b, _)| *b >= w.begin && *b < w.end)
            .map(|(_, n)| n)
            .sum();
        assert!(
            total <= target,
            "window [{}, {}) totals {total}, over target {target}",
            w.begin,
            w.end
        );
    }
}

/// A burst concentrated in a handful of buckets amid many near-empty ones is not diluted by its
/// neighbors: an average-density design would merge the whole (mostly empty) range into one
/// query, but the per-bucket packing here forces a close as soon as the burst's own running total
/// would exceed the target, so each resulting window's total stays within bounds regardless of how
/// many empty buckets surround the burst.
#[test]
fn a_burst_is_not_diluted_by_surrounding_empty_buckets() {
    let t0 = base_time();
    let slice = TimeDelta::hours(1);
    let insert_range = TimeRange::new(t0, t0 + slice * 10);
    // Buckets 4 and 5 hold a burst of 8k blocks each; everything else (8 buckets) is empty.
    let bucket_counts = vec![(t0 + slice * 4, 8_000), (t0 + slice * 5, 8_000)];
    let target = 10_000;
    let windows: Vec<TimeRange> =
        batch_windows(insert_range, slice, &bucket_counts, target).collect();
    assert_tiling(&windows, insert_range, slice);
    assert_eq!(
        windows.len(),
        2,
        "the burst must force a close rather than being diluted into a single whole-range window, \
         got {windows:?}"
    );
    for w in &windows {
        let total: i64 = bucket_counts
            .iter()
            .filter(|(b, _)| *b >= w.begin && *b < w.end)
            .map(|(_, n)| n)
            .sum();
        assert!(
            total <= target,
            "window [{}, {}) totals {total}, over target {target}",
            w.begin,
            w.end
        );
    }
}

/// The one case `batch_windows` cannot bound further: a single bucket whose own count already
/// exceeds `target_rows_per_query` still forms one batch on its own (the loop never splits a
/// bucket), matching today's per-bucket behavior rather than a new hazard. Placed at the very
/// start of `insert_range` (no leading empty bucket) so the resulting window is exactly one bucket
/// wide -- an oversized bucket preceded by empty buckets still returns only its own row count
/// (empty buckets contribute none), but may absorb those empty buckets' *time* span into the same
/// window since they never force an earlier close on their own.
#[test]
fn a_single_oversized_bucket_still_forms_its_own_window() {
    let t0 = base_time();
    let slice = TimeDelta::hours(1);
    let insert_range = TimeRange::new(t0, t0 + slice * 2);
    let bucket_counts = vec![(t0, 1_000_000)];
    let windows: Vec<TimeRange> =
        batch_windows(insert_range, slice, &bucket_counts, 1_000).collect();
    assert_tiling(&windows, insert_range, slice);
    assert_eq!(
        windows[0],
        TimeRange::new(t0, t0 + slice),
        "the oversized bucket must form exactly one window on its own, got {windows:?}"
    );
}

/// Zero blocks (an empty `bucket_counts`) over a range yields one window covering the whole range.
#[test]
fn zero_blocks_yields_one_window() {
    let t0 = base_time();
    let slice = TimeDelta::hours(1);
    let insert_range = TimeRange::new(t0, t0 + slice * 5);
    let windows: Vec<TimeRange> = batch_windows(insert_range, slice, &[], 250_000).collect();
    assert_tiling(&windows, insert_range, slice);
    assert_eq!(windows.len(), 1, "zero blocks must yield a single window");
    assert_eq!(windows[0], insert_range);
}

/// A single-bucket range yields one window, regardless of that bucket's count.
#[test]
fn single_bucket_range_yields_one_window() {
    let t0 = base_time();
    let slice = TimeDelta::hours(1);
    let insert_range = TimeRange::new(t0, t0 + slice);
    let bucket_counts = vec![(t0, 42)];
    let windows: Vec<TimeRange> = batch_windows(insert_range, slice, &bucket_counts, 10).collect();
    assert_eq!(
        windows.len(),
        1,
        "a single-bucket range must yield one window"
    );
    assert_eq!(windows[0], insert_range);
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
