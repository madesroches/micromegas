use datafusion::arrow::array::{
    Array, BinaryArray, BinaryDictionaryBuilder, DictionaryArray, Int32Array, Int64Array,
    ListArray, StringArray,
};
use datafusion::arrow::datatypes::{DataType, Field, Int32Type, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::prelude::SessionContext;
use std::sync::Arc;

// --- shared harness, following jsonb_path_query_tests.rs's conventions ---

fn parse_json_to_jsonb(json_str: &str) -> Vec<u8> {
    let parsed = jsonb::parse_value(json_str.as_bytes()).expect("failed to parse test JSON");
    let mut buffer = vec![];
    parsed.write_to_vec(&mut buffer);
    buffer
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
        .map(|s| s.map(parse_json_to_jsonb))
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

/// Builds through `BinaryDictionaryBuilder`, which dedups identical JSONB blobs into one
/// dictionary entry — good for exercising the repeated-blob fast path, but it cannot produce a
/// null key, a null values-slot, or an unreferenced-slot case (every input string gets a
/// dictionary entry). Use `build_hand_built_dict_array` for those.
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

/// Hand-builds a `DictionaryArray<Int32Type>` from caller-supplied keys and (nullable) JSONB
/// values, for the null-key / null-values-slot / unreferenced-slot cases `create_dict_table`
/// cannot produce.
fn build_hand_built_dict_array(
    keys: Vec<Option<i32>>,
    values: Vec<Option<Vec<u8>>>,
) -> DictionaryArray<Int32Type> {
    let keys_array = Int32Array::from(keys);
    let value_refs: Vec<Option<&[u8]>> = values.iter().map(|v| v.as_deref()).collect();
    let values_array: BinaryArray = value_refs.into_iter().collect();
    DictionaryArray::<Int32Type>::try_new(keys_array, Arc::new(values_array))
        .expect("failed to build hand-built dictionary array")
}

fn register_dict_table(
    ctx: &SessionContext,
    table_name: &str,
    dict_array: DictionaryArray<Int32Type>,
) {
    let schema = Arc::new(Schema::new(vec![Field::new(
        "data",
        dict_array.data_type().clone(),
        true,
    )]));
    let batch =
        RecordBatch::try_new(schema, vec![Arc::new(dict_array)]).expect("failed to create batch");
    ctx.register_batch(table_name, batch)
        .expect("failed to register batch");
}

async fn collect_batches(ctx: &SessionContext, sql: &str) -> Vec<RecordBatch> {
    let df = ctx.sql(sql).await.expect("SQL query failed");
    df.collect().await.expect("failed to collect results")
}

async fn collect_error(ctx: &SessionContext, sql: &str) -> String {
    match ctx.sql(sql).await {
        Ok(df) => df
            .collect()
            .await
            .expect_err("expected query to fail")
            .to_string(),
        Err(e) => e.to_string(),
    }
}

fn total_rows(batches: &[RecordBatch]) -> usize {
    batches.iter().map(|b| b.num_rows()).sum()
}

fn collect_string_column(batches: &[RecordBatch], idx: usize) -> Vec<Option<String>> {
    let mut out = vec![];
    for batch in batches {
        let col = batch
            .column(idx)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("expected StringArray");
        for i in 0..col.len() {
            out.push(if col.is_null(i) {
                None
            } else {
                Some(col.value(i).to_string())
            });
        }
    }
    out
}

fn collect_i32_column(batches: &[RecordBatch], idx: usize) -> Vec<i32> {
    let mut out = vec![];
    for batch in batches {
        let col = batch
            .column(idx)
            .as_any()
            .downcast_ref::<Int32Array>()
            .expect("expected Int32Array");
        for i in 0..col.len() {
            out.push(col.value(i));
        }
    }
    out
}

fn collect_i64_column(batches: &[RecordBatch], idx: usize) -> Vec<i64> {
    let mut out = vec![];
    for batch in batches {
        let col = batch
            .column(idx)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("expected Int64Array");
        for i in 0..col.len() {
            out.push(col.value(i));
        }
    }
    out
}

/// Collects a `Dictionary<Int32, Utf8>` column (the shape `jsonb_format_json`/`jsonb_as_string`
/// return) into per-row optional strings.
fn collect_dict_utf8_column(batches: &[RecordBatch], idx: usize) -> Vec<Option<String>> {
    let mut out = vec![];
    for batch in batches {
        let col = batch.column(idx);
        let dict = col
            .as_any()
            .downcast_ref::<DictionaryArray<Int32Type>>()
            .expect("expected dict array");
        let values = dict
            .values()
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("expected utf8 dict values");
        for i in 0..dict.len() {
            if dict.is_null(i) {
                out.push(None);
            } else {
                let key = dict.keys().value(i) as usize;
                out.push(Some(values.value(key).to_string()));
            }
        }
    }
    out
}

fn list_column(batch: &RecordBatch, idx: usize) -> &ListArray {
    batch
        .column(idx)
        .as_any()
        .downcast_ref::<ListArray>()
        .expect("expected ListArray")
}

// --- 1. Return types ---

#[tokio::test]
async fn test_jsonb_entries_return_type() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"a": 1}"#]);
    let batches = collect_batches(&ctx, "SELECT jsonb_entries(data) FROM t").await;
    match batches[0].schema().field(0).data_type() {
        DataType::List(item) => match item.data_type() {
            DataType::Struct(fields) => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name(), "key");
                assert_eq!(*fields[0].data_type(), DataType::Utf8);
                assert!(!fields[0].is_nullable());
                assert_eq!(fields[1].name(), "value");
                assert_eq!(*fields[1].data_type(), DataType::Binary);
                assert!(fields[1].is_nullable());
            }
            other => panic!("expected a Struct list item, got {other:?}"),
        },
        other => panic!("expected List, got {other:?} (not a plain List — regression!)"),
    }
}

