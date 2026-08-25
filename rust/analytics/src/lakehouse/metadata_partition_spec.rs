use super::{
    dataframe_time_bounds::DataFrameTimeBounds,
    view::{PartitionSpec, ViewMetadata},
};
use crate::{
    lakehouse::write_partition::{PartitionRowSet, RetireMatch, write_partition_from_rows},
    response_writer::Logger,
    sql_arrow_bridge::rows_to_record_batch,
    time::TimeRange,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use datafusion::{
    arrow::{compute::cast, datatypes::Schema, record_batch::RecordBatch},
    prelude::*,
};
use futures::TryStreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use sqlx::{Row, postgres::PgRow};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

/// Flush threshold on the estimated byte size of the pending chunk -- bounds peak memory to one
/// ~8 MB chunk, not one day's worth of Postgres rows. Byte-based like the Parquet writer's own
/// 100 MB flush (`write_partition.rs`), because a row-count threshold bounds nothing when a few
/// rows carry MB-sized properties/objects_metadata payloads. Deliberately the only flush metric.
const SOURCE_BYTES_PER_BATCH: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub struct MetadataPartitionSpec {
    pub view_metadata: ViewMetadata,
    pub schema: Arc<Schema>,
    pub insert_range: TimeRange,
    pub record_count: i64,
    pub data_sql: Arc<String>,
    pub compute_time_bounds: Arc<dyn DataFrameTimeBounds>,
    /// The sort guarantee this partition's rows will carry, per the caller's `data_sql`'s
    /// `ORDER BY` (e.g. `Some(["insert_time"])` for `BlocksView`). Recorded on `Partition` as-is.
    pub sort_order: Option<Vec<String>>,
}

#[expect(clippy::too_many_arguments)]
pub async fn fetch_metadata_partition_spec(
    pool: &sqlx::PgPool,
    source_count_query: &str,
    data_sql: Arc<String>,
    view_metadata: ViewMetadata,
    schema: Arc<Schema>,
    insert_range: TimeRange,
    compute_time_bounds: Arc<dyn DataFrameTimeBounds>,
    sort_order: Option<Vec<String>>,
) -> Result<MetadataPartitionSpec> {
    //todo: extract this query to allow join (instead of source_table)
    let row = instrument_named!(
        sqlx::query(source_count_query)
            .bind(insert_range.begin)
            .bind(insert_range.end)
            .fetch_one(pool),
        "sql_select_source_count"
    )
    .await
    .with_context(|| "select count source metadata")?;
    Ok(MetadataPartitionSpec {
        view_metadata,
        schema,
        insert_range,
        record_count: row.try_get("count").with_context(|| "reading count")?,
        data_sql,
        compute_time_bounds,
        sort_order,
    })
}

/// Estimates a row's payload size by summing its raw column value byte lengths, counting `NULL`
/// and any non-byte-backed value as 0. This deliberately tracks the JSONB/binary columns
/// (`properties`, `objects_metadata`, `dependencies_metadata`) that dominate blocks-view row
/// width -- an allocator-exact footprint is not needed, only a flush-decision estimate.
fn estimate_row_bytes(row: &PgRow) -> usize {
    let mut total = 0usize;
    for i in 0..row.len() {
        if let Ok(raw) = row.try_get_raw(i)
            && let Ok(bytes) = raw.as_bytes()
        {
            total += bytes.len();
        }
    }
    total
}

/// Aligns a Postgres-inferred batch to the declared file schema's column *types*, casting column
/// by column where they differ. `sql_arrow_bridge`'s mapping is keyed on the Postgres type name
/// alone, so it cannot know that a `TEXT` column is declared as something narrower downstream:
/// `blocks.audience` is `Dictionary(Int32, Utf8)` in the file schema (matching every other view's
/// audience column) but arrives as plain `Utf8`.
///
/// Positional, matching how the parquet writer zips declared fields against batch columns with no
/// name check (see `write_partition::check_non_nullable_columns`). Nullability and field metadata
/// come from the *batch*, never from the declared schema, so this stays a pure type alignment and
/// leaves the `NOT NULL` verdict to the write path's own guard.
pub fn cast_to_file_schema(batch: RecordBatch, file_schema: &Schema) -> Result<RecordBatch> {
    let batch_schema = batch.schema();
    let mut fields = Vec::with_capacity(batch_schema.fields().len());
    let mut columns = Vec::with_capacity(batch.num_columns());
    let mut any_cast = false;
    for (i, field) in batch_schema.fields().iter().enumerate() {
        let column = batch.column(i);
        match file_schema.fields().get(i) {
            Some(declared) if declared.data_type() != field.data_type() => {
                columns.push(cast(column, declared.data_type()).with_context(|| {
                    format!(
                        "casting column {i} ({}) from {:?} to the declared {:?}",
                        field.name(),
                        field.data_type(),
                        declared.data_type()
                    )
                })?);
                fields.push(Arc::new(
                    field
                        .as_ref()
                        .clone()
                        .with_data_type(declared.data_type().clone()),
                ));
                any_cast = true;
            }
            _ => {
                columns.push(column.clone());
                fields.push(field.clone());
            }
        }
    }
    if !any_cast {
        return Ok(batch);
    }
    RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)
        .with_context(|| "rebuilding the batch after casting to the declared file schema")
}

