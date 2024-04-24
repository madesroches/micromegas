use crate::sql_arrow_bridge::make_column_reader;
use anyhow::{Context, Result};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::common::arrow::array::ListBuilder;
use datafusion::common::arrow::array::StructBuilder;
use datafusion::common::cast::as_struct_array;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use sqlx::Row;

#[derive(Debug, Clone)]
pub struct AnalyticsService {
    data_lake: DataLakeConnection,
}

impl AnalyticsService {
    pub fn new(data_lake: DataLakeConnection) -> Self {
        Self { data_lake }
    }

    pub async fn query_processes(&self, limit: i64) -> Result<RecordBatch> {
        let mut connection = self.data_lake.db_pool.acquire().await?;
        let rows = sqlx::query(
            "SELECT process_id, tsc_frequency
             FROM processes
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&mut *connection)
        .await?;
        if rows.is_empty() {
            return make_empty_record_batch();
        }

        let mut field_readers = vec![];
        for column in rows[0].columns() {
            field_readers
                .push(make_column_reader(column).with_context(|| "error building column reader")?);
        }

        let fields: Vec<_> = field_readers.iter().map(|reader| reader.field()).collect();
        let mut list_builder = ListBuilder::new(StructBuilder::from_fields(fields, rows.len()));
        let struct_builder: &mut StructBuilder = list_builder.values();
        for r in rows {
            for reader in &field_readers {
                reader.extract_column_from_row(&r, struct_builder)?;
            }
            struct_builder.append(true);
        }
        list_builder.append(true);
        let array = list_builder.finish();
        Ok(as_struct_array(array.values())
            .with_context(|| "casting list values to struct srray")?
            .into())
    }
}

fn make_empty_record_batch() -> Result<RecordBatch> {
    let mut list_builder = ListBuilder::new(StructBuilder::from_fields([], 0));
    let array = list_builder.finish();
    Ok(as_struct_array(array.values())
        .with_context(|| "casting list values to struct srray")?
        .into())
}
