//! Query-driven materialization of on-demand SQL views (DDL views with no `update_group`).
//!
//! A query over `query_range` fills and refreshes the partitions covering that range, so nothing
//! is materialized for ranges nobody reads. The range is cut along the bucket ladder (see
//! `partition_ladder`), each bucket's freshness is probed with the view's own authored
//! `count_src_query`, and the stale ones are rewritten with its `extract_query`.
//!
//! No partition is built from more source data than the view's byte cap (`max_jit_src_data`),
//! except at the finest rung: a stale unit whose source bytes exceed the cap is rewritten as finer
//! ladder buckets, committed together with the retire of the unit's rows, and promotion never
//! merges rows into a partition that would exceed the cap in bytes or in source bytes.
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
    partition::Partition,
    partition_cache::PartitionCache,
    partition_ladder::{
        BytesFn, LADDER, PlannedBucket, align_outward, finer_rung, plan_by_source_bytes,
        plan_dense_buckets, select_rung,
    },
    partition_source_data::hash_to_object_count,
    view::{PartitionSpec, View, ViewMetadata},
    write_partition::{PartitionOverlapConflict, delete_if_orphan, insert_partitions_replacing},
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

/// Whether `partition` belongs to one of `sources`: same view set, instance and file schema.
fn is_source_partition(partition: &Partition, sources: &[ViewMetadata]) -> bool {
    sources.iter().any(|s| {
        partition.view_metadata.view_set_name == s.view_set_name
            && partition.view_metadata.view_instance_id == s.view_instance_id
            && partition.view_metadata.file_schema_hash == s.file_schema_hash
    })
}

/// Bytes of source data an extract over `range` reads: the `file_size` of the `sources`'
/// partitions, each prorated by the share of its insert range that lies in `range`. A zero-width
/// partition counts whole when its insert time is in `[begin, end)`. Prorating assumes data is
/// spread evenly in insert time, which is good enough for a cap: extracts filter their sources by
/// insert time. Partitions of other views, or of a source under another file schema, count for
/// nothing.
pub fn source_bytes(partitions: &[Partition], sources: &[ViewMetadata], range: TimeRange) -> i64 {
    let mut total: i64 = 0;
    for partition in partitions {
        if !is_source_partition(partition, sources) {
            continue;
        }
        let (begin, end) = (partition.begin_insert_time(), partition.end_insert_time());
        if begin == end {
            if range.begin <= begin && begin < range.end {
                total = total.saturating_add(partition.file_size);
            }
            continue;
        }
        let overlap = end.min(range.end) - begin.max(range.begin);
        if overlap <= TimeDelta::zero() {
            continue;
        }
        let width = (end - begin).num_microseconds().unwrap_or(i64::MAX).max(1);
        let shared = overlap.num_microseconds().unwrap_or(i64::MAX);
        let prorated = i128::from(partition.file_size) * i128::from(shared) / i128::from(width);
        total = total.saturating_add(i64::try_from(prorated).unwrap_or(i64::MAX));
    }
    total
}

/// Restricts a source snapshot to the partitions of `sources`, so repeated `source_bytes` calls
/// don't scan every view's partitions.
fn source_partitions(cache: &PartitionCache, sources: &[ViewMetadata]) -> Vec<Partition> {
    cache
        .partitions
        .iter()
        .filter(|p| is_source_partition(p, sources))
        .cloned()
        .collect()
}

/// What to do with a unit judged stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleUnitPlan {
    /// Write the unit as one partition.
    WriteWhole,
    /// Replace every row of `range` (the unit's) with partitions over `leaves`.
    Replace {
        range: TimeRange,
        leaves: Vec<PlannedBucket>,
    },
}

/// A unit within the byte cap is written whole. Otherwise it is rebuilt as the leaves
/// `plan_by_source_bytes` picks, which replace the unit's rows in one commit, however coarse those
/// rows were. A unit that is itself one finest-rung bucket has nothing to split, so it is written
/// whole whatever its size.
pub fn plan_stale_unit(unit: &Unit, bytes: BytesFn<'_>, cap: i64) -> StaleUnitPlan {
    if bytes(unit.range) <= cap {
        return StaleUnitPlan::WriteWhole;
    }
    let leaves: Vec<PlannedBucket> = plan_by_source_bytes(unit.range, bytes, cap)
        .into_iter()
        .map(|leaf| PlannedBucket {
            closed: unit.closed,
            ..leaf
        })
        .collect();
    match leaves.as_slice() {
        [only] if only.range == unit.range => StaleUnitPlan::WriteWhole,
        _ => StaleUnitPlan::Replace {
            range: unit.range,
            leaves,
        },
    }
}

