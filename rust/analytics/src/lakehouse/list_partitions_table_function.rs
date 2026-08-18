use super::audience_guard::{AudienceGuard, IdKind};
use super::read_scope::ReadScope;
use crate::sql_arrow_bridge::rows_to_record_batch;
use async_trait::async_trait;
use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::datatypes::Field;
use datafusion::arrow::datatypes::Schema;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::datatypes::TimeUnit;
use datafusion::catalog::Session;
use datafusion::catalog::TableFunctionArgs;
use datafusion::catalog::TableFunctionImpl;
use datafusion::catalog::TableProvider;
use datafusion::datasource::TableType;
use datafusion::datasource::memory::{DataSourceExec, MemorySourceConfig};
use datafusion::error::DataFusionError;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::prelude::Expr;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use sqlx::Row;
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

/// A DataFusion `TableFunctionImpl` for listing lakehouse partitions.
#[derive(Debug)]
pub struct ListPartitionsTableFunction {
    lake: Arc<DataLakeConnection>,
    guard: Arc<AudienceGuard>,
}

impl ListPartitionsTableFunction {
    pub fn new(lake: Arc<DataLakeConnection>, guard: Arc<AudienceGuard>) -> Self {
        Self { lake, guard }
    }
}

impl TableFunctionImpl for ListPartitionsTableFunction {
    fn call_with_args(
        &self,
        _args: TableFunctionArgs,
    ) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        Ok(Arc::new(ListPartitionsTableProvider {
            lake: self.lake.clone(),
            guard: self.guard.clone(),
        }))
    }
}

/// A DataFusion `TableProvider` for listing lakehouse partitions.
#[derive(Debug)]
pub struct ListPartitionsTableProvider {
    pub lake: Arc<DataLakeConnection>,
    guard: Arc<AudienceGuard>,
}

#[async_trait]
impl TableProvider for ListPartitionsTableProvider {
    fn schema(&self) -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("view_set_name", DataType::Utf8, false),
            Field::new("view_instance_id", DataType::Utf8, false),
            Field::new(
                "begin_insert_time",
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                false,
            ),
            Field::new(
                "end_insert_time",
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                false,
            ),
            Field::new(
                "min_event_time",
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                true,
            ),
            Field::new(
                "max_event_time",
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                true,
            ),
            Field::new(
                "updated",
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                false,
            ),
            Field::new("file_path", DataType::Utf8, true),
            Field::new("file_size", DataType::Int64, false),
            Field::new("file_schema_hash", DataType::Binary, false),
            Field::new("source_data_hash", DataType::Binary, false),
            Field::new("num_rows", DataType::Int64, false),
            Field::new("partition_format_version", DataType::Int32, false),
            Field::new(
                "sort_order",
                DataType::List(Arc::new(Field::new("tag", DataType::Utf8, false))),
                true,
            ),
            Field::new(
                "max_sort_key_time",
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                true,
            ),
        ]))
    }

    fn table_type(&self) -> TableType {
        TableType::Temporary
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        // Query Enforcement Prong B (#1371): `ReadScope::All` keeps today's path unchanged,
        // including the `LIMIT` pushdown. A restricted caller instead fetches every row
        // unlimited, row-filters per plan §8, then truncates to `limit` in Rust -- filtering
        // after a pushed-down limit would return fewer rows than asked for while more matching
        // rows exist, silently wrong.
        let restricted = *self.guard.read_scope() != ReadScope::All;
        let query = if !restricted {
            // Build query with optional LIMIT clause pushed down to PostgreSQL.
            // DataFusion only pushes the limit when it's safe to do so (i.e., when there
            // are no WHERE clauses that could filter rows). When filters are present,
            // DataFusion passes limit=None and applies the limit after filtering.
            // Important: DataFusion trusts us to apply the limit - if we ignore it,
            // too many rows will be returned to the client.
            if let Some(n) = limit {
                format!(
                    "SELECT view_set_name,
                            view_instance_id,
                            begin_insert_time,
                            end_insert_time,
                            min_event_time,
                            max_event_time,
                            updated,
                            file_path,
                            file_size,
                            file_schema_hash,
                            source_data_hash,
                            num_rows,
                            partition_format_version,
                            sort_order,
                            max_sort_key_time
                     FROM lakehouse_partitions
                     LIMIT {n};"
                )
            } else {
                "SELECT view_set_name,
                        view_instance_id,
                        begin_insert_time,
                        end_insert_time,
                        min_event_time,
                        max_event_time,
                        updated,
                        file_path,
                        file_size,
                        file_schema_hash,
                        source_data_hash,
                        num_rows,
                        partition_format_version,
                        sort_order,
                        max_sort_key_time
                 FROM lakehouse_partitions;"
                    .to_string()
            }
        } else {
            // Deliberately unbounded (no `LIMIT`/`OFFSET` paging here): paginating this fetch
            // would need either (a) holding a connection/transaction across the paging loop while
            // `filter_rows` re-enters this same `db_pool` for every audience-resolution chunk --
            // self-deadlocking once concurrent restricted scans exceed the pool's connection
            // count -- or (b) a stable, indexable ordering key to page on, which
            // `lakehouse_partitions` doesn't have (no primary key, `file_path` is nullable, and
            // the only exclusion constraint covers just overlap within one `(view_set_name,
            // view_instance_id, file_schema_hash)` group). A single unbounded fetch avoids both.
            "SELECT view_set_name,
                    view_instance_id,
                    begin_insert_time,
                    end_insert_time,
                    min_event_time,
                    max_event_time,
                    updated,
                    file_path,
                    file_size,
                    file_schema_hash,
                    source_data_hash,
                    num_rows,
                    partition_format_version,
                    sort_order,
                    max_sort_key_time
             FROM lakehouse_partitions;"
                .to_string()
        };

        let rows = instrument_named!(
            sqlx::query(&query).fetch_all(&self.lake.db_pool),
            "sql_select_list_partitions"
        )
        .await
        .map_err(|e| DataFusionError::External(e.into()))?;

        let rb = if !restricted {
            if rows.is_empty() {
                // `rows_to_record_batch` maps an empty slice to a **zero-field** empty batch
                // (`make_empty_record_batch`), which doesn't match this provider's 15-column
                // schema and fails once `MemoryStream` projects it. Build the empty batch
                // directly from this provider's own schema instead -- this is the normal state
                // of a fresh deployment with no partitions materialized yet.
                RecordBatch::new_empty(self.schema())
            } else {
                rows_to_record_batch(&rows).map_err(|e| DataFusionError::External(e.into()))?
            }
        } else {
            let filtered = self.filter_rows(rows, limit).await?;
            if filtered.is_empty() {
                // Same zero-field-batch pitfall as above: a `ReadScope::Audiences` caller with
                // no readable partitions hits this as its steady state, not just an edge case.
                RecordBatch::new_empty(self.schema())
            } else {
                let truncated: Vec<_> = match limit {
                    Some(n) => filtered.into_iter().take(n).collect(),
                    None => filtered,
                };
                rows_to_record_batch(&truncated).map_err(|e| DataFusionError::External(e.into()))?
            }
        };

        let source = MemorySourceConfig::try_new(
            &[vec![rb]],
            self.schema(),
            projection.map(|v| v.to_owned()),
        )?;
        Ok(DataSourceExec::from_data_source(source))
    }
}

