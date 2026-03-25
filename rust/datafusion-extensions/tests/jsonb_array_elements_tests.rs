use datafusion::arrow::array::{Array, BinaryArray, RecordBatch, StringArray};
use datafusion::arrow::compute;
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::catalog::{TableFunctionImpl, TableProvider};
use datafusion::logical_expr::Cast;
use datafusion::prelude::{Expr, SessionContext};
use datafusion::scalar::ScalarValue;
use micromegas_datafusion_extensions::jsonb::array_elements::{
    JsonbArrayElementsTableFunction, JsonbArrayElementsTableProvider,
};
use std::sync::Arc;

fn parse_json_to_jsonb(json_str: &str) -> Vec<u8> {
    let parsed = jsonb::parse_value(json_str.as_bytes()).expect("failed to parse test JSON");
    let mut buffer = vec![];
    parsed.write_to_vec(&mut buffer);
    buffer
}

fn create_provider(json_str: &str) -> JsonbArrayElementsTableProvider {
    let jsonb_bytes = parse_json_to_jsonb(json_str);
    let scalar = ScalarValue::Binary(Some(jsonb_bytes));
    JsonbArrayElementsTableProvider::from_scalar(scalar).expect("failed to create provider")
}

async fn collect_results(
    provider: &JsonbArrayElementsTableProvider,
    limit: Option<usize>,
) -> RecordBatch {
    let ctx = SessionContext::new();
    let state = ctx.state();
    let plan = provider
        .scan(&state, None, &[], limit)
        .await
        .expect("scan failed");
    let task_ctx = state.task_ctx();
    let batches = datafusion::physical_plan::collect(plan, task_ctx)
        .await
        .expect("collect failed");
    assert_eq!(batches.len(), 1, "expected exactly one batch");
    batches.into_iter().next().expect("no batches")
}

