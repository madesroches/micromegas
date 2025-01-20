use anyhow::{Context, Result};

pub fn get_column<'a, T: core::any::Any>(
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
