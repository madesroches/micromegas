use super::{
    batch_update::PartitionCreationStrategy,
    dataframe_time_bounds::{DataFrameTimeBounds, NamedColumnsTimeBounds},
    partition_cache::{NullPartitionProvider, PartitionCache},
    query::make_session_context,
    view::{PartitionSpec, View},
    view_factory::ViewFactory,
};
use crate::{
    lakehouse::{sql_partition_spec::fetch_sql_partition_spec, view::ViewMetadata},
    record_batch_transformer::RecordBatchTransformer,
    time::{datetime_to_scalar, TimeRange},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::{
        array::{PrimitiveBuilder, RecordBatch, StringBuilder},
        datatypes::{DataType, Field, Int32Type, Schema, TimeUnit, TimestampNanosecondType},
    },
    execution::runtime_env::RuntimeEnv,
    prelude::*,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::levels::Level;
use std::hash::Hash;
use std::hash::Hasher;
use std::{hash::DefaultHasher, sync::Arc};

/// A builder for creating log entries for export.
pub struct ExportLogBuilder {
    times: PrimitiveBuilder<TimestampNanosecondType>,
    levels: PrimitiveBuilder<Int32Type>,
    msgs: StringBuilder,
}

impl ExportLogBuilder {
    #[expect(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            times: PrimitiveBuilder::new(),
            levels: PrimitiveBuilder::new(),
            msgs: StringBuilder::new(),
        }
    }

    pub fn append(&mut self, level: Level, msg: &str) {
        let now = Utc::now();
        self.times
            .append_value(now.timestamp_nanos_opt().unwrap_or_default());
        self.levels.append_value(level as i32);
        self.msgs.append_value(msg);
    }

    pub fn finish(mut self) -> Result<RecordBatch> {
        RecordBatch::try_new(
            make_export_log_schema(),
            vec![
                Arc::new(self.times.finish().with_timezone_utc()),
                Arc::new(self.levels.finish()),
                Arc::new(self.msgs.finish()),
            ],
        )
        .with_context(|| "building record batch")
    }
}

/// A view for exporting log data.
#[derive(Debug)]
pub struct ExportLogView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    time_column_name: Arc<String>,
    count_src_query: Arc<String>,
    extract_query: Arc<String>,
    exporter: Arc<dyn RecordBatchTransformer>,
    log_schema: Arc<Schema>,
    view_factory: Arc<ViewFactory>,
    update_group: Option<i32>,
    max_partition_delta_from_source: TimeDelta,
    max_partition_delta_from_merge: TimeDelta,
}

/// Creates the Arrow schema for the export log.
pub fn make_export_log_schema() -> Arc<Schema> {
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
        count_src_query: Arc<String>,
        extract_query: Arc<String>,
        exporter: Arc<dyn RecordBatchTransformer>,
        lake: Arc<DataLakeConnection>,
        view_factory: Arc<ViewFactory>,
        update_group: Option<i32>,
        max_partition_delta_from_source: TimeDelta,
        max_partition_delta_from_merge: TimeDelta,
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
        let sql = extract_query
            .replace("{begin}", &now_str)
            .replace("{end}", &now_str);
        let _extracted_df = ctx.sql(&sql).await?;
        Ok(Self {
            view_set_name,
            view_instance_id: Arc::new(String::from("global")),
            time_column_name: Arc::new(String::from("time")),
            count_src_query,
            extract_query,
            exporter,
            log_schema: make_export_log_schema(),
            view_factory,
            update_group,
            max_partition_delta_from_source,
            max_partition_delta_from_merge,
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
        runtime: Arc<RuntimeEnv>,
        lake: Arc<DataLakeConnection>,
        existing_partitions: Arc<PartitionCache>,
        insert_range: TimeRange,
    ) -> Result<Arc<dyn PartitionSpec>> {
        let view_meta = ViewMetadata {
            view_set_name: self.get_view_set_name(),
            view_instance_id: self.get_view_instance_id(),
            file_schema_hash: self.get_file_schema_hash(),
        };
        let partitions_in_range = Arc::new(existing_partitions.filter_insert_range(insert_range));
        let ctx = make_session_context(
            runtime.clone(),
            lake.clone(),
            partitions_in_range.clone(),
            None,
            self.view_factory.clone(),
        )
        .await
        .with_context(|| "make_session_context")?;
        let count_src_sql = self
            .count_src_query
            .replace("{begin}", &insert_range.begin.to_rfc3339())
            .replace("{end}", &insert_range.end.to_rfc3339());
        let extract_sql = self
            .extract_query
            .replace("{begin}", &insert_range.begin.to_rfc3339())
            .replace("{end}", &insert_range.end.to_rfc3339());
        Ok(Arc::new(
            fetch_sql_partition_spec(
                ctx,
                self.exporter.clone(),
                self.get_time_bounds(),
                self.log_schema.clone(),
                count_src_sql,
                extract_sql,
                view_meta,
                insert_range,
            )
            .await
            .with_context(|| "fetch_sql_partition_spec")?,
        ))
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
	_runtime: Arc<RuntimeEnv>,
        _lake: Arc<DataLakeConnection>,
        _query_range: Option<TimeRange>,
    ) -> Result<()> {
        Ok(())
    }

    fn make_time_filter(&self, begin: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Expr>> {
        Ok(vec![
            col(&**self.time_column_name).lt_eq(lit(datetime_to_scalar(end))),
            col(&**self.time_column_name).gt_eq(lit(datetime_to_scalar(begin))),
        ])
    }

    fn get_time_bounds(&self) -> Arc<dyn DataFrameTimeBounds> {
        Arc::new(NamedColumnsTimeBounds::new(
            self.time_column_name.clone(),
            self.time_column_name.clone(),
        ))
    }

    fn get_update_group(&self) -> Option<i32> {
        self.update_group
    }

    fn get_max_partition_time_delta(&self, strategy: &PartitionCreationStrategy) -> TimeDelta {
        match strategy {
            PartitionCreationStrategy::Abort | PartitionCreationStrategy::CreateFromSource => {
                self.max_partition_delta_from_source
            }
            PartitionCreationStrategy::MergeExisting(_partitions) => {
                self.max_partition_delta_from_merge
            }
        }
    }
}
