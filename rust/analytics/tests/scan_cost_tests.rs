//! Offline (no live DB) tests for the plan-time scan-cost estimator.

use datafusion::arrow::array::{Int64Array, StringArray, TimestampNanosecondArray};
use datafusion::arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::catalog::memory::DataSourceExec;
use datafusion::datasource::physical_plan::FileScanConfig;
use datafusion::execution::SessionStateBuilder;
use datafusion::parquet::arrow::ArrowWriter;
use datafusion::parquet::file::metadata::{ParquetMetaData, ParquetMetaDataReader};
use datafusion::parquet::file::properties::WriterProperties;
use datafusion::physical_plan::joins::CrossJoinExec;
use datafusion::physical_plan::memory::LazyMemoryExec;
use datafusion::physical_plan::{ExecutionPlan, ExecutionPlanProperties};
use datafusion::prelude::{SessionConfig, SessionContext};
use micromegas_analytics::lakehouse::metadata_cache::MetadataCache;
use micromegas_analytics::lakehouse::partition::Partition;
use micromegas_analytics::lakehouse::partitioned_execution_plan::{
    OrderingBounds, ScanOrdering, make_partitioned_execution_plan,
};
use micromegas_analytics::lakehouse::scan_cost::{CostPrecision, estimate_scan_cost};
use micromegas_analytics::lakehouse::view::ScanSortColumn;
use std::sync::Arc;

mod common;
use common::scan_fixtures::{make_empty_partition, make_partition, make_reader_factory};

const OPAQUE_COST: u64 = 1_000_000;

fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new(
            "ts",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("name", DataType::Utf8, false),
        Field::new("payload", DataType::Utf8, false),
        Field::new("value", DataType::Int64, false),
    ]))
}

struct TestFile {
    size: i64,
    footer: ParquetMetaData,
}

impl TestFile {
    fn chunk_bytes(&self, column: usize) -> u64 {
        self.footer
            .row_groups()
            .iter()
            .map(|rg| rg.column(column).compressed_size() as u64)
            .sum()
    }
}

/// Two row groups, with columns of very different sizes.
fn write_file(seed: i64) -> TestFile {
    let n = 2000_i64;
    let batch = RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(
                TimestampNanosecondArray::from((0..n).map(|i| i * 1000).collect::<Vec<_>>())
                    .with_timezone("+00:00"),
            ),
            Arc::new(StringArray::from(
                (0..n).map(|i| format!("n{}", i % 7)).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                (0..n)
                    .map(|i| {
                        format!(
                            "{:x}",
                            (i + seed).wrapping_mul(0x9E37_79B9_7F4A_7C15u64 as i64)
                        )
                    })
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Int64Array::from((0..n).collect::<Vec<_>>())),
        ],
    )
    .unwrap();
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(1000))
        .build();
    let mut buffer = vec![];
    let mut writer = ArrowWriter::try_new(&mut buffer, schema(), Some(props)).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
    let size = buffer.len() as i64;
    let footer = ParquetMetaDataReader::new()
        .parse_and_finish(&bytes::Bytes::from(buffer))
        .unwrap();
    assert_eq!(footer.num_row_groups(), 2);
    TestFile { size, footer }
}

async fn cache_footer(cache: &MetadataCache, path: &str, file: &TestFile) {
    cache
        .insert(path.to_owned(), Arc::new(file.footer.clone()), 1)
        .await;
}

fn plan_with(
    state: &dyn datafusion::catalog::Session,
    cache: &Arc<MetadataCache>,
    partitions: Vec<Partition>,
    projection: Option<Vec<usize>>,
    ordering: &ScanOrdering,
) -> Arc<dyn ExecutionPlan> {
    make_partitioned_execution_plan(
        schema(),
        make_reader_factory(cache.clone()),
        state,
        projection.as_ref(),
        &[],
        None,
        Arc::new(partitions),
        ordering,
    )
    .expect("plan should build")
}

fn plan(
    cache: &Arc<MetadataCache>,
    partitions: Vec<Partition>,
    projection: Option<Vec<usize>>,
) -> Arc<dyn ExecutionPlan> {
    let ctx = SessionContext::new();
    plan_with(
        &ctx.state(),
        cache,
        partitions,
        projection,
        &ScanOrdering::Unordered,
    )
}

fn new_cache() -> Arc<MetadataCache> {
    Arc::new(MetadataCache::new(10 * 1024 * 1024))
}