impl ListPartitionsTableProvider {
    /// Bounds only how many rows' worth of ids a single `readable_ids` round trip resolves --
    /// it does NOT bound the row fetch from Postgres, which is a single unbounded `fetch_all`
    /// (see the comment on that query in [`ListPartitionsTableProvider::scan`]). Keeps a
    /// restricted caller's `LIMIT n` from resolving every distinct `view_instance_id` in
    /// `lakehouse_partitions` up front: already-fetched rows are processed in chunks of this
    /// size, stopping as soon as `limit` matching rows have been kept, so a small `LIMIT` costs
    /// at most a handful of audience-resolution round trips instead of one unbounded batch.
    const RESOLVE_CHUNK_ROWS: usize = 1_000;

    /// Row filtering for a `ReadScope::Audiences` caller (plan §8): a `view_instance_id` that
    /// parses as a `Uuid` is kept iff it resolves (as `IdKind::ProcessOrStream`) to a readable
    /// audience; the literal `'global'` is kept iff `AudienceGuard::global_rows_visible` says so
    /// for that row's `view_set_name`; anything else is dropped (fail-closed -- nothing produces
    /// such a value today). Filters the `sqlx` row vector *before* `rows_to_record_batch` rather
    /// than the built `RecordBatch` after: simpler than a `take` kernel over 15 columns, and
    /// leaves the schema construction untouched. Processes `rows` in bounded chunks
    /// ([`Self::RESOLVE_CHUNK_ROWS`]) and returns as soon as `limit` matching rows have been kept.
    async fn filter_rows(
        &self,
        rows: Vec<sqlx::postgres::PgRow>,
        limit: Option<usize>,
    ) -> datafusion::error::Result<Vec<sqlx::postgres::PgRow>> {
        let mut kept = Vec::new();
        let mut rows_iter = rows.into_iter();
        loop {
            let chunk: Vec<_> = (&mut rows_iter).take(Self::RESOLVE_CHUNK_ROWS).collect();
            if chunk.is_empty() {
                break;
            }
            let mut candidate_ids: Vec<Uuid> = Vec::with_capacity(chunk.len());
            let mut seen_ids: HashSet<Uuid> = HashSet::with_capacity(chunk.len());
            let mut row_ids: Vec<Option<Uuid>> = Vec::with_capacity(chunk.len());
            for row in &chunk {
                let view_instance_id: &str = row
                    .try_get("view_instance_id")
                    .map_err(|e| DataFusionError::External(e.into()))?;
                let id = Uuid::parse_str(view_instance_id).ok();
                if let Some(id) = id
                    && seen_ids.insert(id)
                {
                    candidate_ids.push(id);
                }
                row_ids.push(id);
            }
            let readable = self
                .guard
                .readable_ids(&candidate_ids, IdKind::ProcessOrStream)
                .await?;
            for (row, id) in chunk.into_iter().zip(row_ids) {
                let keep = match id {
                    Some(uuid) => readable.contains(&uuid),
                    None => {
                        let view_instance_id: &str = row
                            .try_get("view_instance_id")
                            .map_err(|e| DataFusionError::External(e.into()))?;
                        let view_set_name: &str = row
                            .try_get("view_set_name")
                            .map_err(|e| DataFusionError::External(e.into()))?;
                        view_instance_id == "global"
                            && self.guard.global_rows_visible(view_set_name)
                    }
                };
                if keep {
                    kept.push(row);
                    if let Some(n) = limit
                        && kept.len() >= n
                    {
                        return Ok(kept);
                    }
                }
            }
        }
        Ok(kept)
    }
}