#[tokio::test]
async fn test_simple_array() {
    let provider = create_provider(r#"[1, 2, 3]"#);
    let batch = collect_results(&provider, None).await;

    assert_eq!(batch.num_rows(), 3);
    assert_eq!(batch.num_columns(), 1);

    let values = batch
        .column(0)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("value column should be BinaryArray");

    for i in 0..values.len() {
        assert!(!values.value(i).is_empty(), "value should not be empty");
    }
}

#[tokio::test]
async fn test_array_of_objects() {
    let provider = create_provider(r#"[{"name": "Alice"}, {"name": "Bob"}]"#);
    let batch = collect_results(&provider, None).await;

    assert_eq!(batch.num_rows(), 2);
    assert_eq!(batch.num_columns(), 1);

    let values = batch
        .column(0)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("value column should be BinaryArray");

    for i in 0..values.len() {
        assert!(!values.value(i).is_empty());
        let _raw = jsonb::RawJsonb::new(values.value(i));
    }
}

#[tokio::test]
async fn test_empty_array() {
    let provider = create_provider(r#"[]"#);
    let batch = collect_results(&provider, None).await;

    assert_eq!(batch.num_rows(), 0);
    assert_eq!(batch.num_columns(), 1);
}

#[tokio::test]
async fn test_object_input_error() {
    let provider = create_provider(r#"{"a": 1}"#);
    let ctx = SessionContext::new();
    let state = ctx.state();
    let result = provider.scan(&state, None, &[], None).await;

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not a JSONB array"),
        "unexpected error: {err_msg}"
    );
}

#[tokio::test]
async fn test_scalar_input_error() {
    let provider = create_provider(r#"42"#);
    let ctx = SessionContext::new();
    let state = ctx.state();
    let result = provider.scan(&state, None, &[], None).await;

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not a JSONB array"),
        "unexpected error: {err_msg}"
    );
}

#[tokio::test]
async fn test_nested_values() {
    let provider = create_provider(r#"[{"nested": true}, [1, 2, 3], "hello", 42, null]"#);
    let batch = collect_results(&provider, None).await;

    assert_eq!(batch.num_rows(), 5);

    let values = batch
        .column(0)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("value column should be BinaryArray");

    for i in 0..values.len() {
        let val_bytes = values.value(i);
        assert!(
            !val_bytes.is_empty(),
            "value at index {i} should not be empty"
        );
        let _raw = jsonb::RawJsonb::new(val_bytes);
    }
}

#[tokio::test]
async fn test_limit() {
    let provider = create_provider(r#"[1, 2, 3, 4, 5]"#);
    let batch = collect_results(&provider, Some(2)).await;

    assert_eq!(batch.num_rows(), 2);
}

#[tokio::test]
async fn test_schema() {
    let provider = create_provider(r#"[1]"#);
    let schema = provider.schema();

    assert_eq!(schema.fields().len(), 1);

    let value_field = schema.field(0);
    assert_eq!(value_field.name(), "value");
    assert_eq!(*value_field.data_type(), DataType::Binary);
    assert!(!value_field.is_nullable());
}

#[tokio::test]
async fn test_sql_integration() {
    let ctx = SessionContext::new();
    micromegas_datafusion_extensions::register_extension_udfs(&ctx);

    let jsonb_bytes = parse_json_to_jsonb(r#"["apple", "banana", "cherry"]"#);
    let schema = Arc::new(Schema::new(vec![Field::new(
        "data",
        DataType::Binary,
        false,
    )]));
    let array: Arc<BinaryArray> = Arc::new(BinaryArray::from(vec![jsonb_bytes.as_slice()]));
    let batch = RecordBatch::try_new(schema, vec![array]).expect("failed to create batch");
    ctx.register_batch("test_table", batch)
        .expect("failed to register batch");

    let df = ctx
        .sql("SELECT value FROM jsonb_array_elements((SELECT data FROM test_table))")
        .await
        .expect("SQL query failed");

    let results = df.collect().await.expect("failed to collect results");
    assert_eq!(results.len(), 1);
    let result_batch = &results[0];
    assert_eq!(result_batch.num_rows(), 3);
}

#[tokio::test]
async fn test_with_jsonb_parse_expression() {
    let ctx = SessionContext::new();
    micromegas_datafusion_extensions::register_extension_udfs(&ctx);

    let df = ctx
        .sql("SELECT value FROM jsonb_array_elements(jsonb_parse('[1, 2, 3]'))")
        .await
        .expect("SQL query failed");

    let results = df.collect().await.expect("failed to collect results");
    assert_eq!(results.len(), 1);
    let batch = &results[0];
    assert_eq!(batch.num_rows(), 3);
}

#[tokio::test]
async fn test_composability_with_jsonb_as_string() {
    let ctx = SessionContext::new();
    micromegas_datafusion_extensions::register_extension_udfs(&ctx);

    let df = ctx
        .sql("SELECT jsonb_as_string(value) as val FROM jsonb_array_elements(jsonb_parse('[\"hello\", \"world\"]'))")
        .await
        .expect("SQL query failed");

    let results = df.collect().await.expect("failed to collect results");
    assert_eq!(results.len(), 1);
    let batch = &results[0];
    assert_eq!(batch.num_rows(), 2);

    let val_col =
        compute::cast(batch.column(0), &DataType::Utf8).expect("failed to cast val column to Utf8");
    let vals = val_col
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("val column should be castable to StringArray");

    let mut val_list: Vec<String> = (0..vals.len()).map(|i| vals.value(i).to_string()).collect();
    val_list.sort();
    assert_eq!(val_list, vec!["hello", "world"]);
}

#[tokio::test]
async fn test_composability_with_jsonb_get() {
    let ctx = SessionContext::new();
    micromegas_datafusion_extensions::register_extension_udfs(&ctx);

    let df = ctx
        .sql(
            "SELECT jsonb_as_string(jsonb_get(value, 'name')) as name \
             FROM jsonb_array_elements(jsonb_parse('[{\"name\": \"Alice\"}, {\"name\": \"Bob\"}]'))",
        )
        .await
        .expect("SQL query failed");

    let results = df.collect().await.expect("failed to collect results");
    assert_eq!(results.len(), 1);
    let batch = &results[0];
    assert_eq!(batch.num_rows(), 2);

    let name_col =
        compute::cast(batch.column(0), &DataType::Utf8).expect("failed to cast name column");
    let names = name_col
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("name column should be StringArray");

    let mut name_list: Vec<String> = (0..names.len())
        .map(|i| names.value(i).to_string())
        .collect();
    name_list.sort();
    assert_eq!(name_list, vec!["Alice", "Bob"]);
}

#[tokio::test]
async fn test_multiple_rows_concatenated() {
    let ctx = SessionContext::new();
    micromegas_datafusion_extensions::register_extension_udfs(&ctx);

    let jsonb1 = parse_json_to_jsonb(r#"[1, 2]"#);
    let jsonb2 = parse_json_to_jsonb(r#"[3, 4]"#);
    let schema = Arc::new(Schema::new(vec![Field::new(
        "data",
        DataType::Binary,
        false,
    )]));
    let array: Arc<BinaryArray> = Arc::new(BinaryArray::from(vec![
        jsonb1.as_slice(),
        jsonb2.as_slice(),
    ]));
    let batch = RecordBatch::try_new(schema, vec![array]).expect("failed to create batch");
    ctx.register_batch("multi_table", batch)
        .expect("failed to register batch");

    let df = ctx
        .sql("SELECT value FROM jsonb_array_elements((SELECT data FROM multi_table))")
        .await
        .expect("SQL query failed");

    let results = df.collect().await.expect("failed to collect results");
    assert_eq!(results.len(), 1);
    let result_batch = &results[0];
    assert_eq!(result_batch.num_rows(), 4);
}

#[test]
fn test_call_accepts_cast_expression() {
    let func = JsonbArrayElementsTableFunction::new();
    let inner = Expr::Literal(ScalarValue::Binary(Some(vec![])), None);
    let cast_expr = Expr::Cast(Cast::new(Box::new(inner), DataType::Binary));
    let result = func.call(&[cast_expr]);
    assert!(
        result.is_ok(),
        "call() should accept Cast expression, got: {result:?}"
    );
}
