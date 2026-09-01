//! Regression tests pinning the planning assumptions ordered `GROUP BY` merges rely on.
//!
//! Given a scan that exposes one ordered plan partition per already-sorted file (files overlapping
//! arbitrarily on the leading, non-temporal sort column), DataFusion 54.1 must plan an ordered
//! `GROUP BY` merge query as a fully streaming pipeline: a k-way `SortPreservingMergeExec` over the
//! file partitions, order-aware `AggregateExec`s (`ordering_mode=Sorted`), and **no** blocking
//! `SortExec`. The negative controls below pin the three ways a view author (or a config change)
//! can lose that property: not declaring the ordering, choosing `GROUP BY` keys the sort columns
//! are not a prefix of, and writing an enrichment join with the ordered side on the build side.
//!
//! These tests are planning-only: no database, no object store access, no Parquet file is ever
//! opened (the fabricated file paths are never read because no stream is executed).

use datafusion::arrow::compute::SortOptions;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use datafusion::catalog::memory::DataSourceExec;
use datafusion::catalog::{Session, TableProvider};
use datafusion::datasource::listing::PartitionedFile;
use datafusion::datasource::physical_plan::{FileScanConfigBuilder, ParquetSource};
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::logical_expr::TableType;
use datafusion::physical_expr::{LexOrdering, PhysicalSortExpr, expressions::Column};
use datafusion::physical_plan::{ExecutionPlan, displayable};
use datafusion::prelude::*;
use std::sync::Arc;

/// Mirrors the shape of the motivating use case -- an aggregate metrics view rolling `measures` up
/// by `(name, time_bin)`. `name` and `unit` are `Dictionary(Int32, Utf8)` exactly as in
/// `metrics_table_schema()`: a dictionary-encoded leading sort column is the realistic case, and
/// nothing here is specific to it (a plain `Utf8` leading column plans identically).
fn spike_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(
            "name",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "time_bin",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new(
            "unit",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new("measure", DataType::Int64, false),
    ]))
}

fn lex_ordering_name_time_bin(schema: &SchemaRef) -> LexOrdering {
    let sort_exprs = ["name", "time_bin"]
        .iter()
        .map(|name| {
            PhysicalSortExpr::new(
                Arc::new(
                    Column::new_with_schema(name, schema).expect("column should exist in schema"),
                ),
                SortOptions {
                    descending: false,
                    nulls_first: false,
                },
            )
        })
        .collect::<Vec<_>>();
    LexOrdering::new(sort_exprs).expect("non-empty ordering")
}

/// A stand-in for the `ScanOrdering::PerFile` scan mode this plan introduces: `num_files`
/// single-file file groups, every one of them declaring the same `(name, time_bin)` ordering. The
/// files are understood to overlap arbitrarily on `name`, so no per-file min/max statistics are
/// attached -- confirming the design's claim that single-file groups have their declared ordering
/// accepted without the statistics `Concatenated` mode needs.
#[derive(Debug)]
struct PerFileOrderedProvider {
    schema: SchemaRef,
    num_files: usize,
    declare_ordering: bool,
}

impl PerFileOrderedProvider {
    fn new(num_files: usize, declare_ordering: bool) -> Self {
        Self {
            schema: spike_schema(),
            num_files,
            declare_ordering,
        }
    }
}

#[async_trait::async_trait]
impl TableProvider for PerFileOrderedProvider {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        let file_groups: Vec<_> = (0..self.num_files)
            .map(|i| vec![PartitionedFile::new(format!("part_{i}.parquet"), 1024)].into())
            .collect();
        let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
        let source = Arc::new(ParquetSource::new(self.schema.clone()));
        let mut builder = FileScanConfigBuilder::new(object_store_url, source)
            .with_limit(limit)
            .with_projection_indices(projection.cloned())?
            .with_file_groups(file_groups);
        if self.declare_ordering {
            builder = builder.with_output_ordering(vec![lex_ordering_name_time_bin(&self.schema)]);
        }
        Ok(Arc::new(DataSourceExec::new(Arc::new(builder.build()))))
    }
}

/// The optimizer settings the per-file merge branch sets, including
/// `enable_round_robin_repartition = false` -- see
/// `default_round_robin_repartition_fans_the_merge_out_to_target_partitions` for why the other
/// four settings alone are not enough.
fn streaming_merge_session(provider: PerFileOrderedProvider) -> SessionContext {
    let mut config = SessionConfig::new();
    let options = config.options_mut();
    options.optimizer.repartition_file_scans = false;
    options.optimizer.prefer_existing_sort = true;
    options.optimizer.repartition_aggregations = false;
    options.optimizer.repartition_joins = false;
    options.optimizer.enable_round_robin_repartition = false;
    let ctx = SessionContext::new_with_config(config);
    ctx.register_table("source", Arc::new(provider))
        .expect("register_table");
    ctx
}

