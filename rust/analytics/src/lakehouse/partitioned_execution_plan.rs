use super::{partition::Partition, reader_factory::ReaderFactory, view::ScanSortColumn};
use crate::{
    dfext::predicate::filters_to_predicate,
    time::{TimeRange, datetime_to_scalar},
};
use anyhow::Context;
use chrono::{DateTime, Utc};
use datafusion::{
    arrow::{compute::SortOptions, datatypes::SchemaRef},
    catalog::{Session, memory::DataSourceExec},
    common::stats::Precision,
    datasource::{
        listing::PartitionedFile,
        physical_plan::{FileScanConfigBuilder, ParquetSource},
    },
    execution::object_store::ObjectStoreUrl,
    physical_expr::{LexOrdering, PhysicalSortExpr},
    physical_plan::{ColumnStatistics, ExecutionPlan, Statistics},
    prelude::*,
};
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// Which pair of bounds on `Partition` a declared ordering's leading column is checked against.
#[derive(Clone, Copy, Debug)]
pub enum OrderingBounds {
    /// `min_event_time()` / `max_event_time()` -- `Option`, absent for empty partitions.
    EventTime,
    /// `begin_insert_time()` / `end_insert_time()` -- always present.
    InsertTime,
}

/// Reads the pair of bounds a declared ordering's leading column is checked against, per
/// `OrderingBounds`. `InsertTime` bounds are always present; `EventTime` bounds are `None` for
/// empty partitions (callers are expected to have already filtered those out).
///
/// The `EventTime` upper bound prefers `max_sort_key_time` (the partition's true recorded max of
/// the leading sort column, e.g. `begin` for `thread_spans`) and falls back to `max_event_time`
/// (the max span *end*, a merely conservative stand-in) when it is `None` -- true for every
/// partition written before that column existed, and for any view that never declares a
/// `Concatenated` event-time ordering. This single change point upgrades all three consumers of
/// `partition_bounds` coherently: the sort key stays `min_event_time`; the non-overlap check below
/// compares the previous partition's *true* max `begin` against the next's block-derived min
/// (which can never strictly overlap for cuts at block boundaries of a producer using a shared
/// flush timestamp); and `attach_ordering_statistics` attaches a tighter, exact `begin` max
/// statistic. The fallback preserves bit-for-bit behavior for legacy partitions.
fn partition_bounds(
    p: &Partition,
    bounds: OrderingBounds,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    match bounds {
        OrderingBounds::EventTime => p
            .min_event_time()
            .zip(p.max_sort_key_time().or(p.max_event_time())),
        OrderingBounds::InsertTime => Some((p.begin_insert_time(), p.end_insert_time())),
    }
}