#[tokio::test]
async fn test_jsonb_elements_return_type() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"[1]"#]);
    let batches = collect_batches(&ctx, "SELECT jsonb_elements(data) FROM t").await;
    match batches[0].schema().field(0).data_type() {
        DataType::List(item) => assert_eq!(*item.data_type(), DataType::Binary),
        other => panic!("expected List, got {other:?} (not a plain List — regression!)"),
    }
}

#[tokio::test]
async fn test_jsonb_path_elements_return_type() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"a": 1}"#]);
    let batches = collect_batches(&ctx, "SELECT jsonb_path_elements(data, '$.a') FROM t").await;
    match batches[0].schema().field(0).data_type() {
        DataType::List(item) => assert_eq!(*item.data_type(), DataType::Binary),
        other => panic!("expected List, got {other:?} (not a plain List — regression!)"),
    }
}

// --- 2. jsonb_entries ---

#[tokio::test]
async fn test_entries_object_fields() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"a": 1, "b": "x"}"#]);
    let batches = collect_batches(
        &ctx,
        "SELECT kv['key'] AS k FROM (SELECT unnest(jsonb_entries(data)) AS kv FROM t) ORDER BY k",
    )
    .await;
    assert_eq!(
        collect_string_column(&batches, 0),
        vec![Some("a".to_string()), Some("b".to_string())]
    );
}

#[tokio::test]
async fn test_entries_array_index_keys() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"[10, 20, 30]"#]);
    let batches = collect_batches(
        &ctx,
        "SELECT kv['key'] AS k FROM (SELECT unnest(jsonb_entries(data)) AS kv FROM t) ORDER BY k",
    )
    .await;
    assert_eq!(
        collect_string_column(&batches, 0),
        vec![
            Some("0".to_string()),
            Some("1".to_string()),
            Some("2".to_string())
        ]
    );
}