/// The motivating aggregate-metrics merge query: group strictly by the two declared sort columns
/// and carry `unit` through as `first_value` rather than as a third group key, so the aggregate gets
/// the fully `Sorted` group mode. Verified to plan identically with `first_value(unit ORDER BY
/// time_bin)` and with a fuller measure set (`count`/`min`/`max`/`avg`/`sum`).
const MERGE_QUERY: &str = "SELECT name, time_bin, first_value(unit) AS unit, sum(measure) AS total \
                           FROM source GROUP BY name, time_bin ORDER BY name, time_bin";

async fn plan_query(ctx: &SessionContext, query: &str) -> Arc<dyn ExecutionPlan> {
    ctx.sql(query)
        .await
        .expect("merge query should plan")
        .create_physical_plan()
        .await
        .expect("physical plan should build")
}

async fn plan_string(ctx: &SessionContext, query: &str) -> (Arc<dyn ExecutionPlan>, String) {
    let plan = plan_query(ctx, query).await;
    let plan_str = displayable(plan.as_ref()).indent(true).to_string();
    (plan, plan_str)
}

#[tokio::test]
async fn ordered_group_by_over_per_file_scan_streams_end_to_end() {
    let ctx = streaming_merge_session(PerFileOrderedProvider::new(3, true));
    let (plan, plan_str) = plan_string(&ctx, MERGE_QUERY).await;

    let partition_count = plan.properties().output_partitioning().partition_count();
    assert_eq!(
        partition_count, 1,
        "the merge plan must be single-partition so execute_stream cannot coalesce and destroy \
         the ordering, got {partition_count} partitions:\n{plan_str}"
    );
    assert!(
        plan_str.contains("SortPreservingMergeExec"),
        "expected the k ordered file partitions to be coalesced by a streaming k-way \
         SortPreservingMergeExec, got:\n{plan_str}"
    );
    // Note "SortPreservingMergeExec" does not contain the substring "SortExec", so this doubles as
    // a check that the runtime `ordering_honored` heuristic in `QueryMerger` reads naturally.
    assert!(
        !plan_str.contains("SortExec"),
        "a blocking SortExec anywhere in the plan defeats the whole approach -- the merge would \
         buffer the full merge range again, got:\n{plan_str}"
    );
    // The aggregate must itself consume the ordering (GroupOrdering) and emit groups
    // incrementally, not merely sit under a satisfied top-level ORDER BY: a hash aggregate would
    // accumulate every (name, time_bin) group for the whole range in memory, which is the failure
    // this plan exists to avoid.
    assert!(
        plan_str.contains("ordering_mode=Sorted"),
        "expected AggregateExec to report ordering_mode=Sorted, got:\n{plan_str}"
    );
    // No round-robin fan-out: exactly k = 3 scan partitions feed the merge.
    assert!(
        !plan_str.contains("RepartitionExec"),
        "expected no repartitioning between the scan and the merge so peak memory scales with \
         k = 3, got:\n{plan_str}"
    );
}

#[tokio::test]
async fn undeclared_per_file_scan_keeps_blocking_sort_negative_control() {
    // Same query and same optimizer settings, but the scan declares no ordering: the plan must
    // fall back to a blocking SortExec. Without this control, the test above could pass for
    // reasons unrelated to the declared per-file ordering.
    let ctx = streaming_merge_session(PerFileOrderedProvider::new(3, false));
    let (_, plan_str) = plan_string(&ctx, MERGE_QUERY).await;
    assert!(
        plan_str.contains("SortExec"),
        "a scan that declares no ordering must still be sorted, got:\n{plan_str}"
    );
}

