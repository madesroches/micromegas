//! The normalized shape of a DDL-defined (or seeded) materialized view -- the three queries plus
//! every option -- and the one `validate_view_definition` function shared by the DDL executor
//! (`rust/public/src/servers/view_ddl.rs`) and the registry loader (`view_registry.rs`), so a
//! definition the loader would skip can never be accepted at `CREATE` time in the first place.

use super::{
    lakehouse_context::LakehouseContext,
    materialized_view::MaterializedView,
    partition_cache::NullPartitionProvider,
    query::make_session_context,
    read_scope::CallerContext,
    session_configurator::{NoOpSessionConfigurator, SessionConfigurator},
    sql_batch_view::{SqlBatchView, guarded_sql_options},
    sql_partition_spec::plan_sorted_extract,
    view::{ScanSortColumn, View},
    view_factory::ViewFactory,
};
use crate::time::TimeRange;
use anyhow::{Context, Result};
use chrono::{TimeDelta, Utc};
use datafusion::{
    arrow::datatypes::{DataType, Schema, TimeUnit},
    common::{
        TableReference,
        tree_node::{TreeNode, TreeNodeRecursion},
    },
    datasource::{DefaultTableSource, MemTable},
    execution::runtime_env::RuntimeEnv,
    logical_expr::{Expr, LogicalPlan, TableScan, Volatility, expr::ScalarFunction},
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The non-query, non-`update_group` options a `ViewDefinition` carries, serialized as JSON in
/// `lakehouse_view_set_definitions.view_options`. `update_group` lives in its own column instead
/// (reload() sorts and the registry loader projects on it), and `definition_sql` is the verbatim
/// DDL text kept for display/audit only -- neither belongs here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewOptions {
    pub min_time_column: String,
    pub max_time_column: String,
    pub source_partition_delta: String,
    pub merge_partition_delta: String,
    pub merge_sort_order: Option<Vec<String>>,
}

/// The normalized shape of a `CREATE MATERIALIZED VIEW` definition -- every option plus the three
/// queries -- independent of whether it arrived over DDL or was seeded by a migration
/// (`log_stats_view.rs`'s `log_stats_view_definition`).
#[derive(Debug, Clone, PartialEq)]
pub struct ViewDefinition {
    pub view_set_name: String,
    pub extract_query: String,
    pub count_src_query: String,
    pub merge_partitions_query: String,
    pub update_group: i32,
    pub options: ViewOptions,
}

/// Parses a `"<n> <unit>"` string (e.g. `"1 day"`, `"2 hours"`) into a `TimeDelta`. Accepted units,
/// singular or plural: `second(s)`, `minute(s)`, `hour(s)`, `day(s)`.
pub fn parse_time_delta(s: &str) -> Result<TimeDelta> {
    let s = s.trim();
    let (num_str, unit) = s
        .split_once(char::is_whitespace)
        .with_context(|| format!("parse_time_delta: expected '<n> <unit>', got {s:?}"))?;
    let n: i64 = num_str
        .trim()
        .parse()
        .with_context(|| format!("parse_time_delta: invalid integer {num_str:?} in {s:?}"))?;
    if n <= 0 {
        anyhow::bail!("parse_time_delta: expected a positive integer, got {n} in {s:?}");
    }
    match unit.trim().to_lowercase().as_str() {
        "second" | "seconds" => Ok(TimeDelta::seconds(n)),
        "minute" | "minutes" => Ok(TimeDelta::minutes(n)),
        "hour" | "hours" => Ok(TimeDelta::hours(n)),
        "day" | "days" => Ok(TimeDelta::days(n)),
        other => anyhow::bail!("parse_time_delta: unknown unit {other:?} in {s:?}"),
    }
}

/// Fast, redundant, name-only pre-check -- also enforced, exhaustively, by check 1 of
/// [`validate_view_definition`], which remains the sole enforcement point since it is the only
/// check that also runs on rows the registry loader reads back. `^[a-z_][a-z0-9_]{0,254}$`, not
/// starting with `__` (which would collide with a view's own `__<name>__partitions` internal
/// registration), and not `source` (substituted for `{source}` in every merge query and registered
/// on every merge's session context).
pub fn check_view_set_name_charset(name: &str) -> Result<()> {
    let bytes = name.as_bytes();
    let valid_charset = !bytes.is_empty()
        && bytes.len() <= 255
        && matches!(bytes[0], b'a'..=b'z' | b'_')
        && bytes
            .iter()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_'));
    if !valid_charset {
        anyhow::bail!("view set name {name:?} must match ^[a-z_][a-z0-9_]{{0,254}}$");
    }
    if name.starts_with("__") {
        anyhow::bail!(
            "view set name {name:?} must not start with '__' (reserved for internal partition tables)"
        );
    }
    if name == "source" {
        anyhow::bail!(
            "view set name 'source' is reserved (substituted for '{{source}}' in merge queries)"
        );
    }
    Ok(())
}

