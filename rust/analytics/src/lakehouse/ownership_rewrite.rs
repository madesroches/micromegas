//! `OwnershipRewrite` -- Query Enforcement Prong A (#1370, AbAC Stage 2).
//!
//! An `AnalyzerRule` that injects an audience-filtering predicate into every
//! `MaterializedView`-backed `TableScan` a query plan touches, based on the caller's `ReadScope`
//! (#1369, AbAC Stage 1, `read_scope.rs`) and [`super::read_scope::IsolationConfig`]'s
//! `MICROMEGAS_PUBLIC_VIEW_SETS`. See `tasks/1370_ownership_rewrite_plan.md` for the original
//! design rationale and `tasks/1482_audience_column_plan.md` for the switch, described below,
//! from a `property_get` semi-join to a direct filter on the physical `audience` column; this
//! comment records only what a future reader of this file needs close at hand.
//!
//! ## `ReadScope::All` is a true no-op
//!
//! `ReadScope::All` is the internal/maintenance marker -- never the output of a real `ReadPolicy`
//! (see `read_scope.rs`). `analyze()` returns the plan unchanged when constructed with it, and
//! `query.rs::make_session_context` does not even construct this rule for `ReadScope::All`
//! callers, so their `ViewFactory` need not carry `processes`/`streams` either.
//!
//! ## The physical `audience` column (#1482)
//!
//! `blocks`, `processes`, `streams`, `log_entries`, `measures`, and `log_stats` each carry a
//! physical, non-nullable `audience` column -- extracted once from Postgres at the `blocks`
//! view's materialization and propagated structurally into every downstream view, the same way
//! `processes.properties` already does. Those six views are consequently filtered with a bare
//! `Filter` on that column (§5 below): no semi-join, no `property_get`, no per-process aggregate.
//! `async_events`,
//! `thread_spans`, `net_spans`, `otel_spans`, and `images` don't carry the column yet (out of
//! scope for #1482 -- see that plan's Future Work), so they keep the `process_id`/`EXISTS`
//! machinery below, which still resolves through `__processes__partitions`.
//!
//! ## One audience per process, not per row (surviving branches only)
//!
//! `__processes__partitions` (the raw, un-merged `SqlBatchView` partitions `self.processes_source`
//! scans) can carry more than one historical row per `process_id` -- the partition-level
//! `GROUP BY` in `processes_view.rs`'s transform query only collapses rows *within* a partition,
//! not across a long-lived process's entire history. What changed under #1482: those rows can no
//! longer *disagree* (a process's audience is write-once and always present, §6 of that plan), so
//! filtering the six column-carrying views one row at a time (§5 below) is sound without any
//! aggregate at all. The `net_spans`/`otel_spans`/`images` semi-join (§4) and the
//! `async_events`/`thread_spans` `EXISTS` shapes (§async_events/§thread_spans below) still
//! resolve through
//! `per_process_audience` -- `Aggregate(GROUP BY process_id, MAX(audience) AS resolved_audience)`
//! -- purely because those five views have no `audience` column of their own to filter directly;
//! the aggregate is no longer doing any "reconcile disagreeing rows" work, since #1482 already
//! ruled that out.
//!
//! ## Branch table
//!
//! Keyed on `MaterializedView::get_view().get_file_schema()`: whether it has an `audience` field,
//! failing that whether it has a `process_id` field -- not on a hardcoded view-set list, so a
//! future view set that gains either column falls into the matching branch automatically:
//!
//! | View set | Branch |
//! |---|---|
//! | `processes`, `streams`, `blocks`, `log_entries`, `measures`, `log_stats` | §5 (new): direct `audience IN (...)` filter on the view's own column -- no join |
//! | `net_spans`, `otel_spans`, `images` | §4: semi-join, `process_id IN (subquery)` against `per_process_audience` (outer `process_id` cast to `Utf8`: it is `Dictionary(Int32, Utf8)` in these, and nothing coerces an uncorrelated `IN` subquery's join keys once `DecorrelatePredicateSubquery` turns it into a `LeftSemi` join -- the analyzer's own `TypeCoercion` has already run by the time this rule executes) |
//! | `async_events` | no `process_id` column either -- `§async_events`: literal-valued `EXISTS`, keyed on `get_view_instance_id()` (the process_id string, canonicalized -- see `canonical_view_instance_id`) |
//! | `thread_spans` | no `process_id` **or** `stream_id` column -- `§thread_spans`: two-hop literal `EXISTS`, through `streams` (`get_view_instance_id()` is the stream_id string, canonicalized the same way) |
//! | anything in `public_view_sets` | §7: no predicate at all -- checked before any of the above |
//! | anything else | `analyze()` returns `Err` (`DataFusionError::Plan`) rather than silently leaving the scan unfiltered -- a future view set must add itself to this table, not fall through |
//!
//! Adding a new view set to a `ViewFactory` means adding it to this table too, and to the match
//! arms in [`OwnershipRewrite::predicate_for`] below.
//!
//! ## No I/O in `analyze()`
//!
//! `in_subquery`/`exists` build uncorrelated subquery `Expr`s; `DecorrelatePredicateSubquery` (an
//! optimizer, not analyzer, rule) turns them into joins later. `processes_source`/`streams_source`
//! come from `MaterializedView::new(..., query_range: None)`, which is synchronous and does no I/O
//! -- the actual `processes`/`streams` scans happen during normal execution, exactly like
//! `TableScanRewrite`'s injected time filter.
//!
//! ## `micromegas.audience` is server-written and authenticated (AbAC Stage 5, #1373; #1482)
//!
//! Every process is stamped with `micromegas.audience` at registration -- from the authenticated
//! credential's `AuthContext.bound_audience` when present, or the deployment's
//! `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` otherwise (#1482 §0) -- so there is no more unstamped
//! state: a client can neither assert, suppress, nor omit the stamp. Registration
//! (`insert_process`/`register_otel_process`) rejects a same-`process_id`, different-audience
//! re-registration outright -- needed because the OTLP `process_id` derivation formula is public,
//! so without this guard a credential could pre-register (via the native path) the exact
//! `process_id` a victim audience's OTLP producer would later derive, silently exposing that
//! audience's data to the squatter; this is a confidentiality gap, not merely an integrity one.
//! What remains open, tracked separately: a credential bound to one audience that knows another
//! audience's already-registered `process_id`/`stream_id` can still append events to it over
//! `insert_stream`/`insert_block` (Stage 5b, an integrity-only gap -- see
//! `rust/ingestion/src/web_ingestion_service.rs`'s doc comments on those two methods). There is no
//! in-product enforcement knob left for this gap; the mitigation is operational -- provision
//! only audience-bound DB-backed ingestion credentials, and don't run ingestion with an
//! env-keyring key, OIDC, or `--disable-auth` alongside them -- see the "Residual gap"
//! admonition in `mkdocs/docs/admin/authentication.md` and this stage's `CHANGELOG.md` entry).

