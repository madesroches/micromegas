use super::{
    partition::Partition,
    partition_cache::PartitionCache,
    partition_source_data::hash_to_object_count,
    query::query_partitions,
    view::View,
    write_partition::{write_partition_from_rows, PartitionRowSet},
};
use crate::{dfext::min_max_time_df::min_max_time_dataframe, response_writer::Logger};
use anyhow::Result;
use chrono::{DateTime, Utc};
use datafusion::prelude::*;
use futures::stream::StreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;
use xxhash_rust::xxh32::xxh32;

fn partition_set_stats(
    view: Arc<dyn View>,
    filtered_partitions: &[Partition],
) -> Result<(i64, i64)> {
    let mut sum_size: i64 = 0;
    let mut source_hash: i64 = 0;
    let latest_file_schema_hash = view.get_file_schema_hash();
    for p in filtered_partitions {
        // for some time all the hashes will actually be the number of events in the source data
        // when views have different hash algos, we should delegate to the view the creation of the merged hash
        source_hash = if p.source_data_hash.len() == std::mem::size_of::<i64>() {
            source_hash + hash_to_object_count(&p.source_data_hash)?
        } else {
            //previous hash algo
            xxh32(&p.source_data_hash, source_hash as u32).into()
        };

        sum_size += p.file_size;

        if p.view_metadata.file_schema_hash != latest_file_schema_hash {
            anyhow::bail!(
                "incompatible file schema with [{},{}]",
                p.begin_insert_time.to_rfc3339(),
                p.end_insert_time.to_rfc3339()
            );
        }
    }
    Ok((sum_size, source_hash))
}

pub async fn create_merged_partition(
    existing_partitions: Arc<PartitionCache>,
    lake: Arc<DataLakeConnection>,
    view: Arc<dyn View>,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let view_set_name = &view.get_view_set_name();
    let view_instance_id = &view.get_view_instance_id();
    let desc = format!(
        "[{}, {}] {view_set_name} {view_instance_id}",
        begin_insert.to_rfc3339(),
        end_insert.to_rfc3339()
    );
    // we are not looking for intersecting partitions, but only those that fit completely in the range
    // otherwise we'd get duplicated records
    let mut filtered_partitions = existing_partitions
        .filter_inside_range(view_set_name, view_instance_id, begin_insert, end_insert)
        .partitions;
    if filtered_partitions.len() < 2 {
        logger
            .write_log_entry(format!("{desc}: not enough partitions to merge"))
            .await?;
        return Ok(());
    }
    let (sum_size, source_hash) = partition_set_stats(view.clone(), &filtered_partitions)?;
    logger
        .write_log_entry(format!(
            "{desc}: merging {} partitions sum_size={sum_size}",
            filtered_partitions.len()
        ))
        .await?;
    let merge_query = view
        .get_merge_partitions_query()
        .replace("{source}", "source");
    filtered_partitions.sort_by_key(|p| p.begin_insert_time);
    let merged_df = query_partitions(
        lake.clone(),
        view.get_file_schema(),
        filtered_partitions,
        &merge_query,
    )
    .await?;
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let join_handle = tokio::spawn(write_partition_from_rows(
        lake.clone(),
        view.get_meta(),
        view.get_file_schema(),
        begin_insert,
        end_insert,
        source_hash.to_le_bytes().to_vec(),
        rx,
        logger.clone(),
    ));
    let mut stream = merged_df.execute_stream().await?;
    let ctx = SessionContext::new();
    while let Some(rb_res) = stream.next().await {
        let rb = rb_res?;
        let (mintime, maxtime) = min_max_time_dataframe(
            ctx.read_batch(rb.clone())?,
            &view.get_min_event_time_column_name(),
            &view.get_max_event_time_column_name(),
        )
        .await?;
        tx.send(PartitionRowSet {
            min_time_row: mintime,
            max_time_row: maxtime,
            rows: rb,
        })
        .await?;
    }
    drop(tx);
    join_handle.await??;
    Ok(())
}