#[tokio::test]
async fn default_round_robin_repartition_fans_the_merge_out_to_target_partitions() {
    // `enable_round_robin_repartition` defaults to true, and the other four settings above
    // do not disable it. The plan still streams (no SortExec) and is still
    // correct, but an order-preserving RoundRobinBatch(target_partitions) lands between the scan
    // and the partial aggregate: the SortPreservingMergeExec then merges target_partitions streams
    // instead of k, and there are that many partial-aggregate working sets. Peak memory would
    // scale with target_partitions rather than with k, which is why the merge branch must also set
    // `enable_round_robin_repartition = false`.
    let mut config = SessionConfig::new().with_target_partitions(8);
    {
        let options = config.options_mut();
        options.optimizer.repartition_file_scans = false;
        options.optimizer.prefer_existing_sort = true;
        options.optimizer.repartition_aggregations = false;
        options.optimizer.repartition_joins = false;
    }
    let ctx = SessionContext::new_with_config(config);
    ctx.register_table("source", Arc::new(PerFileOrderedProvider::new(3, true)))
        .expect("register_table");
    let (_, plan_str) = plan_string(&ctx, MERGE_QUERY).await;
    assert!(
        plan_str.contains("RoundRobinBatch") && plan_str.contains("preserve_order=true"),
        "expected the default round-robin repartition to fan the ordered scan out; if DataFusion \
         stopped doing this, the extra setting is no longer needed:\n{plan_str}"
    );
}

#[tokio::test]
async fn single_file_per_file_scan_needs_no_merge_operator() {
    // The k == 1 shape: neither SortExec nor SortPreservingMergeExec is required, and that is
    // not a regression.
    let ctx = streaming_merge_session(PerFileOrderedProvider::new(1, true));
    let (plan, plan_str) = plan_string(&ctx, MERGE_QUERY).await;
    assert_eq!(
        plan.properties().output_partitioning().partition_count(),
        1,
        "single-file scan must stay single-partition:\n{plan_str}"
    );
    assert!(
        !plan_str.contains("SortExec"),
        "a single already-sorted file needs no sort at all, got:\n{plan_str}"
    );
}

#[tokio::test]
async fn group_by_keys_the_sort_columns_do_not_prefix_fall_back_to_blocking_sort() {
    // The authoring requirement: the declared sort columns must be a prefix
    // subset of the merge query's GROUP BY keys. Grouping by `time_bin` alone -- the *second*
    // declared column -- loses GroupOrdering entirely and reinstates a blocking SortExec.
    let ctx = streaming_merge_session(PerFileOrderedProvider::new(3, true));
    let (_, plan_str) = plan_string(
        &ctx,
        "SELECT time_bin, sum(measure) AS total FROM source GROUP BY time_bin ORDER BY time_bin",
    )
    .await;
    assert!(
        plan_str.contains("SortExec") && !plan_str.contains("ordering_mode=Sorted"),
        "grouping by a non-prefix of the declared sort columns must not be mistaken for a \
         streaming shape, got:\n{plan_str}"
    );

    // A strict prefix of the declared columns, by contrast, does stream.
    let (_, prefix_plan) = plan_string(
        &ctx,
        "SELECT name, sum(measure) AS total FROM source GROUP BY name ORDER BY name",
    )
    .await;
    assert!(
        !prefix_plan.contains("SortExec") && prefix_plan.contains("ordering_mode=Sorted"),
        "grouping by a prefix of the declared sort columns must stream, got:\n{prefix_plan}"
    );
}

#[tokio::test]
async fn extra_group_by_key_outside_the_sort_order_still_streams() {
    // Generality guard, not the shape the metrics view uses (it groups strictly by the two sort
    // columns and takes `first_value(unit)` -- see MERGE_QUERY). A view that does group by an extra
    // key outside its sort order drops to `ordering_mode=PartiallySorted`, which still emits
    // incrementally -- it holds only the groups for the current `(name, time_bin)` prefix value --
    // and needs no blocking sort. So declaring a short sort key does not force every group key
    // into it.
    let ctx = streaming_merge_session(PerFileOrderedProvider::new(3, true));
    let (plan, plan_str) = plan_string(
        &ctx,
        "SELECT name, unit, time_bin, sum(measure) AS total FROM source \
         GROUP BY name, unit, time_bin ORDER BY name, time_bin",
    )
    .await;
    assert_eq!(
        plan.properties().output_partitioning().partition_count(),
        1,
        "must stay single-partition:\n{plan_str}"
    );
    assert!(
        !plan_str.contains("SortExec") && plan_str.contains("ordering_mode=PartiallySorted"),
        "an extra GROUP BY key outside the sort order must degrade to PartiallySorted, not to a \
         blocking sort, got:\n{plan_str}"
    );
}

