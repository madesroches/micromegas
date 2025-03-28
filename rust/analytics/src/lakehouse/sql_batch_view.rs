use super::{
    batch_update::PartitionCreationStrategy,
    materialized_view::MaterializedView,
    merge::{PartitionMerger, QueryMerger},
    partition::Partition,
    partition_cache::{NullPartitionProvider, PartitionCache},
    query::make_session_context,
    sql_partition_spec::fetch_sql_partition_spec,
    view::{PartitionSpec, View, ViewMetadata},
    view_factory::ViewFactory,
};
use crate::time::TimeRange;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::datatypes::Schema, execution::SendableRecordBatchStream, prelude::*,
    scalar::ScalarValue, sql::TableReference,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::hash::Hash;
use std::hash::Hasher;
use std::{hash::DefaultHasher, sync::Arc};

pub type MergerMaker = dyn Fn(Arc<Schema>) -> Arc<dyn PartitionMerger>;

/// SQL-defined view updated in batch
#[derive(Debug)]
pub struct SqlBatchView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    min_event_time_column: Arc<String>,
    max_event_time_column: Arc<String>,
    count_src_query: Arc<String>,
    transform_query: Arc<String>,
    merge_partitions_query: Arc<String>,
    schema: Arc<Schema>,
    merger: Arc<dyn PartitionMerger>,
    view_factory: Arc<ViewFactory>,
    update_group: Option<i32>,
    max_partition_delta_from_source: TimeDelta,
    max_partition_delta_from_merge: TimeDelta,
}

impl SqlBatchView {
    #[allow(clippy::too_many_arguments)]
    /// # Arguments
    ///
    /// * `view_set_name` - name of the table
    /// * `min_event_time_column` - min(column) should result in the first timestamp in a dataframe
    /// * `max_event_time_column` - max(column) should result in the last timestamp in a dataframe
    /// * `count_src_query` - used to count the rows of the underlying data to know if a cached partition is up to date
    /// * `transform_query` - used to transform the source data into a cached partition
    /// * `merge_partitions_query` - used to merge multiple partitions into a single one (and user queries which are one multiple partitions by default)
    /// * `lake` - data lake
    /// * `view_factory` - all views accessible to the `count_src_query`
    /// * `update_group` - tells the daemon which view should be materialized and in what order
    pub async fn new(
        view_set_name: Arc<String>,
        min_event_time_column: Arc<String>,
        max_event_time_column: Arc<String>,
        count_src_query: Arc<String>,
        transform_query: Arc<String>,
        merge_partitions_query: Arc<String>,
        lake: Arc<DataLakeConnection>,
        view_factory: Arc<ViewFactory>,
        update_group: Option<i32>,
        max_partition_delta_from_source: TimeDelta,
        max_partition_delta_from_merge: TimeDelta,
        merger_maker: Option<&MergerMaker>,
    ) -> Result<Self> {
        let null_part_provider = Arc::new(NullPartitionProvider {});
        let ctx = make_session_context(lake, null_part_provider, None, view_factory.clone())
            .await
            .with_context(|| "make_session_context")?;
        let now_str = Utc::now().to_rfc3339();
        let sql = transform_query
            .replace("{begin}", &now_str)
            .replace("{end}", &now_str);
        let transformed_df = ctx.sql(&sql).await?;
        let schema = transformed_df.schema().inner().clone();
        let merger = merger_maker.unwrap_or(&|schema| {
            let merge_query = Arc::new(merge_partitions_query.replace("{source}", "source"));
            Arc::new(QueryMerger::new(schema, merge_query))
        })(schema.clone());

        Ok(Self {
            view_set_name,
            view_instance_id: Arc::new(String::from("global")),
            min_event_time_column,
            max_event_time_column,
            count_src_query,
            transform_query,
            merge_partitions_query,
            schema,
            merger,
            view_factory,
            update_group,
            max_partition_delta_from_source,
            max_partition_delta_from_merge,
        })
    }
}