#[tokio::test]
async fn test_entries_nested_value_round_trips_as_jsonb() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"obj": {"nested": true}}"#]);
    let batches = collect_batches(
        &ctx,
        "SELECT jsonb_format_json(kv['value']) AS v FROM (SELECT unnest(jsonb_entries(data)) AS kv FROM t)",
    )
    .await;
    assert_eq!(
        collect_dict_utf8_column(&batches, 0),
        vec![Some(r#"{"nested":true}"#.to_string())]
    );
}

#[tokio::test]
async fn test_entries_scalar_and_null_produce_zero_rows() {
    let ctx = setup_ctx();
    create_nullable_binary_table(&ctx, "t", &[Some("42"), None, Some(r#"{"a": 1}"#)]);
    let batches = collect_batches(
        &ctx,
        "SELECT kv['key'] AS k FROM (SELECT unnest(jsonb_entries(data)) AS kv FROM t)",
    )
    .await;
    assert_eq!(total_rows(&batches), 1);
}

#[tokio::test]
async fn test_entries_empty_object_zero_rows() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{}"#]);
    let batches = collect_batches(&ctx, "SELECT unnest(jsonb_entries(data)) AS kv FROM t").await;
    assert_eq!(total_rows(&batches), 0);
}

#[tokio::test]
async fn test_entries_undecodable_bytes_error() {
    let ctx = setup_ctx();
    let schema = Arc::new(Schema::new(vec![Field::new(
        "data",
        DataType::Binary,
        false,
    )]));
    let bad: &[u8] = &[0xFF, 0xFE];
    let array: Arc<BinaryArray> = Arc::new(BinaryArray::from(vec![bad]));
    let batch = RecordBatch::try_new(schema, vec![array]).expect("failed to create batch");
    ctx.register_batch("t", batch)
        .expect("failed to register batch");

    let err_msg = collect_error(&ctx, "SELECT jsonb_entries(data) FROM t").await;
    assert!(
        err_msg.to_lowercase().contains("invalid"),
        "unexpected error: {err_msg}"
    );
}

// --- 3. jsonb_elements ---

#[tokio::test]
async fn test_elements_array_one_row_per_element() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"[1, 2, 3]"#]);
    let batches = collect_batches(
        &ctx,
        "SELECT jsonb_as_i64(e) AS v FROM (SELECT unnest(jsonb_elements(data)) AS e FROM t) ORDER BY v",
    )
    .await;
    assert_eq!(collect_i64_column(&batches, 0), vec![1, 2, 3]);
}

#[tokio::test]
async fn test_elements_object_and_scalar_are_null() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"a": 1}"#, "42"]);
    let batches = collect_batches(&ctx, "SELECT jsonb_elements(data) FROM t").await;
    let list = list_column(&batches[0], 0);
    assert!(list.is_null(0), "object input should produce a NULL list");
    assert!(list.is_null(1), "scalar input should produce a NULL list");
}

#[tokio::test]
async fn test_elements_empty_array_is_empty_not_null() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"[]"#]);
    let batches = collect_batches(&ctx, "SELECT jsonb_elements(data) FROM t").await;
    let list = list_column(&batches[0], 0);
    assert!(!list.is_null(0), "empty array should not be NULL");
    assert_eq!(list.value(0).len(), 0);

    let unnested = collect_batches(&ctx, "SELECT unnest(jsonb_elements(data)) AS e FROM t").await;
    assert_eq!(total_rows(&unnested), 0);
}

// --- 4. jsonb_path_elements ---

#[tokio::test]
async fn test_path_elements_wildcard() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"items": [1, 2, 3]}"#]);
    let batches = collect_batches(
        &ctx,
        "SELECT jsonb_as_i64(e) AS v FROM (SELECT unnest(jsonb_path_elements(data, '$.items[*]')) AS e FROM t) ORDER BY v",
    )
    .await;
    assert_eq!(collect_i64_column(&batches, 0), vec![1, 2, 3]);
}

#[tokio::test]
async fn test_path_elements_nested_path() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"a": {"b": {"c": [10, 20]}}}"#]);
    let batches = collect_batches(
        &ctx,
        "SELECT jsonb_as_i64(e) AS v FROM (SELECT unnest(jsonb_path_elements(data, '$.a.b.c[*]')) AS e FROM t) ORDER BY v",
    )
    .await;
    assert_eq!(collect_i64_column(&batches, 0), vec![10, 20]);
}