/// Sorts the non-empty partitions by their leading-column bound ascending (tiebreak `file_path`)
/// and verifies that adjacent partitions' ranges do not overlap. This makes the declared scan
/// ordering self-contained: the file group is guaranteed to concatenate in globally-sorted order,
/// independent of the order the partition cache returned.
///
/// Returns an error if any adjacent pair overlaps: the declared ordering cannot be honored, so we
/// fail loudly instead of silently emitting a mis-ordered scan. For `OrderingBounds::EventTime`,
/// block-boundary tick overlap no longer trips this check for partitions carrying
/// `max_sort_key_time` (`partition_bounds` reads it in preference to `max_event_time`): the
/// producer now stamps one shared timestamp per flush (matching the Unreal producer), so
/// consecutive blocks touch exactly, and the recorded bound reflects that. Legacy partitions
/// written before that column existed fall back to the old, looser `max_event_time` bound and so
/// remain a residual until they are rebuilt -- which happens automatically on their next query,
/// since `ThreadSpansView::SCHEMA_VERSION`'s bump makes every pre-existing partition stale by
/// schema hash; this is a self-healing residual, not an admin-retire dependency. The two remaining
/// residual causes are: an insert-time inversion straddling a JIT segment boundary (a genuine
/// row-level overlap, correctly rejected); and TSC-frequency estimation drift across
/// materialization epochs (for `tsc_frequency == 0` processes whose blocks were materialized under
/// different clock estimates) -- fixed the same way, by retiring the affected stream's partitions
/// so they rebuild with a single, consistent converter. See the ordering-invariant notes on
/// `View::get_scan_output_ordering`. For `OrderingBounds::InsertTime` an overlap indicates a
/// genuine partitioning bug -- input partitions are expected to be non-overlapping in insert_time
/// by construction.
fn sort_and_check_non_overlapping(
    mut partitions: Vec<&Partition>,
    bounds: OrderingBounds,
) -> datafusion::error::Result<Vec<&Partition>> {
    partitions.sort_by(|a, b| {
        partition_bounds(a, bounds)
            .map(|(begin, _)| begin)
            .cmp(&partition_bounds(b, bounds).map(|(begin, _)| begin))
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    for pair in partitions.windows(2) {
        let prev = pair[0];
        let next = pair[1];
        if let (Some((_, prev_max)), Some((next_min, _))) = (
            partition_bounds(prev, bounds),
            partition_bounds(next, bounds),
        ) && prev_max > next_min
        {
            return Err(datafusion::error::DataFusionError::Internal(format!(
                "declared scan ordering violated: partition {:?} (range ending {prev_max}) overlaps partition {:?} (range starting {next_min}). \
                 If either partition predates max_sort_key_time (schema v8), this heals itself on its next query: a schema-hash bump makes it stale and it \
                 rebuilds automatically, carrying the exact bound, with no admin action needed. Otherwise, for event-time ordering the remaining causes are an \
                 insert-time inversion straddling a JIT segment boundary, or -- for tsc_frequency == 0 processes -- TSC-frequency re-estimation drift across \
                 materialization epochs spanning a clock adjustment; both are fixed by retiring the affected stream's partitions so they rebuild with a single, \
                 consistent time converter. See the rustdoc on sort_and_check_non_overlapping (partitioned_execution_plan.rs) and the ordering-invariant notes on \
                 View::get_scan_output_ordering in view.rs for the full cause list.",
                prev.file_path, next.file_path
            )));
        }
    }
    Ok(partitions)
}

/// Attaches the leading `output_ordering` column's min/max statistics to a `PartitionedFile`,
/// using `Precision::Inexact` since the bounds read from `Partition` (per `OrderingBounds`) are
/// not necessarily the column's exact min/max -- though for `EventTime` bounds when
/// `max_sort_key_time` is recorded, the attached max happens to be exact; `Precision::Inexact` is
/// still correct to declare, since an exact statistic is a legal special case of an inexact one.
/// DataFusion's multi-file-group ordering validation (`is_ordering_valid_for_file_groups`)
/// requires these statistics to be present -- without them the declared ordering is silently
/// dropped for any file group with more than one file.
fn attach_ordering_statistics(
    mut file: PartitionedFile,
    schema: &SchemaRef,
    leading_column: &ScanSortColumn,
    partition: &Partition,
    bounds: OrderingBounds,
) -> datafusion::error::Result<PartitionedFile> {
    let mut stats = Statistics::new_unknown(schema);
    if let Some((min_time, max_time)) = partition_bounds(partition, bounds) {
        let idx = schema.index_of(&leading_column.column)?;
        stats.column_statistics[idx] = ColumnStatistics::new_unknown()
            .with_min_value(Precision::Inexact(datetime_to_scalar(min_time)))
            .with_max_value(Precision::Inexact(datetime_to_scalar(max_time)));
    }
    file = file.with_statistics(Arc::new(stats));
    Ok(file)
}

/// How a partition scan's declared output ordering is realized.
///
/// Two scan shapes exist: a single sequential file group spanning every input partition
/// (`Unordered`, `Concatenated`) and one file group per input partition (`PerFile`, scanned by k
/// readers for a downstream `SortPreservingMergeExec`). `Unordered` and `Concatenated` build and
/// execute the identical single-file-group scan -- they differ only in whether the resulting order
/// is declared to DataFusion, not in scan shape or execution strategy (see
/// `ScanOrdering::declares_concatenated_ordering`).
#[derive(Clone, Debug)]
pub enum ScanOrdering {
    /// No declared ordering (today's default).
    Unordered,
    /// All files form one sequential file group that concatenates in globally-sorted order.
    /// Requires non-overlapping leading-column bounds (checked against `bounds`).
    Concatenated {
        columns: Vec<ScanSortColumn>,
        bounds: OrderingBounds,
    },
    /// Each file is internally sorted by `columns`; files may overlap arbitrarily. The scan
    /// yields one ordered plan partition per file, for a downstream `SortPreservingMergeExec`.
    /// `columns` should be non-empty -- `SqlBatchView::with_merge_sort_order` rejects an empty
    /// list at construction, but this enum and `View::get_scan_output_ordering` are public, so
    /// that is not the only construction path. An empty `columns` is still safe: it never
    /// certifies (see `Partition::certifies_sort_order`), so `make_partitioned_execution_plan`
    /// degrades it to `Unordered` for any non-empty partition rather than planning and recording a
    /// vacuous ordering.
    PerFile { columns: Vec<ScanSortColumn> },
}

impl ScanOrdering {
    /// True when this ordering declares a concatenating scan's global order (`Concatenated`).
    /// `PerFile` returns `false` because a per-file ordering only becomes a global one through a
    /// downstream merge -- it is the other strategy, not an undeclared version of this one. A
    /// bool, not the columns themselves: nothing on the concatenating path inspects the declared
    /// columns, only whether an ordering was declared at all (see
    /// `QueryMerger::execute_concatenated_merge` in `merge.rs`).
    pub fn declares_concatenated_ordering(&self) -> bool {
        matches!(self, ScanOrdering::Concatenated { .. })
    }
}

/// Builds the `LexOrdering` declaring the already-satisfied output ordering of the scan, matching
/// DataFusion's default `ORDER BY` semantics (ASC NULLS LAST unless `descending`).
pub fn make_lex_ordering(
    schema: &SchemaRef,
    output_ordering: &[ScanSortColumn],
) -> datafusion::error::Result<Option<LexOrdering>> {
    let sort_exprs = output_ordering
        .iter()
        .map(|c| {
            let col =
                datafusion::physical_expr::expressions::Column::new_with_schema(&c.column, schema)?;
            Ok(PhysicalSortExpr::new(
                Arc::new(col),
                SortOptions {
                    descending: c.descending,
                    // Match DataFusion's default ORDER BY semantics: ASC NULLS LAST, DESC NULLS
                    // FIRST. Hardcoding `false` here would declare `DESC NULLS LAST`, which fails
                    // to satisfy a descending query's `DESC NULLS FIRST` requirement and silently
                    // keeps a redundant Sort.
                    nulls_first: c.descending,
                },
            ))
        })
        .collect::<datafusion::error::Result<Vec<_>>>()?;
    Ok(LexOrdering::new(sort_exprs))
}

/// Bails unless `plan` is a single-partition physical plan. `subject` names the query for the
/// error message (e.g. `format!("merge query {:?}", query)` or `format!("extract query for
/// {view}")`); `reason` supplies the full, call-site-specific trailing sentence(s) explaining what
/// a non-single-partition plan means here -- executing such a plan would coalesce partitions and
/// silently destroy a declared ordering before it is safe to record or execute. Shared by the
/// query-execution paths that must verify this before executing:
/// `QueryMerger::execute_concatenated_merge` (only when its ordering is declared),
/// `QueryMerger::execute_sorted_merge`, and `SqlPartitionSpec::execute_extract_query`.
pub fn assert_single_partition(
    plan: &Arc<dyn ExecutionPlan>,
    subject: &str,
    insert_range: TimeRange,
    reason: &str,
) -> anyhow::Result<()> {
    let partition_count = plan.properties().output_partitioning().partition_count();
    if partition_count != 1 {
        anyhow::bail!(
            "{subject} (insert_range=[{}, {}]) produced a {partition_count}-partition physical \
             plan; {reason}",
            insert_range.begin.to_rfc3339(),
            insert_range.end.to_rfc3339()
        );
    }
    Ok(())
}

/// Bails unless `plan`'s output ordering satisfies the declared `columns`, defensively
/// re-verifying that a `sort_order` guarantee about to be recorded on a fresh partition is
/// truthful. `label` is a short noun phrase used in the intermediate error-context messages (e.g.
/// `"per-file merge"` or `"extract-query"`); `subject` names the query for the bail message;
/// `reason` supplies the full, call-site-specific trailing text of the bail message (what was
/// declared, and any guidance for diagnosing a mismatch). Shared by the two paths that record a
/// `sort_order` guarantee: `QueryMerger::execute_sorted_merge` and
/// `SqlPartitionSpec::execute_extract_query`. `QueryMerger::execute_concatenated_merge` does not
/// call this -- its ordering is a structural property of the sorted, non-overlapping file group
/// rather than a query-plan sort DataFusion could get wrong.
pub fn assert_ordering_satisfied(
    plan: &Arc<dyn ExecutionPlan>,
    columns: &[ScanSortColumn],
    label: &str,
    subject: &str,
    insert_range: TimeRange,
    reason: &str,
) -> anyhow::Result<()> {
    let lex = make_lex_ordering(&plan.schema(), columns)
        .with_context(|| format!("building the declared {label} ordering"))?
        .with_context(|| format!("declared {label} columns must be non-empty"))?;
    let ordering_satisfied = plan
        .properties()
        .equivalence_properties()
        .ordering_satisfy(lex)
        .with_context(|| format!("checking {label} plan output ordering"))?;
    if !ordering_satisfied {
        anyhow::bail!(
            "{subject} (insert_range=[{}, {}]) produced a physical plan whose output ordering \
             does not satisfy {reason}",
            insert_range.begin.to_rfc3339(),
            insert_range.end.to_rfc3339()
        );
    }
    Ok(())
}

/// Creates a partitioned execution plan for scanning Parquet files.
///
/// `scan_ordering` declares how the scan's already-satisfied ordering (see
/// `View::get_scan_output_ordering`), if any, is realized:
/// - `Unordered`: no ordering is declared to DataFusion.
/// - `Concatenated { columns, bounds }`: the file group is sorted by the leading column's bound
///   (read per `bounds`) and checked for non-overlap (erroring if violated), per-file min/max
///   statistics are attached so DataFusion accepts the declared ordering, and the ordering is
///   attached to the resulting `FileScanConfig` so `EnforceSorting` can elide a redundant `Sort`
///   node.
/// - `PerFile { columns }`: gated by `Partition::certifies_sort_order` -- if every non-empty
///   partition's recorded `sort_order` certifies `columns`, each non-empty partition becomes its
///   own single-file group (no overlap check, no per-file statistics -- DataFusion's multi-file-group
///   ordering validation only needs those to prove cross-file order *within* a group, and
///   single-file groups pass trivially), all declaring the same `columns` ordering for a
///   downstream `SortPreservingMergeExec`. If any non-empty partition fails to certify, this
///   degrades to the same plan shape as `Unordered`.
#[span_fn]
#[expect(clippy::too_many_arguments)]
pub fn make_partitioned_execution_plan(
    schema: SchemaRef,
    reader_factory: Arc<ReaderFactory>,
    state: &dyn Session,
    projection: Option<&Vec<usize>>,
    filters: &[Expr],
    limit: Option<usize>,
    partitions: Arc<Vec<Partition>>,
    scan_ordering: &ScanOrdering,
) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
    let predicate = filters_to_predicate(schema.clone(), state, filters)?;

    let non_empty_partitions: Vec<&Partition> =
        partitions.iter().filter(|p| !p.is_empty()).collect();

    // A PerFile declaration that any non-empty partition does not certify degrades to Unordered:
    // both consumers (user-query scans and merge scans) fall back to sorting rather than trust a
    // stale or uncertified guarantee.
    let scan_ordering = match scan_ordering {
        ScanOrdering::PerFile { columns }
            if !non_empty_partitions
                .iter()
                .all(|p| p.certifies_sort_order(columns)) =>
        {
            &ScanOrdering::Unordered
        }
        other => other,
    };

    match scan_ordering {
        ScanOrdering::Unordered => build_unordered_or_concatenated_plan(
            schema,
            reader_factory,
            predicate,
            projection,
            limit,
            non_empty_partitions,
            None,
        ),
        ScanOrdering::Concatenated { columns, bounds } => {
            let non_empty_partitions =
                sort_and_check_non_overlapping(non_empty_partitions, *bounds)?;
            build_unordered_or_concatenated_plan(
                schema,
                reader_factory,
                predicate,
                projection,
                limit,
                non_empty_partitions,
                Some((columns.as_slice(), *bounds)),
            )
        }
        ScanOrdering::PerFile { columns } => build_per_file_plan(
            schema,
            reader_factory,
            predicate,
            projection,
            limit,
            non_empty_partitions,
            columns,
        ),
    }
}

/// Builds the `Unordered` (`ordering: None`) and `Concatenated` (`ordering: Some((columns,
/// bounds))`) scan shapes: every non-empty partition in one sequential file group.
fn build_unordered_or_concatenated_plan(
    schema: SchemaRef,
    reader_factory: Arc<ReaderFactory>,
    predicate: Arc<dyn datafusion::physical_expr::PhysicalExpr>,
    projection: Option<&Vec<usize>>,
    limit: Option<usize>,
    non_empty_partitions: Vec<&Partition>,
    ordering: Option<(&[ScanSortColumn], OrderingBounds)>,
) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
    let mut file_group = vec![];
    for part in &non_empty_partitions {
        let file_path = part.file_path.as_ref().ok_or_else(|| {
            datafusion::error::DataFusionError::Internal(format!(
                "non-empty partition has no file_path: num_rows={}",
                part.num_rows
            ))
        })?;
        let mut pf = PartitionedFile::new(file_path, part.file_size as u64);
        if let Some((output_ordering, ordering_bounds)) = ordering
            && let Some(leading_column) = output_ordering.first()
        {
            pf = attach_ordering_statistics(pf, &schema, leading_column, part, ordering_bounds)?;
        }
        file_group.push(pf);
    }

    // If all partitions are empty, return EmptyExec with projected schema
    if file_group.is_empty() {
        return empty_exec(schema, projection);
    }

    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    let source = Arc::new(
        ParquetSource::new(schema.clone())
            .with_predicate(predicate)
            .with_parquet_file_reader_factory(reader_factory),
    );
    let mut builder = FileScanConfigBuilder::new(object_store_url, source)
        .with_limit(limit)
        .with_projection_indices(projection.cloned())?
        .with_file_groups(vec![file_group.into()]);

    if let Some((output_ordering, _)) = ordering
        && let Some(lex) = make_lex_ordering(&schema, output_ordering)?
    {
        builder = builder.with_output_ordering(vec![lex]);
    }
    let file_scan_config = builder.build();
    Ok(Arc::new(DataSourceExec::new(Arc::new(file_scan_config))))
}

