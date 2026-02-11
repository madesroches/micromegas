use std::sync::Arc;

use arrow::array::{Int32Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use datafusion_wasm::WasmQueryEngine;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

/// Build valid Arrow IPC stream bytes with columns (id: Int32, name: Utf8).
fn create_test_ipc(ids: &[i32], names: &[&str]) -> Vec<u8> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int32, false),
        Field::new("name", DataType::Utf8, false),
    ]));

    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![
            Arc::new(Int32Array::from(ids.to_vec())),
            Arc::new(StringArray::from(names.to_vec())),
        ],
    )
    .expect("failed to create RecordBatch");

    let mut buf = Vec::new();
    {
        let mut writer =
            StreamWriter::try_new(&mut buf, &schema).expect("failed to create StreamWriter");
        writer.write(&batch).expect("failed to write batch");
        writer.finish().expect("failed to finish stream");
    }
    buf
}

#[wasm_bindgen_test]
fn test_engine_creation() {
    let _engine = WasmQueryEngine::new();
}

#[wasm_bindgen_test]
fn test_register_table() {
    let engine = WasmQueryEngine::new();
    let ipc = create_test_ipc(&[1, 2, 3], &["alice", "bob", "carol"]);
    let row_count = engine
        .register_table("data", &ipc)
        .expect("register_table should succeed");
    assert_eq!(row_count, 3);
}

#[wasm_bindgen_test]
async fn test_execute_sql() {
    let engine = WasmQueryEngine::new();
    let ipc = create_test_ipc(&[1, 2, 3], &["alice", "bob", "carol"]);
    engine
        .register_table("data", &ipc)
        .expect("register_table should succeed");

    let result_bytes = engine
        .execute_sql("SELECT * FROM data")
        .await
        .expect("execute_sql should succeed");

    let cursor = std::io::Cursor::new(&result_bytes);
    let reader = StreamReader::try_new(cursor, None).expect("failed to read IPC result");
    let schema = reader.schema();
    assert_eq!(schema.fields().len(), 2);
    assert_eq!(schema.field(0).name(), "id");
    assert_eq!(schema.field(1).name(), "name");

    let batches: Vec<_> = reader.into_iter().collect::<Result<Vec<_>, _>>().unwrap();
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 3);
}

#[wasm_bindgen_test]
async fn test_aggregate_query() {
    let engine = WasmQueryEngine::new();
    let ipc = create_test_ipc(
        &[1, 2, 3, 4],
        &["alice", "bob", "alice", "bob"],
    );
    engine
        .register_table("data", &ipc)
        .expect("register_table should succeed");

    let result_bytes = engine
        .execute_sql("SELECT name, count(*) as cnt FROM data GROUP BY name ORDER BY cnt DESC")
        .await
        .expect("aggregate query should succeed");

    let cursor = std::io::Cursor::new(&result_bytes);
    let reader = StreamReader::try_new(cursor, None).expect("failed to read IPC result");
    let batches: Vec<_> = reader.into_iter().collect::<Result<Vec<_>, _>>().unwrap();
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 2);

    // Both alice and bob appear twice
    let batch = &batches[0];
    let cnt_col = batch
        .column(1)
        .as_any()
        .downcast_ref::<arrow::array::Int64Array>()
        .expect("cnt column should be Int64");
    assert_eq!(cnt_col.value(0), 2);
    assert_eq!(cnt_col.value(1), 2);
}

#[wasm_bindgen_test]
async fn test_invalid_sql() {
    let engine = WasmQueryEngine::new();
    let result = engine.execute_sql("SELECT * FROM nonexistent").await;
    assert!(result.is_err(), "query on nonexistent table should fail");
}

#[wasm_bindgen_test]
fn test_invalid_ipc_bytes() {
    let engine = WasmQueryEngine::new();
    let result = engine.register_table("bad", &[0, 1, 2, 3]);
    assert!(result.is_err(), "garbage bytes should fail to register");
}

#[wasm_bindgen_test]
async fn test_reset() {
    let engine = WasmQueryEngine::new();
    let ipc = create_test_ipc(&[1], &["alice"]);
    engine
        .register_table("data", &ipc)
        .expect("register_table should succeed");

    engine.reset();

    let result = engine.execute_sql("SELECT * FROM data").await;
    assert!(
        result.is_err(),
        "query should fail after reset clears tables"
    );
}
