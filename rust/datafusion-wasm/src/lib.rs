use std::sync::Arc;

use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use datafusion::execution::SessionStateBuilder;
use datafusion::prelude::*;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct WasmQueryEngine {
    ctx: SessionContext,
}

#[wasm_bindgen]
impl WasmQueryEngine {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        let state = SessionStateBuilder::new()
            .with_default_features()
            .build();
        Self {
            ctx: SessionContext::new_with_state(state),
        }
    }

    /// Register Arrow IPC stream bytes as a named table.
    /// Returns the number of rows registered.
    pub fn register_table(&self, name: &str, ipc_bytes: &[u8]) -> Result<u32, JsValue> {
        let cursor = std::io::Cursor::new(ipc_bytes);
        let reader = StreamReader::try_new(cursor, None)
            .map_err(|e| JsValue::from_str(&format!("Failed to read IPC stream: {e}")))?;

        let schema = reader.schema();
        let mut batches = Vec::new();
        let mut row_count: u32 = 0;

        for batch_result in reader {
            let batch = batch_result
                .map_err(|e| JsValue::from_str(&format!("Failed to read batch: {e}")))?;
            row_count += batch.num_rows() as u32;
            batches.push(batch);
        }

        let table = datafusion::datasource::MemTable::try_new(schema, vec![batches])
            .map_err(|e| JsValue::from_str(&format!("Failed to create MemTable: {e}")))?;

        self.ctx
            .register_table(name, Arc::new(table))
            .map_err(|e| JsValue::from_str(&format!("Failed to register table: {e}")))?;

        Ok(row_count)
    }

    /// Execute SQL, return Arrow IPC stream bytes.
    pub async fn execute_sql(&self, sql: &str) -> Result<Vec<u8>, JsValue> {
        let df = self
            .ctx
            .sql(sql)
            .await
            .map_err(|e| JsValue::from_str(&format!("SQL error: {e}")))?;

        let schema = Arc::new(df.schema().as_arrow().clone());

        let batches = df
            .collect()
            .await
            .map_err(|e| JsValue::from_str(&format!("Execution error: {e}")))?;

        let mut buf = Vec::new();
        {
            let mut writer = StreamWriter::try_new(&mut buf, &schema)
                .map_err(|e| JsValue::from_str(&format!("IPC writer error: {e}")))?;

            for batch in &batches {
                writer
                    .write(batch)
                    .map_err(|e| JsValue::from_str(&format!("IPC write error: {e}")))?;
            }

            writer
                .finish()
                .map_err(|e| JsValue::from_str(&format!("IPC finish error: {e}")))?;
        }

        Ok(buf)
    }

    /// Deregister all tables.
    pub fn reset(&self) {
        let names: Vec<String> = self
            .ctx
            .catalog_names()
            .into_iter()
            .flat_map(|catalog_name| {
                self.ctx
                    .catalog(&catalog_name)
                    .into_iter()
                    .flat_map(move |catalog| {
                        catalog.schema_names().into_iter().flat_map(move |schema_name| {
                            catalog
                                .schema(&schema_name)
                                .map(|schema| {
                                    schema
                                        .table_names()
                                        .into_iter()
                                        .map(move |t| t.to_string())
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default()
                        })
                    })
            })
            .collect();

        for table_name in names {
            let _ = self.ctx.deregister_table(&table_name);
        }
    }
}