/// Builds the `SqlBatchView` a `ViewDefinition` describes. This *is* most of the validation --
/// planning the extract query catches syntax errors, unknown tables and unknown columns, and
/// yields the schema every later check inspects -- so callers (the DDL executor, the registry
/// loader) must call this before [`validate_view_definition`], never after.
pub async fn build_sql_batch_view(
    def: &ViewDefinition,
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
    session_configurator: Arc<dyn SessionConfigurator>,
) -> Result<SqlBatchView> {
    let source_delta = parse_time_delta(&def.options.source_partition_delta)
        .with_context(|| format!("{}: source_partition_delta", def.view_set_name))?;
    let merge_delta = parse_time_delta(&def.options.merge_partition_delta)
        .with_context(|| format!("{}: merge_partition_delta", def.view_set_name))?;
    let view = SqlBatchView::new(
        runtime,
        Arc::new(def.view_set_name.clone()),
        Arc::new(def.options.min_time_column.clone()),
        Arc::new(def.options.max_time_column.clone()),
        Arc::new(def.count_src_query.clone()),
        Arc::new(def.extract_query.clone()),
        Arc::new(def.merge_partitions_query.clone()),
        lake,
        view_factory,
        session_configurator,
        Some(def.update_group),
        source_delta,
        merge_delta,
    )
    .await
    .with_context(|| format!("building SqlBatchView for '{}'", def.view_set_name))?;
    match &def.options.merge_sort_order {
        Some(columns) if !columns.is_empty() => view
            .with_merge_sort_order(columns.iter().map(|c| Arc::new(c.clone())).collect())
            .with_context(|| format!("'{}': with_merge_sort_order", def.view_set_name)),
        _ => Ok(view),
    }
}

fn is_stringy(dt: &DataType) -> bool {
    matches!(dt, DataType::Utf8)
        || matches!(dt, DataType::Dictionary(_, inner) if **inner == DataType::Utf8)
}

/// Check 2: mirrors `OwnershipRewrite::predicate_for`'s own precedence -- an `audience` field, if
/// present, governs regardless of whether a correctly-typed `process_id` field sits beside it.
fn check_audience_reachable(schema: &Schema) -> Result<()> {
    if let Ok(field) = schema.field_with_name("audience") {
        if !is_stringy(field.data_type()) {
            anyhow::bail!(
                "extract_query's 'audience' column must be Utf8 or Dictionary(_, Utf8), got {:?}; \
                 project it as e.g. arrow_cast(max(audience), 'Dictionary(Int32, Utf8)')",
                field.data_type()
            );
        }
        return Ok(());
    }
    if let Ok(field) = schema.field_with_name("process_id") {
        if !is_stringy(field.data_type()) {
            anyhow::bail!(
                "extract_query's 'process_id' column must be Utf8 or Dictionary(_, Utf8), got {:?}",
                field.data_type()
            );
        }
        return Ok(());
    }
    anyhow::bail!(
        "extract_query's schema must carry an 'audience' column (preferred) or a 'process_id' \
         column, so non-admin callers can be filtered by audience -- otherwise the view would \
         silently serve every row to every caller with no error anywhere"
    );
}

/// Check 9: the resolved time column must exist and be a nanosecond timestamp.
fn check_time_column(schema: &Schema, column: &str) -> Result<()> {
    let field = schema
        .field_with_name(column)
        .with_context(|| format!("time column '{column}' not found in extract_query's schema"))?;
    if !matches!(
        field.data_type(),
        DataType::Timestamp(TimeUnit::Nanosecond, _)
    ) {
        anyhow::bail!(
            "time column '{column}' must be Timestamp(Nanosecond, _), got {:?}",
            field.data_type()
        );
    }
    Ok(())
}

/// Check 4: `fetch_sql_partition_spec` requires a `count` field reachable as an `Int64Array`.
fn check_count_query_schema(schema: &Schema) -> Result<()> {
    let field = schema
        .field_with_name("count")
        .with_context(|| "count_src_query must project a 'count' column")?;
    if field.data_type() != &DataType::Int64 {
        anyhow::bail!(
            "count_src_query's 'count' column must be Int64, got {:?}",
            field.data_type()
        );
    }
    Ok(())
}

