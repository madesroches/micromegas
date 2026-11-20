//! Aligned bucket ladder (minute, 5 min, 15 min, 30 min, hour, 6 h, day) used to size JIT partitions from the
//! query range, and the planner that turns per-minute block counts into the buckets to
//! materialize.
//!
//! Every rung divides the next one and `duration_trunc` aligns to the Unix epoch, so buckets of
//! different rungs nest and never straddle each other.

use crate::time::TimeRange;
use chrono::{DateTime, DurationRound, TimeDelta, Utc};

/// Bucket widths, finest first. Each rung divides the next one.
pub const LADDER: [TimeDelta; 7] = [
    TimeDelta::seconds(60),
    TimeDelta::seconds(5 * 60),
    TimeDelta::seconds(15 * 60),
    TimeDelta::seconds(30 * 60),
    TimeDelta::seconds(60 * 60),
    TimeDelta::seconds(6 * 60 * 60),
    TimeDelta::seconds(24 * 60 * 60),
];

/// How many buckets a query should be spread over, give or take a rung.
pub const TARGET_BUCKETS_PER_QUERY: i64 = 24;

/// Largest rung `r` with `range / r >= TARGET_BUCKETS_PER_QUERY`; the finest rung if none.
pub fn select_rung(query_range: TimeRange) -> TimeDelta {
    let range = query_range.end - query_range.begin;
    LADDER
        .iter()
        .rev()
        .find(|rung| range.num_seconds() / rung.num_seconds().max(1) >= TARGET_BUCKETS_PER_QUERY)
        .copied()
        .unwrap_or(LADDER[0])
}

/// The rung right below `rung`, if any.
pub fn finer_rung(rung: TimeDelta) -> Option<TimeDelta> {
    let index = LADDER.iter().position(|r| *r == rung)?;
    index.checked_sub(1).map(|i| LADDER[i])
}

/// One insert-time bucket the planner decided to materialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedBucket {
    /// Half-open, rung-aligned.
    pub range: TimeRange,
    pub rung: TimeDelta,
    /// True when no new block can land in the bucket anymore (see `plan_buckets`).
    pub closed: bool,
    pub nb_blocks: i64,
    pub nb_objects: i64,
}

/// Block and object counts of one non-empty `finest`-wide bucket, keyed by its start.
pub type BucketCount = (DateTime<Utc>, i64, i64);

fn sum_counts(counts: &[BucketCount], range: TimeRange) -> (i64, i64) {
    let first = counts.partition_point(|c| c.0 < range.begin);
    counts[first..]
        .iter()
        .take_while(|c| c.0 < range.end)
        .fold((0, 0), |(blocks, objects), c| (blocks + c.1, objects + c.2))
}

/// `(nb_blocks, nb_objects)` of a bucket. A bucket with no blocks is skipped.
type CountFn<'a> = &'a dyn Fn(TimeRange) -> (i64, i64);

#[allow(clippy::too_many_arguments)]
fn plan_bucket(
    bucket: TimeRange,
    rung: TimeDelta,
    finest: TimeDelta,
    closed_before: DateTime<Utc>,
    count: CountFn<'_>,
    single_finest_bucket: bool,
    max_nb_objects: i64,
    out: &mut Vec<PlannedBucket>,
) {
    let (nb_blocks, nb_objects) = count(bucket);
    // The count query is skipped for a single-bucket range, which cannot need descent.
    if nb_blocks == 0 && !single_finest_bucket {
        return;
    }
    let closed = bucket.end <= closed_before;
    let finer = if rung > finest {
        finer_rung(rung)
    } else {
        None
    };
    match finer {
        Some(finer) if !closed || nb_objects > max_nb_objects => {
            let mut begin = bucket.begin;
            while begin < bucket.end {
                plan_bucket(
                    TimeRange::new(begin, begin + finer),
                    finer,
                    finest,
                    closed_before,
                    count,
                    false,
                    max_nb_objects,
                    out,
                );
                begin += finer;
            }
        }
        _ => out.push(PlannedBucket {
            range: bucket,
            rung,
            closed,
            nb_blocks,
            nb_objects,
        }),
    }
}

/// Plans the buckets to materialize over `insert_range` (rung-aligned) for a query at `rung`.
///
/// A bucket is emitted at `rung` when it is closed (`end <= now - grace`, `grace` absorbing the
/// commit lag between the insert-time stamp and the row becoming visible) and holds at most
/// `max_nb_objects` objects. Otherwise it descends one rung at a time, down to `finest`, where it
/// is emitted as is. The open edge is therefore only ever materialized at `finest`, so every
/// coarser partition was written while its bucket was closed.
///
/// `counts` holds one entry per non-empty `finest`-wide bucket, ascending. It may be empty when
/// `insert_range` is a single `finest` bucket: the planner then emits that bucket as non-empty.
pub fn plan_buckets(
    insert_range: TimeRange,
    rung: TimeDelta,
    finest: TimeDelta,
    now: DateTime<Utc>,
    grace: TimeDelta,
    counts: &[BucketCount],
    max_nb_objects: i64,
) -> Vec<PlannedBucket> {
    let mut out = vec![];
    let single_finest_bucket = insert_range.end - insert_range.begin <= finest;
    let closed_before = now - grace;
    let mut begin = insert_range.begin;
    while begin < insert_range.end {
        plan_bucket(
            TimeRange::new(begin, begin + rung),
            rung,
            finest,
            closed_before,
            &|range| sum_counts(counts, range),
            single_finest_bucket,
            max_nb_objects,
            &mut out,
        );
        begin += rung;
    }
    out
}

