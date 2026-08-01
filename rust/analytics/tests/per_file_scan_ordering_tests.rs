//! Offline (no live DB) regression tests for `ScanOrdering::PerFile`
//! (`tasks/completed/1392_kway_merge_sorted_partitions_plan.md`, Design §1):
//! - certifying partitions each become their own single-file file group, all declaring the same
//!   `LexOrdering`, for a downstream `SortPreservingMergeExec`
//! - the recorded-`sort_order` certification gate: any non-empty partition that fails to certify
//!   degrades the *entire* scan to the plain, single-file-group `Unordered` shape

use chrono::{TimeDelta, Utc};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::physical_plan::displayable;
use datafusion::prelude::SessionContext;
use micromegas_analytics::lakehouse::metadata_cache::MetadataCache;
use micromegas_analytics::lakehouse::partition::Partition;
use micromegas_analytics::lakehouse::partitioned_execution_plan::{
    ScanOrdering, make_partitioned_execution_plan,
};
use micromegas_analytics::lakehouse::reader_factory::ReaderFactory;
use micromegas_analytics::lakehouse::view::{ScanSortColumn, ViewMetadata};
use micromegas_analytics::time::TimeRange;
use std::sync::Arc;

fn spike_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("name", DataType::Utf8, false),
        Field::new("time_bin", DataType::Utf8, false),
        Field::new("unit", DataType::Utf8, false),
    ]))
}

fn make_reader_factory() -> Arc<ReaderFactory> {
    Arc::new(ReaderFactory::new(
        Arc::new(object_store::memory::InMemory::new()),
        Arc::new(MetadataCache::new(1024 * 1024)),
    ))
}

/// Every partition below overlaps arbitrarily on `name` -- the shape `PerFile` exists for -- so
/// distinct, non-overlapping event-time ranges are used only to give each partition a valid
/// `event_time_range`, not to declare any ordering over them.
fn make_partition(file_path: &str, index: i64, sort_order: Option<Vec<String>>) -> Partition {
    let t0 = Utc::now() + TimeDelta::seconds(index * 10);
    Partition {
        view_metadata: ViewMetadata {
            view_set_name: Arc::new("metrics".to_owned()),
            view_instance_id: Arc::new("global".to_owned()),
            file_schema_hash: vec![0],
        },
        insert_time_range: TimeRange::new(t0, t0 + TimeDelta::seconds(10)),
        event_time_range: Some(TimeRange::new(t0, t0 + TimeDelta::seconds(10))),
        updated: t0,
        file_path: Some(file_path.to_owned()),
        file_size: 1024,
        source_data_hash: vec![0],
        num_rows: 10,
        sort_order,
    }
}

fn make_empty_partition(index: i64) -> Partition {
    let t0 = Utc::now() + TimeDelta::seconds(index * 10);
    Partition {
        view_metadata: ViewMetadata {
            view_set_name: Arc::new("metrics".to_owned()),
            view_instance_id: Arc::new("global".to_owned()),
            file_schema_hash: vec![0],
        },
        insert_time_range: TimeRange::new(t0, t0 + TimeDelta::seconds(10)),
        event_time_range: None,
        updated: t0,
        file_path: None,
        file_size: 0,
        source_data_hash: vec![0],
        num_rows: 0,
        sort_order: None,
    }
}

fn name_time_bin_ascending() -> Vec<ScanSortColumn> {
    vec![
        ScanSortColumn {
            column: Arc::new("name".to_owned()),
            descending: false,
        },
        ScanSortColumn {
            column: Arc::new("time_bin".to_owned()),
            descending: false,
        },
    ]
}

async fn build_plan_str(partitions: Vec<Partition>, columns: Vec<ScanSortColumn>) -> String {
    let ctx = SessionContext::new();
    let state = ctx.state();
    let plan = make_partitioned_execution_plan(
        spike_schema(),
        make_reader_factory(),
        &state,
        None,
        &[],
        None,
        Arc::new(partitions),
        &ScanOrdering::PerFile { columns },
    )
    .expect("plan should build");
    displayable(plan.as_ref()).indent(true).to_string()
}

