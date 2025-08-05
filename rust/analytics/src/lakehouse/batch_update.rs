use super::{
    merge::create_merged_partition, partition_cache::PartitionCache,
    partition_source_data::hash_to_object_count, view::View,
};
use crate::{response_writer::Logger, time::TimeRange};
use anyhow::{Context, Result};
use chrono::TimeDelta;
use datafusion::execution::runtime_env::RuntimeEnv;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
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
#[expect(clippy::too_many_arguments)]
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
        let begin = part.begin_insert_time;
        let end = part.end_insert_time;
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

async fn materialize_partition(
    existing_partitions_all_views: Arc<PartitionCache>,
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    insert_range: TimeRange,
    view: Arc<dyn View>,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let view_set_name = view.get_view_set_name();
    let partition_spec = view
        .make_batch_partition_spec(
            runtime.clone(),
            lake.clone(),
            existing_partitions_all_views.clone(),
            insert_range,
        )
        .await
        .with_context(|| "make_batch_partition_spec")?;
    if partition_spec.is_empty() {
        return Ok(());
    }
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
        if let PartitionCreationStrategy::MergeExisting(partition_cache) = &strategy {
            if partition_cache
                .partitions
                .iter()
                .all(|p| (p.end_insert_time - p.begin_insert_time) == new_delta)
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
        }

        return Box::pin(materialize_partition_range(
            existing_partitions_all_views,
            runtime,
            lake,
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
                .write(lake, logger)
                .await
                .with_context(|| "writing partition")?;
        }
        PartitionCreationStrategy::MergeExisting(partitions_to_merge) => {
            create_merged_partition(
                partitions_to_merge,
                existing_partitions_all_views,
                runtime,
                lake,
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
#[expect(clippy::too_many_arguments)]
pub async fn materialize_partition_range(
    existing_partitions_all_views: Arc<PartitionCache>,
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
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
            runtime.clone(),
            lake.clone(),
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
