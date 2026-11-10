//! Query-driven materialization of on-demand SQL views (DDL views with no `update_group`).
//!
//! A query over `query_range` fills and refreshes the partitions covering that range, so nothing
//! is materialized for ranges nobody reads. The range is cut along the bucket ladder (see
//! `partition_ladder`), each bucket's freshness is probed with the view's own authored
//! `count_src_query`, and the stale ones are rewritten with its `extract_query`.
//!
//! A bucket grows into a [`Unit`] that contains every partition row it intersects (an earlier
//! query at a coarser rung, a promotion, or rows left by the daemon before a view was converted),
//! so a write's containment retire always replaces everything it overlaps and the verdict of a
//! unit never depends on another unit.

use super::{
    jit_partitions::{
        BucketSizing, JitPartitionConfig, PartitionFreshnessRow, RangeConvention,
        fetch_freshness_candidates,
    },
    jit_promotion::{PromotionTarget, is_promotable, promote_closed_buckets},
    lakehouse_context::LakehouseContext,
    partition_cache::PartitionCache,
    partition_ladder::{LADDER, PlannedBucket, align_outward, plan_dense_buckets, select_rung},
    partition_source_data::hash_to_object_count,
    view::{View, ViewMetadata},
    write_partition::PartitionOverlapConflict,
};
use crate::{response_writer::ResponseWriter, time::TimeRange};
use anyhow::{Context, Result};
use chrono::{DurationRound, TimeDelta, Utc};
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// A planned bucket, grown until it contains every partition row it intersects, and coalesced
/// with the units it came to overlap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub range: TimeRange,
    /// Indices into the rows the unit was planned from. All of them lie fully inside `range`.
    pub rows: Vec<usize>,
    /// Every bucket the unit absorbed was closed.
    pub closed: bool,
    /// `Some(rung)` when the unit is exactly one planned bucket, not grown.
    pub planned_rung: Option<TimeDelta>,
}

/// Whether a unit's rows already hold what its probe counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitVerdict {
    Covered,
    Stale,
}

struct GrownBucket {
    range: TimeRange,
    original: TimeRange,
    rung: TimeDelta,
    closed: bool,
    nb_buckets: usize,
}

/// Grows each planned bucket until it contains every row that intersects it, then coalesces the
/// units that came to overlap. `planned` is ascending and non-overlapping. Rows are half-open
/// against the unit (`Aligned`): SQL partitions touch at bucket edges, and a neighbor that merely
/// touches is not absorbed.
pub fn plan_units(planned: &[PlannedBucket], rows: &[PartitionFreshnessRow]) -> Vec<Unit> {
    let convention = RangeConvention::Aligned;
    let mut grown: Vec<GrownBucket> = planned
        .iter()
        .map(|bucket| {
            let mut range = bucket.range;
            loop {
                let mut next = range;
                for row in rows.iter().filter(|r| convention.row_intersects(r, range)) {
                    next.begin = next.begin.min(row.begin_insert_time);
                    next.end = next.end.max(row.end_insert_time);
                }
                if next == range {
                    break;
                }
                range = next;
            }
            GrownBucket {
                range,
                original: bucket.range,
                rung: bucket.rung,
                closed: bucket.closed,
                nb_buckets: 1,
            }
        })
        .collect();
    grown.sort_by_key(|g| g.range.begin);
    let mut merged: Vec<GrownBucket> = vec![];
    for g in grown {
        match merged.last_mut() {
            Some(last) if g.range.begin < last.range.end => {
                last.range.end = last.range.end.max(g.range.end);
                last.closed &= g.closed;
                last.nb_buckets += g.nb_buckets;
            }
            _ => merged.push(g),
        }
    }
    merged
        .into_iter()
        .map(|g| Unit {
            rows: rows
                .iter()
                .enumerate()
                .filter(|(_, r)| convention.row_intersects(r, g.range))
                .map(|(i, _)| i)
                .collect(),
            planned_rung: (g.nb_buckets == 1 && g.range == g.original).then_some(g.rung),
            closed: g.closed,
            range: g.range,
        })
        .collect()
}

/// `Covered` when the unit has rows, every row was written with the view's current schema, and
/// the rows' summed `source_data_hash` equals the count the probe returned. This is the
/// maintenance daemon's freshness rule (equality, summed), so a definition gets the same verdicts
/// under either writer.
pub fn judge_unit(
    unit: &Unit,
    rows: &[PartitionFreshnessRow],
    file_schema_hash: &[u8],
    probed_count: i64,
) -> UnitVerdict {
    if unit.rows.is_empty() {
        return UnitVerdict::Stale;
    }
    let mut sum: i64 = 0;
    for &index in &unit.rows {
        let row = &rows[index];
        if row.file_schema_hash != file_schema_hash {
            return UnitVerdict::Stale;
        }
        match hash_to_object_count(&row.source_data_hash) {
            Ok(count) => sum += count,
            Err(_) => return UnitVerdict::Stale,
        }
    }
    if sum == probed_count {
        UnitVerdict::Covered
    } else {
        UnitVerdict::Stale
    }
}

