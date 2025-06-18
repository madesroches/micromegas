use super::{
    partition::Partition,
    partition_cache::PartitionCache,
    partition_source_data::hash_to_object_count,
    query::query_partitions,
    view::View,
    write_partition::{write_partition_from_rows, PartitionRowSet},
};
use crate::{
    dfext::min_max_time_df::min_max_time_dataframe, response_writer::Logger, time::TimeRange,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use datafusion::{
    arrow::datatypes::Schema,
    execution::{runtime_env::RuntimeEnv, SendableRecordBatchStream},
    prelude::*,
};
use futures::stream::StreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::error;
use std::fmt::Debug;
use std::sync::Arc;
use xxhash_rust::xxh32::xxh32;

#[async_trait]
pub trait PartitionMerger: Send + Sync + Debug {
    async fn execute_merge_query(
        &self,
        lake: Arc<DataLakeConnection>,
        partitions: Arc<Vec<Partition>>,
    ) -> Result<SendableRecordBatchStream>;
}

#[derive(Debug)]
pub struct QueryMerger {
    runtime: Arc<RuntimeEnv>,
    file_schema: Arc<Schema>,
    query: Arc<String>,
}

impl QueryMerger {
    pub fn new(runtime: Arc<RuntimeEnv>, file_schema: Arc<Schema>, query: Arc<String>) -> Self {
        Self {
            runtime,
            file_schema,
            query,
        }
    }
}

#[async_trait]
impl PartitionMerger for QueryMerger {
    async fn execute_merge_query(
        &self,
        lake: Arc<DataLakeConnection>,
        partitions: Arc<Vec<Partition>>,
    ) -> Result<SendableRecordBatchStream> {
        let merged_df = query_partitions(
            self.runtime.clone(),
            lake.clone(),
            self.file_schema.clone(),
            partitions,
            &self.query,
        )
        .await?;
        merged_df
            .execute_stream()
            .await
            .with_context(|| "merged_df.execute_stream")
    }
}

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
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    view: Arc<dyn View>,
    insert_range: TimeRange,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let view_set_name = &view.get_view_set_name();
    let view_instance_id = &view.get_view_instance_id();
    let desc = format!(
        "[{}, {}] {view_set_name} {view_instance_id}",
        insert_range.begin.to_rfc3339(),
        insert_range.end.to_rfc3339()
    );
    // we are not looking for intersecting partitions, but only those that fit completely in the range
    // otherwise we'd get duplicated records
    let mut filtered_partitions = existing_partitions
        .filter_inside_range(view_set_name, view_instance_id, insert_range)
        .partitions;
    if filtered_partitions.len() < 2 {
        logger
            .write_log_entry(format!("{desc}: not enough partitions to merge"))
            .await
            .with_context(|| "writing log")?;
        return Ok(());
    }
    let (sum_size, source_hash) = partition_set_stats(view.clone(), &filtered_partitions)
        .with_context(|| "partition_set_stats")?;
    logger
        .write_log_entry(format!(
            "{desc}: merging {} partitions sum_size={sum_size}",
            filtered_partitions.len()
        ))
        .await
        .with_context(|| "write_log_entry")?;
    filtered_partitions.sort_by_key(|p| p.begin_insert_time);
    let mut merged_stream = view
        .merge_partitions(runtime.clone(), lake.clone(), Arc::new(filtered_partitions))
        .await
        .with_context(|| "view.merge_partitions")?;
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let view_copy = view.clone();
    let join_handle = tokio::spawn(async move {
        let res = write_partition_from_rows(
            lake.clone(),
            view_copy.get_meta(),
            view_copy.get_file_schema(),
	    insert_range,
            source_hash.to_le_bytes().to_vec(),
            rx,
            logger.clone(),
        )
        .await;
        if let Err(e) = &res {
            error!("{e:?}");
        }
        res
    });
    let ctx = SessionContext::new_with_config_rt(SessionConfig::default(), runtime);
    while let Some(rb_res) = merged_stream.next().await {
        let rb = rb_res.with_context(|| "receiving record_batch from stream")?;
        let (mintime, maxtime) = min_max_time_dataframe(
            ctx.read_batch(rb.clone()).with_context(|| "read_batch")?,
            &view.get_min_event_time_column_name(),
            &view.get_max_event_time_column_name(),
        )
        .await
        .with_context(|| "min_max_time_dataframe")?;
        tx.send(PartitionRowSet {
            min_time_row: mintime,
            max_time_row: maxtime,
            rows: rb,
        })
        .await
        .with_context(|| "sending partition row set")?;
    }
    drop(tx);
    join_handle.await??;
    Ok(())
}