#[tokio::test]
async fn test_path_elements_no_match_zero_rows() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"a": 1}"#]);
    let batches = collect_batches(
        &ctx,
        "SELECT unnest(jsonb_path_elements(data, '$.missing[*]')) AS e FROM t",
    )
    .await;
    assert_eq!(total_rows(&batches), 0);
}

#[tokio::test]
async fn test_path_elements_invalid_path_errors() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"a": 1}"#]);
    let err_msg = collect_error(&ctx, "SELECT jsonb_path_elements(data, '$[invalid') FROM t").await;
    assert!(
        err_msg.contains("jsonb_path_elements"),
        "unexpected error: {err_msg}"
    );
    assert!(
        err_msg.contains("invalid JSONPath"),
        "unexpected error: {err_msg}"
    );
}

#[tokio::test]
async fn test_path_elements_null_path_zero_rows() {
    let ctx = setup_ctx();
    let json1 = parse_json_to_jsonb(r#"{"a": 1}"#);
    let schema = Arc::new(Schema::new(vec![
        Field::new("data", DataType::Binary, false),
        Field::new("path", DataType::Utf8, true),
    ]));
    let data_array: Arc<BinaryArray> = Arc::new(BinaryArray::from(vec![json1.as_slice()]));
    let path_array: Arc<StringArray> = Arc::new(StringArray::from(vec![None::<&str>]));
    let batch =
        RecordBatch::try_new(schema, vec![data_array, path_array]).expect("failed to create batch");
    ctx.register_batch("t", batch)
        .expect("failed to register batch");

    let batches = collect_batches(
        &ctx,
        "SELECT unnest(jsonb_path_elements(data, path)) AS e FROM t",
    )
    .await;
    assert_eq!(total_rows(&batches), 0);
}

// --- 5. Per-row correlation ---

#[tokio::test]
async fn test_per_row_correlation() {
    let ctx = setup_ctx();
    let json1 = parse_json_to_jsonb(r#"[1, 2]"#);
    let json2 = parse_json_to_jsonb(r#"[3, 4, 5]"#);
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int32, false),
        Field::new("data", DataType::Binary, false),
    ]));
    let id_array = Int32Array::from(vec![1, 2]);
    let data_array: Arc<BinaryArray> =
        Arc::new(BinaryArray::from(vec![json1.as_slice(), json2.as_slice()]));
    let batch = RecordBatch::try_new(schema, vec![Arc::new(id_array), data_array])
        .expect("failed to create batch");
    ctx.register_batch("t", batch)
        .expect("failed to register batch");

    let batches = collect_batches(
        &ctx,
        "SELECT id, jsonb_as_i64(e) AS v FROM (SELECT id, unnest(jsonb_elements(data)) AS e FROM t) ORDER BY id, v",
    )
    .await;
    assert_eq!(collect_i32_column(&batches, 0), vec![1, 1, 2, 2, 2]);
    assert_eq!(collect_i64_column(&batches, 1), vec![1, 2, 3, 4, 5]);
}

// --- 6. Dictionary input ---