/// Picks what to merge within one covered, closed bucket: the whole of `range` when `rows` merge
/// within the byte cap and a rebuild of `range` would read at most `cap` source bytes. Otherwise,
/// when the rows would merge but a cap is exceeded, each child bucket of the next finer rung gets
/// the same treatment over the rows lying inside it. A child that a row straddles is skipped,
/// since the row cannot be split. With fewer than two rows there is nothing to merge.
pub fn split_promotion(
    range: TimeRange,
    rung: TimeDelta,
    rows: &[&PartitionFreshnessRow],
    bytes: BytesFn<'_>,
    cap: i64,
) -> Vec<PromotionTarget> {
    if rows.len() < 2 || !is_promotable(rows, i64::MAX, i64::MAX) {
        return vec![];
    }
    if is_promotable(rows, i64::MAX, cap) && bytes(range) <= cap {
        return vec![PromotionTarget {
            bucket: range,
            inputs: rows.iter().map(|r| (*r).clone()).collect(),
        }];
    }
    let Some(finer) = finer_rung(rung) else {
        return vec![];
    };
    let convention = RangeConvention::Aligned;
    let mut targets = vec![];
    let mut begin = range.begin;
    while begin < range.end {
        let child = TimeRange::new(begin, begin + finer);
        begin += finer;
        if rows
            .iter()
            .any(|r| convention.row_intersects(r, child) && !convention.row_inside(r, child))
        {
            continue;
        }
        let inside: Vec<&PartitionFreshnessRow> = rows
            .iter()
            .filter(|r| convention.row_inside(r, child))
            .copied()
            .collect();
        targets.extend(split_promotion(child, finer, &inside, bytes, cap));
    }
    targets
}

