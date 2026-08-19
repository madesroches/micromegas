use super::{
    answer::Answer, deny_queries_table_function::DenyQueriesTableFunction,
    get_payload_function::GetPayload, lakehouse_context::LakehouseContext,
    list_partitions_table_function::ListPartitionsTableFunction,
    list_query_denials_table_function::ListQueryDenialsTableFunction,
    list_view_sets_table_function::ListViewSetsTableFunction,
    materialize_partitions_table_function::MaterializePartitionsTableFunction,
    parse_block_table_function::ParseBlockTableFunction, partition::Partition,
    partition_cache::QueryPartitionProvider, partitioned_table_provider::PartitionedTableProvider,
    perfetto_trace_table_function::PerfettoTraceTableFunction,
    process_spans_table_function::ProcessSpansTableFunction, reader_factory::ReaderFactory,
    regenerate_partitions_table_function::RegeneratePartitionsTableFunction,
    remove_query_denial_udf::make_remove_query_denial_udf,
    retire_partition_by_file_udf::make_retire_partition_by_file_udf,
    retire_partition_by_metadata_udf::make_retire_partition_by_metadata_udf,
    retire_partitions_table_function::RetirePartitionsTableFunction,
    session_configurator::SessionConfigurator, view::View, view_factory::ViewFactory,
};
use crate::{
    lakehouse::{
        audience_guard::AudienceGuard,
        materialized_view::MaterializedView,
        ownership_rewrite::OwnershipRewrite,
        read_scope::{CallerContext, ReadScope},
        table_scan_rewrite::TableScanRewrite,
        view_instance_table_function::ViewInstanceTableFunction,
    },
    properties::{
        properties_to_dict_udf::PropertiesToDict, properties_to_jsonb_udf::PropertiesToJsonb,
    },
    time::TimeRange,
};
use anyhow::{Context, Result};
use datafusion::{
    arrow::{array::RecordBatch, datatypes::SchemaRef},
    datasource::DefaultTableSource,
    execution::{context::SessionContext, object_store::ObjectStoreUrl, runtime_env::RuntimeEnv},
    logical_expr::{ScalarUDF, TableSource, async_udf::AsyncScalarUDF},
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
    caller: &CallerContext,
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
    // Query Enforcement Prong B (#1371, AbAC Stage 3): one guard, shared (via `Arc`) across every
    // arg-addressed UDTF/UDF this call registers below -- `process_spans`, `perfetto_trace_chunks`,
    // `parse_block`, `get_payload`, and `list_partitions`' row filter. `ReadScope::All` makes every
    // one of its checks a no-op, so this costs nothing for internal/maintenance callers or an
    // auth-unset deployment.
    let audience_guard = Arc::new(AudienceGuard::new(
        caller.read_scope.clone(),
        caller.isolation_config.unstamped_audience.clone(),
        caller.isolation_config.public_view_sets.clone(),
        lakehouse.audience_index().clone(),
    ));
    ctx.register_udtf(
        "list_partitions",
        Arc::new(ListPartitionsTableFunction::new(
            lakehouse.lake().clone(),
            audience_guard.clone(),
        )),
    );
    ctx.register_udtf(
        "list_view_sets",
        Arc::new(ListViewSetsTableFunction::new(view_factory.clone())),
    );
    ctx.register_udtf(
        "perfetto_trace_chunks",
        Arc::new(PerfettoTraceTableFunction::new(
            lakehouse.clone(),
            view_factory.clone(),
            part_provider.clone(),
            audience_guard.clone(),
        )),
    );
    ctx.register_udtf(
        "parse_block",
        Arc::new(ParseBlockTableFunction::new(
            lakehouse.clone(),
            view_factory.clone(),
            part_provider.clone(),
            query_range,
            audience_guard.clone(),
        )),
    );
    ctx.register_udtf(
        "process_spans",
        Arc::new(ProcessSpansTableFunction::new(
            lakehouse.clone(),
            view_factory.clone(),
            part_provider.clone(),
            query_range,
            audience_guard.clone(),
        )),
    );
    ctx.register_udf(
        AsyncScalarUDF::new(Arc::new(GetPayload::new(
            lakehouse.lake().clone(),
            audience_guard.clone(),
        )))
        .into_scalar_udf(),
    );
    // An admin, or a deployment that can never produce one at all (`CallerContext::
    // admin_principal_possible`'s doc comment).
    if caller.is_admin || !caller.admin_principal_possible {
        ctx.register_udtf(
            "retire_partitions",
            Arc::new(RetirePartitionsTableFunction::new(lakehouse.lake().clone())),
        );
        ctx.register_udtf(
            "materialize_partitions",
            Arc::new(MaterializePartitionsTableFunction::new(
                lakehouse.clone(),
                view_factory.clone(),
            )),
        );
        ctx.register_udtf(
            "regenerate_partitions",
            Arc::new(RegeneratePartitionsTableFunction::new(
                lakehouse.clone(),
                view_factory.clone(),
            )),
        );
        ctx.register_udf(
            make_retire_partition_by_file_udf(lakehouse.lake().clone()).into_scalar_udf(),
        );
        ctx.register_udf(
            make_retire_partition_by_metadata_udf(lakehouse.lake().clone()).into_scalar_udf(),
        );
        // Admin-managed query deny list (tasks/query_deny_list_plan.md §8): same admin gate as
        // the five functions above.
        ctx.register_udtf(
            "list_query_denials",
            Arc::new(ListQueryDenialsTableFunction::new(
                lakehouse.query_denials().clone(),
            )),
        );
        ctx.register_udtf(
            "deny_queries",
            Arc::new(DenyQueriesTableFunction::new(
                lakehouse.query_denials().clone(),
                caller.identity.clone(),
            )),
        );
        ctx.register_udf(
            make_remove_query_denial_udf(lakehouse.query_denials().clone()).into_scalar_udf(),
        );
    }
}

