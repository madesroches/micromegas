//! `OwnershipRewrite` -- Query Enforcement Prong A (#1370, AbAC Stage 2).
//!
//! An `AnalyzerRule` that injects an audience-filtering predicate into every
//! `MaterializedView`-backed `TableScan` a query plan touches, based on the caller's `ReadScope`
//! (#1369, AbAC Stage 1, `read_scope.rs`) and the two deployment knobs bundled in
//! [`super::read_scope::IsolationConfig`] (`MICROMEGAS_UNSTAMPED_AUDIENCE`,
//! `MICROMEGAS_PUBLIC_VIEW_SETS`). See `tasks/1370_ownership_rewrite_plan.md` for the full design
//! rationale; this comment records only what a future reader of this file needs close at hand.
//!
//! ## `ReadScope::All` is a true no-op
//!
//! `ReadScope::All` is the internal/maintenance marker -- never the output of a real `ReadPolicy`
//! (see `read_scope.rs`). `analyze()` returns the plan unchanged when constructed with it, and
//! `query.rs::make_session_context` does not even construct this rule for `ReadScope::All`
//! callers, so their `ViewFactory` need not carry `processes`/`streams` either.
//!
//! ## One audience per process, not per row
//!
//! `__processes__partitions` (the raw, un-merged `SqlBatchView` partitions `self.processes_source`
//! scans) can carry more than one historical row per `process_id` -- the partition-level
//! `GROUP BY` in `processes_view.rs`'s transform query only collapses rows *within* a partition,
//! not across a long-lived process's entire history. Filtering those raw rows directly (a bare
//! `WHERE <audience predicate>`) would let a process admitted once, by any one of its historical
//! (possibly pre-stamping, unstamped) rows, stay visible forever to whatever audience that row
//! happened to carry -- even after the process is later stamped with a real, narrower audience.
//! Every branch below therefore resolves one audience per process first
//! (`Aggregate(GROUP BY process_id, MAX(audience) AS resolved_audience)`, `MAX` over a nullable
//! column ignoring `NULL`s so a stamped row always outranks an unstamped one), then filters
//! *that* -- uniformly, including `processes`'s own scan, which gets no separate per-row branch.
//! This assumes a process is stamped with at most one distinct audience over its lifetime (true
//! under Stage 5's design). Stage 3 (#1371, `audience_guard.rs`) landed without revisiting this --
//! Prong B resolves straight from Postgres's current row, so the assumption doesn't even arise
//! there the way it does for this rule's own historical, multi-row aggregate.
//!
//! ## Branch table
//!
//! Keyed on `MaterializedView::get_view().get_view_set_name()` and whether
//! `get_file_schema()` has a `process_id` field -- not on a hardcoded view-set list, so a future
//! view set with a `process_id` column falls into §4 automatically:
//!
//! | View set | `process_id` column? | Branch |
//! |---|---|---|
//! | `processes` | yes (and *is* the audience source) | §3: `process_id IN (subquery)` against its own resolved-per-process aggregate |
//! | `streams`, `blocks`, `log_entries`, `measures`, `net_spans`, `otel_spans`, `images`, `log_stats` | yes | §4: semi-join, `process_id IN (subquery)` (outer `process_id` cast to `Utf8`: it is `Dictionary(Int32, Utf8)` in most of these, and nothing coerces an uncorrelated `IN` subquery's join keys once `DecorrelatePredicateSubquery` turns it into a `LeftSemi` join -- the analyzer's own `TypeCoercion` has already run by the time this rule executes) |
//! | `async_events` | no -- process-scoped, but no column to join on | §5: literal-valued `EXISTS`, keyed on `get_view_instance_id()` (the process_id string, canonicalized -- see `canonical_view_instance_id`) |
//! | `thread_spans` | no -- stream-scoped, no `process_id` **or** `stream_id` column | §6: two-hop literal `EXISTS`, through `streams` (`get_view_instance_id()` is the stream_id string, canonicalized the same way) |
//! | anything in `public_view_sets` | (any) | §7: no predicate at all -- checked before any of the above |
//! | anything else | (any) | `analyze()` returns `Err` (`DataFusionError::Plan`) rather than silently leaving the scan unfiltered -- a future view set must add itself to this table, not fall through |
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
//! ## `micromegas.audience` is now server-written and authenticated (AbAC Stage 5, #1373, landed)
//!
//! [`Self::audience_col`] reads `micromegas.audience` via `property_get` off the `processes`
//! view's `properties` column. Before Stage 5 this was a verbatim snapshot of whatever
//! `ProcessInfo.properties` the instrumented client sent at ingestion, with no reserved-key
//! filtering and no server-side validation. Stage 5 closed that gap: ingestion now strips any
//! client-supplied `micromegas.*` property and writes `micromegas.audience` itself from the
//! authenticated credential's `AuthContext.bound_audience` (each ingestion key is assigned
//! exactly one write audience, Stage 4, #1372) -- a client can no longer assert or suppress the
//! stamp. Registration itself (`insert_process`/`register_otel_process`) now also rejects a
//! same-`process_id`, different-audience re-registration outright -- needed because the OTLP
//! `process_id` derivation formula is public, so without this guard a credential could
//! pre-register (via the native path) the exact `process_id` a victim audience's OTLP producer
//! would later derive, silently exposing that audience's data to the squatter; this is a
//! confidentiality gap, not merely an integrity one. What remains open, tracked separately: a
//! credential bound to one audience that knows another audience's already-registered
//! `process_id`/`stream_id` can still append events to it over `insert_stream`/`insert_block`
//! (Stage 5b, an integrity-only gap -- see `rust/ingestion/src/web_ingestion_service.rs`'s doc
//! comments on those two methods). A second, distinct residual gap sits in the conflict guard
//! itself: its existing-`NULL`-audience branch is deliberately a no-op (so a mid-migration
//! re-registration doesn't lose its process), which lets an audience-less credential
//! pre-register a victim's future `process_id` unstamped -- the victim's later, genuine
//! registration then hits that same `NULL`→no-op branch and never gets stamped, permanently
//! suppressing its audience (a confidentiality gap, not an integrity one, and closed only by
//! `{prefix}_REQUIRE_WRITE_AUDIENCE=true` rejecting the audience-less write up front -- see the
//! "Residual gap" admonition in `mkdocs/docs/admin/authentication.md` and this stage's
//! `CHANGELOG.md` entry).

