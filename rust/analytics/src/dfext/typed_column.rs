use anyhow::{Context, Result};
use datafusion::arrow::array::{
    Array, ArrowPrimitiveType, DictionaryArray, PrimitiveArray, RecordBatch, StringArray,
    StringBuilder,
};
use datafusion::arrow::datatypes::{DataType, Int16Type};

/// Retrieves a typed column from a `RecordBatch` by name.
pub fn typed_column_by_name<'a, T: core::any::Any>(
    rc: &'a datafusion::arrow::array::RecordBatch,
    column_name: &str,
) -> Result<&'a T> {
    let column = rc
        .column_by_name(column_name)
        .with_context(|| format!("getting column {column_name}"))?;
    column
        .as_any()
        .downcast_ref::<T>()
        .with_context(|| format!("casting {column_name}: {:?}", column.data_type()))
}

/// Retrieves a typed column from a `RecordBatch` by index.
pub fn typed_column<T: core::any::Any>(
    rc: &datafusion::arrow::array::RecordBatch,
    index: usize,
) -> Result<&T> {
    let column = rc
        .columns()
        .get(index)
        .with_context(|| format!("getting column {index}"))?;
    column
        .as_any()
        .downcast_ref::<T>()
        .with_context(|| format!("casting {index}: {:?}", column.data_type()))
}

/// Retrieves the single primitive value from the first column of a single-record batch.
pub fn get_only_primitive_value<T: ArrowPrimitiveType>(rbs: &[RecordBatch]) -> Result<T::Native> {
    if rbs.len() != 1 {
        anyhow::bail!(
            "get_only_primitive_value given {} record batches",
            rbs.len()
        );
    }
    let column: &PrimitiveArray<T> = typed_column(&rbs[0], 0)?;
    Ok(column.value(0))
}

/// Retrieves the single string value from the first column of a single-record batch.
pub fn get_only_string_value(rbs: &[RecordBatch]) -> Result<String> {
    if rbs.len() != 1 {
        anyhow::bail!("get_only_string_value given {} record batches", rbs.len());
    }
    let column: &StringArray = typed_column(&rbs[0], 0)?;
    Ok(column.value(0).into())
}

/// Retrieves the single primitive value from a named column of a single-record batch.
pub fn get_single_row_primitive_value_by_name<T: ArrowPrimitiveType>(
    rbs: &[RecordBatch],
    column_name: &str,
) -> Result<T::Native> {
    if rbs.len() != 1 {
        anyhow::bail!(
            "get_single_row_primitive_value given {} record batches",
            rbs.len()
        );
    }
    let column: &PrimitiveArray<T> = typed_column_by_name(&rbs[0], column_name)?;
    Ok(column.value(0))
}

/// Retrieves the single primitive value from an indexed column of a single-record batch.
pub fn get_single_row_primitive_value<T: ArrowPrimitiveType>(
    rbs: &[RecordBatch],
    column_index: usize,
) -> Result<T::Native> {
    if rbs.len() != 1 {
        anyhow::bail!(
            "get_single_row_primitive_value given {} record batches",
            rbs.len()
        );
    }
    let column: &PrimitiveArray<T> = typed_column(&rbs[0], column_index)?;
    Ok(column.value(0))
}

/// Converts a string or dictionary column to a regular StringArray.
/// This handles the case where DataFusion returns Dictionary(Int16, Utf8) arrays
/// which need to be converted to regular StringArray for processing.
pub fn string_column_by_name(rc: &RecordBatch, column_name: &str) -> Result<StringArray> {
    let column = rc
        .column_by_name(column_name)
        .with_context(|| format!("getting column {column_name}"))?;

    match column.data_type() {
        DataType::Utf8 => {
            // Regular string array
            let string_array: &StringArray = column
                .as_any()
                .downcast_ref::<StringArray>()
                .with_context(|| format!("casting {column_name} as StringArray"))?;
            Ok(string_array.clone())
        }
        DataType::Dictionary(key_type, value_type) => {
            // Dictionary encoded string array
            if matches!(key_type.as_ref(), DataType::Int16)
                && matches!(value_type.as_ref(), DataType::Utf8)
            {
                let dict_array: &DictionaryArray<Int16Type> = column
                    .as_any()
                    .downcast_ref::<DictionaryArray<Int16Type>>()
                    .with_context(|| {
                        format!("casting {column_name} as DictionaryArray<Int16Type>")
                    })?;

                // Convert dictionary array to regular string array
                let mut builder = StringBuilder::new();
                for i in 0..dict_array.len() {
                    if dict_array.is_null(i) {
                        builder.append_null();
                    } else {
                        // Get the string value from the dictionary array
                        let key = dict_array.keys().value(i);
                        let values = dict_array.values();
                        let string_values = values
                            .as_any()
                            .downcast_ref::<StringArray>()
                            .with_context(|| {
                                format!(
                                    "Dictionary values are not string array for column {}",
                                    column_name
                                )
                            })?;
                        let value = string_values.value(key as usize);
                        builder.append_value(value);
                    }
                }
                Ok(builder.finish())
            } else {
                anyhow::bail!(
                    "Unsupported dictionary type for column {}: Dictionary({:?}, {:?})",
                    column_name,
                    key_type,
                    value_type
                );
            }
        }
        _ => {
            anyhow::bail!(
                "Column {} is not a string type, found: {:?}",
                column_name,
                column.data_type()
            );
        }
    }
}