/// Picks the units to promote: covered, closed, exactly one planned bucket at the query's `rung`
/// (not grown), holding at least two rows that merge. Pure, like `promotion_candidates`.
pub fn promotion_targets(
    units: &[Unit],
    rows: &[PartitionFreshnessRow],
    verdicts: &[UnitVerdict],
    rung: TimeDelta,
) -> Vec<PromotionTarget> {
    let mut targets = vec![];
    for (unit, verdict) in units.iter().zip(verdicts) {
        if *verdict != UnitVerdict::Covered || !unit.closed || unit.planned_rung != Some(rung) {
            continue;
        }
        let inputs: Vec<&PartitionFreshnessRow> = unit.rows.iter().map(|&i| &rows[i]).collect();
        if is_promotable(&inputs, i64::MAX) {
            targets.push(PromotionTarget {
                bucket: unit.range,
                inputs: inputs.into_iter().cloned().collect(),
            });
        }
    }
    targets
}

fn hull(a: TimeRange, b: TimeRange) -> TimeRange {
    TimeRange::new(a.begin.min(b.begin), a.end.max(b.end))
}

fn contains(outer: TimeRange, inner: TimeRange) -> bool {
    outer.begin <= inner.begin && outer.end >= inner.end
}

/// Fetches the rows over `start`, plans the units and, as long as a unit grew past the fetched
/// range, fetches again over the grown range. Rows written under a previous schema can overlap
/// rows written under the current one, because the no-overlap constraint is per schema hash, so
/// growth can chain.
async fn settle_units(
    pool: &sqlx::PgPool,
    view_meta: &ViewMetadata,
    planned: &[PlannedBucket],
    start: TimeRange,
) -> Result<(Vec<Unit>, Vec<PartitionFreshnessRow>)> {
    let mut fetch_range = start;
    loop {
        let rows = fetch_freshness_candidates(pool, view_meta, fetch_range).await?;
        let units = plan_units(planned, &rows);
        let units_hull = units.iter().map(|u| u.range).reduce(hull);
        match units_hull {
            Some(h) if !contains(fetch_range, h) => fetch_range = hull(fetch_range, h),
            _ => return Ok((units, rows)),
        }
    }
}

/// Brings the on-demand sources up to date over `range`, then snapshots every view's partitions
/// over it. The extract and count sessions read the snapshot and cannot refresh upstream
/// themselves.
async fn snapshot_sources(
    lakehouse: &Arc<LakehouseContext>,
    on_demand_sources: &[Arc<dyn View>],
    range: TimeRange,
) -> Result<Arc<PartitionCache>> {
    for source in on_demand_sources {
        source
            .jit_update(lakehouse.clone(), Some(range))
            .await
            .with_context(|| {
                format!("refreshing on-demand source {}", source.get_view_set_name())
            })?;
    }
    Ok(Arc::new(
        PartitionCache::fetch_overlapping_insert_range(&lakehouse.lake().db_pool, range).await?,
    ))
}

fn is_overlap_conflict(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|e| e.downcast_ref::<PartitionOverlapConflict>().is_some())
}

fn record_outcome(outcome: &'static str) {
    imetric!("on_demand_jit", "count", outcome = outcome, 1_u64);
}

/// Probes `unit` with the view's authored count query and judges it. The returned spec can write
/// exactly `unit.range`.
async fn probe_unit(
    lakehouse: &Arc<LakehouseContext>,
    view: &Arc<dyn View>,
    view_meta: &ViewMetadata,
    cache: &Arc<PartitionCache>,
    unit: &Unit,
    rows: &[PartitionFreshnessRow],
) -> Result<(Arc<dyn super::view::PartitionSpec>, UnitVerdict)> {
    let spec = view
        .make_batch_partition_spec(lakehouse.clone(), cache.clone(), unit.range)
        .await
        .with_context(|| "make_batch_partition_spec")?;
    let probed = hash_to_object_count(&spec.get_source_data_hash())?;
    let verdict = judge_unit(unit, rows, &view_meta.file_schema_hash, probed);
    Ok((spec, verdict))
}