use super::{materialized_view::MaterializedView, read_scope::ReadScope};
use datafusion::{
    arrow::datatypes::{DataType, Field},
    common::{Column, tree_node::Transformed},
    config::ConfigOptions,
    datasource::DefaultTableSource,
    error::DataFusionError,
    functions_aggregate::min_max::max,
    logical_expr::{Filter, LogicalPlan, LogicalPlanBuilder, TableSource},
    optimizer::AnalyzerRule,
    prelude::*,
    scalar::ScalarValue,
    sql::TableReference,
};
use std::sync::Arc;
use uuid::Uuid;

/// Injects an audience predicate into every `MaterializedView` scan (see the module doc comment).
pub struct OwnershipRewrite {
    read_scope: ReadScope,
    public_view_sets: Vec<String>,
    processes_source: Arc<dyn TableSource>,
    streams_source: Arc<dyn TableSource>,
}

impl std::fmt::Debug for OwnershipRewrite {
    /// `dyn TableSource` carries no `Debug` impl, so `processes_source`/`streams_source` are
    /// named but not expanded.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnershipRewrite")
            .field("read_scope", &self.read_scope)
            .field("public_view_sets", &self.public_view_sets)
            .field("processes_source", &"<dyn TableSource>")
            .field("streams_source", &"<dyn TableSource>")
            .finish()
    }
}

impl OwnershipRewrite {
    /// `processes_source`/`streams_source` must be built from `MaterializedView::new(...,
    /// query_range: None)` over the raw partitions (equivalent to `__processes__partitions` /
    /// `__streams__partitions`) -- see `query.rs::make_session_context` and Design §2 of
    /// `tasks/1370_ownership_rewrite_plan.md` for why: the audience lookup must be time-unbounded,
    /// and must not go through the `SqlBatchView`-merged `processes`/`streams` named tables. Used
    /// only by the §4/§async_events/§thread_spans branches -- the six column-carrying views (§5)
    /// need neither.
    pub fn new(
        read_scope: ReadScope,
        public_view_sets: Vec<String>,
        processes_source: Arc<dyn TableSource>,
        streams_source: Arc<dyn TableSource>,
    ) -> Self {
        Self {
            read_scope,
            public_view_sets,
            processes_source,
            streams_source,
        }
    }