/// Converts the accumulated chunk to a `RecordBatch`, computes its event-time bounds, and sends
/// it as a `PartitionRowSet`. Clears `chunk` in place for reuse by the next flush.
async fn flush_chunk(
    chunk: &mut Vec<PgRow>,
    ctx: &SessionContext,
    file_schema: &Schema,
    compute_time_bounds: &Arc<dyn DataFrameTimeBounds>,
    tx: &Sender<Result<PartitionRowSet, anyhow::Error>>,
) -> Result<()> {
    let record_batch = cast_to_file_schema(
        rows_to_record_batch(chunk).with_context(|| "converting rows to record batch")?,
        file_schema,
    )?;
    chunk.clear();
    let event_time_range = compute_time_bounds
        .get_time_bounds(
            ctx.read_batch(record_batch.clone())
                .with_context(|| "read_batch")?,
        )
        .await?;
    tx.send(Ok(PartitionRowSet::new(
        event_time_range,
        record_batch,
        None,
    )))
    .await
    .with_context(|| "sending partition row set")?;
    Ok(())
}

#[async_trait]
impl PartitionSpec for MetadataPartitionSpec {
    fn is_empty(&self) -> bool {
        self.record_count < 1
    }

    fn get_source_data_hash(&self) -> Vec<u8> {
        self.record_count.to_le_bytes().to_vec()
    }

    async fn write(&self, lake: Arc<DataLakeConnection>, logger: Arc<dyn Logger>) -> Result<()> {
        // Allow empty record_count - write_partition_from_rows will create
        // an empty partition record if no data is sent through the channel
        let desc = format!(
            "[{}, {}] {} {}",
            self.view_metadata.view_set_name,
            self.view_metadata.view_instance_id,
            self.insert_range.begin.to_rfc3339(),
            self.insert_range.end.to_rfc3339()
        );
        logger.write_log_entry(format!("writing {desc}")).await?;

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let join_handle = spawn_with_context(write_partition_from_rows(
            lake.clone(),
            self.view_metadata.clone(),
            self.schema.clone(),
            self.insert_range,
            self.get_source_data_hash(),
            self.sort_order.clone(),
            RetireMatch::Containment,
            Vec::new(),
            rx,
            logger.clone(),
        ));

        let stream_result: Result<()> = instrument_named!(
            async {
                if self.record_count > 0 {
                    let mut rows = sqlx::query(&self.data_sql)
                        .bind(self.insert_range.begin)
                        .bind(self.insert_range.end)
                        .fetch(&lake.db_pool);
                    let ctx = SessionContext::new();
                    let mut chunk: Vec<PgRow> = Vec::new();
                    let mut chunk_bytes = 0usize;
                    while let Some(row) = rows.try_next().await? {
                        chunk_bytes += estimate_row_bytes(&row);
                        chunk.push(row);
                        if chunk_bytes >= SOURCE_BYTES_PER_BATCH {
                            flush_chunk(
                                &mut chunk,
                                &ctx,
                                &self.schema,
                                &self.compute_time_bounds,
                                &tx,
                            )
                            .await?;
                            chunk_bytes = 0;
                        }
                    }
                    if !chunk.is_empty() {
                        flush_chunk(
                            &mut chunk,
                            &ctx,
                            &self.schema,
                            &self.compute_time_bounds,
                            &tx,
                        )
                        .await?;
                    }
                }
                Ok(())
            },
            "sql_select_partition_source_data"
        )
        .await;

        match stream_result {
            Ok(()) => {
                drop(tx);
                join_handle.await??;
                Ok(())
            }
            Err(e) => {
                // mirror create_merged_partition's error path: send the abort through the
                // channel before dropping it, so write_partition_from_rows sees an Err item
                // instead of a plain closed-channel end-of-stream and does not commit a
                // partial partition.
                let _ = tx
                    .send(Err(anyhow::anyhow!("metadata partition stream aborted")))
                    .await;
                drop(tx);
                let _ = join_handle.await;
                Err(e)
            }
        }
    }
}
