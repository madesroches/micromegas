//! On-demand promotion of fine JIT partitions into coarser ones.
//!
//! A query over a long range plans coarse buckets (see `partition_ladder`). When a closed bucket
//! is already covered by several finer partitions, they are merged into one, so that this query and
//! the following ones read a few large files instead of many small ones. The work is optional: it
//! never fails the query, and it skips instead of waiting when another query is already on it.

use super::{
    jit_partitions::{Coverage, JitBucket, PartitionFreshnessRow},
    lakehouse_context::LakehouseContext,
    merge::create_merged_partition,
    partition::Partition,
    partition_cache::PartitionCache,
    partition_source_data::hash_to_object_count,
    view::View,
};
use crate::{response_writer::ResponseWriter, time::TimeRange};
use anyhow::{Context, Result};
use chrono::TimeDelta;
use micromegas_ingestion::data_lake_connection::{is_read_only, is_read_only_violation};
use micromegas_tracing::prelude::*;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// A closed bucket whose covering partitions should be merged into one.
#[derive(Debug, Clone)]
pub struct PromotionTarget {
    pub bucket: TimeRange,
    pub inputs: Vec<PartitionFreshnessRow>,
}

fn row_inside(row: &PartitionFreshnessRow, bucket: TimeRange) -> bool {
    row.begin_insert_time >= bucket.begin && row.end_insert_time < bucket.end
}

/// Whether `inputs` are worth merging: at least two of them, within the object cap, all written
/// with the same file schema.
fn is_promotable(inputs: &[&PartitionFreshnessRow], max_nb_objects: i64) -> bool {
    if inputs.len() < 2 {
        return false;
    }
    if inputs
        .iter()
        .any(|r| r.file_schema_hash != inputs[0].file_schema_hash)
    {
        return false;
    }
    let mut sum: i64 = 0;
    for r in inputs {
        match hash_to_object_count(&r.source_data_hash) {
            Ok(count) => sum += count,
            Err(_) => return false,
        }
    }
    sum <= max_nb_objects
}

/// Picks the buckets to promote. A pure function of the planner output, the verdicts and the
/// candidate rows: no object storage is read. A bucket qualifies when the planner emitted it
/// closed at the query's rung (not descended), it was judged covered by at least two of its own
/// rows, and those rows fit the object cap with a single schema hash.
pub fn promotion_candidates(
    buckets: &[JitBucket],
    verdicts: &[Coverage],
    candidates: &[PartitionFreshnessRow],
    query_rung: TimeDelta,
    max_nb_objects: i64,
) -> Vec<PromotionTarget> {
    let mut targets = vec![];
    for (bucket, verdict) in buckets.iter().zip(verdicts) {
        let planned = &bucket.planned;
        if !planned.closed || planned.rung != query_rung || *verdict != Coverage::Covered {
            continue;
        }
        let inputs: Vec<&PartitionFreshnessRow> = candidates
            .iter()
            .filter(|r| row_inside(r, planned.range))
            .collect();
        if is_promotable(&inputs, max_nb_objects) {
            targets.push(PromotionTarget {
                bucket: planned.range,
                inputs: inputs.into_iter().cloned().collect(),
            });
        }
    }
    targets
}

fn promotion_lock_key(view: &dyn View, bucket: TimeRange) -> i64 {
    let mut hasher = DefaultHasher::new();
    "promote".hash(&mut hasher);
    view.get_view_set_name().hash(&mut hasher);
    view.get_view_instance_id().hash(&mut hasher);
    bucket.begin.hash(&mut hasher);
    bucket.end.hash(&mut hasher);
    hasher.finish() as i64
}

fn hull_of<'a>(rows: impl Iterator<Item = &'a PartitionFreshnessRow>) -> Option<TimeRange> {
    let mut hull: Option<TimeRange> = None;
    for r in rows {
        hull = Some(match hull {
            None => TimeRange::new(r.begin_insert_time, r.end_insert_time),
            Some(h) => TimeRange::new(
                h.begin.min(r.begin_insert_time),
                h.end.max(r.end_insert_time),
            ),
        });
    }
    hull
}

fn freshness_row(p: &Partition) -> PartitionFreshnessRow {
    PartitionFreshnessRow {
        begin_insert_time: p.begin_insert_time(),
        end_insert_time: p.end_insert_time(),
        file_schema_hash: p.view_metadata.file_schema_hash.clone(),
        source_data_hash: p.source_data_hash.clone(),
    }
}