/// Check 3's schema-agreement half: field names, types and order must match -- nullability and
/// metadata are deliberately excluded (a `count(*)` extract column is non-nullable while its
/// `sum(count)` merge counterpart is nullable, which the seeded `log_stats` calibration case
/// exercises).
fn check_schema_agrees(extract_schema: &Schema, merge_schema: &Schema) -> Result<()> {
    let extract_fields: Vec<(&str, &DataType)> = extract_schema
        .fields()
        .iter()
        .map(|f| (f.name().as_str(), f.data_type()))
        .collect();
    let merge_fields: Vec<(&str, &DataType)> = merge_schema
        .fields()
        .iter()
        .map(|f| (f.name().as_str(), f.data_type()))
        .collect();
    if extract_fields != merge_fields {
        anyhow::bail!(
            "merge_partitions_query's output schema {merge_fields:?} disagrees with \
             extract_query's {extract_fields:?} (name, type and order must match; nullability and \
             metadata are ignored)"
        );
    }
    Ok(())
}

/// Check 6: walks every `ScalarUDF` reachable from `plan` -- not just its top-level expressions,
/// so a call nested inside another expression (e.g. `now()` inside `date_bin('1 minute',
/// now())`) is still found -- and rejects the first whose `signature().volatility` is not
/// `Immutable`.
fn check_no_volatile_functions(plan: &LogicalPlan, label: &str) -> Result<()> {
    let mut offender: Option<String> = None;
    plan.apply_with_subqueries(|node| {
        node.apply_expressions(|expr| {
            expr.apply(|e| {
                if offender.is_none()
                    && let Expr::ScalarFunction(ScalarFunction { func, .. }) = e
                    && func.signature().volatility != Volatility::Immutable
                {
                    offender = Some(func.name().to_string());
                }
                Ok(TreeNodeRecursion::Continue)
            })
        })
    })?;
    if let Some(name) = offender {
        anyhow::bail!(
            "{label} calls '{name}', a non-immutable function; its result would be frozen into a \
             partition at materialization time (or, in count_src_query, corrupt freshness \
             detection) instead of being recomputed"
        );
    }
    Ok(())
}

/// The four mutating admin-gated table functions plus the three unconditionally-registered but
/// still-mutating ones (see check 7's rationale) -- a stored scan against any of these would be
/// re-executed under `CallerContext::maintenance()` on every daemon tick.
const MUTATING_TABLE_FUNCTIONS: &[&str] = &[
    "retire_partitions",
    "materialize_partitions",
    "regenerate_partitions",
    "deny_queries",
    "view_instance",
    "process_spans",
    "perfetto_trace_chunks",
];

/// Check 7: walks every `TableScan` reachable from `plan`, resolving each to a view set (to
/// accumulate the highest `update_group` read) or to a banned table function name.
///
/// Returns a `datafusion::error::Result` (rather than this module's usual `anyhow::Result`) since
/// it is called from inside `LogicalPlan::apply_with_subqueries`'s closure, which requires that
/// exact type; callers outside that closure convert via `?` as usual (anyhow implements `From`
/// for any `std::error::Error`, which `DataFusionError` is).
fn walk_plan_for_scans(
    plan: &LogicalPlan,
    max_group: &mut Option<i32>,
    mutating_hit: &mut Option<String>,
) -> datafusion::error::Result<()> {
    plan.apply_with_subqueries(|node| {
        if let LogicalPlan::TableScan(ts) = node {
            inspect_scan(ts, max_group, mutating_hit)?;
        }
        Ok(TreeNodeRecursion::Continue)
    })?;
    Ok(())
}

