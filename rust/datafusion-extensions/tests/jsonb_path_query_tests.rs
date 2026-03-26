use datafusion::arrow::array::{
    Array, BinaryArray, BinaryDictionaryBuilder, DictionaryArray, StringArray,
};
use datafusion::arrow::datatypes::{DataType, Field, Int32Type, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::prelude::SessionContext;
use jsonb::RawJsonb;
use std::sync::Arc;

fn parse_json_to_jsonb(json_str: &str) -> Vec<u8> {
    let parsed = jsonb::parse_value(json_str.as_bytes()).expect("failed to parse test JSON");
    let mut buffer = vec![];
    parsed.write_to_vec(&mut buffer);
    buffer
}

fn jsonb_to_json_string(bytes: &[u8]) -> String {
    RawJsonb::new(bytes).to_string()
}

fn setup_ctx() -> SessionContext {
    let ctx = SessionContext::new();
    micromegas_datafusion_extensions::register_extension_udfs(&ctx);
    ctx
}

fn create_binary_table(ctx: &SessionContext, table_name: &str, json_strings: &[&str]) {
    create_nullable_binary_table(
        ctx,
        table_name,
        &json_strings.iter().map(|s| Some(*s)).collect::<Vec<_>>(),
    );
}

fn create_nullable_binary_table(
    ctx: &SessionContext,
    table_name: &str,
    json_strings: &[Option<&str>],
) {
    let jsonb_values: Vec<Option<Vec<u8>>> = json_strings
        .iter()
        .map(|s| s.map(|s| parse_json_to_jsonb(s)))
        .collect();
    let refs: Vec<Option<&[u8]>> = jsonb_values.iter().map(|v| v.as_deref()).collect();
    let schema = Arc::new(Schema::new(vec![Field::new(
        "data",
        DataType::Binary,
        true,
    )]));
    let array: Arc<BinaryArray> = Arc::new(refs.into_iter().collect::<BinaryArray>());
    let batch = RecordBatch::try_new(schema, vec![array]).expect("failed to create batch");
    ctx.register_batch(table_name, batch)
        .expect("failed to register batch");
}

fn create_dict_table(ctx: &SessionContext, table_name: &str, json_strings: &[&str]) {
    let mut builder = BinaryDictionaryBuilder::<Int32Type>::new();
    for json_str in json_strings {
        let jsonb_bytes = parse_json_to_jsonb(json_str);
        builder.append_value(&jsonb_bytes);
    }
    let dict_array = builder.finish();
    let schema = Arc::new(Schema::new(vec![Field::new(
        "data",
        dict_array.data_type().clone(),
        false,
    )]));
    let batch =
        RecordBatch::try_new(schema, vec![Arc::new(dict_array)]).expect("failed to create batch");
    ctx.register_batch(table_name, batch)
        .expect("failed to register batch");
}

async fn query_first_result(ctx: &SessionContext, sql: &str) -> Vec<Option<String>> {
    let df = ctx.sql(sql).await.expect("SQL query failed");
    let batches = df.collect().await.expect("failed to collect results");
    let mut results = vec![];
    for batch in &batches {
        let col = batch.column(0);
        match col.data_type() {
            DataType::Dictionary(_, _) => {
                let dict = col
                    .as_any()
                    .downcast_ref::<DictionaryArray<Int32Type>>()
                    .expect("expected dict array");
                let values = dict
                    .values()
                    .as_any()
                    .downcast_ref::<BinaryArray>()
                    .expect("expected binary values");
                for i in 0..dict.len() {
                    if dict.is_null(i) {
                        results.push(None);
                    } else {
                        let key = dict.keys().value(i) as usize;
                        results.push(Some(jsonb_to_json_string(values.value(key))));
                    }
                }
            }
            _ => panic!("unexpected data type: {:?}", col.data_type()),
        }
    }
    results
}

// --- jsonb_path_query_first tests ---

#[tokio::test]
async fn test_path_query_first_simple_key() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"name": "Alice", "age": 30}"#]);
    let results =
        query_first_result(&ctx, "SELECT jsonb_path_query_first(data, '$.name') FROM t").await;
    assert_eq!(results, vec![Some("\"Alice\"".to_string())]);
}

#[tokio::test]
async fn test_path_query_first_nested_path() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"a": {"b": {"c": 42}}}"#]);
    let results = query_first_result(
        &ctx,
        "SELECT jsonb_path_query_first(data, '$.a.b.c') FROM t",
    )
    .await;
    assert_eq!(results, vec![Some("42".to_string())]);
}

