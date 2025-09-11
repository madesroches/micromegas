use super::{
    answer::Answer, get_payload_function::GetPayload,
    list_partitions_table_function::ListPartitionsTableFunction,
    materialize_partitions_table_function::MaterializePartitionsTableFunction,
    partition::Partition, partition_cache::QueryPartitionProvider,
    partitioned_table_provider::PartitionedTableProvider,
    perfetto_trace_table_function::PerfettoTraceTableFunction, property_get_function::PropertyGet,
    retire_partitions_table_function::RetirePartitionsTableFunction, view::View,
    view_factory::ViewFactory,
};
use crate::{
    dfext::{
        histogram::{
            accessors::{make_count_from_histogram_udf, make_sum_from_histogram_udf},
            histogram_udaf::make_histo_udaf,
            quantile::make_quantile_from_histogram_udf,
            sum_histograms_udaf::sum_histograms_udaf,
            variance::make_variance_from_histogram_udf,
        },
        jsonb::{
            cast::{make_jsonb_as_f64_udf, make_jsonb_as_i64_udf, make_jsonb_as_string_udf},
            format_json::make_jsonb_format_json_udf,
            get::make_jsonb_get_udf,
            parse::make_jsonb_parse_udf,
        },
    },
    lakehouse::{
        materialized_view::MaterializedView, table_scan_rewrite::TableScanRewrite,
        view_instance_table_function::ViewInstanceTableFunction,
    },
    properties_to_dict_udf::PropertiesToDict,
    time::TimeRange,
};
use anyhow::{Context, Result};
use datafusion::{
    arrow::{array::RecordBatch, datatypes::SchemaRef},
    execution::{context::SessionContext, object_store::ObjectStoreUrl, runtime_env::RuntimeEnv},
    logical_expr::{ScalarUDF, async_udf::AsyncScalarUDF},
    prelude::*,
    sql::TableReference,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use object_store::ObjectStore;
use std::sync::Arc;

async fn register_table(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    ctx: &SessionContext,
    object_store: Arc<dyn ObjectStore>,
    view: Arc<dyn View>,
) -> Result<()> {
    let table = MaterializedView::new(
        runtime,
        lake,
        object_store,
        view.clone(),
        part_provider,
        query_range,
    );
    view.register_table(ctx, table).await
}

/// query_partitions_context returns a context to run queries using the partitions as the "source" table
pub async fn query_partitions_context(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    schema: SchemaRef,
    partitions: Arc<Vec<Partition>>,
) -> Result<SessionContext> {
    let object_store = lake.blob_storage.inner();
    let table = PartitionedTableProvider::new(
        schema,
        object_store.clone(),
        partitions,
        lake.db_pool.clone(),
    );
    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    let ctx = SessionContext::new_with_config_rt(SessionConfig::default(), runtime);
    ctx.register_object_store(object_store_url.as_ref(), object_store.clone());
    ctx.register_table(
        TableReference::Bare {
            table: "source".into(),
        },
        Arc::new(table),
    )?;
    register_extension_functions(&ctx);
    Ok(ctx)
}

// query_partitions returns a dataframe, leaving the option of streaming the results
pub async fn query_partitions(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    schema: SchemaRef,
    partitions: Arc<Vec<Partition>>,
    sql: &str,
) -> Result<DataFrame> {
    let ctx = query_partitions_context(runtime, lake, schema, partitions).await?;
    Ok(ctx.sql(sql).await?)
}

/// register functions that are part of the lakehouse architecture
pub fn register_lakehouse_functions(
    ctx: &SessionContext,
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    view_factory: Arc<ViewFactory>,
    object_store: Arc<dyn ObjectStore>,
) {
    ctx.register_udtf(
        "view_instance",
        Arc::new(ViewInstanceTableFunction::new(
            runtime.clone(),
            lake.clone(),
            object_store.clone(),
            view_factory.clone(),
            part_provider.clone(),
            query_range,
        )),
    );
    ctx.register_udtf(
        "list_partitions",
        Arc::new(ListPartitionsTableFunction::new(lake.clone())),
    );
    ctx.register_udtf(
        "retire_partitions",
        Arc::new(RetirePartitionsTableFunction::new(lake.clone())),
    );
    ctx.register_udtf(
        "perfetto_trace_chunks",
        Arc::new(PerfettoTraceTableFunction::new(
            runtime.clone(),
            lake.clone(),
            object_store.clone(),
            view_factory.clone(),
            part_provider.clone(),
        )),
    );
    ctx.register_udtf(
        "materialize_partitions",
        Arc::new(MaterializePartitionsTableFunction::new(
            runtime,
            lake.clone(),
            view_factory.clone(),
        )),
    );
    ctx.register_udf(AsyncScalarUDF::new(Arc::new(GetPayload::new(lake))).into_scalar_udf());
}

/// register functions that are not depended on the lakehouse architecture
pub fn register_extension_functions(ctx: &SessionContext) {
    ctx.register_udf(ScalarUDF::from(PropertyGet::new()));
    ctx.register_udf(ScalarUDF::from(PropertiesToDict::new()));
    ctx.register_udaf(make_histo_udaf());
    ctx.register_udaf(sum_histograms_udaf());
    ctx.register_udf(make_quantile_from_histogram_udf());
    ctx.register_udf(make_variance_from_histogram_udf());
    ctx.register_udf(make_count_from_histogram_udf());
    ctx.register_udf(make_sum_from_histogram_udf());

    ctx.register_udf(make_jsonb_parse_udf());
    ctx.register_udf(make_jsonb_format_json_udf());
    ctx.register_udf(make_jsonb_get_udf());
    ctx.register_udf(make_jsonb_as_string_udf());
    ctx.register_udf(make_jsonb_as_f64_udf());
    ctx.register_udf(make_jsonb_as_i64_udf());
}

pub fn register_functions(
    ctx: &SessionContext,
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    view_factory: Arc<ViewFactory>,
    object_store: Arc<dyn ObjectStore>,
) {
    register_lakehouse_functions(
        ctx,
        runtime,
        lake,
        part_provider,
        query_range,
        view_factory,
        object_store,
    );
    register_extension_functions(ctx);
}

#[span_fn]
pub async fn make_session_context(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    view_factory: Arc<ViewFactory>,
) -> Result<SessionContext> {
    let ctx = SessionContext::new_with_config_rt(SessionConfig::default(), runtime.clone());
    if let Some(range) = &query_range {
        ctx.add_analyzer_rule(Arc::new(TableScanRewrite::new(*range)));
    }
    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    let object_store = lake.blob_storage.inner();
    ctx.register_object_store(object_store_url.as_ref(), object_store.clone());
    register_functions(
        &ctx,
        runtime.clone(),
        lake.clone(),
        part_provider.clone(),
        query_range,
        view_factory.clone(),
        object_store.clone(),
    );
    for view in view_factory.get_global_views() {
        register_table(
            runtime.clone(),
            lake.clone(),
            part_provider.clone(),
            query_range,
            &ctx,
            object_store.clone(),
            view.clone(),
        )
        .await?;
    }
    Ok(ctx)
}

pub async fn query(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    sql: &str,
    view_factory: Arc<ViewFactory>,
) -> Result<Answer> {
    info!("query sql={sql}");
    let ctx = make_session_context(runtime, lake, part_provider, query_range, view_factory)
        .await
        .with_context(|| "make_session_context")?;
    let df = ctx.sql(sql).await?;
    let schema = df.schema().inner().clone();
    let batches: Vec<RecordBatch> = df.collect().await?;
    Ok(Answer::new(schema, batches))
}