/// register functions that are not depended on the lakehouse architecture
#[span_fn]
pub fn register_extension_functions(ctx: &SessionContext) {
    ctx.register_udf(ScalarUDF::from(PropertiesToDict::new()));
    ctx.register_udf(ScalarUDF::from(PropertiesToJsonb::new()));
    micromegas_datafusion_extensions::register_extension_udfs(ctx);
}

#[span_fn]
pub fn register_functions(
    ctx: &SessionContext,
    lakehouse: Arc<LakehouseContext>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    view_factory: Arc<ViewFactory>,
    caller: &CallerContext,
) {
    register_lakehouse_functions(
        ctx,
        lakehouse,
        part_provider,
        query_range,
        view_factory,
        caller,
    );
    register_extension_functions(ctx);
}

#[span_fn]
pub async fn make_session_context(
    lakehouse: Arc<LakehouseContext>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    view_factory: Arc<ViewFactory>,
    configurator: Arc<dyn SessionConfigurator>,
    caller: CallerContext,
) -> Result<SessionContext> {
    // Disable page index reading for backward compatibility with legacy Parquet files
    // Legacy files may have incomplete ColumnIndex metadata (missing null_pages field)
    // which causes errors in DataFusion 51+ with Arrow 57.0 when reading page indexes
    let config = SessionConfig::default()
        .set_bool("datafusion.execution.parquet.enable_page_index", false)
        // Populate `Diagnostic`/`Span` on plan-time errors (unknown column, ambiguous
        // reference, type mismatch, ...) so a caller-facing message can point at a
        // line/column in their SQL text instead of just naming the problem. Off by
        // default in DataFusion; see the FlightSQL error-classification plan.
        .set_bool("datafusion.sql_parser.collect_spans", true)
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
        &caller,
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
    if caller.read_scope != ReadScope::All {
        // ReadScope::All is the internal/maintenance marker (Current State §3 of
        // tasks/1370_ownership_rewrite_plan.md) -- OwnershipRewrite would no-op for it anyway, so
        // skip resolving `processes`/`streams` sources and registering the rule entirely rather
        // than requiring every ReadScope::All caller's ViewFactory to carry them.
        //
        // Must be registered *after* the TableScanRewrite registration above (`query_range.is_some()`
        // block): TableScanRewrite::analyze walks the whole plan with transform_up_with_subqueries
        // and would time-bound the audience lookup's own processes/streams scans if it ran after
        // OwnershipRewrite injected them (analyzer rules each run exactly once, in registration
        // order). The audience lookup must stay time-unbounded (query_range: None below).
        let processes_view = view_factory.get_global_view("processes").with_context(
            || "OwnershipRewrite requires the `processes` global view to be registered",
        )?;
        let streams_view = view_factory.get_global_view("streams").with_context(
            || "OwnershipRewrite requires the `streams` global view to be registered",
        )?;
        let processes_source: Arc<dyn TableSource> =
            Arc::new(DefaultTableSource::new(Arc::new(MaterializedView::new(
                lakehouse.clone(),
                reader_factory.clone(),
                processes_view,
                part_provider.clone(),
                None,
            ))));
        let streams_source: Arc<dyn TableSource> =
            Arc::new(DefaultTableSource::new(Arc::new(MaterializedView::new(
                lakehouse.clone(),
                reader_factory.clone(),
                streams_view,
                part_provider.clone(),
                None,
            ))));
        ctx.add_analyzer_rule(Arc::new(OwnershipRewrite::new(
            caller.read_scope.clone(),
            caller.isolation_config.unstamped_audience.clone(),
            caller.isolation_config.public_view_sets.clone(),
            processes_source,
            streams_source,
        )));
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
    caller: CallerContext,
) -> Result<Answer> {
    info!("query sql={sql}");
    let ctx = make_session_context(
        lakehouse,
        part_provider,
        query_range,
        view_factory,
        configurator,
        caller,
    )
    .await
    .with_context(|| "make_session_context")?;
    let df = ctx.sql(sql).await?;
    let schema = df.schema().inner().clone();
    let batches: Vec<RecordBatch> = df.collect().await?;
    Ok(Answer::new(schema, batches))
}