#[tokio::test]
async fn test_path_query_first_array_index() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"items": [10, 20, 30]}"#]);
    let results = query_first_result(
        &ctx,
        "SELECT jsonb_path_query_first(data, '$.items[1]') FROM t",
    )
    .await;
    assert_eq!(results, vec![Some("20".to_string())]);
}

#[tokio::test]
async fn test_path_query_first_no_match_returns_null() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"name": "Alice"}"#]);
    let results = query_first_result(
        &ctx,
        "SELECT jsonb_path_query_first(data, '$.missing') FROM t",
    )
    .await;
    assert_eq!(results, vec![None]);
}

#[tokio::test]
async fn test_path_query_first_multiple_rows() {
    let ctx = setup_ctx();
    create_binary_table(
        &ctx,
        "t",
        &[r#"{"val": 1}"#, r#"{"val": 2}"#, r#"{"other": 3}"#],
    );
    let results =
        query_first_result(&ctx, "SELECT jsonb_path_query_first(data, '$.val') FROM t").await;
    assert_eq!(
        results,
        vec![Some("1".to_string()), Some("2".to_string()), None]
    );
}

#[tokio::test]
async fn test_path_query_first_dict_input() {
    let ctx = setup_ctx();
    create_dict_table(&ctx, "t", &[r#"{"x": "hello"}"#, r#"{"x": "world"}"#]);
    let results =
        query_first_result(&ctx, "SELECT jsonb_path_query_first(data, '$.x') FROM t").await;
    assert_eq!(
        results,
        vec![Some("\"hello\"".to_string()), Some("\"world\"".to_string()),]
    );
}

#[tokio::test]
async fn test_path_query_first_wildcard_returns_first() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"[1, 2, 3]"#]);
    let results =
        query_first_result(&ctx, "SELECT jsonb_path_query_first(data, '$[*]') FROM t").await;
    assert_eq!(results, vec![Some("1".to_string())]);
}

// --- jsonb_path_query tests ---

#[tokio::test]
async fn test_path_query_all_matches() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"[1, 2, 3]"#]);
    let results = query_first_result(&ctx, "SELECT jsonb_path_query(data, '$[*]') FROM t").await;
    assert_eq!(results, vec![Some("[1,2,3]".to_string())]);
}

#[tokio::test]
async fn test_path_query_nested_array_wildcard() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"items": [{"name": "a"}, {"name": "b"}]}"#]);
    let results = query_first_result(
        &ctx,
        "SELECT jsonb_path_query(data, '$.items[*].name') FROM t",
    )
    .await;
    assert_eq!(results, vec![Some("[\"a\",\"b\"]".to_string())]);
}

#[tokio::test]
async fn test_path_query_no_match_returns_empty_array() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"name": "Alice"}"#]);
    let results =
        query_first_result(&ctx, "SELECT jsonb_path_query(data, '$.missing') FROM t").await;
    assert_eq!(results, vec![Some("[]".to_string())]);
}

#[tokio::test]
async fn test_path_query_dict_input() {
    let ctx = setup_ctx();
    create_dict_table(&ctx, "t", &[r#"{"a": [1, 2]}"#, r#"{"a": [3, 4]}"#]);
    let results = query_first_result(&ctx, "SELECT jsonb_path_query(data, '$.a[*]') FROM t").await;
    assert_eq!(
        results,
        vec![Some("[1,2]".to_string()), Some("[3,4]".to_string())]
    );
}

#[tokio::test]
async fn test_path_query_single_key() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"name": "test", "value": 42}"#]);
    let results = query_first_result(&ctx, "SELECT jsonb_path_query(data, '$.name') FROM t").await;
    assert_eq!(results, vec![Some("[\"test\"]".to_string())]);
}

#[tokio::test]
async fn test_invalid_path_returns_error() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"a": 1}"#]);
    let result = ctx
        .sql("SELECT jsonb_path_query_first(data, '$[invalid') FROM t")
        .await;
    // The error might happen at plan or execution time
    match result {
        Ok(df) => {
            let collect_result = df.collect().await;
            assert!(collect_result.is_err(), "expected error for invalid path");
        }
        Err(_) => {} // Error at plan time is also acceptable
    }
}