#[tokio::test]
async fn full_projection_cached_footer_sums_all_chunks() {
    let file = write_file(1);
    let cache = new_cache();
    cache_footer(&cache, "a.parquet", &file).await;
    let p = plan(
        &cache,
        vec![make_partition("a.parquet", 0, None, file.size)],
        None,
    );
    let est = estimate_scan_cost(&p, &cache, OPAQUE_COST).await;
    let expected: u64 = (0..4).map(|c| file.chunk_bytes(c)).sum();
    assert_eq!(est.bytes, expected);
    assert_eq!(est.precision, CostPrecision::Footer);
    assert_eq!(est.footer_files, 1);
    assert_eq!(est.files, 1);
}

#[tokio::test]
async fn narrow_projection_counts_only_projected_chunks() {
    let file = write_file(1);
    let cache = new_cache();
    cache_footer(&cache, "a.parquet", &file).await;
    let p = plan(
        &cache,
        vec![make_partition("a.parquet", 0, None, file.size)],
        Some(vec![1, 3]),
    );
    let est = estimate_scan_cost(&p, &cache, OPAQUE_COST).await;
    assert_eq!(est.bytes, file.chunk_bytes(1) + file.chunk_bytes(3));
    assert_eq!(est.precision, CostPrecision::Footer);
}

#[tokio::test]
async fn empty_projection_costs_nothing() {
    let file = write_file(1);
    let cache = new_cache();
    cache_footer(&cache, "a.parquet", &file).await;
    let p = plan(
        &cache,
        vec![make_partition("a.parquet", 0, None, file.size)],
        Some(vec![]),
    );
    let est = estimate_scan_cost(&p, &cache, OPAQUE_COST).await;
    assert_eq!(est.bytes, 0);
    assert_eq!(est.partition_bytes, file.size as u64);
}

#[tokio::test]
async fn uncached_file_uses_column_share_of_file_size() {
    let cache = new_cache();
    let p = plan(
        &cache,
        vec![make_partition("a.parquet", 0, None, 4000)],
        Some(vec![0, 1]),
    );
    let est = estimate_scan_cost(&p, &cache, OPAQUE_COST).await;
    assert_eq!(est.bytes, 2000);
    assert_eq!(est.precision, CostPrecision::FileSizeShare);
    assert_eq!(est.footer_files, 0);
}

#[tokio::test]
async fn mixed_cached_and_uncached_sums_and_worst_precision_wins() {
    let file = write_file(1);
    let cache = new_cache();
    cache_footer(&cache, "a.parquet", &file).await;
    let p = plan(
        &cache,
        vec![
            make_partition("a.parquet", 0, None, file.size),
            make_partition("b.parquet", 1, None, 4000),
        ],
        Some(vec![0]),
    );
    let est = estimate_scan_cost(&p, &cache, OPAQUE_COST).await;
    assert_eq!(est.bytes, file.chunk_bytes(0) + 1000);
    assert_eq!(est.precision, CostPrecision::FileSizeShare);
    assert_eq!(est.files, 2);
    assert_eq!(est.footer_files, 1);
    assert_eq!(est.partition_bytes, file.size as u64 + 4000);
}

#[tokio::test]
async fn partition_bytes_ignores_projection_and_cache_state() {
    let file = write_file(1);
    let cached = new_cache();
    cache_footer(&cached, "a.parquet", &file).await;
    let uncached = new_cache();
    for cache in [&cached, &uncached] {
        for projection in [None, Some(vec![0]), Some(vec![])] {
            let p = plan(
                cache,
                vec![make_partition("a.parquet", 0, None, file.size)],
                projection,
            );
            let est = estimate_scan_cost(&p, cache, OPAQUE_COST).await;
            assert_eq!(est.partition_bytes, file.size as u64);
        }
    }
}

#[tokio::test]
async fn all_empty_partitions_cost_nothing_with_best_precision() {
    let cache = new_cache();
    let p = plan(&cache, vec![make_empty_partition(0)], None);
    let est = estimate_scan_cost(&p, &cache, OPAQUE_COST).await;
    assert_eq!(est.bytes, 0);
    assert_eq!(est.precision, CostPrecision::Footer);
    assert_eq!(est.files, 0);
}

