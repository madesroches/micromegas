use anyhow::{Context, Result};

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
