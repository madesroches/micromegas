use datafusion::arrow::array::{Array, BinaryArray, Int64Array, RecordBatch};
use datafusion::arrow::compute;
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::prelude::SessionContext;
use std::sync::Arc;

async fn eval_array_length(ctx: &SessionContext, json_literal: &str) -> Vec<Option<i64>> {
    let sql = format!("SELECT jsonb_array_length(jsonb_parse('{json_literal}')) as len");
    let df = ctx.sql(&sql).await.expect("SQL query failed");
    let results = df.collect().await.expect("failed to collect results");
    assert_eq!(results.len(), 1);
    let batch = &results[0];
    let col = compute::cast(batch.column(0), &DataType::Int64).expect("cast to Int64");
    let arr = col
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("should be Int64Array");
    (0..arr.len())
        .map(|i| {
            if arr.is_null(i) {
                None
            } else {
                Some(arr.value(i))
            }
        })
        .collect()
}

fn make_ctx() -> SessionContext {
    let ctx = SessionContext::new();
    micromegas_datafusion_extensions::register_extension_udfs(&ctx);
    ctx
}

#[tokio::test]
async fn test_basic_array() {
    let ctx = make_ctx();
    let result = eval_array_length(&ctx, "[1, 2, 3]").await;
    assert_eq!(result, vec![Some(3)]);
}

#[tokio::test]
async fn test_empty_array() {
    let ctx = make_ctx();
    let result = eval_array_length(&ctx, "[]").await;
    assert_eq!(result, vec![Some(0)]);
}

#[tokio::test]
async fn test_object_returns_null() {
    let ctx = make_ctx();
    let result = eval_array_length(&ctx, r#"{"a": 1}"#).await;
    assert_eq!(result, vec![None]);
}

#[tokio::test]
async fn test_scalar_returns_null() {
    let ctx = make_ctx();
    let result = eval_array_length(&ctx, "42").await;
    assert_eq!(result, vec![None]);
}

#[tokio::test]
async fn test_string_scalar_returns_null() {
    let ctx = make_ctx();
    let result = eval_array_length(&ctx, r#""hello""#).await;
    assert_eq!(result, vec![None]);
}

#[tokio::test]
async fn test_null_input() {
    let ctx = make_ctx();
    let schema = Arc::new(Schema::new(vec![Field::new("val", DataType::Binary, true)]));
    let array: Arc<BinaryArray> = Arc::new(BinaryArray::from(vec![None::<&[u8]>]));
    let batch = RecordBatch::try_new(schema, vec![array]).expect("failed to create batch");
    ctx.register_batch("null_table", batch)
        .expect("failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_array_length(val) as len FROM null_table")
        .await
        .expect("SQL query failed");
    let results = df.collect().await.expect("failed to collect results");
    assert_eq!(results.len(), 1);
    let result_batch = &results[0];
    assert_eq!(result_batch.num_rows(), 1);
    assert!(result_batch.column(0).is_null(0));
}

#[tokio::test]
async fn test_nested_array() {
    let ctx = make_ctx();
    let result = eval_array_length(&ctx, r#"[[1, 2], [3], [4, 5, 6]]"#).await;
    assert_eq!(result, vec![Some(3)]);
}

#[tokio::test]
async fn test_single_element_array() {
    let ctx = make_ctx();
    let result = eval_array_length(&ctx, r#"["only"]"#).await;
    assert_eq!(result, vec![Some(1)]);
}