/// Plans the buckets to materialize over `insert_range` (rung-aligned) when no per-bucket count
/// is available: every bucket is assumed non-empty and none descends on object count. A closed
/// bucket is emitted at `rung`; only the open edge descends, down to `finest`. A bucket that
/// begins at or after `insert_range.end` is dropped, so the open edge stops at the end of the
/// range instead of planning empty future buckets.
///
/// `nb_blocks` and `nb_objects` of the result are placeholders: the caller probes each bucket
/// itself.
pub fn plan_dense_buckets(
    insert_range: TimeRange,
    rung: TimeDelta,
    finest: TimeDelta,
    now: DateTime<Utc>,
    grace: TimeDelta,
) -> Vec<PlannedBucket> {
    let mut out = vec![];
    let closed_before = now - grace;
    let count = |bucket: TimeRange| {
        if bucket.begin >= insert_range.end {
            (0, 0)
        } else {
            (1, 0)
        }
    };
    let mut begin = insert_range.begin;
    while begin < insert_range.end {
        plan_bucket(
            TimeRange::new(begin, begin + rung),
            rung,
            finest,
            closed_before,
            &count,
            false,
            i64::MAX,
            &mut out,
        );
        begin += rung;
    }
    out
}

/// Source bytes of a bucket.
pub type BytesFn<'a> = &'a (dyn Fn(TimeRange) -> i64 + Sync);

fn is_aligned(instant: DateTime<Utc>, rung: TimeDelta) -> bool {
    instant.timestamp_subsec_nanos() == 0 && instant.timestamp() % rung.num_seconds() == 0
}

fn plan_bucket_by_bytes(
    bucket: TimeRange,
    rung: TimeDelta,
    bytes: BytesFn<'_>,
    cap: i64,
    out: &mut Vec<PlannedBucket>,
) {
    let finer = finer_rung(rung);
    match finer {
        Some(finer) if bytes(bucket) > cap => {
            let mut begin = bucket.begin;
            while begin < bucket.end {
                plan_bucket_by_bytes(TimeRange::new(begin, begin + finer), finer, bytes, cap, out);
                begin += finer;
            }
        }
        _ => out.push(exact_leaf(bucket, rung)),
    }
}

fn exact_leaf(range: TimeRange, rung: TimeDelta) -> PlannedBucket {
    PlannedBucket {
        range,
        rung,
        closed: false,
        nb_blocks: 0,
        nb_objects: 0,
    }
}

/// Tiles `range` with buckets that each cover at most `cap` source bytes (as measured by
/// `bytes`), except at the finest rung, where a bucket is emitted whatever its size. The
/// result is ascending and has no gap.
///
/// The range is first cut into the coarsest aligned ladder buckets that fit it, then every bucket
/// over the cap splits into its finer children, down to `LADDER[0]`. Source bytes alone decide, so
/// a dense range does not probe each intermediate rung. A part of the range narrower than
/// `LADDER[0]` at its edges (a unit grown around rows can end off the minute grid) is emitted as
/// one leaf over its exact range, at `LADDER[0]`.
///
/// `closed`, `nb_blocks` and `nb_objects` of the result are placeholders for the caller to fill.
pub fn plan_by_source_bytes(range: TimeRange, bytes: BytesFn<'_>, cap: i64) -> Vec<PlannedBucket> {
    let finest = LADDER[0];
    let mut out = vec![];
    let first_grid = if is_aligned(range.begin, finest) {
        range.begin
    } else {
        range.begin.duration_trunc(finest).unwrap_or(range.begin) + finest
    };
    let last_grid = range.end.duration_trunc(finest).unwrap_or(range.end);
    if first_grid > last_grid {
        // Not even one minute boundary inside the range.
        out.push(exact_leaf(range, finest));
        return out;
    }
    if range.begin < first_grid {
        out.push(exact_leaf(TimeRange::new(range.begin, first_grid), finest));
    }
    let mut begin = first_grid;
    while begin < last_grid {
        let rung = LADDER
            .iter()
            .rev()
            .find(|r| is_aligned(begin, **r) && begin + **r <= last_grid)
            .copied()
            .unwrap_or(finest);
        plan_bucket_by_bytes(
            TimeRange::new(begin, begin + rung),
            rung,
            bytes,
            cap,
            &mut out,
        );
        begin += rung;
    }
    if last_grid < range.end {
        out.push(exact_leaf(TimeRange::new(last_grid, range.end), finest));
    }
    out
}

/// Truncates `range` outward to whole `rung` buckets.
pub fn align_outward(range: TimeRange, rung: TimeDelta) -> anyhow::Result<TimeRange> {
    Ok(TimeRange::new(
        range.begin.duration_trunc(rung)?,
        range.end.duration_trunc(rung)? + rung,
    ))
}