/// Picks the units to promote: covered, closed, exactly one planned bucket at the query's `rung`
/// (not grown). `split_promotion` then decides what to merge within each, under the byte cap.
/// Pure, like `promotion_candidates`.
pub fn promotion_targets(
    units: &[Unit],
    rows: &[PartitionFreshnessRow],
    verdicts: &[UnitVerdict],
    rung: TimeDelta,
    bytes: BytesFn<'_>,
    cap: i64,
) -> Vec<PromotionTarget> {
    let mut targets = vec![];
    for (unit, verdict) in units.iter().zip(verdicts) {
        if *verdict != UnitVerdict::Covered || !unit.closed || unit.planned_rung != Some(rung) {
            continue;
        }
        let inputs: Vec<&PartitionFreshnessRow> = unit.rows.iter().map(|&i| &rows[i]).collect();
        targets.extend(split_promotion(unit.range, rung, &inputs, bytes, cap));
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

/// Writes a stale unit as `plan` says. A whole write retires and inserts in one transaction. A
/// replacement writes every leaf's file unregistered, one at a time, then commits the retire of
/// the unit's rows and all the leaves together, so readers see either the old rows or the whole
/// replacement. A leaf over the cap (only possible at the finest rung) is logged and counted as
/// `over_cap`, in addition to the `written` outcome.
#[allow(clippy::too_many_arguments)]
async fn write_stale_unit(
    lakehouse: &Arc<LakehouseContext>,
    view: &Arc<dyn View>,
    view_meta: &ViewMetadata,
    cache: &Arc<PartitionCache>,
    unit: &Unit,
    whole: &Arc<dyn PartitionSpec>,
    plan: &StaleUnitPlan,
    bytes: BytesFn<'_>,
    cap: i64,
) -> Result<()> {
    let lake = lakehouse.lake();
    let logger: Arc<dyn crate::response_writer::Logger> = Arc::new(ResponseWriter::new(None));
    let over_cap = |range: TimeRange| {
        let nb_bytes = bytes(range);
        if nb_bytes > cap {
            warn!(
                "on-demand partition [{}, {}) reads {nb_bytes} source bytes, over the cap of {cap}, \
                 and cannot be split further",
                range.begin.to_rfc3339(),
                range.end.to_rfc3339()
            );
            record_outcome("over_cap");
        }
    };
    match plan {
        StaleUnitPlan::WriteWhole => {
            whole.write(lake.clone(), logger).await?;
            over_cap(unit.range);
        }
        StaleUnitPlan::Replace { range, leaves } => {
            let mut written: Vec<Partition> = Vec::with_capacity(leaves.len());
            for leaf in leaves {
                let result = async {
                    let spec = view
                        .make_batch_partition_spec(lakehouse.clone(), cache.clone(), leaf.range)
                        .await
                        .with_context(|| "make_batch_partition_spec")?;
                    spec.write_file(lake.clone(), logger.clone()).await
                }
                .await;
                match result {
                    Ok(partition) => written.push(partition),
                    Err(e) => {
                        // Unregistered files are in no `temporary_files` row, nothing else will
                        // reclaim them.
                        for file_path in written.iter().filter_map(|p| p.file_path.as_ref()) {
                            if let Err(cleanup_err) = delete_if_orphan(lake, file_path).await {
                                warn!("delete_if_orphan failed for {file_path}: {cleanup_err}");
                            }
                        }
                        return Err(e).with_context(|| {
                            format!(
                                "writing on-demand leaf [{}, {})",
                                leaf.range.begin.to_rfc3339(),
                                leaf.range.end.to_rfc3339()
                            )
                        });
                    }
                }
            }
            insert_partitions_replacing(lake, view_meta, *range, &written, logger).await?;
            for partition in &written {
                if let Some(file_path) = &partition.file_path {
                    lake.warm_object(file_path, partition.file_size);
                }
            }
            for leaf in leaves {
                over_cap(leaf.range);
            }
        }
    }
    record_outcome("written");
    Ok(())
}

/// A concurrent reader wrote an overlapping unit first. Looks at the rows again, recomputes the
/// unit (it may have grown), and either drops the write because the concurrent one already
/// covers it or writes once more, through the same byte-cap rule as the first attempt. A second
/// conflict is an error.
#[allow(clippy::too_many_arguments)]
async fn resolve_conflict(
    lakehouse: &Arc<LakehouseContext>,
    view: &Arc<dyn View>,
    on_demand_sources: &[Arc<dyn View>],
    extract_sources: &[ViewMetadata],
    max_jit_src_bytes: i64,
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
            let partitions = source_partitions(cache, extract_sources);
            let bytes = |range: TimeRange| source_bytes(&partitions, extract_sources, range);
            let plan = plan_stale_unit(grown, &bytes, max_jit_src_bytes);
            write_stale_unit(
                lakehouse,
                view,
                view_meta,
                cache,
                grown,
                &spec,
                &plan,
                &bytes,
                max_jit_src_bytes,
            )
            .await
            .with_context(|| "writing the on-demand unit after a conflict")?;
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
/// snapshotted. `extract_sources` are the views the extract query scans, whose snapshotted
/// partitions measure the source bytes of a range against `max_jit_src_bytes`. `view` is owned
/// because promotion hands it to `create_merged_partition`.
#[span_fn]
pub async fn update_on_demand_view(
    lakehouse: Arc<LakehouseContext>,
    view: Arc<dyn View>,
    on_demand_sources: &[Arc<dyn View>],
    extract_sources: &[ViewMetadata],
    max_jit_src_bytes: i64,
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
    let mut src_partitions = source_partitions(&cache, extract_sources);

    let mut verdicts = Vec::with_capacity(units.len());
    for unit in &units {
        let (spec, verdict) =
            probe_unit(&lakehouse, &view, &view_meta, &cache, unit, &rows).await?;
        verdicts.push(verdict);
        if verdict == UnitVerdict::Covered {
            record_outcome("covered");
            continue;
        }
        let bytes = |range: TimeRange| source_bytes(&src_partitions, extract_sources, range);
        let plan = plan_stale_unit(unit, &bytes, max_jit_src_bytes);
        match write_stale_unit(
            &lakehouse,
            &view,
            &view_meta,
            &cache,
            unit,
            &spec,
            &plan,
            &bytes,
            max_jit_src_bytes,
        )
        .await
        {
            Ok(()) => {}
            Err(e) if is_overlap_conflict(&e) => {
                resolve_conflict(
                    &lakehouse,
                    &view,
                    on_demand_sources,
                    extract_sources,
                    max_jit_src_bytes,
                    &view_meta,
                    unit,
                    rung,
                    &mut snapshot_range,
                    &mut cache,
                )
                .await?;
                src_partitions = source_partitions(&cache, extract_sources);
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

    let bytes = |range: TimeRange| source_bytes(&src_partitions, extract_sources, range);
    let targets = promotion_targets(&units, &rows, &verdicts, rung, &bytes, max_jit_src_bytes);
    promote_closed_buckets(
        lakehouse,
        view,
        targets,
        i64::MAX,
        max_jit_src_bytes,
        RangeConvention::Aligned,
    )
    .await;
    Ok(())
}