#[tokio::test]
async fn order_by_extending_past_the_declared_scan_ordering_reinstates_a_blocking_sort() {
    // A general DataFusion planning fact, independent of anything a SqlBatchView merge-query
    // author writes (the merge query has no author-written ORDER BY at all -- QueryMerger applies
    // the sort programmatically from the declared columns). This test
    // instead pins the underlying planning behavior an author-supplied query could still produce
    // via an extra order-sensitive aggregate argument, e.g. `first_value(x ORDER BY y)` for a `y`
    // outside the declared scan ordering: an ORDER BY requirement extending past what the scan
    // declares (here, past `(name, time_bin)`) cannot be satisfied by the per-file ordering alone,
    // so the whole result is buffered and sorted.
    let ctx = streaming_merge_session(PerFileOrderedProvider::new(3, true));
    let (_, plan_str) = plan_string(
        &ctx,
        "SELECT name, time_bin, unit, sum(measure) AS total FROM source \
         GROUP BY name, time_bin, unit ORDER BY name, time_bin, unit",
    )
    .await;
    assert!(
        plan_str.contains("SortExec"),
        "an ORDER BY extending past the declared scan ordering must be caught as a memory \
         regression; if DataFusion learned to stream this, the finding no longer holds:\n{plan_str}"
    );
}

#[tokio::test]
async fn reversed_group_by_key_order_still_streams() {
    // GROUP BY key order does not matter -- `GROUP BY time_bin, name` still streams.
    let ctx = streaming_merge_session(PerFileOrderedProvider::new(3, true));
    let (plan, plan_str) = plan_string(
        &ctx,
        "SELECT name, time_bin, first_value(unit) AS unit, sum(measure) AS total FROM source \
         GROUP BY time_bin, name ORDER BY name, time_bin",
    )
    .await;
    assert_eq!(
        plan.properties().output_partitioning().partition_count(),
        1,
        "must stay single-partition:\n{plan_str}"
    );
    assert!(
        !plan_str.contains("SortExec") && plan_str.contains("ordering_mode=Sorted"),
        "GROUP BY key order must not affect streaming, got:\n{plan_str}"
    );
}

#[tokio::test]
async fn first_value_with_explicit_inner_order_by_still_streams() {
    // `first_value(unit ORDER BY time_bin)` keeps ordering_mode=Sorted, just like the
    // no-inner-ORDER-BY form MERGE_QUERY uses.
    let ctx = streaming_merge_session(PerFileOrderedProvider::new(3, true));
    let (plan, plan_str) = plan_string(
        &ctx,
        "SELECT name, time_bin, first_value(unit ORDER BY time_bin) AS unit, sum(measure) AS total \
         FROM source GROUP BY name, time_bin ORDER BY name, time_bin",
    )
    .await;
    assert_eq!(
        plan.properties().output_partitioning().partition_count(),
        1,
        "must stay single-partition:\n{plan_str}"
    );
    assert!(
        !plan_str.contains("SortExec") && plan_str.contains("ordering_mode=Sorted"),
        "an order-sensitive aggregate argument whose ORDER BY does not extend past the declared \
         scan ordering must still stream, got:\n{plan_str}"
    );
}

#[tokio::test]
async fn fuller_measure_set_still_streams() {
    // A fuller measure set (count/min/max/avg/sum) plans identically to the single-sum
    // MERGE_QUERY shape -- this is a finding about plan shape only (composable aggregates over
    // already-aggregated rows, e.g. sum(count) not count(*), is a separate requirement this
    // planning-only test does not check).
    let ctx = streaming_merge_session(PerFileOrderedProvider::new(3, true));
    let (plan, plan_str) = plan_string(
        &ctx,
        "SELECT name, time_bin, count(measure) AS c, min(measure) AS mn, max(measure) AS mx, \
         avg(measure) AS av, sum(measure) AS sm FROM source GROUP BY name, time_bin \
         ORDER BY name, time_bin",
    )
    .await;
    assert_eq!(
        plan.properties().output_partitioning().partition_count(),
        1,
        "must stay single-partition:\n{plan_str}"
    );
    assert!(
        !plan_str.contains("SortExec") && plan_str.contains("ordering_mode=Sorted"),
        "a fuller measure set must plan identically to the single-sum shape, got:\n{plan_str}"
    );
}

#[tokio::test]
async fn cte_internal_order_by_is_discarded_by_a_later_join() {
    // The extract query's ORDER BY must be top-level. A CTE-internal ORDER BY that is later
    // joined does not count -- relational joins carry no row-order guarantee over their inputs, so
    // the join discards it. This is what SqlPartitionSpec::write's declared-path plan verification
    // relies on: it checks ordering_satisfy against the *actual* physical plan, so a
    // CTE-internal-only ORDER BY would correctly fail that check rather than falsely certify the
    // fresh partition.
    let ctx = streaming_merge_session(PerFileOrderedProvider::new(3, true));
    register_dim(&ctx).await;
    let plan = plan_query(
        &ctx,
        "WITH sorted AS (SELECT name, time_bin, measure FROM source ORDER BY name, time_bin) \
         SELECT s.name, s.time_bin, s.measure FROM sorted s JOIN dim d ON s.name = d.name",
    )
    .await;
    let lex = lex_ordering_name_time_bin(&spike_schema());
    let satisfied = plan
        .properties()
        .equivalence_properties()
        .ordering_satisfy(lex)
        .expect("ordering_satisfy should not error");
    assert!(
        !satisfied,
        "a CTE-internal ORDER BY must not survive a later join -- if it did, the extract-query \
         contract's top-level requirement would be unnecessarily strict, got:\n{}",
        displayable(plan.as_ref()).indent(true)
    );
}