use super::{
    audience_guard::AUDIENCE_PROPERTY, materialized_view::MaterializedView, read_scope::ReadScope,
};
use datafusion::{
    arrow::datatypes::DataType,
    common::{Column, tree_node::Transformed},
    config::ConfigOptions,
    datasource::DefaultTableSource,
    error::DataFusionError,
    functions_aggregate::min_max::max,
    logical_expr::{Filter, LogicalPlan, LogicalPlanBuilder, ScalarUDF, TableSource},
    optimizer::AnalyzerRule,
    prelude::*,
    scalar::ScalarValue,
    sql::TableReference,
};
use micromegas_datafusion_extensions::properties::property_get::PropertyGet;
use std::sync::Arc;
use uuid::Uuid;

/// Injects an audience predicate into every `MaterializedView` scan (see the module doc comment).
pub struct OwnershipRewrite {
    read_scope: ReadScope,
    unstamped_audience: Option<String>,
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
            .field("unstamped_audience", &self.unstamped_audience)
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
    /// and must not go through the `SqlBatchView`-merged `processes`/`streams` named tables.
    pub fn new(
        read_scope: ReadScope,
        unstamped_audience: Option<String>,
        public_view_sets: Vec<String>,
        processes_source: Arc<dyn TableSource>,
        streams_source: Arc<dyn TableSource>,
    ) -> Self {
        Self {
            read_scope,
            unstamped_audience,
            public_view_sets,
            processes_source,
            streams_source,
        }
    }

    /// `property_get(properties, AUDIENCE_PROPERTY)`, cast to `Utf8`. `property_get` returns
    /// `Dictionary(Int32, Utf8)` (`property_get.rs`), and this rule runs strictly *after*
    /// DataFusion's built-in `TypeCoercion` analyzer pass (`Analyzer::new()`'s only two built-ins
    /// are `[ResolveGroupingFunction, TypeCoercion]`), so nothing coerces the expressions it
    /// injects -- every one of them is explicitly typed here instead. The property name itself is
    /// [`AUDIENCE_PROPERTY`] (`audience_guard.rs`), Prong B's constant too -- one definition
    /// shared by both prongs, rather than each inlining its own copy of the literal.
    fn audience_col() -> Expr {
        cast(
            ScalarUDF::from(PropertyGet::new())
                .call(vec![col("properties"), lit(AUDIENCE_PROPERTY)]),
            DataType::Utf8,
        )
    }