    /// `Aggregate(GROUP BY process_id, MAX(audience) AS resolved_audience)` over the raw,
    /// time-unbounded `processes` partitions -- built once per `analyze()` call and reused at
    /// every scan site the traversal visits (see the module doc comment). Only reached by the
    /// §4/§async_events/§thread_spans branches now that `processes` itself carries a physical
    /// `audience` column and filters it directly (§5); the aggregate still exists for those
    /// three because they have no `audience` column of their own, not to reconcile disagreeing
    /// rows (#1482 §6 rules that out).
    fn per_process_audience(&self) -> datafusion::error::Result<LogicalPlan> {
        LogicalPlanBuilder::scan(
            "__processes__partitions",
            self.processes_source.clone(),
            None,
        )?
        .aggregate(
            vec![col("process_id")],
            vec![max(col("audience")).alias("resolved_audience")],
        )?
        .build()
    }

    /// The caller's audience set. Only ever called after `analyze()` has short-circuited on
    /// `ReadScope::All`, so the `All` arm here is unreachable in practice; it degrades to "no
    /// audiences" rather than panicking if that invariant is ever violated.
    fn audiences(&self) -> &[String] {
        match &self.read_scope {
            ReadScope::All => &[],
            ReadScope::Audiences(a) => a,
        }
    }

    /// `resolved_audience IN (audiences)`, or `lit(false)` when `audiences` is empty -- the
    /// fail-closed reading of "caller has no audiences" rather than emitting `IN ()` and leaving
    /// its behavior to DataFusion (`ReadScope::Audiences` can legitimately resolve to an empty
    /// set -- a caller matching no grant resolves to `{public}`, but a bare-array read-only
    /// audience with no matching selector contributes nothing). No `coalesce` any more: the
    /// column is `NOT NULL` (#1482 §0), so there is no unstamped case to fall back for.
    fn resolved_predicate(&self) -> Expr {
        let audiences = self.audiences();
        if audiences.is_empty() {
            return lit(false);
        }
        col("resolved_audience").in_list(
            audiences
                .iter()
                .map(|a| lit(ScalarValue::Utf8(Some(a.clone()))))
                .collect(),
            false,
        )
    }

    /// §5 (new, #1482): `audience IN (caller audiences)`; `false` for an empty set (fail-closed,
    /// as [`Self::resolved_predicate`] already does). The column is `NOT NULL`, so there is no
    /// unstamped case.
    fn audience_column_predicate(&self, table_name: &TableReference, field: &Field) -> Expr {
        let audiences = self.audiences();
        if audiences.is_empty() {
            return lit(false);
        }
        let raw = Expr::Column(Column::new(Some(table_name.clone()), "audience"));
        // This rule runs after DataFusion's own TypeCoercion pass, so the Dictionary(Int32, Utf8)
        // column every one of the six views carries must be cast to compare against Utf8
        // literals. The cast is not a pruning barrier: `PruningPredicate` rewrites `cast(col) op
        // lit` by applying the same cast to the column's min/max statistics
        // (`datafusion-pruning`'s `rewrite_expr_to_prunable`), and a Dictionary -> Utf8 cast is on
        // its supported list (`verify_support_type_for_prune` unwraps dictionaries), with `IN`
        // lists up to 20 items expanded to such comparisons. The bare-column arm below is
        // therefore just a no-op-cast skip, kept for a future view that carries plain `Utf8`.
        let lhs = if field.data_type() == &DataType::Utf8 {
            raw
        } else {
            cast(raw, DataType::Utf8)
        };
        lhs.in_list(
            audiences
                .iter()
                .map(|a| lit(ScalarValue::Utf8(Some(a.clone()))))
                .collect(),
            false,
        )
    }

    /// §3/§4's shared `IN`-subquery plan: `per_process_audience` filtered by `resolved_predicate`,
    /// projected down to `process_id` alone -- the one column an `InSubquery` may project.
    fn in_subquery_plan(
        per_process_audience: &LogicalPlan,
        resolved_predicate: &Expr,
    ) -> datafusion::error::Result<Arc<LogicalPlan>> {
        Ok(Arc::new(
            LogicalPlanBuilder::from(per_process_audience.clone())
                .filter(resolved_predicate.clone())?
                .project(vec![col("process_id")])?
                .build()?,
        ))
    }

