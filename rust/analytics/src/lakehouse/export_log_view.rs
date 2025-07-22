use super::{
    partition_cache::{NullPartitionProvider, PartitionCache},
    query::make_session_context,
    view::{PartitionSpec, View},
    view_factory::ViewFactory,
};
use crate::time::{datetime_to_scalar, TimeRange};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::datatypes::{DataType, Field, Schema, TimeUnit},
    execution::runtime_env::RuntimeEnv,
    prelude::*,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::info;
use std::hash::Hash;
use std::hash::Hasher;
use std::{hash::DefaultHasher, sync::Arc};

#[derive(Debug)]
pub struct ExportLogView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    log_schema: Arc<Schema>,
    update_group: Option<i32>,
}

fn make_log_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new(
            "time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("level", DataType::Int32, false),
        Field::new("msg", DataType::Utf8, false),
    ]))
}

impl ExportLogView {
    #[expect(clippy::too_many_arguments)]
    pub async fn new(
        runtime: Arc<RuntimeEnv>,
        view_set_name: Arc<String>,
        _count_src_query: Arc<String>,
        transform_query: Arc<String>,
        lake: Arc<DataLakeConnection>,
        view_factory: Arc<ViewFactory>,
        update_group: Option<i32>,
        _max_partition_delta_from_source: TimeDelta,
    ) -> Result<Self> {
        let null_part_provider = Arc::new(NullPartitionProvider {});
        let ctx = make_session_context(
            runtime.clone(),
            lake,
            null_part_provider,
            None,
            view_factory.clone(),
        )
        .await
        .with_context(|| "make_session_context")?;
        let now_str = Utc::now().to_rfc3339();
        let sql = transform_query
            .replace("{begin}", &now_str)
            .replace("{end}", &now_str);
        let _transformed_df = ctx.sql(&sql).await?;
        Ok(Self {
            view_set_name,
            view_instance_id: Arc::new(String::from("global")),
            log_schema: make_log_schema(),
            update_group,
        })
    }
}

#[async_trait]
impl View for ExportLogView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.view_instance_id.clone()
    }

    async fn make_batch_partition_spec(
        &self,
        _runtime: Arc<RuntimeEnv>,
        _lake: Arc<DataLakeConnection>,
        _existing_partitions: Arc<PartitionCache>,
        _insert_range: TimeRange,
    ) -> Result<Arc<dyn PartitionSpec>> {
        info!("ExportLogView::make_batch_partition_spec");
        anyhow::bail!("unimplemented")
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        let mut hasher = DefaultHasher::new();
        self.log_schema.hash(&mut hasher);
        hasher.finish().to_le_bytes().to_vec()
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        self.log_schema.clone()
    }

    async fn jit_update(
        &self,
        _lake: Arc<DataLakeConnection>,
        _query_range: Option<TimeRange>,
    ) -> Result<()> {
        Ok(())
    }

    fn make_time_filter(&self, begin: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Expr>> {
        Ok(vec![
            col("time").lt_eq(lit(datetime_to_scalar(end))),
            col("time").gt_eq(lit(datetime_to_scalar(begin))),
        ])
    }

    fn get_min_event_time_column_name(&self) -> Arc<String> {
        Arc::new("time".into())
    }

    fn get_max_event_time_column_name(&self) -> Arc<String> {
        Arc::new("time".into())
    }

    fn get_update_group(&self) -> Option<i32> {
        self.update_group
    }
}