#[tokio::test]
async fn test_composable_with_format_json() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"user": {"name": "Alice", "age": 30}}"#]);
    let df = ctx
        .sql("SELECT jsonb_format_json(jsonb_path_query_first(data, '$.user')) FROM t")
        .await
        .expect("SQL failed");
    let batches = df.collect().await.expect("collect failed");
    let batch = &batches[0];
    let col = batch.column(0);
    let dict = col
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("expected dict");
    let values = dict
        .values()
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("expected string values");
    let key = dict.keys().value(0) as usize;
    let json_str = values.value(key);
    // Should be a valid JSON object
    assert!(json_str.contains("\"name\""));
    assert!(json_str.contains("\"Alice\""));
}

#[tokio::test]
async fn test_composable_with_jsonb_as_string() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"greeting": "hello world"}"#]);
    let df = ctx
        .sql("SELECT jsonb_as_string(jsonb_path_query_first(data, '$.greeting')) FROM t")
        .await
        .expect("SQL failed");
    let batches = df.collect().await.expect("collect failed");
    let batch = &batches[0];
    let col = batch.column(0);
    let dict = col
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("expected dict");
    let values = dict
        .values()
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("expected string values");
    let key = dict.keys().value(0) as usize;
    assert_eq!(values.value(key), "hello world");
}

