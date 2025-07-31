use super::{
    merge::PartitionMerger, partition::Partition, partition_cache::PartitionCache,
    view_factory::ViewFactory,
};
use crate::{
    lakehouse::{
        partitioned_table_provider::PartitionedTableProvider, query::make_session_context,
    },
    time::datetime_to_scalar,
};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::datatypes::Schema,
    error::DataFusionError,
    execution::{SendableRecordBatchStream, runtime_env::RuntimeEnv},
    physical_plan::stream::RecordBatchReceiverStreamBuilder,
    sql::TableReference,
};
use futures::TryStreamExt;
use futures::{StreamExt, stream};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::{debug, error, info};
use std::sync::Arc;

/// Statistics about a set of partitions.
struct PartitionStats {
    pub num_rows: i64,
    pub min_event_time: DateTime<Utc>,
    pub max_event_time: DateTime<Utc>,
}

fn compute_partition_stats(partitions: &[Partition]) -> Result<PartitionStats> {
    if partitions.is_empty() {
        anyhow::bail!("compute_partition_stats given empty partition set");
    }
    let first = partitions.first().unwrap();
    let state = PartitionStats {
        num_rows: first.file_metadata.file_metadata().num_rows(),
        min_event_time: first.min_event_time,
        max_event_time: first.max_event_time,
    };
    Ok(partitions
        .iter()
        .skip(1)
        .fold(state, |state, part| PartitionStats {
            num_rows: state.num_rows + part.file_metadata.file_metadata().num_rows(),
            min_event_time: state.min_event_time.min(part.min_event_time),
            max_event_time: state.max_event_time.max(part.max_event_time),
        }))
}

/// Merges multiple partitions by splitting the work in batches to use less memory.
/// The batches are based on event times.
#[derive(Debug)]
pub struct BatchPartitionMerger {
    /// runtime: datafusion runtime
    runtime: Arc<RuntimeEnv>,
    /// file_schema: arrow schema of the parquet files
    file_schema: Arc<Schema>,
    /// view_factory: allows joins in merge query
    view_factory: Arc<ViewFactory>,
    /// merge_batch_query: merge query with begin & end placeholders
    merge_batch_query: String,
    /// batch size to aim for
    approx_nb_rows_per_batch: i64,
}

impl BatchPartitionMerger {
    pub fn new(
        runtime: Arc<RuntimeEnv>,
        file_schema: Arc<Schema>,
        view_factory: Arc<ViewFactory>,
        merge_batch_query: String,
        approx_nb_rows_per_batch: i64,
    ) -> Self {
        Self {
            runtime,
            file_schema,
            view_factory,
            merge_batch_query,
            approx_nb_rows_per_batch,
        }
    }
}

#[async_trait]
impl PartitionMerger for BatchPartitionMerger {
    async fn execute_merge_query(
        &self,
        lake: Arc<DataLakeConnection>,
        partitions_to_merge: Arc<Vec<Partition>>,
        partitions_all_views: Arc<PartitionCache>,
    ) -> Result<SendableRecordBatchStream> {
        info!("execute_merge_query");
        let stats = compute_partition_stats(partitions_to_merge.as_ref())?;
        let nb_batches = ((stats.num_rows / self.approx_nb_rows_per_batch) + 1) as i32;
        let batch_time_delta = ((stats.max_event_time - stats.min_event_time) / nb_batches)
            + TimeDelta::nanoseconds(1);

        let runtime = self.runtime.clone();
        let file_schema = self.file_schema.clone();
        let ctx = make_session_context(
            runtime,
            lake.clone(),
            partitions_all_views,
            None,
            self.view_factory.clone(),
        )
        .await?;
        let src_table = PartitionedTableProvider::new(
            file_schema,
            lake.blob_storage.inner(),
            partitions_to_merge,
        );
        ctx.register_table(
            TableReference::Bare {
                table: "source".into(),
            },
            Arc::new(src_table),
        )?;
        let df_template = ctx.sql(&self.merge_batch_query).await.map_err(|e| {
            DataFusionError::Execution(format!("building template for merge query: {e:?}"))
        })?;

        let mut builder = RecordBatchReceiverStreamBuilder::new(self.file_schema.clone(), 10);
        let sender = builder.tx();
        builder.spawn(async move {
            let mut streams_stream = stream::iter(0..nb_batches)
                .map(|i| {
                    let begin = stats.min_event_time + (batch_time_delta * i);
                    let end = begin + batch_time_delta;
                    debug!("merging batch {begin} {end}");
                    df_template
                        .clone()
                        .with_param_values(vec![
                            ("begin", datetime_to_scalar(begin)),
                            ("end", datetime_to_scalar(end)),
                        ])
                        .map(|df| async {
                            tokio::spawn(df.execute_stream())
                                .await
                                .map_err(|e| DataFusionError::External(e.into()))
                        })
                })
                .try_buffered(2);

            while let Some(stream_res) = streams_stream.next().await {
                let mut merge_stream = stream_res??;
                let sender = sender.clone();
                while let Some(rb_res) = merge_stream.next().await {
                    if let Err(e) = sender.send(rb_res).await {
                        error!("sending record batch: {e:?}");
                    }
                }
            }
            Ok(())
        });
        debug!("building merge stream");
        Ok(builder.build())
    }
}
