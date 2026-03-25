use datafusion::arrow::array::{Array, BinaryArray, RecordBatch, StringArray};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::catalog::{TableFunctionImpl, TableProvider};
use datafusion::logical_expr::Cast;
use datafusion::prelude::{Expr, SessionContext};
use datafusion::scalar::ScalarValue;
use micromegas_datafusion_extensions::jsonb::each::{JsonbEachTableFunction, JsonbEachTableProvider};
use std::sync::Arc;

fn parse_json_to_jsonb(json_str: &str) -> Vec<u8> {
    let parsed = jsonb::parse_value(json_str.as_bytes()).expect("failed to parse test JSON");
    let mut buffer = vec![];
    parsed.write_to_vec(&mut buffer);
    buffer
}

fn create_jsonb_each_provider(json_str: &str) -> JsonbEachTableProvider {
    let jsonb_bytes = parse_json_to_jsonb(json_str);
    let scalar = ScalarValue::Binary(Some(jsonb_bytes));
    JsonbEachTableProvider::from_scalar(scalar).expect("failed to create provider")
}

async fn collect_jsonb_each(
    provider: &JsonbEachTableProvider,
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
async fn test_simple_object() {
    let provider = create_jsonb_each_provider(r#"{"a": 1, "b": "hello"}"#);
    let batch = collect_jsonb_each(&provider, None).await;

    assert_eq!(batch.num_rows(), 2);
    assert_eq!(batch.num_columns(), 2);

    let keys = batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("key column should be StringArray");
    let values = batch
        .column(1)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("value column should be BinaryArray");

    // Collect keys into a set for order-independent comparison
    let mut key_set: Vec<String> = (0..keys.len()).map(|i| keys.value(i).to_string()).collect();
    key_set.sort();
    assert_eq!(key_set, vec!["a", "b"]);

    // Verify values are valid JSONB
    for i in 0..values.len() {
        assert!(!values.value(i).is_empty(), "value should not be empty");
    }
}

#[tokio::test]
async fn test_empty_object() {
    let provider = create_jsonb_each_provider(r#"{}"#);
    let batch = collect_jsonb_each(&provider, None).await;

    assert_eq!(batch.num_rows(), 0);
    assert_eq!(batch.num_columns(), 2);
}

#[tokio::test]
async fn test_array_input() {
    let provider = create_jsonb_each_provider(r#"[10, "hello", true]"#);
    let batch = collect_jsonb_each(&provider, None).await;

    assert_eq!(batch.num_rows(), 3);

    let keys = batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("key column should be StringArray");
    let key_list: Vec<String> = (0..keys.len()).map(|i| keys.value(i).to_string()).collect();
    assert_eq!(key_list, vec!["0", "1", "2"]);

    let values = batch
        .column(1)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("value column should be BinaryArray");
    for i in 0..values.len() {
        assert!(!values.value(i).is_empty());
    }
}

#[tokio::test]
async fn test_scalar_error() {
    let provider = create_jsonb_each_provider(r#"42"#);
    let ctx = SessionContext::new();
    let state = ctx.state();
    let result = provider.scan(&state, None, &[], None).await;

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not a JSONB object or array"),
        "unexpected error: {err_msg}"
    );
}

#[tokio::test]
async fn test_nested_values() {
    let provider =
        create_jsonb_each_provider(r#"{"obj": {"nested": true}, "arr": [1, 2, 3], "str": "hi"}"#);
    let batch = collect_jsonb_each(&provider, None).await;

    assert_eq!(batch.num_rows(), 3);

    let keys = batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("key column should be StringArray");
    let values = batch
        .column(1)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("value column should be BinaryArray");

    // All values should be valid non-empty JSONB bytes
    for i in 0..values.len() {
        let val_bytes = values.value(i);
        assert!(
            !val_bytes.is_empty(),
            "value for key '{}' should not be empty",
            keys.value(i)
        );
        // Verify each value is parseable as RawJsonb
        let _raw = jsonb::RawJsonb::new(val_bytes);
    }
}

#[tokio::test]
async fn test_limit() {
    let provider = create_jsonb_each_provider(r#"{"a": 1, "b": 2, "c": 3, "d": 4, "e": 5}"#);
    let batch = collect_jsonb_each(&provider, Some(1)).await;

    assert_eq!(batch.num_rows(), 1);
}

#[tokio::test]
async fn test_schema() {
    let provider = create_jsonb_each_provider(r#"{"x": 1}"#);
    let schema = provider.schema();

    assert_eq!(schema.fields().len(), 2);

    let key_field = schema.field(0);
    assert_eq!(key_field.name(), "key");
    assert_eq!(*key_field.data_type(), DataType::Utf8);
    assert!(!key_field.is_nullable());

    let value_field = schema.field(1);
    assert_eq!(value_field.name(), "value");
    assert_eq!(*value_field.data_type(), DataType::Binary);
    assert!(!value_field.is_nullable());
}

#[tokio::test]
async fn test_sql_integration() {
    let ctx = SessionContext::new();
    micromegas_datafusion_extensions::register_extension_udfs(&ctx);

    // Create a table with a JSONB Binary column
    let jsonb_bytes = parse_json_to_jsonb(r#"{"name": "test", "version": "1.0"}"#);
    let schema = Arc::new(Schema::new(vec![Field::new(
        "props",
        DataType::Binary,
        false,
    )]));
    let array: Arc<BinaryArray> = Arc::new(BinaryArray::from(vec![jsonb_bytes.as_slice()]));
    let batch = RecordBatch::try_new(schema, vec![array]).expect("failed to create batch");
    ctx.register_batch("test_table", batch)
        .expect("failed to register batch");

    let df = ctx
        .sql("SELECT key, value FROM jsonb_each((SELECT props FROM test_table))")
        .await
        .expect("SQL query failed");

    let results = df.collect().await.expect("failed to collect results");
    assert_eq!(results.len(), 1);
    let result_batch = &results[0];
    assert_eq!(result_batch.num_rows(), 2);

    let keys = result_batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("key column should be StringArray");
    let mut key_list: Vec<String> = (0..keys.len()).map(|i| keys.value(i).to_string()).collect();
    key_list.sort();
    assert_eq!(key_list, vec!["name", "version"]);
}

#[tokio::test]
async fn test_multiple_rows_concatenated() {
    let ctx = SessionContext::new();
    micromegas_datafusion_extensions::register_extension_udfs(&ctx);

    // Create a table with multiple JSONB rows — results should be concatenated
    let jsonb1 = parse_json_to_jsonb(r#"{"a": 1}"#);
    let jsonb2 = parse_json_to_jsonb(r#"{"b": 2}"#);
    let schema = Arc::new(Schema::new(vec![Field::new(
        "props",
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
        .sql("SELECT key, value FROM jsonb_each((SELECT props FROM multi_table))")
        .await
        .expect("SQL query failed");

    let results = df.collect().await.expect("failed to collect results");
    assert_eq!(results.len(), 1);
    let result_batch = &results[0];
    assert_eq!(result_batch.num_rows(), 2);

    let keys = result_batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("key column should be StringArray");
    let mut key_list: Vec<String> = (0..keys.len()).map(|i| keys.value(i).to_string()).collect();
    key_list.sort();
    assert_eq!(key_list, vec!["a", "b"]);
}

#[tokio::test]
async fn test_jsonb_each_with_jsonb_parse_expression() {
    let ctx = SessionContext::new();
    micromegas_datafusion_extensions::register_extension_udfs(&ctx);

    let df = ctx
        .sql("SELECT key, value FROM jsonb_each(jsonb_parse('{\"a\": 1, \"b\": 2}'))")
        .await
        .expect("SQL query failed");

    let results = df.collect().await.expect("failed to collect results");
    assert_eq!(results.len(), 1);
    let batch = &results[0];
    assert_eq!(batch.num_rows(), 2);

    let keys = batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("key column should be StringArray");
    let mut key_list: Vec<String> = (0..keys.len()).map(|i| keys.value(i).to_string()).collect();
    key_list.sort();
    assert_eq!(key_list, vec!["a", "b"]);
}

#[tokio::test]
async fn test_jsonb_each_with_jsonb_parse_composability() {
    let ctx = SessionContext::new();
    micromegas_datafusion_extensions::register_extension_udfs(&ctx);

    let df = ctx
        .sql("SELECT key, jsonb_as_string(value) as val FROM jsonb_each(jsonb_parse('{\"x\": \"hello\", \"y\": \"world\"}'))")
        .await
        .expect("SQL query failed");

    let results = df.collect().await.expect("failed to collect results");
    assert_eq!(results.len(), 1);
    let batch = &results[0];
    assert_eq!(batch.num_rows(), 2);

    let keys = batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("key column should be StringArray");
    let vals = batch
        .column(1)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("val column should be StringArray");

    let mut rows: Vec<(String, String)> = (0..keys.len())
        .map(|i| (keys.value(i).to_string(), vals.value(i).to_string()))
        .collect();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            ("x".to_string(), "hello".to_string()),
            ("y".to_string(), "world".to_string()),
        ]
    );
}

#[test]
fn test_call_accepts_cast_expression() {
    let func = JsonbEachTableFunction::new();
    // Construct a Cast expression — neither Literal nor ScalarSubquery
    let inner = Expr::Literal(ScalarValue::Binary(Some(vec![])), None);
    let cast_expr = Expr::Cast(Cast::new(Box::new(inner), DataType::Binary));
    let result = func.call(&[cast_expr]);
    assert!(
        result.is_ok(),
        "call() should accept Cast expression, got: {result:?}"
    );
}
