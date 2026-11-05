//! Aligned bucket ladder (minute, 5 min, hour, 6 h, day) used to size JIT partitions from the
//! query range, and the planner that turns per-minute block counts into the buckets to
//! materialize.
//!
//! Every rung divides the next one and `duration_trunc` aligns to the Unix epoch, so buckets of
//! different rungs nest and never straddle each other.

use crate::time::TimeRange;
use chrono::{DateTime, DurationRound, TimeDelta, Utc};

/// Bucket widths, finest first. Each rung divides the next one.
pub const LADDER: [TimeDelta; 5] = [
    TimeDelta::seconds(60),
    TimeDelta::seconds(5 * 60),
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

#[allow(clippy::too_many_arguments)]
fn plan_bucket(
    bucket: TimeRange,
    rung: TimeDelta,
    finest: TimeDelta,
    closed_before: DateTime<Utc>,
    counts: &[BucketCount],
    single_finest_bucket: bool,
    max_nb_objects: i64,
    out: &mut Vec<PlannedBucket>,
) {
    let (nb_blocks, nb_objects) = sum_counts(counts, bucket);
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
                    counts,
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
            counts,
            single_finest_bucket,
            max_nb_objects,
            &mut out,
        );
        begin += rung;
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