    /// §async_events's literal-valued `EXISTS`: `per_process_audience` filtered down to the single process
    /// named by `process_id_literal`, conjuncted with `resolved_predicate`. No projection: unlike
    /// `InSubquery`, `EXISTS` has no single-column-projection requirement, so the two-column
    /// (`process_id`, `resolved_audience`) shape is fine.
    fn exists_for_process(
        per_process_audience: &LogicalPlan,
        resolved_predicate: &Expr,
        process_id_literal: &str,
    ) -> datafusion::error::Result<Expr> {
        let subquery = LogicalPlanBuilder::from(per_process_audience.clone())
            .filter(
                col("process_id")
                    .eq(lit(process_id_literal))
                    .and(resolved_predicate.clone()),
            )?
            .build()?;
        Ok(exists(Arc::new(subquery)))
    }

    /// §thread_spans's two-hop literal `EXISTS`: resolve `stream_id_literal` (the `thread_spans`
    /// `view_instance_id`) through `streams` into the process it belongs to, then apply the same
    /// `per_process_audience`/`resolved_predicate` check §5 uses directly.
    fn exists_for_stream(
        &self,
        per_process_audience: &LogicalPlan,
        resolved_predicate: &Expr,
        stream_id_literal: &str,
    ) -> datafusion::error::Result<Expr> {
        let subquery =
            LogicalPlanBuilder::scan("__streams__partitions", self.streams_source.clone(), None)?
                .filter(col("stream_id").eq(lit(stream_id_literal)))?
                .join(
                    LogicalPlanBuilder::from(per_process_audience.clone())
                        .filter(resolved_predicate.clone())?
                        .build()?,
                    JoinType::Inner,
                    (vec!["process_id"], vec!["process_id"]),
                    None,
                )?
                .build()?;
        Ok(exists(Arc::new(subquery)))
    }

    /// Parses `view.get_view_instance_id()` as a `Uuid` and returns its canonical
    /// lowercase-hyphenated string form -- the form actually stored in `process_id`/`stream_id`
    /// columns. `Uuid::parse_str` accepts several equivalent textual spellings (uppercase,
    /// hyphen-less, braced) of the same UUID, but
    /// only this canonical form round-trips through the data; using the raw, caller-supplied
    /// spelling directly in a literal comparison would silently fail to match for any other
    /// spelling of a legitimately materialized instance id. Defensive `Err` on parse failure --
    /// the view constructors (`AsyncEventsView::new`/`ThreadSpansView::new`) already validate this
    /// as a UUID at construction time, so this should not happen in practice.
    fn canonical_view_instance_id(
        view_set_name: &Arc<String>,
        view: &Arc<dyn super::view::View>,
    ) -> datafusion::error::Result<String> {
        let raw = view.get_view_instance_id();
        Uuid::parse_str(raw.as_str())
            .map(|uuid| uuid.hyphenated().to_string())
            .map_err(|e| {
                DataFusionError::Plan(format!(
                    "OwnershipRewrite: view set '{view_set_name}' view_instance_id '{raw}' is \
                     not a valid UUID: {e}"
                ))
            })
    }

