//! Plans a view definition's merge the way the maintenance daemon would -- same session, same
//! planning function -- without executing it, to tell an author whether the merge keeps its
//! streaming per-file k-way shape or silently falls back to buffering its whole input.

use super::{
    lakehouse_context::LakehouseContext,
    merge::{make_merge_session_context, plan_sorted_merge},
    partition::Partition,
    partition_cache::PartitionCache,
    partitioned_execution_plan::ScanOrdering,
    partitioned_table_provider::PartitionedTableProvider,
    read_scope::CallerContext,
    session_configurator::SessionConfigurator,
    view::{View, ViewMetadata},
    view_definition::{ViewDefinition, build_sql_batch_view},
    view_factory::ViewFactory,
};
use crate::time::TimeRange;
use anyhow::{Context, Result};
use chrono::{TimeDelta, Utc};
use datafusion::common::TableReference;
use std::sync::Arc;

/// The verdict of [`check_merge_streaming`] for one view set.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeStreamingReport {
    pub view_set_name: String,
    pub merge_sort_order: Vec<String>,
    /// `false` when a `SortExec` survives in the merge's physical plan.
    pub streams: bool,
    /// The first plan line containing `SortExec`.
    pub sort_exec_line: Option<String>,
    pub plan: String,
}

/// A non-empty partition that certifies `sort_order`, so the planner keeps it as its own
/// ordered file group instead of dropping it or falling back to an unordered scan.
fn synthetic_partition(
    def: &ViewDefinition,
    sort_order: &[String],
    index: i64,
    insert_range: TimeRange,
) -> Partition {
    let begin = insert_range.begin + TimeDelta::seconds(index * 10);
    let range = TimeRange::new(begin, begin + TimeDelta::seconds(10));
    Partition {
        view_metadata: ViewMetadata {
            view_set_name: Arc::new(def.view_set_name.clone()),
            view_instance_id: Arc::new("global".to_owned()),
            file_schema_hash: vec![0],
        },
        insert_time_range: range,
        event_time_range: Some(range),
        updated: begin,
        file_path: Some(format!("merge_check_{index}.parquet")),
        file_size: 1024,
        source_data_hash: vec![0],
        num_rows: 10,
        sort_order: Some(sort_order.to_vec()),
        max_sort_key_time: None,
    }
}

/// Returns `None` when the definition declares no `merge_sort_order` -- its merge has no
/// streaming shape to lose. Only merge queries over `source` are exact: other views a merge query
/// references plan as empty scans here.
pub async fn check_merge_streaming(
    def: &ViewDefinition,
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    session_configurator: Arc<dyn SessionConfigurator>,
) -> Result<Option<MergeStreamingReport>> {
    let sort_order = match &def.options.merge_sort_order {
        Some(columns) if !columns.is_empty() => columns.clone(),
        _ => return Ok(None),
    };
    let view = build_sql_batch_view(
        def,
        lakehouse.runtime().clone(),
        lakehouse.lake().clone(),
        view_factory.clone(),
        session_configurator.clone(),
    )
    .await?;
    let ScanOrdering::PerFile { columns } = view.get_scan_output_ordering() else {
        anyhow::bail!(
            "'{}' declares a merge_sort_order but its scan ordering is not per-file",
            def.view_set_name
        );
    };

    let now = Utc::now();
    let insert_range = TimeRange::new(now, now + TimeDelta::hours(1));
    let ctx = make_merge_session_context(
        lakehouse.clone(),
        Arc::new(PartitionCache::empty(insert_range)),
        Some(insert_range),
        view_factory,
        session_configurator,
        CallerContext::maintenance(),
    )
    .await?;
    // Two partitions reproduce the multi-file k-way shape of any real merge (k >= 2).
    let partitions = Arc::new(vec![
        synthetic_partition(def, &sort_order, 0, insert_range),
        synthetic_partition(def, &sort_order, 1, insert_range),
    ]);
    let source = PartitionedTableProvider::with_scan_ordering(
        view.get_file_schema(),
        lakehouse.reader_factory().clone(),
        partitions,
        ScanOrdering::PerFile {
            columns: columns.clone(),
        },
    );
    ctx.register_table(
        TableReference::Bare {
            table: "source".into(),
        },
        Arc::new(source),
    )?;

    let merge_sql = def.merge_partitions_query.replace("{source}", "source");
    let planned = plan_sorted_merge(&ctx, &merge_sql, &columns, insert_range)
        .await
        .with_context(|| format!("planning the merge of '{}'", def.view_set_name))?;
    let sort_exec_line = planned
        .plan_text
        .lines()
        .find(|line| line.contains("SortExec"))
        .map(|line| line.trim().to_owned());
    Ok(Some(MergeStreamingReport {
        view_set_name: def.view_set_name.clone(),
        merge_sort_order: sort_order,
        streams: planned.ordering_honored,
        sort_exec_line,
        plan: planned.plan_text,
    }))
}
