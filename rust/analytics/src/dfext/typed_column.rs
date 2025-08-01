use anyhow::{Context, Result};
use datafusion::arrow::array::{ArrowPrimitiveType, PrimitiveArray, RecordBatch, StringArray};

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
