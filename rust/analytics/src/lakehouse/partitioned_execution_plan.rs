use super::{partition::Partition, reader_factory::ReaderFactory, view::ScanSortColumn};
use crate::{dfext::predicate::filters_to_predicate, time::datetime_to_scalar};
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

/// Sorts the non-empty partitions by `min_event_time()` ascending (tiebreak `file_path`) and
/// verifies that adjacent partitions' event-time ranges do not overlap. This makes the declared
/// scan ordering self-contained: the file group is guaranteed to concatenate in globally-sorted
/// order, independent of the order the partition cache returned.
///
/// Returns an error if any adjacent pair overlaps: the declared ordering cannot be honored, so we
/// fail loudly instead of silently emitting a mis-ordered scan. The most likely cause is
/// TSC-frequency estimation drift across materialization epochs (for `tsc_frequency == 0`
/// processes whose blocks were materialized under different clock estimates); the fix is to
/// retire the affected stream's partitions so they rebuild with a single, consistent converter.
fn sort_and_check_non_overlapping(
    mut partitions: Vec<&Partition>,
) -> datafusion::error::Result<Vec<&Partition>> {
    partitions.sort_by(|a, b| {
        a.min_event_time()
            .cmp(&b.min_event_time())
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    for pair in partitions.windows(2) {
        let prev = pair[0];
        let next = pair[1];
        if let (Some(prev_max), Some(next_min)) = (prev.max_event_time(), next.min_event_time())
            && prev_max > next_min
        {
            return Err(datafusion::error::DataFusionError::Execution(format!(
                "declared scan ordering violated: partition {:?} (event-time range ending {prev_max}) overlaps partition {:?} (event-time range starting {next_min}). \
                 This can happen when a stream's blocks were registered out of event-time order, or -- for tsc_frequency == 0 processes -- when TSC-frequency \
                 re-estimation drifted across materialization epochs spanning a clock adjustment (see the ordering-invariant notes on View::get_scan_output_ordering in view.rs). \
                 Retire the affected stream's partitions so they rebuild with a single, consistent time converter.",
                prev.file_path, next.file_path
            )));
        }
    }
    Ok(partitions)
}

/// Attaches `begin`-column (the leading `output_ordering` column) min/max statistics to a
/// `PartitionedFile`, using `Precision::Inexact` because a partition's `min_event_time()` /
/// `max_event_time()` are bounds on `begin` (derived from the first block's begin and the last
/// block's end), not its exact min/max. DataFusion's multi-file-group ordering validation
/// (`is_ordering_valid_for_file_groups`) requires these statistics to be present -- without them
/// the declared ordering is silently dropped for any file group with more than one file.
fn attach_ordering_statistics(
    mut file: PartitionedFile,
    schema: &SchemaRef,
    leading_column: &ScanSortColumn,
    partition: &Partition,
) -> datafusion::error::Result<PartitionedFile> {
    let mut stats = Statistics::new_unknown(schema);
    if let (Some(min_time), Some(max_time)) =
        (partition.min_event_time(), partition.max_event_time())
    {
        let idx = schema.index_of(&leading_column.column)?;
        stats.column_statistics[idx] = ColumnStatistics::new_unknown()
            .with_min_value(Precision::Inexact(datetime_to_scalar(min_time)))
            .with_max_value(Precision::Inexact(datetime_to_scalar(max_time)));
    }
    file = file.with_statistics(Arc::new(stats));
    Ok(file)
}

/// Builds the `LexOrdering` declaring the already-satisfied output ordering of the scan, matching
/// DataFusion's default `ORDER BY` semantics (ASC NULLS LAST unless `descending`).
fn make_lex_ordering(
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

/// Creates a partitioned execution plan for scanning Parquet files.
///
/// `output_ordering` declares an ordering the scan's rows already satisfy (see
/// `View::get_scan_output_ordering`). When non-empty, the file group is sorted by
/// `min_event_time()` and checked for non-overlap (erroring if violated), per-file `begin`
/// min/max statistics are attached so DataFusion accepts the declared ordering, and the ordering
/// is attached to the resulting `FileScanConfig` so `EnforceSorting` can elide a redundant `Sort`
/// node. When empty, behavior is unchanged from before this parameter existed.
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
    output_ordering: &[ScanSortColumn],
) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
    let predicate = filters_to_predicate(schema.clone(), state, filters)?;

    let non_empty_partitions: Vec<&Partition> =
        partitions.iter().filter(|p| !p.is_empty()).collect();
    let non_empty_partitions = if output_ordering.is_empty() {
        non_empty_partitions
    } else {
        sort_and_check_non_overlapping(non_empty_partitions)?
    };

    let mut file_group = vec![];
    for part in &non_empty_partitions {
        let file_path = part.file_path.as_ref().ok_or_else(|| {
            datafusion::error::DataFusionError::Internal(format!(
                "non-empty partition has no file_path: num_rows={}",
                part.num_rows
            ))
        })?;
        let mut pf = PartitionedFile::new(file_path, part.file_size as u64);
        if let Some(leading_column) = output_ordering.first() {
            pf = attach_ordering_statistics(pf, &schema, leading_column, part)?;
        }
        file_group.push(pf);
    }

    // If all partitions are empty, return EmptyExec with projected schema
    if file_group.is_empty() {
        use datafusion::physical_plan::empty::EmptyExec;
        let projected_schema = if let Some(projection) = projection {
            Arc::new(schema.project(projection)?)
        } else {
            schema
        };
        return Ok(Arc::new(EmptyExec::new(projected_schema)));
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

    if let Some(lex) = make_lex_ordering(&schema, output_ordering)? {
        builder = builder.with_output_ordering(vec![lex]);
    }
    let file_scan_config = builder.build();
    Ok(Arc::new(DataSourceExec::new(Arc::new(file_scan_config))))
}