    /// Builds the predicate to wrap `mat_view`'s scan in. `Ok(None)` means "no predicate at all"
    /// (§7's public-view-set skip); `Err` is §7's fallback for a view set matching none of the
    /// branches -- see the module doc comment's branch table.
    fn predicate_for(
        &self,
        table_name: &TableReference,
        mat_view: &MaterializedView,
        per_process_audience: &LogicalPlan,
        resolved_predicate: &Expr,
        in_subquery_plan: &Arc<LogicalPlan>,
    ) -> datafusion::error::Result<Option<Expr>> {
        let view = mat_view.get_view();
        let view_set_name = view.get_view_set_name();
        if self
            .public_view_sets
            .iter()
            .any(|s| s.as_str() == view_set_name.as_str())
        {
            return Ok(None);
        }
        // §5 (new, #1482): views carrying a physical `audience` column -- processes, streams,
        // blocks, log_entries, measures, log_stats (global and per-process instances alike).
        // Filtered directly, no semi-join, no property_get. Checked ahead of §4 so a view set
        // that has *both* an `audience` and a `process_id` column (all six do) takes this
        // cheaper branch; keyed on schema introspection, so a view set that gains the column
        // later (the JIT span/image views, see the plan's Future Work) upgrades automatically
        // with no edit here.
        if let Ok(field) = view.get_file_schema().field_with_name("audience") {
            return Ok(Some(self.audience_column_predicate(table_name, field)));
        }
        // Qualified with the outer scan's own table name: once `DecorrelatePredicateSubquery`
        // joins this scan with `in_subquery_plan`'s subquery, the combined schema holds a
        // `process_id` column from both sides, so a bare, unqualified `col("process_id")` is
        // ambiguous and `into_optimized_plan()` fails.
        let outer_process_id = Expr::Column(Column::new(Some(table_name.clone()), "process_id"));
        if view.get_file_schema().field_with_name("process_id").is_ok() {
            // §4: process_id-**column** views with no `audience` column of their own --
            // net_spans, otel_spans, images.
            return Ok(Some(in_subquery(
                cast(outer_process_id, DataType::Utf8),
                in_subquery_plan.clone(),
            )));
        }
        if view_set_name.as_str() == "async_events" {
            // §async_events: process-scoped, no process_id column -- the view_instance_id *is*
            // the process_id string (`AsyncEventsView::new` parses it as a `Uuid`). Canonicalized
            // before building the literal -- see `canonical_view_instance_id`'s doc comment for why.
            let process_id_literal = Self::canonical_view_instance_id(&view_set_name, &view)?;
            return Ok(Some(Self::exists_for_process(
                per_process_audience,
                resolved_predicate,
                process_id_literal.as_str(),
            )?));
        }
        if view_set_name.as_str() == "thread_spans" {
            // §thread_spans: stream-scoped, no process_id or stream_id column -- resolve through
            // `streams`. Same canonicalization as §async_events, keyed on `stream_id` instead of
            // `process_id`.
            let stream_id_literal = Self::canonical_view_instance_id(&view_set_name, &view)?;
            return Ok(Some(self.exists_for_stream(
                per_process_audience,
                resolved_predicate,
                stream_id_literal.as_str(),
            )?));
        }
        Err(DataFusionError::Plan(format!(
            "OwnershipRewrite: no audience rule defined for view set '{view_set_name}'"
        )))
    }

    fn rewrite_plan(
        &self,
        plan: LogicalPlan,
        per_process_audience: &LogicalPlan,
        resolved_predicate: &Expr,
        in_subquery_plan: &Arc<LogicalPlan>,
    ) -> datafusion::error::Result<Transformed<LogicalPlan>> {
        let LogicalPlan::TableScan(ts) = &plan else {
            return Ok(Transformed::no(plan));
        };
        let Some(table_source) = ts.source.downcast_ref::<DefaultTableSource>() else {
            return Ok(Transformed::no(plan));
        };
        let Some(mat_view) = table_source
            .table_provider
            .downcast_ref::<MaterializedView>()
        else {
            // Not a MaterializedView (e.g. a table function's output) -- nothing to filter.
            return Ok(Transformed::no(plan));
        };
        let Some(predicate) = self.predicate_for(
            &ts.table_name,
            mat_view,
            per_process_audience,
            resolved_predicate,
            in_subquery_plan,
        )?
        else {
            // §7: public view set -- no predicate.
            return Ok(Transformed::no(plan));
        };
        let filter = Filter::try_new(predicate, Arc::new(plan.clone()))?;
        Ok(Transformed::yes(LogicalPlan::Filter(filter)))
    }
}

impl AnalyzerRule for OwnershipRewrite {
    fn name(&self) -> &str {
        "ownership_rewrite"
    }

    fn analyze(
        &self,
        plan: LogicalPlan,
        _options: &ConfigOptions,
    ) -> datafusion::error::Result<LogicalPlan> {
        if self.read_scope == ReadScope::All {
            // The internal/maintenance marker -- never a ReadPolicy's output. A true no-op, not
            // "filter to a set containing everything" (see the module doc comment).
            return Ok(plan);
        }
        let per_process_audience = self.per_process_audience()?;
        let resolved_predicate = self.resolved_predicate();
        let in_subquery_plan = Self::in_subquery_plan(&per_process_audience, &resolved_predicate)?;
        plan.transform_up_with_subqueries(|node| {
            self.rewrite_plan(
                node,
                &per_process_audience,
                &resolved_predicate,
                &in_subquery_plan,
            )
        })
        .map(|res| res.data)
    }
}