#[tokio::test]
async fn test_dict_input_entries_repeated_blobs() {
    let ctx = setup_ctx();
    create_dict_table(
        &ctx,
        "t",
        &[r#"{"a": 1, "b": 2}"#, r#"{"a": 1, "b": 2}"#, r#"{"c": 3}"#],
    );
    let batches = collect_batches(
        &ctx,
        "SELECT kv['key'] AS k FROM (SELECT unnest(jsonb_entries(data)) AS kv FROM t) ORDER BY k",
    )
    .await;
    assert_eq!(
        collect_string_column(&batches, 0),
        vec![
            Some("a".to_string()),
            Some("a".to_string()),
            Some("b".to_string()),
            Some("b".to_string()),
            Some("c".to_string()),
        ]
    );
}

#[tokio::test]
async fn test_dict_input_elements_repeated_blobs() {
    let ctx = setup_ctx();
    create_dict_table(&ctx, "t", &[r#"[1, 2]"#, r#"[1, 2]"#, r#"[3]"#]);
    let batches = collect_batches(
        &ctx,
        "SELECT jsonb_as_i64(e) AS v FROM (SELECT unnest(jsonb_elements(data)) AS e FROM t) ORDER BY v",
    )
    .await;
    assert_eq!(collect_i64_column(&batches, 0), vec![1, 1, 2, 2, 3]);
}

#[tokio::test]
async fn test_dict_input_path_elements_repeated_blobs() {
    let ctx = setup_ctx();
    create_dict_table(
        &ctx,
        "t",
        &[
            r#"{"items": [1, 2]}"#,
            r#"{"items": [1, 2]}"#,
            r#"{"items": [3]}"#,
        ],
    );
    let batches = collect_batches(
        &ctx,
        "SELECT jsonb_as_i64(e) AS v FROM (SELECT unnest(jsonb_path_elements(data, '$.items[*]')) AS e FROM t) ORDER BY v",
    )
    .await;
    assert_eq!(collect_i64_column(&batches, 0), vec![1, 1, 2, 2, 3]);
}

#[tokio::test]
async fn test_dict_null_key_matches_row_by_row() {
    let ctx = setup_ctx();
    let v0 = parse_json_to_jsonb(r#"[1, 2]"#);
    let v1 = parse_json_to_jsonb(r#"[3, 4]"#);
    let dict_array =
        build_hand_built_dict_array(vec![Some(0), None, Some(1)], vec![Some(v0), Some(v1)]);
    register_dict_table(&ctx, "dict_t", dict_array);
    let dict_batches = collect_batches(
        &ctx,
        "SELECT jsonb_as_i64(e) AS v FROM (SELECT unnest(jsonb_elements(data)) AS e FROM dict_t) ORDER BY v",
    )
    .await;

    create_nullable_binary_table(&ctx, "row_t", &[Some("[1, 2]"), None, Some("[3, 4]")]);
    let row_batches = collect_batches(
        &ctx,
        "SELECT jsonb_as_i64(e) AS v FROM (SELECT unnest(jsonb_elements(data)) AS e FROM row_t) ORDER BY v",
    )
    .await;

    let dict_values = collect_i64_column(&dict_batches, 0);
    let row_values = collect_i64_column(&row_batches, 0);
    assert_eq!(dict_values, row_values);
    assert_eq!(dict_values, vec![1, 2, 3, 4]);
}

#[tokio::test]
async fn test_dict_referenced_null_values_slot_fails_like_existing_udf() {
    let ctx = setup_ctx();
    // Key 0 references values[0], which is an Arrow-null (empty-bytes) slot.
    let dict_array = build_hand_built_dict_array(vec![Some(0)], vec![None]);
    register_dict_table(&ctx, "t", dict_array);

    let elements_err = collect_error(&ctx, "SELECT unnest(jsonb_elements(data)) FROM t").await;
    // jsonb_array_length also reads this dictionary slot via key_index without checking
    // values.is_null (array_length.rs) and propagates the decode failure (unlike
    // jsonb_format_json's `to_string()`, which is a lenient, infallible fallback that treats
    // empty/invalid bytes as JSON `null` instead of erroring) — confirming the fast path fails
    // the same way as the rest of the create_binary_accessor-adjacent code.
    let array_length_err = collect_error(&ctx, "SELECT jsonb_array_length(data) FROM t").await;

    assert!(
        elements_err.to_lowercase().contains("invalid"),
        "unexpected error: {elements_err}"
    );
    assert!(
        array_length_err.to_lowercase().contains("invalid"),
        "unexpected error: {array_length_err}"
    );
}

#[tokio::test]
async fn test_dict_sliced_unreferenced_undecodable_slot_does_not_error() {
    let ctx = setup_ctx();
    let v0 = parse_json_to_jsonb(r#"[1, 2]"#);
    let v1 = parse_json_to_jsonb(r#"[3, 4]"#);
    let bad = vec![0xFFu8, 0xFE, 0xFD];
    let dict_array = build_hand_built_dict_array(
        vec![Some(0), Some(1), Some(2)],
        vec![Some(v0), Some(v1), Some(bad)],
    );
    // Slice off the row referencing the undecodable slot; the values array (including the
    // undecodable blob at index 2) is retained unchanged by slicing.
    let sliced = Array::slice(&dict_array, 0, 2);
    let sliced_dict = sliced
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("slice of a DictionaryArray should stay a DictionaryArray")
        .clone();
    register_dict_table(&ctx, "t", sliced_dict);

    let batches = collect_batches(
        &ctx,
        "SELECT jsonb_as_i64(e) AS v FROM (SELECT unnest(jsonb_elements(data)) AS e FROM t) ORDER BY v",
    )
    .await;
    assert_eq!(collect_i64_column(&batches, 0), vec![1, 2, 3, 4]);
}

#[tokio::test]
async fn test_path_elements_dict_all_null_keys_invalid_path_never_parsed() {
    let ctx = setup_ctx();
    // No key is non-null, so no values slot is ever referenced — the invalid path must never be
    // parsed, matching eval_jsonb_path_query's behavior when no row has both a non-null JSONB and
    // a non-null path.
    let dict_array = build_hand_built_dict_array(
        vec![None, None],
        vec![Some(parse_json_to_jsonb(r#"{"a": 1}"#))],
    );
    let schema = Arc::new(Schema::new(vec![
        Field::new("data", dict_array.data_type().clone(), true),
        Field::new("path", DataType::Utf8, false),
    ]));
    let path_array: Arc<StringArray> = Arc::new(StringArray::from(vec!["$[invalid", "$[invalid"]));
    let batch = RecordBatch::try_new(schema, vec![Arc::new(dict_array), path_array])
        .expect("failed to create batch");
    ctx.register_batch("t", batch)
        .expect("failed to register batch");

    let batches = collect_batches(&ctx, "SELECT jsonb_path_elements(data, path) FROM t").await;
    let list = list_column(&batches[0], 0);
    assert!(list.is_null(0));
    assert!(list.is_null(1));
}

#[tokio::test]
async fn test_path_elements_dict_referenced_key_invalid_path_errors() {
    let ctx = setup_ctx();
    let dict_array = build_hand_built_dict_array(
        vec![Some(0)],
        vec![Some(parse_json_to_jsonb(r#"{"a": 1}"#))],
    );
    let schema = Arc::new(Schema::new(vec![
        Field::new("data", dict_array.data_type().clone(), true),
        Field::new("path", DataType::Utf8, false),
    ]));
    let path_array: Arc<StringArray> = Arc::new(StringArray::from(vec!["$[invalid"]));
    let batch = RecordBatch::try_new(schema, vec![Arc::new(dict_array), path_array])
        .expect("failed to create batch");
    ctx.register_batch("t", batch)
        .expect("failed to register batch");

    let err_msg = collect_error(&ctx, "SELECT jsonb_path_elements(data, path) FROM t").await;
    assert!(
        err_msg.contains("jsonb_path_elements"),
        "unexpected error: {err_msg}"
    );
    assert!(
        err_msg.contains("invalid JSONPath"),
        "unexpected error: {err_msg}"
    );
}

#[tokio::test]
async fn test_path_elements_dict_input_null_path_falls_back_row_by_row() {
    let ctx = setup_ctx();
    let json1 = parse_json_to_jsonb(r#"{"items": [1, 2]}"#);
    let json2 = parse_json_to_jsonb(r#"{"items": [3, 4]}"#);
    let mut builder = BinaryDictionaryBuilder::<Int32Type>::new();
    builder.append_value(&json1);
    builder.append_value(&json2);
    let dict_array = builder.finish();
    let schema = Arc::new(Schema::new(vec![
        Field::new("data", dict_array.data_type().clone(), false),
        Field::new("path", DataType::Utf8, true),
    ]));
    let path_array: Arc<StringArray> = Arc::new(StringArray::from(vec![Some("$.items[*]"), None]));
    let batch = RecordBatch::try_new(schema, vec![Arc::new(dict_array), path_array])
        .expect("failed to create batch");
    ctx.register_batch("t", batch)
        .expect("failed to register batch");

    let batches = collect_batches(
        &ctx,
        "SELECT jsonb_as_i64(e) AS v FROM (SELECT unnest(jsonb_path_elements(data, path)) AS e FROM t) ORDER BY v",
    )
    .await;
    // Row 1's NULL path contributes zero rows; only row 0's [1, 2] shows up.
    assert_eq!(collect_i64_column(&batches, 0), vec![1, 2]);
}

#[tokio::test]
async fn test_path_elements_dict_input_varying_path_falls_back() {
    let ctx = setup_ctx();
    // Same underlying blob (the dictionary builder dedups it to one entry) with two different,
    // both non-null, paths per row — must not take the constant-path fast path.
    let json = parse_json_to_jsonb(r#"{"a": [1, 2], "b": [9]}"#);
    let mut builder = BinaryDictionaryBuilder::<Int32Type>::new();
    builder.append_value(&json);
    builder.append_value(&json);
    let dict_array = builder.finish();
    let schema = Arc::new(Schema::new(vec![
        Field::new("data", dict_array.data_type().clone(), false),
        Field::new("path", DataType::Utf8, false),
    ]));
    let path_array: Arc<StringArray> = Arc::new(StringArray::from(vec!["$.a[*]", "$.b[*]"]));
    let batch = RecordBatch::try_new(schema, vec![Arc::new(dict_array), path_array])
        .expect("failed to create batch");
    ctx.register_batch("t", batch)
        .expect("failed to register batch");

    let batches = collect_batches(
        &ctx,
        "SELECT jsonb_as_i64(e) AS v FROM (SELECT unnest(jsonb_path_elements(data, path)) AS e FROM t) ORDER BY v",
    )
    .await;
    assert_eq!(collect_i64_column(&batches, 0), vec![1, 2, 9]);
}

// --- 7. Composition ---

#[tokio::test]
async fn test_composition_as_string_and_get() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"[{"name": "Alice"}, {"name": "Bob"}]"#]);
    let batches = collect_batches(
        &ctx,
        "SELECT jsonb_as_string(jsonb_get(e, 'name')) AS name FROM (SELECT unnest(jsonb_elements(data)) AS e FROM t) ORDER BY name",
    )
    .await;
    assert_eq!(
        collect_dict_utf8_column(&batches, 0),
        vec![Some("Alice".to_string()), Some("Bob".to_string())]
    );
}

#[tokio::test]
async fn test_composition_group_by_and_distinct() {
    let ctx = setup_ctx();
    create_binary_table(&ctx, "t", &[r#"{"a": 1, "b": 2}"#, r#"{"a": 3, "c": 4}"#]);

    let group_batches = collect_batches(
        &ctx,
        "SELECT kv['key'] AS k, count(*) AS n FROM (SELECT unnest(jsonb_entries(data)) AS kv FROM t) GROUP BY k ORDER BY k",
    )
    .await;
    assert_eq!(
        collect_string_column(&group_batches, 0),
        vec![
            Some("a".to_string()),
            Some("b".to_string()),
            Some("c".to_string())
        ]
    );
    assert_eq!(collect_i64_column(&group_batches, 1), vec![2, 1, 1]);

    let distinct_batches = collect_batches(
        &ctx,
        "SELECT DISTINCT kv['key'] AS k FROM (SELECT unnest(jsonb_entries(data)) AS kv FROM t) ORDER BY k",
    )
    .await;
    assert_eq!(
        collect_string_column(&distinct_batches, 0),
        vec![
            Some("a".to_string()),
            Some("b".to_string()),
            Some("c".to_string())
        ]
    );
}
