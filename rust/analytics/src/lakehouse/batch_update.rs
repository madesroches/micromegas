use super::{
    lakehouse_context::LakehouseContext, merge::create_merged_partition,
    partition_cache::PartitionCache, partition_source_data::hash_to_object_count, view::View,
};
use crate::{response_writer::Logger, time::TimeRange};
use anyhow::{Context, Result};
use chrono::TimeDelta;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// Defines the strategy for creating a new partition.
pub enum PartitionCreationStrategy {
    /// Create the partition from the source data.
    CreateFromSource,
    /// Merge existing partitions.
    MergeExisting(Arc<PartitionCache>),
    /// Abort the partition creation.
    Abort,
}

// verify_overlapping_partitions returns true to continue and make a new partition,
// returns false to abort (existing partition is up to date or there is a problem)
async fn verify_overlapping_partitions(
    existing_partitions_all_views: &PartitionCache,
    insert_range: TimeRange,
    view_set_name: &str,
    view_instance_id: &str,
    file_schema_hash: &[u8],
    source_data_hash: &[u8],
    logger: Arc<dyn Logger>,
) -> Result<PartitionCreationStrategy> {
    let desc = format!(
        "[{}, {}] {view_set_name} {view_instance_id}",
        insert_range.begin.to_rfc3339(),
        insert_range.end.to_rfc3339()
    );
    if source_data_hash.len() != std::mem::size_of::<i64>() {
        anyhow::bail!("Source data hash should be a i64");
    }
    let nb_source_events = hash_to_object_count(source_data_hash)?;
    let filtered = existing_partitions_all_views.filter(
        view_set_name,
        view_instance_id,
        file_schema_hash,
        insert_range,
    );
    if filtered.partitions.is_empty() {
        logger
            .write_log_entry(format!("{desc}: matching partitions not found"))
            .await?;
        return Ok(PartitionCreationStrategy::CreateFromSource);
    }
    let mut existing_source_hash: i64 = 0;
    let nb_existing_partitions = filtered.partitions.len();
    for part in &filtered.partitions {
        let begin = part.begin_insert_time();
        let end = part.end_insert_time();
        if begin < insert_range.begin || end > insert_range.end {
            logger
                .write_log_entry(format!(
                    "{desc}: found overlapping partition [{}, {}], aborting the update",
                    begin.to_rfc3339(),
                    end.to_rfc3339()
                ))
                .await?;
            return Ok(PartitionCreationStrategy::Abort);
        }
        if part.source_data_hash.len() == std::mem::size_of::<i64>() {
            existing_source_hash += hash_to_object_count(&part.source_data_hash)?
        } else {
            // old hash that does not represent the number of events
            logger
                .write_log_entry(format!(
                    "{desc}: found partition with incompatible source hash: recreate"
                ))
                .await?;
            return Ok(PartitionCreationStrategy::CreateFromSource);
        }
    }

    if nb_source_events != existing_source_hash {
        logger
            .write_log_entry(format!(
                "{desc}: existing partitions do not match source data ({nb_source_events} vs {existing_source_hash}) : creating a new partition"
            ))
            .await?;
        return Ok(PartitionCreationStrategy::CreateFromSource);
    }

    if nb_existing_partitions > 1 {
        return Ok(PartitionCreationStrategy::MergeExisting(Arc::new(filtered)));
    }

    logger
        .write_log_entry(format!(
            "{desc}: already up to date, nb_source_events={nb_source_events}"
        ))
        .await?;
    Ok(PartitionCreationStrategy::Abort)
}

