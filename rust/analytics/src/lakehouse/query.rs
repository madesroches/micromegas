use super::{
    answer::Answer, get_payload_function::GetPayload, lakehouse_context::LakehouseContext,
    list_partitions_table_function::ListPartitionsTableFunction,
    list_view_sets_table_function::ListViewSetsTableFunction,
    materialize_partitions_table_function::MaterializePartitionsTableFunction,
    partition::Partition, partition_cache::QueryPartitionProvider,
    partitioned_table_provider::PartitionedTableProvider,
    perfetto_trace_table_function::PerfettoTraceTableFunction, reader_factory::ReaderFactory,
    retire_partition_by_file_udf::make_retire_partition_by_file_udf,
    retire_partition_by_metadata_udf::make_retire_partition_by_metadata_udf,
    retire_partitions_table_function::RetirePartitionsTableFunction,
    session_configurator::SessionConfigurator, view::View, view_factory::ViewFactory,
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
            keys::make_jsonb_object_keys_udf,
            parse::make_jsonb_parse_udf,
        },
    },
    lakehouse::{
        materialized_view::MaterializedView, table_scan_rewrite::TableScanRewrite,
        view_instance_table_function::ViewInstanceTableFunction,
    },
    properties::{
        properties_to_dict_udf::{PropertiesLength, PropertiesToArray, PropertiesToDict},
        properties_to_jsonb_udf::PropertiesToJsonb,
        property_get::PropertyGet,
    },
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
use micromegas_tracing::prelude::*;
use std::sync::Arc;

#[span_fn]
async fn register_table(
    lakehouse: Arc<LakehouseContext>,
    reader_factory: Arc<ReaderFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    ctx: &SessionContext,
    view: Arc<dyn View>,
) -> Result<()> {
    let table = MaterializedView::new(
        lakehouse,
        reader_factory,
        view.clone(),
        part_provider,
        query_range,
    );
    view.register_table(ctx, table).await
}