fn record_skip(bucket: TimeRange, reason: &'static str) {
    info!(
        "jit promotion skipped_{reason} [{}, {})",
        bucket.begin.to_rfc3339(),
        bucket.end.to_rfc3339()
    );
    imetric!("jit_promotion", "count", reason = reason, 1_u64);
}

enum Promotion {
    Promoted,
    Skipped(&'static str),
}

async fn promote_one(
    lakehouse: &Arc<LakehouseContext>,
    view: &Arc<dyn View>,
    target: &PromotionTarget,
    max_nb_objects: i64,
) -> Result<Promotion> {
    let pool = &lakehouse.lake().db_pool;
    let mut lock_transaction = pool.begin().await.with_context(|| "begin")?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1);")
        .bind(promotion_lock_key(view.as_ref(), target.bucket))
        .fetch_one(&mut *lock_transaction)
        .await
        .with_context(|| "pg_try_advisory_xact_lock")?;
    if !locked {
        return Ok(Promotion::Skipped("locked"));
    }
    let setting: String = sqlx::query_scalar("SHOW transaction_read_only")
        .fetch_one(&mut *lock_transaction)
        .await
        .with_context(|| "SHOW transaction_read_only")?;
    if is_read_only(&setting) {
        return Ok(Promotion::Skipped("read_only"));
    }

    // Another query may have promoted the bucket between the snapshot and the lock.
    let fetched = PartitionCache::fetch_inclusive_insert_range_for_view(
        pool,
        view.get_view_set_name(),
        view.get_view_instance_id(),
        target.bucket,
    )
    .await?;
    let (inside, others): (Vec<Partition>, Vec<Partition>) = fetched
        .partitions
        .into_iter()
        .partition(|p| row_inside(&freshness_row(p), target.bucket));
    let rows: Vec<PartitionFreshnessRow> = inside.iter().map(freshness_row).collect();
    let row_refs: Vec<&PartitionFreshnessRow> = rows.iter().collect();
    let straddled = others.iter().any(|p| {
        p.begin_insert_time() < target.bucket.end && p.end_insert_time() >= target.bucket.begin
    });
    let hull = hull_of(rows.iter());
    if straddled
        || !is_promotable(&row_refs, max_nb_objects)
        || hull != hull_of(target.inputs.iter())
    {
        return Ok(Promotion::Skipped("changed"));
    }
    let hull = hull.with_context(|| "promotable bucket without rows")?;

    let mut cache = PartitionCache::empty(hull);
    cache.partitions = inside;
    let cache = Arc::new(cache);
    create_merged_partition(
        cache.clone(),
        cache,
        lakehouse.clone(),
        view.clone(),
        hull,
        Arc::new(ResponseWriter::new(None)),
    )
    .await
    .with_context(|| "create_merged_partition")?;
    lock_transaction.commit().await.with_context(|| "commit")?;
    Ok(Promotion::Promoted)
}

/// Merges each target's partitions into one, in turn. Never fails: a target that errors is logged
/// and counted, and the query still reads a complete set of partitions because a merge retires its
/// inputs in the same transaction that inserts the result.
///
/// The lock transaction stays open across the merge, which takes a second connection from the same
/// pool, so a promoting query holds two connections.
pub async fn promote_closed_buckets(
    lakehouse: Arc<LakehouseContext>,
    view: Arc<dyn View>,
    targets: Vec<PromotionTarget>,
    max_nb_objects: i64,
) {
    for target in targets {
        match promote_one(&lakehouse, &view, &target, max_nb_objects).await {
            Ok(Promotion::Promoted) => {
                info!(
                    "jit promotion promoted [{}, {})",
                    target.bucket.begin.to_rfc3339(),
                    target.bucket.end.to_rfc3339()
                );
                imetric!("jit_promotion", "count", reason = "promoted", 1_u64);
            }
            Ok(Promotion::Skipped(reason)) => record_skip(target.bucket, reason),
            Err(e) if is_read_only_violation(&e) => {
                debug!("jit promotion on a read-only connection: {e:#}");
                imetric!("jit_promotion", "count", reason = "read_only", 1_u64);
            }
            Err(e) => {
                warn!(
                    "jit promotion failed [{}, {}): {e:#}",
                    target.bucket.begin.to_rfc3339(),
                    target.bucket.end.to_rfc3339()
                );
                imetric!("jit_promotion_failed", "count", 1_u64);
            }
        }
    }
}