/// Re-checks the same partial-overlap condition `verify_overlapping_partitions` guards, without
/// its source-hash freshness comparison (the "already up to date" `Abort`, which regeneration
/// exists to bypass). Used only by `regenerate_partition`.
///
/// Filters the existing-partitions snapshot hash-agnostically (`filter_insert_range` +
/// view/instance match) rather than via `PartitionCache::filter`'s hash-exact match: a
/// partially-overlapping partition written under an older schema hash must still be caught here,
/// since `retire_partitions`'s range-containment delete is equally hash-agnostic and would not
/// retire it either.
fn verify_force_regeneration_alignment(
    existing_partitions_all_views: &PartitionCache,
    insert_range: TimeRange,
    view_set_name: &str,
    view_instance_id: &str,
) -> Result<()> {
    let filtered = existing_partitions_all_views.filter_insert_range(insert_range);
    for part in &filtered.partitions {
        if *part.view_metadata.view_set_name != view_set_name
            || *part.view_metadata.view_instance_id != view_instance_id
        {
            continue;
        }
        let begin = part.begin_insert_time();
        let end = part.end_insert_time();
        if begin < insert_range.begin || end > insert_range.end {
            anyhow::bail!(
                "regenerate_partitions: requested range [{}, {}] does not fully contain \
                 existing partition [{}, {}] for {view_set_name}/{view_instance_id} -- \
                 range/delta must exactly cover the partition(s) being regenerated",
                insert_range.begin.to_rfc3339(),
                insert_range.end.to_rfc3339(),
                begin.to_rfc3339(),
                end.to_rfc3339(),
            );
        }
    }
    Ok(())
}

#[span_fn]
async fn materialize_partition(
    existing_partitions_all_views: Arc<PartitionCache>,
    lakehouse: Arc<LakehouseContext>,
    insert_range: TimeRange,
    view: Arc<dyn View>,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let view_set_name = view.get_view_set_name();
    let partition_spec = view
        .make_batch_partition_spec(
            lakehouse.clone(),
            existing_partitions_all_views.clone(),
            insert_range,
        )
        .await
        .with_context(|| "make_batch_partition_spec")?;
    // Allow empty partition specs to be written - write_partition_from_rows
    // will create an empty partition record
    let view_instance_id = view.get_view_instance_id();
    let strategy = verify_overlapping_partitions(
        &existing_partitions_all_views,
        insert_range,
        &view_set_name,
        &view_instance_id,
        &view.get_file_schema_hash(),
        &partition_spec.get_source_data_hash(),
        logger.clone(),
    )
    .await
    .with_context(|| "verify_overlapping_partitions")?;
    if let PartitionCreationStrategy::Abort = &strategy {
        return Ok(());
    }

    let new_delta = view.get_max_partition_time_delta(&strategy);
    if new_delta < (insert_range.end - insert_range.begin) {
        if let PartitionCreationStrategy::MergeExisting(partition_cache) = &strategy
            && partition_cache
                .partitions
                .iter()
                .all(|p| (p.end_insert_time() - p.begin_insert_time()) == new_delta)
        {
            let desc = format!(
                "[{}, {}] {view_set_name} {view_instance_id}",
                insert_range.begin.to_rfc3339(),
                insert_range.end.to_rfc3339()
            );
            logger
                .write_log_entry(format!("{desc}: subpartitions already present",))
                .await?;
            return Ok(());
        }

        return Box::pin(materialize_partition_range(
            existing_partitions_all_views,
            lakehouse.clone(),
            view,
            insert_range,
            new_delta,
            logger,
        ))
        .await
        .with_context(|| "materialize_partition_range");
    }

    match strategy {
        PartitionCreationStrategy::CreateFromSource => {
            partition_spec
                .write(lakehouse.lake().clone(), logger)
                .await
                .with_context(|| "writing partition")?;
        }
        PartitionCreationStrategy::MergeExisting(partitions_to_merge) => {
            create_merged_partition(
                partitions_to_merge,
                existing_partitions_all_views,
                lakehouse,
                view,
                insert_range,
                logger,
            )
            .await
            .with_context(|| "create_merged_partition")?;
        }
        PartitionCreationStrategy::Abort => {}
    }

    Ok(())
}

/// Materializes partitions within a given time range.
#[span_fn]
pub async fn materialize_partition_range(
    existing_partitions_all_views: Arc<PartitionCache>,
    lakehouse: Arc<LakehouseContext>,
    view: Arc<dyn View>,
    insert_range: TimeRange,
    partition_time_delta: TimeDelta,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let mut begin_part = insert_range.begin;
    let mut end_part = begin_part + partition_time_delta;
    while end_part <= insert_range.end {
        let partition_insert_range = TimeRange::new(begin_part, end_part);
        let insert_time_filtered =
            Arc::new(existing_partitions_all_views.filter_insert_range(partition_insert_range));
        materialize_partition(
            insert_time_filtered,
            lakehouse.clone(),
            partition_insert_range,
            view.clone(),
            logger.clone(),
        )
        .await
        .with_context(|| "materialize_partition")?;
        begin_part = end_part;
        end_part = begin_part + partition_time_delta;
    }
    Ok(())
}