#[tokio::test]
async fn top_level_order_by_satisfies_the_declared_columns() {
    // Positive control for the same requirement: a genuinely top-level ORDER BY does satisfy the
    // declared columns -- what SqlPartitionSpec::write's plan verification relies on to accept a
    // fresh extract query.
    let ctx = streaming_merge_session(PerFileOrderedProvider::new(3, true));
    let plan = plan_query(
        &ctx,
        "SELECT name, time_bin, measure FROM source ORDER BY name, time_bin",
    )
    .await;
    let lex = lex_ordering_name_time_bin(&spike_schema());
    let satisfied = plan
        .properties()
        .equivalence_properties()
        .ordering_satisfy(lex)
        .expect("ordering_satisfy should not error");
    assert!(
        satisfied,
        "a top-level ORDER BY matching the declared columns must satisfy them, got:\n{}",
        displayable(plan.as_ref()).indent(true)
    );
}

/// Registers an empty dimension table for the enrichment-join tests below.
async fn register_dim(ctx: &SessionContext) {
    ctx.sql("CREATE TABLE dim (name VARCHAR, label VARCHAR)")
        .await
        .expect("create dim")
        .collect()
        .await
        .expect("create dim");
}

const AGG_SUBQUERY: &str =
    "SELECT name, time_bin, sum(measure) AS total FROM source GROUP BY name, time_bin";

#[tokio::test]
async fn enrichment_join_with_the_ordered_side_on_the_build_side_reinstates_a_blocking_sort() {
    // The naturally-phrased enrichment join is a trap. `repartition_joins = false` keeps the join
    // in `CollectLeft` mode, but `CollectLeft` buffers the *left* input and takes its output
    // ordering from the *right* (probe) input. Writing `<ordered aggregate> LEFT JOIN dim` puts
    // the aggregate on the build side: DataFusion collects the entire aggregate result into
    // memory and then has to re-sort, which is exactly the blowup this plan avoids.
    let ctx = streaming_merge_session(PerFileOrderedProvider::new(3, true));
    register_dim(&ctx).await;
    let (_, plan_str) = plan_string(
        &ctx,
        &format!(
            "SELECT a.name, a.time_bin, a.total, d.label FROM ({AGG_SUBQUERY}) a \
             LEFT JOIN dim d ON a.name = d.name ORDER BY a.name, a.time_bin"
        ),
    )
    .await;
    assert!(
        plan_str.contains("mode=CollectLeft") && plan_str.contains("SortExec"),
        "expected the ordered side on the build side to force a blocking re-sort; if DataFusion \
         learned to preserve build-side ordering, the authoring constraint can be relaxed:\n\
         {plan_str}"
    );
}

#[tokio::test]
async fn enrichment_join_with_the_ordered_side_on_the_probe_side_keeps_streaming() {
    // The formulation a view author must use instead: the small dimension table on the left
    // (build) side and the ordered aggregate on the right (probe) side, i.e. `dim JOIN <agg>` or
    // `dim RIGHT JOIN <agg>`. The ordering then flows through the join untouched.
    for join in ["JOIN", "RIGHT JOIN"] {
        let ctx = streaming_merge_session(PerFileOrderedProvider::new(3, true));
        register_dim(&ctx).await;
        let (plan, plan_str) = plan_string(
            &ctx,
            &format!(
                "SELECT a.name, a.time_bin, a.total, d.label FROM dim d {join} ({AGG_SUBQUERY}) a \
                 ON a.name = d.name ORDER BY a.name, a.time_bin"
            ),
        )
        .await;
        assert_eq!(
            plan.properties().output_partitioning().partition_count(),
            1,
            "`dim {join} agg` must stay single-partition:\n{plan_str}"
        );
        assert!(
            !plan_str.contains("SortExec"),
            "`dim {join} agg` must preserve the probe-side ordering without re-sorting, \
             got:\n{plan_str}"
        );
    }
}