/// A concurrent reader wrote an overlapping unit first. Looks at the rows again, recomputes the
/// unit (it may have grown), and either drops the write because the concurrent one already
/// covers it or writes once more. A second conflict is an error.
#[allow(clippy::too_many_arguments)]
async fn resolve_conflict(
    lakehouse: &Arc<LakehouseContext>,
    view: &Arc<dyn View>,
    on_demand_sources: &[Arc<dyn View>],
    view_meta: &ViewMetadata,
    unit: &Unit,
    rung: TimeDelta,
    snapshot_range: &mut TimeRange,
    cache: &mut Arc<PartitionCache>,
) -> Result<()> {
    let pool = &lakehouse.lake().db_pool;
    let pseudo_bucket = PlannedBucket {
        range: unit.range,
        rung: unit.planned_rung.unwrap_or(rung),
        closed: unit.closed,
        nb_blocks: 0,
        nb_objects: 0,
    };
    let (units, rows) = settle_units(pool, view_meta, &[pseudo_bucket], unit.range).await?;
    let [grown] = units.as_slice() else {
        anyhow::bail!(
            "recomputing a conflicted unit yielded {} units",
            units.len()
        );
    };
    if !contains(*snapshot_range, grown.range) {
        *snapshot_range = hull(*snapshot_range, grown.range);
        *cache = snapshot_sources(lakehouse, on_demand_sources, *snapshot_range).await?;
    }
    let (spec, verdict) = probe_unit(lakehouse, view, view_meta, cache, grown, &rows).await?;
    match verdict {
        UnitVerdict::Covered => {
            info!(
                "on-demand unit [{}, {}) is now covered by a concurrent writer: dropping write",
                unit.range.begin.to_rfc3339(),
                unit.range.end.to_rfc3339()
            );
            record_outcome("conflict_dropped");
        }
        UnitVerdict::Stale => {
            spec.write(
                lakehouse.lake().clone(),
                Arc::new(ResponseWriter::new(None)),
            )
            .await
            .with_context(|| "writing the on-demand unit after a conflict")?;
            record_outcome("written");
        }
    }
    Ok(())
}

fn default_grace() -> TimeDelta {
    match JitPartitionConfig::default().bucket_sizing {
        BucketSizing::RangeDriven { grace } => grace,
        BucketSizing::Fixed(_) => TimeDelta::zero(),
    }
}

/// Brings an on-demand view up to date for `query_range`: sizes buckets from the range, probes
/// each one, rewrites the stale ones, then promotes closed history to the range's rung.
///
/// `on_demand_sources` are refreshed over the range the units cover before source partitions are
/// snapshotted. `view` is owned because promotion hands it to `create_merged_partition`.
#[span_fn]
pub async fn update_on_demand_view(
    lakehouse: Arc<LakehouseContext>,
    view: Arc<dyn View>,
    on_demand_sources: &[Arc<dyn View>],
    query_range: TimeRange,
) -> Result<()> {
    let now = Utc::now();
    let rung = select_rung(query_range);
    let aligned = align_outward(query_range, rung)?;
    // Without the clamp the open edge would materialize empty buckets up to the end of the rung.
    let end_of_minute = now.duration_trunc(LADDER[0])? + LADDER[0];
    let insert_range = TimeRange::new(aligned.begin, aligned.end.min(end_of_minute));
    let planned = plan_dense_buckets(insert_range, rung, LADDER[0], now, default_grace());
    let (Some(first), Some(last)) = (planned.first(), planned.last()) else {
        return Ok(());
    };
    let pool = &lakehouse.lake().db_pool;
    let view_meta = view.get_meta();
    let (units, rows) = settle_units(
        pool,
        &view_meta,
        &planned,
        TimeRange::new(first.range.begin, last.range.end),
    )
    .await?;

    let mut snapshot_range = units
        .iter()
        .map(|u| u.range)
        .reduce(hull)
        .with_context(|| "planned buckets without units")?;
    let mut cache = snapshot_sources(&lakehouse, on_demand_sources, snapshot_range).await?;

    let mut verdicts = Vec::with_capacity(units.len());
    for unit in &units {
        let (spec, verdict) =
            probe_unit(&lakehouse, &view, &view_meta, &cache, unit, &rows).await?;
        verdicts.push(verdict);
        if verdict == UnitVerdict::Covered {
            record_outcome("covered");
            continue;
        }
        match spec
            .write(
                lakehouse.lake().clone(),
                Arc::new(ResponseWriter::new(None)),
            )
            .await
        {
            Ok(()) => record_outcome("written"),
            Err(e) if is_overlap_conflict(&e) => {
                resolve_conflict(
                    &lakehouse,
                    &view,
                    on_demand_sources,
                    &view_meta,
                    unit,
                    rung,
                    &mut snapshot_range,
                    &mut cache,
                )
                .await?;
            }
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "writing on-demand unit [{}, {})",
                        unit.range.begin.to_rfc3339(),
                        unit.range.end.to_rfc3339()
                    )
                });
            }
        }
    }

    let targets = promotion_targets(&units, &rows, &verdicts, rung);
    promote_closed_buckets(lakehouse, view, targets, i64::MAX, RangeConvention::Aligned).await;
    Ok(())
}
