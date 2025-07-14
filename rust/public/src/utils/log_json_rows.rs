use anyhow::Result;
use datafusion::arrow::array::StringArray;
use datafusion::arrow::{
    array::{RecordBatch, StructArray},
    datatypes::Field,
    json::{writer::make_encoder, EncoderOptions},
};
use micromegas_analytics::dfext::typed_column::typed_column_by_name;
use micromegas_tracing::info;
use micromegas_tracing::intern_string::intern_string;
use micromegas_tracing::property_set::{Property, PropertySet};
use std::{str::from_utf8, sync::Arc};

/// Logs rows from a `RecordBatch` as JSON, with specified columns converted to properties.
///
/// This function is useful for logging structured data from a `RecordBatch` in a human-readable format.
pub async fn log_json_rows(
    target: &'static str,
    rbs: &[RecordBatch],
    columns_as_properties: &[&str],
) -> Result<()> {
    let options = EncoderOptions::default();
    for batch in rbs {
        let mut prop_columns = vec![];
        for prop_name in columns_as_properties {
            let c: &StringArray = typed_column_by_name(batch, prop_name)?;
            prop_columns.push(c);
        }
        let mut buffer = Vec::with_capacity(16 * 1024);
        let array = StructArray::from(batch.clone());
        let field = Arc::new(Field::new_struct(
            "",
            batch.schema().fields().clone(),
            false,
        ));
        let mut encoder = make_encoder(&field, &array, &options)?;
        assert!(!encoder.has_nulls(), "root cannot be nullable");
        for idx in 0..batch.num_rows() {
            let mut properties = vec![Property::new("target", target)];
            for prop_index in 0..columns_as_properties.len() {
                properties.push(Property::new(
                    intern_string(columns_as_properties[prop_index]),
                    intern_string(prop_columns[prop_index].value(idx)),
                ));
            }
            let pset = PropertySet::find_or_create(properties);

            encoder.encode(idx, &mut buffer);
            info!(properties:pset, "{}", from_utf8(&buffer)?);
            buffer.clear();
        }
        drop(prop_columns);
    }
    Ok(())
}