/// Regenerates the partition(s) covering `insert_range` directly from source data, bypassing the
/// "already up to date" freshness check `materialize_partition_range` would otherwise stop at. See
/// `tasks/blocks_view_ordered_merges_plan.md`'s Design §3.
///
/// **Invariant callers must uphold**: `(begin, end, delta)` must exactly cover the boundaries of
/// the partition(s) being regenerated -- a misaligned range/delta means the new partition's range
/// does not fully contain the old one, so `retire_partitions` never retires it, leaving silent
/// duplicate rows. This is enforced by validating that `delta` exactly tiles `(begin, end)` before
/// any partition is written, and (per bucket) by `verify_force_regeneration_alignment`.
///
/// Both checks read a snapshot and are advisory UX: the authoritative guard is the
/// `lakehouse_partitions_no_overlap` exclusion constraint, which makes the insert fail loudly if
/// a conflicting partition was committed concurrently (e.g. by the maintenance daemon merging
/// buckets after the snapshot was taken).
#[span_fn]
pub async fn regenerate_partition_range(
    existing_partitions_all_views: Arc<PartitionCache>,
    lakehouse: Arc<LakehouseContext>,
    view: Arc<dyn View>,
    insert_range: TimeRange,
    partition_time_delta: TimeDelta,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    // chrono::TimeDelta implements no Rem/% operator, so tile-checking is done on nanoseconds.
    let span = (insert_range.end - insert_range.begin)
        .num_nanoseconds()
        .expect("time range span should fit in an i64 number of nanoseconds");
    let step = partition_time_delta
        .num_nanoseconds()
        .expect("partition_time_delta should fit in an i64 number of nanoseconds");
    if !(step > 0 && span >= step && span % step == 0) {
        anyhow::bail!(
            "regenerate_partitions: delta ({partition_time_delta}) does not exactly tile the \
             requested range [{}, {}] -- range/delta must exactly cover the partition(s) being \
             regenerated",
            insert_range.begin.to_rfc3339(),
            insert_range.end.to_rfc3339(),
        );
    }
    let mut begin_part = insert_range.begin;
    let mut end_part = begin_part + partition_time_delta;
    while end_part <= insert_range.end {
        let bucket = TimeRange::new(begin_part, end_part);
        let filtered = Arc::new(existing_partitions_all_views.filter_insert_range(bucket));
        regenerate_partition(
            filtered,
            lakehouse.clone(),
            bucket,
            view.clone(),
            logger.clone(),
        )
        .await
        .with_context(|| "regenerate_partition")?;
        begin_part = end_part;
        end_part = begin_part + partition_time_delta;
    }
    Ok(())
}

/// Regenerates a single bucket from source, replacing whatever aligned partition(s) currently
/// cover it. Unlike `materialize_partition` it always writes from source -- never merges, never
/// aborts on freshness -- and never subdivides: the bucket is exactly one partition. The
/// transactional retire+insert in the write path replaces the old partition(s) atomically, so a
/// failure rolls back and leaves the existing partition untouched.
#[span_fn]
async fn regenerate_partition(
    existing_partitions_all_views: Arc<PartitionCache>,
    lakehouse: Arc<LakehouseContext>,
    insert_range: TimeRange,
    view: Arc<dyn View>,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let view_set_name = view.get_view_set_name();
    let view_instance_id = view.get_view_instance_id();
    verify_force_regeneration_alignment(
        &existing_partitions_all_views,
        insert_range,
        &view_set_name,
        &view_instance_id,
    )
    .with_context(|| "verify_force_regeneration_alignment")?;
    let partition_spec = view
        .make_batch_partition_spec(
            lakehouse.clone(),
            existing_partitions_all_views,
            insert_range,
        )
        .await
        .with_context(|| "make_batch_partition_spec")?;
    partition_spec
        .write(lakehouse.lake().clone(), logger)
        .await
        .with_context(|| "writing partition")
}