#[tokio::test]
async fn certifying_partitions_get_one_file_group_each_with_declared_ordering() {
    let partitions = vec![
        make_partition(
            "a.parquet",
            0,
            Some(vec!["name".to_owned(), "time_bin".to_owned()]),
        ),
        make_partition(
            "b.parquet",
            1,
            Some(vec!["name".to_owned(), "time_bin".to_owned()]),
        ),
        make_partition(
            "c.parquet",
            2,
            Some(vec!["name".to_owned(), "time_bin".to_owned()]),
        ),
    ];
    let plan_str = build_plan_str(partitions, name_time_bin_ascending()).await;
    assert!(
        plan_str.contains("3 groups"),
        "expected one single-file file group per certifying partition, got:\n{plan_str}"
    );
    assert!(
        plan_str.contains("output_ordering=[name"),
        "expected the declared ordering to reach the scan, got:\n{plan_str}"
    );
}

#[tokio::test]
async fn one_uncertifying_partition_degrades_the_whole_scan() {
    let partitions = vec![
        make_partition(
            "a.parquet",
            0,
            Some(vec!["name".to_owned(), "time_bin".to_owned()]),
        ),
        // Never merged/regenerated under the new declaration -- no sort_order recorded.
        make_partition("b.parquet", 1, None),
    ];
    let plan_str = build_plan_str(partitions, name_time_bin_ascending()).await;
    assert!(
        !plan_str.contains("output_ordering="),
        "a single uncertifying partition must degrade the entire scan to unordered, got:\n{plan_str}"
    );
    assert!(
        plan_str.contains("1 group:"),
        "the degraded scan should be the single sequential file group Unordered shape, got:\n{plan_str}"
    );
}

#[tokio::test]
async fn empty_partitions_certify_vacuously_and_do_not_block_certification() {
    let partitions = vec![
        make_partition(
            "a.parquet",
            0,
            Some(vec!["name".to_owned(), "time_bin".to_owned()]),
        ),
        make_empty_partition(1),
    ];
    let plan_str = build_plan_str(partitions, name_time_bin_ascending()).await;
    assert!(
        plan_str.contains("1 group:") && plan_str.contains("output_ordering=[name"),
        "an empty partition (no sort_order recorded) must not defeat certification, got:\n{plan_str}"
    );
}

#[tokio::test]
async fn descending_declared_column_can_never_certify() {
    let partitions = vec![
        make_partition("a.parquet", 0, Some(vec!["name".to_owned()])),
        make_partition("b.parquet", 1, Some(vec!["name".to_owned()])),
    ];
    let descending_columns = vec![ScanSortColumn {
        column: Arc::new("name".to_owned()),
        descending: true,
    }];
    let plan_str = build_plan_str(partitions, descending_columns).await;
    assert!(
        !plan_str.contains("output_ordering="),
        "a descending declared column can never be certified by an ascending-implied recorded \
         sort_order, so the scan must degrade to unordered, got:\n{plan_str}"
    );
}

#[tokio::test]
async fn declared_columns_a_strict_prefix_of_recorded_are_certified() {
    let partitions = vec![
        make_partition(
            "a.parquet",
            0,
            Some(vec![
                "name".to_owned(),
                "time_bin".to_owned(),
                "unit".to_owned(),
            ]),
        ),
        make_partition(
            "b.parquet",
            1,
            Some(vec![
                "name".to_owned(),
                "time_bin".to_owned(),
                "unit".to_owned(),
            ]),
        ),
    ];
    // Declaring only a prefix of the recorded order is still a valid (weaker) guarantee.
    let declared = vec![ScanSortColumn {
        column: Arc::new("name".to_owned()),
        descending: false,
    }];
    let plan_str = build_plan_str(partitions, declared).await;
    assert!(
        plan_str.contains("2 groups"),
        "a declared prefix of the recorded sort_order should certify, got:\n{plan_str}"
    );
    assert!(
        plan_str.contains("output_ordering=[name"),
        "expected the declared ordering to reach the scan, got:\n{plan_str}"
    );
}