/// query_partitions_context returns a context to run queries using the partitions as the "source" table
#[span_fn]
pub async fn query_partitions_context(
    runtime: Arc<RuntimeEnv>,
    reader_factory: Arc<ReaderFactory>,
    object_store: Arc<dyn object_store::ObjectStore>,
    schema: SchemaRef,
    partitions: Arc<Vec<Partition>>,
) -> Result<SessionContext> {
    let table = PartitionedTableProvider::new(schema, reader_factory, partitions);
    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    let ctx = SessionContext::new_with_config_rt(SessionConfig::default(), runtime);
    ctx.register_object_store(object_store_url.as_ref(), object_store);
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
#[span_fn]
pub async fn query_partitions(
    runtime: Arc<RuntimeEnv>,
    reader_factory: Arc<ReaderFactory>,
    object_store: Arc<dyn object_store::ObjectStore>,
    schema: SchemaRef,
    partitions: Arc<Vec<Partition>>,
    sql: &str,
) -> Result<DataFrame> {
    let ctx =
        query_partitions_context(runtime, reader_factory, object_store, schema, partitions).await?;
    Ok(ctx.sql(sql).await?)
}

/// register functions that are part of the lakehouse architecture
#[span_fn]
pub fn register_lakehouse_functions(
    ctx: &SessionContext,
    lakehouse: Arc<LakehouseContext>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    view_factory: Arc<ViewFactory>,
) {
    ctx.register_udtf(
        "view_instance",
        Arc::new(ViewInstanceTableFunction::new(
            lakehouse.clone(),
            view_factory.clone(),
            part_provider.clone(),
            query_range,
        )),
    );
    ctx.register_udtf(
        "list_partitions",
        Arc::new(ListPartitionsTableFunction::new(lakehouse.lake().clone())),
    );
    ctx.register_udtf(
        "list_view_sets",
        Arc::new(ListViewSetsTableFunction::new(view_factory.clone())),
    );
    ctx.register_udtf(
        "retire_partitions",
        Arc::new(RetirePartitionsTableFunction::new(lakehouse.lake().clone())),
    );
    ctx.register_udtf(
        "perfetto_trace_chunks",
        Arc::new(PerfettoTraceTableFunction::new(
            lakehouse.clone(),
            view_factory.clone(),
            part_provider.clone(),
        )),
    );
    ctx.register_udtf(
        "materialize_partitions",
        Arc::new(MaterializePartitionsTableFunction::new(
            lakehouse.clone(),
            view_factory.clone(),
        )),
    );
    ctx.register_udf(
        AsyncScalarUDF::new(Arc::new(GetPayload::new(lakehouse.lake().clone()))).into_scalar_udf(),
    );
    ctx.register_udf(make_retire_partition_by_file_udf(lakehouse.lake().clone()).into_scalar_udf());
    ctx.register_udf(
        make_retire_partition_by_metadata_udf(lakehouse.lake().clone()).into_scalar_udf(),
    );
}

/// register functions that are not depended on the lakehouse architecture
#[span_fn]
pub fn register_extension_functions(ctx: &SessionContext) {
    ctx.register_udf(ScalarUDF::from(PropertyGet::new()));
    ctx.register_udf(ScalarUDF::from(PropertiesToDict::new()));
    ctx.register_udf(ScalarUDF::from(PropertiesToArray::new()));
    ctx.register_udf(ScalarUDF::from(PropertiesToJsonb::new()));
    ctx.register_udf(ScalarUDF::from(PropertiesLength::new()));
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
    ctx.register_udf(make_jsonb_object_keys_udf());
}

#[span_fn]
pub fn register_functions(
    ctx: &SessionContext,
    lakehouse: Arc<LakehouseContext>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    view_factory: Arc<ViewFactory>,
) {
    register_lakehouse_functions(ctx, lakehouse, part_provider, query_range, view_factory);
    register_extension_functions(ctx);
}

#[span_fn]
pub async fn make_session_context(
    lakehouse: Arc<LakehouseContext>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    view_factory: Arc<ViewFactory>,
    configurator: Arc<dyn SessionConfigurator>,
) -> Result<SessionContext> {
    // Disable page index reading for backward compatibility with legacy Parquet files
    // Legacy files may have incomplete ColumnIndex metadata (missing null_pages field)
    // which causes errors in DataFusion 51+ with Arrow 57.0 when reading page indexes
    let config = SessionConfig::default()
        .set_bool("datafusion.execution.parquet.enable_page_index", false)
        .with_information_schema(true);
    let ctx = SessionContext::new_with_config_rt(config, lakehouse.runtime().clone());
    if let Some(range) = &query_range {
        ctx.add_analyzer_rule(Arc::new(TableScanRewrite::new(*range)));
    }
    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    let object_store = lakehouse.lake().blob_storage.inner();
    ctx.register_object_store(object_store_url.as_ref(), object_store);
    let reader_factory = lakehouse.reader_factory().clone();
    register_functions(
        &ctx,
        lakehouse.clone(),
        part_provider.clone(),
        query_range,
        view_factory.clone(),
    );
    for view in view_factory.get_global_views() {
        register_table(
            lakehouse.clone(),
            reader_factory.clone(),
            part_provider.clone(),
            query_range,
            &ctx,
            view.clone(),
        )
        .await?;
    }
    // Apply custom configuration
    configurator.configure(&ctx).await?;
    Ok(ctx)
}

#[span_fn]
pub async fn query(
    lakehouse: Arc<LakehouseContext>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    sql: &str,
    view_factory: Arc<ViewFactory>,
    configurator: Arc<dyn SessionConfigurator>,
) -> Result<Answer> {
    info!("query sql={sql}");
    let ctx = make_session_context(
        lakehouse,
        part_provider,
        query_range,
        view_factory,
        configurator,
    )
    .await
    .with_context(|| "make_session_context")?;
    let df = ctx.sql(sql).await?;
    let schema = df.schema().inner().clone();
    let batches: Vec<RecordBatch> = df.collect().await?;
    Ok(Answer::new(schema, batches))
}