    /// `Aggregate(GROUP BY process_id, MAX(audience_col) AS resolved_audience)` over the raw,
    /// time-unbounded `processes` partitions -- built once per `analyze()` call and reused at
    /// every scan site the traversal visits (see the module doc comment).
    fn per_process_audience(&self) -> datafusion::error::Result<LogicalPlan> {
        LogicalPlanBuilder::scan(
            "__processes__partitions",
            self.processes_source.clone(),
            None,
        )?
        .aggregate(
            vec![col("process_id")],
            vec![max(Self::audience_col()).alias("resolved_audience")],
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

    /// `coalesce(resolved_audience, unstamped_audience) IN (audiences)`, or `lit(false)` when
    /// `audiences` is empty -- the fail-closed reading of "caller has no audiences" rather than
    /// emitting `IN ()` and leaving its behavior to DataFusion (`ReadScope::Audiences` can
    /// legitimately resolve to an empty set -- a caller matching no grant resolves to `{public}`,
    /// but a bare-array read-only audience with no matching selector contributes nothing).
    ///
    /// `coalesce` is applied to the already-aggregated `resolved_audience` column, never to the
    /// pre-aggregate, per-row audience expression: applying it per row first would let the
    /// constant default outrank a real stamped value under `MAX`'s plain string ordering (e.g.
    /// `"alice-laptop"` sorts below `"public"`), silently resolving a stamped process to the
    /// wrong audience.
    fn resolved_predicate(&self) -> Expr {
        let audiences = self.audiences();
        if audiences.is_empty() {
            return lit(false);
        }
        let resolved_audience = match &self.unstamped_audience {
            Some(u) => coalesce(vec![
                col("resolved_audience"),
                lit(ScalarValue::Utf8(Some(u.clone()))),
            ]),
            None => col("resolved_audience"),
        };
        resolved_audience.in_list(
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

    /// §5's literal-valued `EXISTS`: `per_process_audience` filtered down to the single process
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

    /// §6's two-hop literal `EXISTS`: resolve `stream_id_literal` (the `thread_spans`
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
        // Qualified with the outer scan's own table name: once `DecorrelatePredicateSubquery`
        // joins this scan with `in_subquery_plan`'s subquery, the combined schema holds a
        // `process_id` column from both sides (for §3, the *same* underlying table on both
        // sides of a self-referential join), so a bare, unqualified `col("process_id")` is
        // ambiguous and `into_optimized_plan()` fails.
        let outer_process_id = Expr::Column(Column::new(Some(table_name.clone()), "process_id"));
        if view_set_name.as_str() == "processes" {
            // §3: the audience source's own scan -- same `process_id IN (subquery)` shape as §4.
            // No cast needed: `processes.process_id` (the outer scan's own column) is already
            // `Utf8`, unlike most of §4's outer views.
            return Ok(Some(in_subquery(
                outer_process_id,
                in_subquery_plan.clone(),
            )));
        }
        if view.get_file_schema().field_with_name("process_id").is_ok() {
            // §4: process_id-**column** views (streams, blocks, log_entries, measures, net_spans,
            // otel_spans, images, log_stats).
            return Ok(Some(in_subquery(
                cast(outer_process_id, DataType::Utf8),
                in_subquery_plan.clone(),
            )));
        }
        if view_set_name.as_str() == "async_events" {
            // §5: process-scoped, no process_id column -- the view_instance_id *is* the
            // process_id string (`AsyncEventsView::new` parses it as a `Uuid`). Canonicalized
            // before building the literal -- see `canonical_view_instance_id`'s doc comment for why.
            let process_id_literal = Self::canonical_view_instance_id(&view_set_name, &view)?;
            return Ok(Some(Self::exists_for_process(
                per_process_audience,
                resolved_predicate,
                process_id_literal.as_str(),
            )?));
        }
        if view_set_name.as_str() == "thread_spans" {
            // §6: stream-scoped, no process_id or stream_id column -- resolve through `streams`.
            // Same canonicalization as §5, keyed on `stream_id` instead of `process_id`.
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