#[async_trait]
impl View for SqlBatchView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.view_instance_id.clone()
    }

    async fn make_batch_partition_spec(
        &self,
        lake: Arc<DataLakeConnection>,
        existing_partitions: Arc<PartitionCache>,
        begin_insert: DateTime<Utc>,
        end_insert: DateTime<Utc>,
    ) -> Result<Arc<dyn PartitionSpec>> {
        let view_meta = ViewMetadata {
            view_set_name: self.get_view_set_name(),
            view_instance_id: self.get_view_instance_id(),
            file_schema_hash: self.get_file_schema_hash(),
        };
        let partitions_in_range =
            Arc::new(existing_partitions.filter_insert_range(begin_insert, end_insert));
        let ctx = make_session_context(
            lake.clone(),
            partitions_in_range.clone(),
            None,
            self.view_factory.clone(),
        )
        .await
        .with_context(|| "make_session_context")?;

        let count_src_sql = self
            .count_src_query
            .replace("{begin}", &begin_insert.to_rfc3339())
            .replace("{end}", &end_insert.to_rfc3339());

        let transform_sql = self
            .transform_query
            .replace("{begin}", &begin_insert.to_rfc3339())
            .replace("{end}", &end_insert.to_rfc3339());

        Ok(Arc::new(
            fetch_sql_partition_spec(
                ctx,
                count_src_sql,
                transform_sql,
                self.min_event_time_column.clone(),
                self.max_event_time_column.clone(),
                view_meta,
                begin_insert,
                end_insert,
            )
            .await
            .with_context(|| "fetch_sql_partition_spec")?,
        ))
    }
    fn get_file_schema_hash(&self) -> Vec<u8> {
        let mut hasher = DefaultHasher::new();
        self.schema.hash(&mut hasher);
        hasher.finish().to_le_bytes().to_vec()
    }
    fn get_file_schema(&self) -> Arc<Schema> {
        self.schema.clone()
    }
    async fn jit_update(
        &self,
        _lake: Arc<DataLakeConnection>,
        _query_range: Option<TimeRange>,
    ) -> Result<()> {
        Ok(())
    }
    fn make_time_filter(&self, begin: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Expr>> {
        let utc: Arc<str> = Arc::from("+00:00");
        Ok(vec![
            col(&*self.min_event_time_column).lt_eq(lit(ScalarValue::TimestampNanosecond(
                end.timestamp_nanos_opt(),
                Some(utc.clone()),
            ))),
            col(&*self.max_event_time_column).gt_eq(lit(ScalarValue::TimestampNanosecond(
                begin.timestamp_nanos_opt(),
                Some(utc.clone()),
            ))),
        ])
    }

    fn get_min_event_time_column_name(&self) -> Arc<String> {
        self.min_event_time_column.clone()
    }

    fn get_max_event_time_column_name(&self) -> Arc<String> {
        self.max_event_time_column.clone()
    }

    async fn register_table(&self, ctx: &SessionContext, table: MaterializedView) -> Result<()> {
        let view_name = self.get_view_set_name().to_string();
        let partitions_table_name = format!("__{view_name}__partitions");
        ctx.register_table(
            TableReference::Bare {
                table: partitions_table_name.clone().into(),
            },
            Arc::new(table),
        )?;
        let df = ctx
            .sql(
                &self
                    .merge_partitions_query
                    .replace("{source}", &partitions_table_name),
            )
            .await?;
        ctx.register_table(
            TableReference::Bare {
                table: view_name.into(),
            },
            df.into_view(),
        )?;
        Ok(())
    }

    async fn merge_partitions(
        &self,
        lake: Arc<DataLakeConnection>,
        partitions: Vec<Partition>,
    ) -> Result<SendableRecordBatchStream> {
        self.merger.execute_merge_query(lake, partitions).await
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