/// Builds the `PerFile` scan shape: one single-file file group per non-empty partition, all
/// declaring the same `columns` ordering. No overlap check and no per-file statistics -- see the
/// module-level rustdoc on `ScanOrdering::PerFile`.
fn build_per_file_plan(
    schema: SchemaRef,
    reader_factory: Arc<ReaderFactory>,
    predicate: Arc<dyn datafusion::physical_expr::PhysicalExpr>,
    projection: Option<&Vec<usize>>,
    limit: Option<usize>,
    non_empty_partitions: Vec<&Partition>,
    columns: &[ScanSortColumn],
) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
    let mut file_groups = vec![];
    for part in &non_empty_partitions {
        let file_path = part.file_path.as_ref().ok_or_else(|| {
            datafusion::error::DataFusionError::Internal(format!(
                "non-empty partition has no file_path: num_rows={}",
                part.num_rows
            ))
        })?;
        let pf = PartitionedFile::new(file_path, part.file_size as u64);
        file_groups.push(vec![pf].into());
    }

    if file_groups.is_empty() {
        return empty_exec(schema, projection);
    }

    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    let source = Arc::new(
        ParquetSource::new(schema.clone())
            .with_predicate(predicate)
            .with_parquet_file_reader_factory(reader_factory),
    );
    let mut builder = FileScanConfigBuilder::new(object_store_url, source)
        .with_limit(limit)
        .with_projection_indices(projection.cloned())?
        .with_file_groups(file_groups);

    if let Some(lex) = make_lex_ordering(&schema, columns)? {
        builder = builder.with_output_ordering(vec![lex]);
    }
    let file_scan_config = builder.build();
    Ok(Arc::new(DataSourceExec::new(Arc::new(file_scan_config))))
}

fn empty_exec(
    schema: SchemaRef,
    projection: Option<&Vec<usize>>,
) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
    use datafusion::physical_plan::empty::EmptyExec;
    let projected_schema = if let Some(projection) = projection {
        Arc::new(schema.project(projection)?)
    } else {
        schema
    };
    Ok(Arc::new(EmptyExec::new(projected_schema)))
}