#[tokio::test]
async fn test_path_query_first_filter_expression() {
    let ctx = setup_ctx();
    create_binary_table(
        &ctx,
        "t",
        &[r#"{"items": [{"key": "foo", "value": 1}, {"key": "bar", "value": 2}]}"#],
    );
    let results = query_first_result(
        &ctx,
        r#"SELECT jsonb_path_query_first(data, '$.items[*]?(@.key=="bar").value') FROM t"#,
    )
    .await;
    assert_eq!(results, vec![Some("2".to_string())]);
}

#[tokio::test]
async fn test_path_query_filter_expression() {
    let ctx = setup_ctx();
    create_binary_table(
        &ctx,
        "t",
        &[r#"[{"name": "Group", "v": 1}, {"name": "Other", "v": 2}, {"name": "Group", "v": 3}]"#],
    );
    let results = query_first_result(
        &ctx,
        r#"SELECT jsonb_path_query(data, '$[*]?(@.name=="Group").v') FROM t"#,
    )
    .await;
    assert_eq!(results, vec![Some("[1,3]".to_string())]);
}

#[tokio::test]
async fn test_path_query_first_null_input() {
    let ctx = setup_ctx();
    create_nullable_binary_table(&ctx, "t", &[Some(r#"{"a": 1}"#), None, Some(r#"{"a": 3}"#)]);
    let results =
        query_first_result(&ctx, "SELECT jsonb_path_query_first(data, '$.a') FROM t").await;
    assert_eq!(
        results,
        vec![Some("1".to_string()), None, Some("3".to_string())]
    );
}

#[tokio::test]
async fn test_path_query_null_input() {
    let ctx = setup_ctx();
    create_nullable_binary_table(&ctx, "t", &[None, Some(r#"{"a": 1}"#)]);
    let results = query_first_result(&ctx, "SELECT jsonb_path_query(data, '$.a') FROM t").await;
    assert_eq!(results, vec![None, Some("[1]".to_string())]);
}

#[tokio::test]
async fn test_path_query_first_empty_object() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{}"#]);
    let results = query_first_result(
        &ctx,
        "SELECT jsonb_path_query_first(data, '$.anything') FROM t",
    )
    .await;
    assert_eq!(results, vec![None]);
}

#[tokio::test]
async fn test_path_query_empty_array() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"[]"#]);
    let results = query_first_result(&ctx, "SELECT jsonb_path_query(data, '$[*]') FROM t").await;
    assert_eq!(results, vec![Some("[]".to_string())]);
}

#[tokio::test]
async fn test_path_query_first_per_row_path() {
    let ctx = setup_ctx();
    // Build a table with both a data column and a path column
    let json1 = parse_json_to_jsonb(r#"{"a": 1, "b": 2}"#);
    let json2 = parse_json_to_jsonb(r#"{"a": 10, "b": 20}"#);
    let json3 = parse_json_to_jsonb(r#"{"a": 100, "b": 200}"#);
    let schema = Arc::new(Schema::new(vec![
        Field::new("data", DataType::Binary, false),
        Field::new("path", DataType::Utf8, false),
    ]));
    let data_array: Arc<BinaryArray> = Arc::new(BinaryArray::from(vec![
        json1.as_slice(),
        json2.as_slice(),
        json3.as_slice(),
    ]));
    let path_array: Arc<StringArray> = Arc::new(StringArray::from(vec!["$.a", "$.b", "$.a"]));
    let batch =
        RecordBatch::try_new(schema, vec![data_array, path_array]).expect("failed to create batch");
    ctx.register_batch("t", batch)
        .expect("failed to register batch");

    let results =
        query_first_result(&ctx, "SELECT jsonb_path_query_first(data, path) FROM t").await;
    assert_eq!(
        results,
        vec![
            Some("1".to_string()),
            Some("20".to_string()),
            Some("100".to_string()),
        ]
    );
}

// --- filter predicate tests (SQL/JSON path syntax from docs) ---

#[tokio::test]
async fn test_path_query_filter_by_string_field() {
    let ctx = setup_ctx();
    create_binary_table(
        &ctx,
        "t",
        &[r#"{"items": [{"type": "active", "id": 1}, {"type": "inactive", "id": 2}]}"#],
    );
    let results = query_first_result(
        &ctx,
        r#"SELECT jsonb_path_query(data, '$.items[*] ? (@.type == "active")') FROM t"#,
    )
    .await;
    assert_eq!(
        results,
        vec![Some(r#"[{"id":1,"type":"active"}]"#.to_string())]
    );
}

#[tokio::test]
async fn test_path_query_filter_numeric_comparison() {
    let ctx = setup_ctx();
    create_binary_table(
        &ctx,
        "t",
        &[r#"{"scores": [{"name": "Alice", "val": 85}, {"name": "Bob", "val": 42}]}"#],
    );
    let results = query_first_result(
        &ctx,
        r#"SELECT jsonb_path_query(data, '$.scores[*] ? (@.val > 50)') FROM t"#,
    )
    .await;
    assert_eq!(
        results,
        vec![Some(r#"[{"name":"Alice","val":85}]"#.to_string())]
    );
}

#[tokio::test]
async fn test_path_query_first_filter_returns_first_match() {
    let ctx = setup_ctx();
    create_binary_table(
        &ctx,
        "t",
        &[r#"{"users": [{"role": "admin", "name": "Alice"}, {"role": "user", "name": "Bob"}]}"#],
    );
    let results = query_first_result(
        &ctx,
        r#"SELECT jsonb_path_query_first(data, '$.users[*] ? (@.role == "admin")') FROM t"#,
    )
    .await;
    assert_eq!(
        results,
        vec![Some(r#"{"name":"Alice","role":"admin"}"#.to_string())]
    );
}

#[tokio::test]
async fn test_path_query_filter_multiple_matches() {
    let ctx = setup_ctx();
    create_binary_table(
        &ctx,
        "t",
        &[
            r#"{"items": [{"type": "active", "id": 1}, {"type": "inactive", "id": 2}, {"type": "active", "id": 3}]}"#,
        ],
    );
    let results = query_first_result(
        &ctx,
        r#"SELECT jsonb_path_query(data, '$.items[*] ? (@.type == "active")') FROM t"#,
    )
    .await;
    assert_eq!(
        results,
        vec![Some(
            r#"[{"id":1,"type":"active"},{"id":3,"type":"active"}]"#.to_string()
        )]
    );
}

#[tokio::test]
async fn test_path_query_filter_no_match_returns_empty() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"items": [{"type": "active", "id": 1}]}"#]);
    let results = query_first_result(
        &ctx,
        r#"SELECT jsonb_path_query(data, '$.items[*] ? (@.type == "deleted")') FROM t"#,
    )
    .await;
    assert_eq!(results, vec![Some("[]".to_string())]);
}

#[tokio::test]
async fn test_path_query_filter_nested_path() {
    let ctx = setup_ctx();
    create_binary_table(
        &ctx,
        "t",
        &[
            r#"{"teams": [{"players": [{"type": "human", "name": "Alice"}, {"type": "bot", "name": "Bot1"}]}, {"players": [{"type": "human", "name": "Bob"}]}]}"#,
        ],
    );
    let results = query_first_result(
        &ctx,
        r#"SELECT jsonb_path_query(data, '$.teams[*].players[*] ? (@.type == "human")') FROM t"#,
    )
    .await;
    assert_eq!(
        results,
        vec![Some(
            r#"[{"name":"Alice","type":"human"},{"name":"Bob","type":"human"}]"#.to_string()
        )]
    );
}
