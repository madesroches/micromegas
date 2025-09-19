use criterion::{Criterion, black_box, criterion_group, criterion_main};
use datafusion::arrow::array::{
    Array, ArrayRef, DictionaryArray, GenericBinaryArray, GenericListArray, StringArray,
    StructArray,
};
use datafusion::arrow::buffer::OffsetBuffer;
use datafusion::arrow::datatypes::{DataType, Field, Int32Type};
use datafusion::config::ConfigOptions;
use datafusion::logical_expr::{ColumnarValue, ScalarFunctionArgs, ScalarUDFImpl};
use jsonb::Value;
use micromegas_analytics::properties::property_get::PropertyGet;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Create simple test data for quick benchmarking
fn create_list_struct_simple(rows: usize) -> ArrayRef {
    let mut all_keys = Vec::new();
    let mut all_values = Vec::new();
    let mut offsets = vec![0i32];

    for row in 0..rows {
        // 3 properties per row
        for prop in 0..3 {
            all_keys.push(format!("key{}", prop));
            all_values.push(format!("value{}", row));
        }
        offsets.push(offsets.last().unwrap() + 3);
    }

    let key_array = StringArray::from(all_keys);
    let value_array = StringArray::from(all_values);

    let struct_array = StructArray::from(vec![
        (
            Arc::new(Field::new("key", DataType::Utf8, false)),
            Arc::new(key_array) as ArrayRef,
        ),
        (
            Arc::new(Field::new("value", DataType::Utf8, false)),
            Arc::new(value_array) as ArrayRef,
        ),
    ]);

    let list_field = Field::new(
        "item",
        DataType::Struct(
            vec![
                Field::new("key", DataType::Utf8, false),
                Field::new("value", DataType::Utf8, false),
            ]
            .into(),
        ),
        false,
    );

    Arc::new(GenericListArray::<i32>::new(
        Arc::new(list_field),
        OffsetBuffer::new(offsets.into()),
        Arc::new(struct_array),
        None,
    ))
}

fn create_dict_binary_simple(rows: usize) -> ArrayRef {
    // Create 2 unique JSONB objects for compression
    let mut binary_values = Vec::new();

    for set_id in 0..2 {
        let mut map = BTreeMap::new();
        for prop in 0..3 {
            map.insert(
                format!("key{}", prop),
                Value::String(Cow::Borrowed(Box::leak(
                    format!("value{}", set_id).into_boxed_str(),
                ))),
            );
        }

        let jsonb = Value::Object(map);
        let mut buffer = Vec::new();
        jsonb.write_to_vec(&mut buffer);
        binary_values.push(buffer);
    }

    let binary_refs: Vec<Option<&[u8]>> =
        binary_values.iter().map(|v| Some(v.as_slice())).collect();
    let binary_array = GenericBinaryArray::<i32>::from(binary_refs);

    // Create dictionary keys that point to the unique JSONB objects
    let keys: Vec<i32> = (0..rows).map(|i| (i % 2) as i32).collect();

    let dict_array = DictionaryArray::<Int32Type>::try_new(keys.into(), Arc::new(binary_array))
        .expect("Failed to create dictionary array");

    Arc::new(dict_array)
}

fn run_benchmark(properties: ArrayRef, names: ArrayRef) {
    let property_get = PropertyGet::new();

    let args = ScalarFunctionArgs {
        args: vec![
            ColumnarValue::Array(properties.clone()),
            ColumnarValue::Array(names.clone()),
        ],
        arg_fields: vec![
            Arc::new(Field::new(
                "properties",
                properties.data_type().clone(),
                true,
            )),
            Arc::new(Field::new("names", names.data_type().clone(), false)),
        ],
        number_rows: properties.len(),
        return_field: Arc::new(Field::new(
            "result",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        )),
        config_options: Arc::new(ConfigOptions::default()),
    };

    property_get
        .invoke_with_args(args)
        .expect("property_get should succeed");
}

fn bench_format_comparison(c: &mut Criterion) {
    let rows = 1000;

    // Create test data
    let list_struct_props = create_list_struct_simple(rows);
    let dict_binary_props = create_dict_binary_simple(rows);
    let names: Arc<StringArray> = Arc::new(StringArray::from(vec!["key0"; rows]));

    let mut group = c.benchmark_group("property_get_comparison");

    group.bench_function("List<Struct>", |b| {
        b.iter(|| {
            run_benchmark(
                black_box(list_struct_props.clone()),
                black_box(names.clone()),
            )
        })
    });

    group.bench_function("Dictionary<Int32, Binary>", |b| {
        b.iter(|| {
            run_benchmark(
                black_box(dict_binary_props.clone()),
                black_box(names.clone()),
            )
        })
    });

    group.finish();
}

criterion_group!(benches, bench_format_comparison);
criterion_main!(benches);
