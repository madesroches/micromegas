use crate::{
    metadata::{find_process, list_process_streams_tagged},
    metrics_table::metrics_table_schema,
    time::ConvertTicks,
};

use super::{
    block_partition_spec::BlockPartitionSpec,
    jit_partitions::{
        generate_jit_partitions, is_jit_partition_up_to_date, write_partition_from_blocks,
    },
    metrics_block_processor::MetricsBlockProcessor,
    partition_source_data::fetch_partition_source_data,
    view::{PartitionSpec, View, ViewMetadata},
    view_factory::ViewMaker,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::{
    arrow::datatypes::Schema, catalog::TableProvider, execution::context::SessionContext,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;
use uuid::Uuid;

const VIEW_SET_NAME: &str = "measures";

pub struct MetricsViewMaker {}

impl ViewMaker for MetricsViewMaker {
    fn make_view(&self, view_instance_id: &str) -> Result<Arc<dyn View>> {
        Ok(Arc::new(MetricsView::new(view_instance_id)?))
    }
}

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
        lake: Arc<DataLakeConnection>,
        begin_insert: DateTime<Utc>,
        end_insert: DateTime<Utc>,
    ) -> Result<Arc<dyn PartitionSpec>> {
        if *self.view_instance_id != "global" {
            anyhow::bail!("not supported for jit queries... should it?");
        }
        let source_data = fetch_partition_source_data(lake, begin_insert, end_insert, "metrics")
            .await
            .with_context(|| "fetch_partition_source_data")?;
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
        vec![0]
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(metrics_table_schema())
    }

    async fn jit_update(
        &self,
        lake: Arc<DataLakeConnection>,
        begin_query: DateTime<Utc>,
        end_query: DateTime<Utc>,
    ) -> Result<()> {
        if *self.view_instance_id == "global" {
            // this view instance is updated using the deamon
            return Ok(());
        }
        let mut connection = lake.db_pool.acquire().await?;
        let process = Arc::new(
            find_process(
                &mut connection,
                &self
                    .process_id
                    .with_context(|| "getting a view's process_id")?,
            )
            .await
            .with_context(|| "find_process")?,
        );

        let streams = list_process_streams_tagged(&mut connection, process.process_id, "metrics")
            .await
            .with_context(|| "list_process_streams_tagged")?;
        let mut all_partitions = vec![];
        for stream in streams {
            let mut partitions = generate_jit_partitions(
                &mut connection,
                begin_query,
                end_query,
                Arc::new(stream),
                process.clone(),
            )
            .await
            .with_context(|| "generate_jit_partitions")?;
            all_partitions.append(&mut partitions);
        }
        drop(connection);
        let view_meta = ViewMetadata {
            view_set_name: self.get_view_set_name(),
            view_instance_id: self.get_view_instance_id(),
            file_schema_hash: self.get_file_schema_hash(),
        };

        let convert_ticks = ConvertTicks::new(&process);
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

    async fn make_filtering_table_provider(
        &self,
        ctx: &SessionContext,
        full_table_name: &str,
        begin: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Arc<dyn TableProvider>> {
        let row_filter = ctx
            .sql(&format!(
                "SELECT * from {full_table_name} WHERE time BETWEEN '{}' AND '{}';",
                begin.to_rfc3339(),
                end.to_rfc3339(),
            ))
            .await?;
        Ok(row_filter.into_view())
    }
}
