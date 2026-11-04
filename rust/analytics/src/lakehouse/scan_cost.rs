use super::metadata_cache::MetadataCache;
use datafusion::{
    arrow::datatypes::{DataType, Schema},
    catalog::memory::DataSourceExec,
    datasource::{
        listing::PartitionedFile,
        memory::MemorySourceConfig,
        physical_plan::{FileScanConfig, FileSource, ParquetSource},
    },
    parquet::file::metadata::ParquetMetaData,
    physical_plan::{ExecutionPlan, empty::EmptyExec, placeholder_row::PlaceholderRowExec},
};
use std::collections::HashSet;
use std::sync::Arc;

/// How trustworthy an estimate is. Ordered best to worst, so the worst contributor is the max.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CostPrecision {
    /// Every scanned file was sized from a cached parquet footer.
    Footer,
    /// At least one file was sized as its share of the file size.
    FileSizeShare,
    /// At least one leaf hides its work from the plan, so a flat cost was charged for it.
    Opaque,
}

impl CostPrecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Footer => "footer",
            Self::FileSizeShare => "file_size_share",
            Self::Opaque => "opaque",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanCostEstimate {
    /// Estimated compressed bytes the scans will request.
    pub bytes: u64,
    /// Sum of `file_size` over the distinct files scanned: exact and unprojected.
    pub partition_bytes: u64,
    pub files: u64,
    /// Files estimated from a cached footer.
    pub footer_files: u64,
    /// Leaves whose work is invisible at plan time.
    pub opaque_nodes: u64,
    /// Worst precision among contributors. With no contributors it is `Footer`, the best.
    pub precision: CostPrecision,
}

impl Default for ScanCostEstimate {
    fn default() -> Self {
        Self {
            bytes: 0,
            partition_bytes: 0,
            files: 0,
            footer_files: 0,
            opaque_nodes: 0,
            precision: CostPrecision::Footer,
        }
    }
}

impl ScanCostEstimate {
    fn degrade(&mut self, precision: CostPrecision) {
        self.precision = self.precision.max(precision);
    }
}

/// The single place footers are looked up, so swapping the cache implementation only touches
/// this function. It never loads: a miss is costed from the file size instead, because loading
/// would put object-store I/O in front of the admission gate.
async fn cached_footer(cache: &MetadataCache, path: &str) -> Option<Arc<ParquetMetaData>> {
    cache.get(path).await
}

fn leaf_count(data_type: &DataType) -> u64 {
    match data_type {
        DataType::Struct(fields) => fields.iter().map(|f| leaf_count(f.data_type())).sum(),
        DataType::List(f)
        | DataType::LargeList(f)
        | DataType::FixedSizeList(f, _)
        | DataType::Map(f, _)
        | DataType::ListView(f)
        | DataType::LargeListView(f) => leaf_count(f.data_type()),
        DataType::Union(fields, _) => fields.iter().map(|(_, f)| leaf_count(f.data_type())).sum(),
        _ => 1,
    }
}

fn footer_projected_bytes(footer: &ParquetMetaData, projected_roots: &[usize]) -> u64 {
    let schema = footer.file_metadata().schema_descr();
    let mut total = 0_u64;
    for row_group in footer.row_groups() {
        for leaf in 0..schema.num_columns() {
            if projected_roots.contains(&schema.get_column_root_idx(leaf)) {
                total += row_group.column(leaf).compressed_size().max(0) as u64;
            }
        }
    }
    total
}

fn share_projected_bytes(file_size: u64, file_schema: &Schema, projected_roots: &[usize]) -> u64 {
    let total_leaves: u64 = file_schema
        .fields()
        .iter()
        .map(|f| leaf_count(f.data_type()))
        .sum();
    if total_leaves == 0 {
        return 0;
    }
    let projected_leaves: u64 = projected_roots
        .iter()
        .filter_map(|i| file_schema.fields().get(*i))
        .map(|f| leaf_count(f.data_type()))
        .sum();
    (file_size as u128 * projected_leaves as u128 / total_leaves as u128) as u64
}

fn distinct_files(config: &FileScanConfig) -> Vec<&PartitionedFile> {
    let mut seen = HashSet::new();
    config
        .file_groups
        .iter()
        .flat_map(|g| g.iter())
        .filter(|pf| seen.insert(pf.object_meta.location.as_ref()))
        .collect()
}

async fn estimate_file_scan(
    exec: &DataSourceExec,
    metadata_cache: &MetadataCache,
    opaque_leaf_cost: u64,
    out: &mut ScanCostEstimate,
) {
    if let Some((config, source)) = exec.downcast_to_file_source::<ParquetSource>() {
        let file_schema = config.file_schema();
        let n_fields = file_schema.fields().len();
        let projected_roots: Vec<usize> = match FileSource::projection(source) {
            Some(p) => p
                .column_indices()
                .into_iter()
                .filter(|i| *i < n_fields)
                .collect(),
            None => (0..n_fields).collect(),
        };
        for pf in distinct_files(config) {
            let file_size = pf.object_meta.size;
            out.files += 1;
            out.partition_bytes += file_size;
            match cached_footer(metadata_cache, pf.object_meta.location.as_ref()).await {
                Some(footer) => {
                    out.footer_files += 1;
                    out.bytes += footer_projected_bytes(&footer, &projected_roots);
                }
                None => {
                    out.bytes += share_projected_bytes(file_size, file_schema, &projected_roots);
                    out.degrade(CostPrecision::FileSizeShare);
                }
            }
        }
    } else if let Some(config) = exec.data_source().downcast_ref::<FileScanConfig>() {
        // Non-parquet (static CSV/JSON tables): no footer or column-share path.
        for pf in distinct_files(config) {
            out.files += 1;
            out.bytes += pf.object_meta.size;
            out.partition_bytes += pf.object_meta.size;
            out.degrade(CostPrecision::FileSizeShare);
        }
    } else {
        out.bytes += opaque_leaf_cost;
        out.opaque_nodes += 1;
        out.degrade(CostPrecision::Opaque);
    }
}

async fn visit(
    plan: &Arc<dyn ExecutionPlan>,
    metadata_cache: &MetadataCache,
    opaque_leaf_cost: u64,
    out: &mut ScanCostEstimate,
) {
    if let Some(exec) = plan.downcast_ref::<DataSourceExec>() {
        if exec
            .data_source()
            .downcast_ref::<MemorySourceConfig>()
            .is_some()
        {
            return;
        }
        estimate_file_scan(exec, metadata_cache, opaque_leaf_cost, out).await;
        return;
    }
    if plan.is::<EmptyExec>() || plan.is::<PlaceholderRowExec>() {
        return;
    }
    let children = plan.children();
    if children.is_empty() {
        out.bytes += opaque_leaf_cost;
        out.opaque_nodes += 1;
        out.degrade(CostPrecision::Opaque);
        return;
    }
    for child in children {
        Box::pin(visit(child, metadata_cache, opaque_leaf_cost, out)).await;
    }
}

/// Estimates the compressed parquet bytes `plan` will scan, from the scan nodes alone. Never
/// fails: anything it cannot interpret is costed as an opaque leaf. Joins, aggregates and sorts
/// add nothing; their memory is not predictable from the plan and the pool reservation stays
/// the authority for it.
pub async fn estimate_scan_cost(
    plan: &Arc<dyn ExecutionPlan>,
    metadata_cache: &MetadataCache,
    opaque_leaf_cost: u64,
) -> ScanCostEstimate {
    let mut estimate = ScanCostEstimate::default();
    visit(plan, metadata_cache, opaque_leaf_cost, &mut estimate).await;
    estimate
}