fn inspect_scan(
    ts: &TableScan,
    max_group: &mut Option<i32>,
    mutating_hit: &mut Option<String>,
) -> datafusion::error::Result<()> {
    // Checked first, and unconditionally: `view_instance` resolves to a `MaterializedView` just
    // like a well-behaved DDL view scan does (see `ViewInstanceTableFunction::call_with_args`),
    // so the name check must run before the downcast branch below would otherwise `return Ok(())`
    // and hide it.
    // DataFusion's `TableFactor::Table` path (the one `foo('a', 'b')` call syntax actually takes,
    // as opposed to the `TABLE(foo(...))` / `LATERAL` `TableFactor::Function` form) names the
    // resulting scan `"{name}()"`, not `"{name}"` -- strip that suffix so the comparison below
    // matches either form.
    let raw_name = ts.table_name.table();
    let leaf = raw_name
        .strip_suffix("()")
        .unwrap_or(raw_name)
        .to_lowercase();
    if mutating_hit.is_none() && MUTATING_TABLE_FUNCTIONS.contains(&leaf.as_str()) {
        *mutating_hit = Some(leaf);
        return Ok(());
    }
    if let Some(default_source) = ts.source.downcast_ref::<DefaultTableSource>()
        && let Some(mat_view) = default_source
            .table_provider
            .downcast_ref::<MaterializedView>()
    {
        if let Some(group) = mat_view.get_view().get_update_group() {
            *max_group = Some(max_group.map_or(group, |m| m.max(group)));
        }
        return Ok(());
    }
    // A scan against a DDL/base SqlBatchView's user-visible name (not its
    // `__<name>__partitions` form) resolves through a view-backed logical plan rather than a
    // `MaterializedView` directly -- recurse into it to find the real scan.
    if let Some(inner) = ts.source.get_logical_plan() {
        return walk_plan_for_scans(&inner, max_group, mutating_hit);
    }
    Ok(())
}

/// Builds a session context mirroring `SqlBatchView::new`'s own -- `CallerContext::maintenance()`
/// over `factory`, with the real `session_configurator` -- for checks that must see exactly what
/// the daemon will plan against (extract_query, count_src_query, and the `table_exist` probe).
async fn build_full_validation_ctx(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    factory: Arc<ViewFactory>,
    session_configurator: Arc<dyn SessionConfigurator>,
) -> Result<datafusion::execution::context::SessionContext> {
    let lakehouse = Arc::new(LakehouseContext::new(lake, runtime)?);
    make_session_context(
        lakehouse,
        Arc::new(NullPartitionProvider {}),
        None,
        factory,
        session_configurator,
        CallerContext::maintenance(),
    )
    .await
    .with_context(|| "make_session_context (full validation context)")
}

/// Builds the deliberately narrower session context check 3 plans `merge_partitions_query`
/// against: a non-admin caller (`CallerContext::internal()`) and `NoOpSessionConfigurator`
/// regardless of what real configurator the deployment uses -- so a merge query naming an
/// admin-gated UDTF or a `SessionConfigurator`-registered static table fails to plan here, for
/// that reason, rather than resolving only because validation happened to run under a
/// maintenance/admin caller.
async fn build_narrow_validation_ctx(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    factory: Arc<ViewFactory>,
) -> Result<datafusion::execution::context::SessionContext> {
    let lakehouse = Arc::new(LakehouseContext::new(lake, runtime)?);
    make_session_context(
        lakehouse,
        Arc::new(NullPartitionProvider {}),
        None,
        factory,
        Arc::new(NoOpSessionConfigurator),
        CallerContext::internal(),
    )
    .await
    .with_context(|| "make_session_context (narrow validation context)")
}