#[tokio::test]
async fn join_sums_both_scans() {
    let file = write_file(1);
    let cache = new_cache();
    cache_footer(&cache, "a.parquet", &file).await;
    let left = plan(
        &cache,
        vec![make_partition("a.parquet", 0, None, file.size)],
        Some(vec![3]),
    );
    let right = plan(
        &cache,
        vec![make_partition("b.parquet", 0, None, 4000)],
        Some(vec![0]),
    );
    let join: Arc<dyn ExecutionPlan> = Arc::new(CrossJoinExec::new(left, right));
    let est = estimate_scan_cost(&join, &cache, OPAQUE_COST).await;
    assert_eq!(est.bytes, file.chunk_bytes(3) + 1000);
    assert_eq!(est.files, 2);
}

#[tokio::test]
async fn opaque_leaf_adds_flat_cost() {
    let cache = new_cache();
    let stub: Arc<dyn ExecutionPlan> = Arc::new(LazyMemoryExec::try_new(schema(), vec![]).unwrap());
    let est = estimate_scan_cost(&stub, &cache, OPAQUE_COST).await;
    assert_eq!(est.bytes, OPAQUE_COST);
    assert_eq!(est.opaque_nodes, 1);
    assert_eq!(est.precision, CostPrecision::Opaque);
}

#[tokio::test]
async fn scan_shapes_agree_with_unordered() {
    let file_a = write_file(1);
    let file_b = write_file(2);
    let cache = new_cache();
    cache_footer(&cache, "a.parquet", &file_a).await;
    cache_footer(&cache, "b.parquet", &file_b).await;
    let sort_order = Some(vec!["ts".to_owned()]);
    let parts = || {
        vec![
            make_partition("a.parquet", 0, sort_order.clone(), file_a.size),
            make_partition("b.parquet", 1, sort_order.clone(), file_b.size),
        ]
    };
    let columns = vec![ScanSortColumn {
        column: Arc::new("ts".to_owned()),
        descending: false,
    }];
    let ctx = SessionContext::new();
    let state = ctx.state();
    let orderings = [
        ScanOrdering::Unordered,
        ScanOrdering::Concatenated {
            columns: columns.clone(),
            bounds: OrderingBounds::EventTime,
        },
        ScanOrdering::PerFile { columns },
    ];
    let mut estimates = vec![];
    for ordering in &orderings {
        let p = plan_with(&state, &cache, parts(), Some(vec![0, 2]), ordering);
        estimates.push(estimate_scan_cost(&p, &cache, OPAQUE_COST).await);
    }
    let expected = file_a.chunk_bytes(0)
        + file_a.chunk_bytes(2)
        + file_b.chunk_bytes(0)
        + file_b.chunk_bytes(2);
    for est in &estimates {
        assert_eq!(est.bytes, expected);
        assert_eq!(est.files, 2);
    }
}

fn count_partitioned_files(plan: &Arc<dyn ExecutionPlan>) -> usize {
    if let Some(exec) = plan.downcast_ref::<DataSourceExec>()
        && let Some(config) = exec.data_source().downcast_ref::<FileScanConfig>()
    {
        return config.file_groups.iter().map(|g| g.len()).sum();
    }
    plan.children()
        .into_iter()
        .map(count_partitioned_files)
        .sum()
}

#[tokio::test]
async fn byte_range_split_file_is_counted_once() {
    let file = write_file(1);
    let cache = new_cache();
    cache_footer(&cache, "a.parquet", &file).await;
    let config = SessionConfig::new()
        .with_target_partitions(4)
        .with_repartition_file_scans(true)
        .with_repartition_file_min_size(1);
    let state = SessionStateBuilder::new()
        .with_config(config)
        .with_default_features()
        .build();
    let mut p = plan_with(
        &state,
        &cache,
        vec![make_partition("a.parquet", 0, None, file.size)],
        None,
        &ScanOrdering::Unordered,
    );
    for rule in state.physical_optimizers() {
        p = rule.optimize(p, state.config_options()).unwrap();
    }
    assert!(
        count_partitioned_files(&p) > 1,
        "the optimizer should have split the file into byte ranges"
    );
    assert!(p.output_partitioning().partition_count() > 1);
    let est = estimate_scan_cost(&p, &cache, OPAQUE_COST).await;
    let expected: u64 = (0..4).map(|c| file.chunk_bytes(c)).sum();
    assert_eq!(est.bytes, expected);
    assert_eq!(est.files, 1);
    assert_eq!(est.partition_bytes, file.size as u64);
}
