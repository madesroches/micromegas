use crate::{
    metadata::{find_process, list_process_streams_tagged},
    metrics_table::metrics_table_schema,
    time::{make_time_converter_from_db, TimeRange},
};

use super::{
    batch_update::PartitionCreationStrategy,
    block_partition_spec::BlockPartitionSpec,
    jit_partitions::{
        generate_jit_partitions, is_jit_partition_up_to_date, write_partition_from_blocks,
        JitPartitionConfig,
    },
    metrics_block_processor::MetricsBlockProcessor,
    partition_cache::PartitionCache,
    partition_source_data::fetch_partition_source_data,
    view::{PartitionSpec, View, ViewMetadata},
    view_factory::ViewMaker,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::datatypes::Schema,
    execution::runtime_env::RuntimeEnv,
    logical_expr::{col, Between, Expr},
    scalar::ScalarValue,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;
use uuid::Uuid;

const VIEW_SET_NAME: &str = "measures";
lazy_static::lazy_static! {
    static ref TIME_COLUMN: Arc<String> = Arc::new( String::from("time"));
}

#[derive(Debug)]
pub struct MetricsViewMaker {}

impl ViewMaker for MetricsViewMaker {
    fn make_view(&self, view_instance_id: &str) -> Result<Arc<dyn View>> {
        Ok(Arc::new(MetricsView::new(view_instance_id)?))
    }
}

#[derive(Debug)]
pub struct MetricsView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    process_id: Option<sqlx::types::Uuid>,
}

impl MetricsView {
    pub fn new(view_instance_id: &str) -> Result<Self> {
        let process_id = if view_instance_id == "global" {
            None
        } else {
            Some(Uuid::parse_str(view_instance_id).with_context(|| "Uuid::parse_str")?)
        };
        Ok(Self {
            view_set_name: Arc::new(String::from(VIEW_SET_NAME)),
            view_instance_id: Arc::new(view_instance_id.into()),
            process_id,
        })
    }
}

#[async_trait]
impl View for MetricsView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.view_instance_id.clone()
    }

    async fn make_batch_partition_spec(
        &self,
        runtime: Arc<RuntimeEnv>,
        lake: Arc<DataLakeConnection>,
        existing_partitions: Arc<PartitionCache>,
        begin_insert: DateTime<Utc>,
        end_insert: DateTime<Utc>,
    ) -> Result<Arc<dyn PartitionSpec>> {
        if *self.view_instance_id != "global" {
            anyhow::bail!("not supported for jit queries... should it?");
        }
        let source_data = Arc::new(
            fetch_partition_source_data(
                runtime,
                lake,
                existing_partitions,
                begin_insert,
                end_insert,
                "metrics",
            )
            .await
            .with_context(|| "fetch_partition_source_data")?,
        );
        Ok(Arc::new(BlockPartitionSpec {
            view_metadata: ViewMetadata {
                view_set_name: self.view_set_name.clone(),
                view_instance_id: self.view_instance_id.clone(),
                file_schema_hash: self.get_file_schema_hash(),
            },
            schema: self.get_file_schema(),
            begin_insert,
            end_insert,
            source_data,
            block_processor: Arc::new(MetricsBlockProcessor {}),
        }))
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![4]
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(metrics_table_schema())
    }

    async fn jit_update(
        &self,
        lake: Arc<DataLakeConnection>,
        query_range: Option<TimeRange>,
    ) -> Result<()> {
        if *self.view_instance_id == "global" {
            // this view instance is updated using the deamon
            return Ok(());
        }
        let process = Arc::new(
            find_process(
                &lake.db_pool,
                &self
                    .process_id
                    .with_context(|| "getting a view's process_id")?,
            )
            .await
            .with_context(|| "find_process")?,
        );

        let query_range =
            query_range.unwrap_or_else(|| TimeRange::new(process.start_time, chrono::Utc::now()));

        let streams = list_process_streams_tagged(&lake.db_pool, process.process_id, "metrics")
            .await
            .with_context(|| "list_process_streams_tagged")?;
        let convert_ticks = make_time_converter_from_db(&lake.db_pool, &process).await?;
        let mut all_partitions = vec![];
        for stream in streams {
            let mut partitions = generate_jit_partitions(
                &JitPartitionConfig::default(),
                &lake.db_pool,
                &query_range,
                Arc::new(stream),
                process.clone(),
                &convert_ticks,
            )
            .await
            .with_context(|| "generate_jit_partitions")?;
            all_partitions.append(&mut partitions);
        }
        let view_meta = ViewMetadata {
            view_set_name: self.get_view_set_name(),
            view_instance_id: self.get_view_instance_id(),
            file_schema_hash: self.get_file_schema_hash(),
        };

        for part in all_partitions {
            if !is_jit_partition_up_to_date(&lake.db_pool, view_meta.clone(), &convert_ticks, &part)
                .await?
            {
                write_partition_from_blocks(
                    lake.clone(),
                    view_meta.clone(),
                    self.get_file_schema(),
                    part,
                    Arc::new(MetricsBlockProcessor {}),
                )
                .await
                .with_context(|| "write_partition_from_blocks")?;
            }
        }
        Ok(())
    }

    fn make_time_filter(&self, begin: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Expr>> {
        let utc: Arc<str> = Arc::from("+00:00");
        Ok(vec![Expr::Between(Between::new(
            col("time").into(),
            false,
            Expr::Literal(
                ScalarValue::TimestampNanosecond(begin.timestamp_nanos_opt(), Some(utc.clone())),
                None,
            )
            .into(),
            Expr::Literal(
                ScalarValue::TimestampNanosecond(end.timestamp_nanos_opt(), Some(utc.clone())),
                None,
            )
            .into(),
        ))])
    }

    fn get_min_event_time_column_name(&self) -> Arc<String> {
        TIME_COLUMN.clone()
    }

    fn get_max_event_time_column_name(&self) -> Arc<String> {
        TIME_COLUMN.clone()
    }

    fn get_update_group(&self) -> Option<i32> {
        if *(self.get_view_instance_id()) == "global" {
            Some(2000)
        } else {
            None
        }
    }

    fn get_max_partition_time_delta(&self, _strategy: &PartitionCreationStrategy) -> TimeDelta {
        TimeDelta::hours(1)
    }
}