/// Every check runs before a definition's row is written, in this one function shared by the DDL
/// executor and the registry loader -- so a definition the loader would skip can never be
/// accepted at `CREATE` time in the first place. `view` must already be the result of
/// [`build_sql_batch_view`] over `def` -- constructing it *is* most of the validation (it plans
/// the extract query and yields the schema every check below inspects). `factory` is the factory
/// `view` was built against -- every other definition, for the DDL executor, or only the
/// definitions in a strictly lower `update_group` processed so far, for the registry loader --
/// which check 7's ordering check reads.
pub async fn validate_view_definition(
    def: &ViewDefinition,
    view: &SqlBatchView,
    factory: &Arc<ViewFactory>,
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    session_configurator: Arc<dyn SessionConfigurator>,
) -> Result<()> {
    let schema = view.get_file_schema();

    // Check 1: name.
    check_view_set_name_charset(&def.view_set_name)?;
    if factory.get_global_view(&def.view_set_name).is_some()
        || factory.get_view_sets().contains_key(&def.view_set_name)
    {
        anyhow::bail!(
            "'{}' is a code-driven view set and cannot be redefined by DDL",
            def.view_set_name
        );
    }

    // Check 5: placeholders (purely textual, checked early).
    if !def.count_src_query.contains("{begin}") || !def.count_src_query.contains("{end}") {
        anyhow::bail!("count_src_query must contain both '{{begin}}' and '{{end}}'");
    }
    if !def.merge_partitions_query.contains("{source}") {
        anyhow::bail!("merge_partitions_query must contain '{{source}}'");
    }

    // Check 9: time columns.
    check_time_column(&schema, &def.options.min_time_column)?;
    check_time_column(&schema, &def.options.max_time_column)?;

    // Check 2: audience reachability.
    check_audience_reachable(&schema)?;

    let full_ctx = build_full_validation_ctx(
        runtime.clone(),
        lake.clone(),
        factory.clone(),
        session_configurator.clone(),
    )
    .await?;
    if full_ctx
        .table_exist(def.view_set_name.as_str())
        .unwrap_or(false)
    {
        anyhow::bail!(
            "'{}' collides with a table already registered in the session (a static table?)",
            def.view_set_name
        );
    }

    // Check 3: merge query plans, and its output schema agrees with the extract schema.
    let narrow_ctx =
        build_narrow_validation_ctx(runtime.clone(), lake.clone(), factory.clone()).await?;
    let empty_source = Arc::new(
        MemTable::try_new(schema.clone(), vec![vec![]])
            .with_context(|| "building the empty source table for merge-query validation")?,
    );
    narrow_ctx
        .register_table(
            TableReference::Bare {
                table: "source".into(),
            },
            empty_source,
        )
        .with_context(|| "registering the empty source table")?;
    let merge_sql = def.merge_partitions_query.replace("{source}", "source");
    let merge_df = narrow_ctx
        .sql_with_options(&merge_sql, guarded_sql_options())
        .await
        .with_context(|| {
            format!(
                "planning merge_partitions_query for '{}'",
                def.view_set_name
            )
        })?;
    check_schema_agrees(&schema, merge_df.schema().as_arrow())?;

    // The `{begin}`/`{end}`-substituted extract/count query texts checks 4/6/7/8 all plan
    // against. A zero-width range is enough -- these builds are never executed.
    let now = Utc::now();
    let now_str = now.to_rfc3339();
    let extract_sql = def
        .extract_query
        .replace("{begin}", &now_str)
        .replace("{end}", &now_str);
    let count_sql = def
        .count_src_query
        .replace("{begin}", &now_str)
        .replace("{end}", &now_str);

    let extract_df = full_ctx
        .sql_with_options(&extract_sql, guarded_sql_options())
        .await
        .with_context(|| format!("planning extract_query for '{}'", def.view_set_name))?;
    let count_df = full_ctx
        .sql_with_options(&count_sql, guarded_sql_options())
        .await
        .with_context(|| format!("planning count_src_query for '{}'", def.view_set_name))?;

    // Check 4: count query shape.
    check_count_query_schema(count_df.schema().as_arrow())?;

    // Checks 6 & 7, over the three plans already built above -- no extra planning cost.
    let mut max_group: Option<i32> = None;
    let mut mutating_hit: Option<String> = None;
    for (label, plan) in [
        ("extract_query", extract_df.logical_plan()),
        ("merge_partitions_query", merge_df.logical_plan()),
        ("count_src_query", count_df.logical_plan()),
    ] {
        check_no_volatile_functions(plan, label)?;
        walk_plan_for_scans(plan, &mut max_group, &mut mutating_hit)?;
    }
    if let Some(name) = mutating_hit {
        anyhow::bail!(
            "'{}' scans '{name}', a mutating or session-scoped table function, which cannot be \
             stored in a materialized-view definition",
            def.view_set_name
        );
    }
    if let Some(max_group) = max_group
        && def.update_group <= max_group
    {
        anyhow::bail!(
            "'{}' has update_group {}, which must be strictly greater than {max_group}, the \
             highest update_group among the view sets it reads",
            def.view_set_name,
            def.update_group
        );
    }

    // Check 8: merge_sort_order, if declared, applies and plans against extract_query.
    if let Some(columns) = &def.options.merge_sort_order {
        let scan_columns: Vec<ScanSortColumn> = columns
            .iter()
            .map(|c| ScanSortColumn {
                column: Arc::new(c.clone()),
                descending: false,
            })
            .collect();
        let subject = format!("validation of '{}'", def.view_set_name);
        let extract_df_for_sort = full_ctx
            .sql_with_options(&extract_sql, guarded_sql_options())
            .await
            .with_context(|| format!("re-planning extract_query for '{}'", def.view_set_name))?;
        plan_sorted_extract(
            extract_df_for_sort,
            Some(&scan_columns),
            &subject,
            TimeRange::new(now, now),
        )
        .await
        .with_context(|| {
            format!(
                "'{}': merge_sort_order does not apply to extract_query",
                def.view_set_name
            )
        })?;
    }

    Ok(())
}
