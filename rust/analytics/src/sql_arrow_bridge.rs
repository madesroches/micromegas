use anyhow::{Context, Result};
use datafusion::arrow::array::Array;
use datafusion::arrow::array::ArrayBuilder;
use datafusion::arrow::array::BinaryBuilder;
use datafusion::arrow::array::ListBuilder;
use datafusion::arrow::array::PrimitiveBuilder;
use datafusion::arrow::array::StringBuilder;
use datafusion::arrow::array::StructBuilder;
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::datatypes::Field;
use datafusion::arrow::datatypes::Fields;
use datafusion::arrow::datatypes::Int32Type;
use datafusion::arrow::datatypes::Int64Type;
use datafusion::arrow::datatypes::TimeUnit;
use datafusion::arrow::datatypes::TimestampNanosecondType;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::common::cast::as_struct_array;
use micromegas_telemetry::property::Property;
use sqlx::Column;
use sqlx::Row;
use sqlx::TypeInfo;
use sqlx::postgres::{PgColumn, PgRow};
use std::sync::Arc;

use crate::arrow_utils::make_empty_record_batch;
use crate::properties::dictionary_builder::PropertiesDictionaryBuilder;

/// A unified trait for reading columns from database rows that can handle both
/// regular and dictionary-encoded columns.
pub trait ColumnReader {
    /// Extract column data from a single row and append to a struct builder.
    /// For dictionary columns, this should collect data for later processing.
    fn extract_column_from_row(
        &self,
        _row: &PgRow,
        _struct_builder: &mut StructBuilder,
    ) -> Result<()> {
        // Default implementation for dictionary columns - do nothing in individual rows
        Ok(())
    }

    /// Extract column data from a single row and append to a struct builder using a custom field index.
    /// This is used when the struct builder field ordering differs from the original SQL column ordering.
    fn extract_column_from_row_with_field_index(
        &self,
        _row: &PgRow,
        _struct_builder: &mut StructBuilder,
        _field_index: usize,
    ) -> Result<()> {
        // Default implementation delegates to the original method (for dictionary columns)
        self.extract_column_from_row(_row, _struct_builder)
    }

    /// Extract all column data from all rows and return the final array.
    /// Regular columns should use the struct builder approach, dictionary columns
    /// should process all rows at once.
    fn extract_all_from_rows(&self, _rows: &[PgRow]) -> Result<Option<Arc<dyn Array>>> {
        // Default implementation for regular columns - return None to use struct builder
        Ok(None)
    }

    fn field(&self) -> Field;
}

/// A `ColumnReader` for string columns.
pub struct StringColumnReader {
    pub field: Field,
    pub column_ordinal: usize,
}

impl ColumnReader for StringColumnReader {
    fn extract_column_from_row(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
    ) -> Result<()> {
        let value: &str = row
            .try_get(self.column_ordinal)
            .with_context(|| "try_get failed on row")?;
        let field_builder = struct_builder
            .field_builder::<StringBuilder>(self.column_ordinal)
            .with_context(|| "getting field builder for string column")?;
        field_builder.append_value(value);
        Ok(())
    }

    fn extract_column_from_row_with_field_index(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
        field_index: usize,
    ) -> Result<()> {
        let value: &str = row
            .try_get(self.column_ordinal)
            .with_context(|| "try_get failed on row")?;
        let field_builder = struct_builder
            .field_builder::<StringBuilder>(field_index)
            .with_context(|| "getting field builder for string column")?;
        field_builder.append_value(value);
        Ok(())
    }

    fn field(&self) -> Field {
        self.field.clone()
    }
}

/// A `ColumnReader` for UUID columns.
pub struct UuidColumnReader {
    pub field: Field,
    pub column_ordinal: usize,
}

impl ColumnReader for UuidColumnReader {
    fn extract_column_from_row(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
    ) -> Result<()> {
        let value: Option<sqlx::types::uuid::Uuid> = row
            .try_get(self.column_ordinal)
            .with_context(|| "try_get failed on row")?;
        let field_builder = struct_builder
            .field_builder::<StringBuilder>(self.column_ordinal)
            .with_context(|| "getting field builder for string column")?;
        if let Some(uuid) = value {
            field_builder.append_value(
                uuid.hyphenated()
                    .encode_lower(&mut sqlx::types::uuid::Uuid::encode_buffer()),
            );
        } else {
            field_builder.append_null();
        }
        Ok(())
    }

    fn extract_column_from_row_with_field_index(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
        field_index: usize,
    ) -> Result<()> {
        let value: Option<sqlx::types::uuid::Uuid> = row
            .try_get(self.column_ordinal)
            .with_context(|| "try_get failed on row")?;
        let field_builder = struct_builder
            .field_builder::<StringBuilder>(field_index)
            .with_context(|| "getting field builder for string column")?;
        if let Some(uuid) = value {
            field_builder.append_value(
                uuid.hyphenated()
                    .encode_lower(&mut sqlx::types::uuid::Uuid::encode_buffer()),
            );
        } else {
            field_builder.append_null();
        }
        Ok(())
    }

    fn field(&self) -> Field {
        self.field.clone()
    }
}

/// A `ColumnReader` for `i64` columns.
pub struct Int64ColumnReader {
    pub field: Field,
    pub column_ordinal: usize,
}

impl ColumnReader for Int64ColumnReader {
    fn extract_column_from_row(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
    ) -> Result<()> {
        let value: i64 = row
            .try_get(self.column_ordinal)
            .with_context(|| "try_get failed on row")?;
        let field_builder = struct_builder
            .field_builder::<PrimitiveBuilder<Int64Type>>(self.column_ordinal)
            .with_context(|| "getting field builder for int64 column")?;
        field_builder.append_value(value);
        Ok(())
    }

    fn extract_column_from_row_with_field_index(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
        field_index: usize,
    ) -> Result<()> {
        let value: i64 = row
            .try_get(self.column_ordinal)
            .with_context(|| "try_get failed on row")?;
        let field_builder = struct_builder
            .field_builder::<PrimitiveBuilder<Int64Type>>(field_index)
            .with_context(|| "getting field builder for int64 column")?;
        field_builder.append_value(value);
        Ok(())
    }

    fn field(&self) -> Field {
        self.field.clone()
    }
}

/// A `ColumnReader` for `i32` columns.
pub struct Int32ColumnReader {
    pub field: Field,
    pub column_ordinal: usize,
}

impl ColumnReader for Int32ColumnReader {
    fn extract_column_from_row(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
    ) -> Result<()> {
        let value: i32 = row
            .try_get(self.column_ordinal)
            .with_context(|| "try_get failed on row")?;
        let field_builder = struct_builder
            .field_builder::<PrimitiveBuilder<Int32Type>>(self.column_ordinal)
            .with_context(|| "getting field builder for int32 column")?;
        field_builder.append_value(value);
        Ok(())
    }

    fn extract_column_from_row_with_field_index(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
        field_index: usize,
    ) -> Result<()> {
        let value: i32 = row
            .try_get(self.column_ordinal)
            .with_context(|| "try_get failed on row")?;
        let field_builder = struct_builder
            .field_builder::<PrimitiveBuilder<Int32Type>>(field_index)
            .with_context(|| "getting field builder for int32 column")?;
        field_builder.append_value(value);
        Ok(())
    }

    fn field(&self) -> Field {
        self.field.clone()
    }
}

/// A `ColumnReader` for timestamp columns.
pub struct TimestampColumnReader {
    pub field: Field,
    pub column_ordinal: usize,
}

impl ColumnReader for TimestampColumnReader {
    fn extract_column_from_row(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
    ) -> Result<()> {
        use sqlx::types::chrono::{DateTime, Utc};
        let value: DateTime<Utc> = row
            .try_get(self.column_ordinal)
            .with_context(|| "try_get failed on row")?;
        let field_builder = struct_builder
            .field_builder::<PrimitiveBuilder<TimestampNanosecondType>>(self.column_ordinal)
            .with_context(|| "getting field builder for timestamp column")?;
        field_builder.append_value(value.timestamp_nanos_opt().unwrap_or(0));
        Ok(())
    }

    fn extract_column_from_row_with_field_index(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
        field_index: usize,
    ) -> Result<()> {
        use sqlx::types::chrono::{DateTime, Utc};
        let value: DateTime<Utc> = row
            .try_get(self.column_ordinal)
            .with_context(|| "try_get failed on row")?;
        let field_builder = struct_builder
            .field_builder::<PrimitiveBuilder<TimestampNanosecondType>>(field_index)
            .with_context(|| "getting field builder for timestamp column")?;
        field_builder.append_value(value.timestamp_nanos_opt().unwrap_or(0));
        Ok(())
    }

    fn field(&self) -> Field {
        self.field.clone()
    }
}

/// A `ColumnReader` for string array columns.
pub struct StringArrayColumnReader {
    pub field: Field,
    pub column_ordinal: usize,
}

impl ColumnReader for StringArrayColumnReader {
    fn extract_column_from_row(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
    ) -> Result<()> {
        let strings: Vec<String> = row
            .try_get(self.column_ordinal)
            .with_context(|| "try_get failed on row")?;
        let list_builder = struct_builder
            .field_builder::<ListBuilder<Box<dyn ArrayBuilder>>>(self.column_ordinal)
            .with_context(|| "getting field builder for string array column")?;
        let string_builder = list_builder
            .values()
            .as_any_mut()
            .downcast_mut::<StringBuilder>()
            .unwrap();
        for v in strings {
            string_builder.append_value(v);
        }
        list_builder.append(true);
        Ok(())
    }

    fn extract_column_from_row_with_field_index(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
        field_index: usize,
    ) -> Result<()> {
        let strings: Vec<String> = row
            .try_get(self.column_ordinal)
            .with_context(|| "try_get failed on row")?;
        let list_builder = struct_builder
            .field_builder::<ListBuilder<Box<dyn ArrayBuilder>>>(field_index)
            .with_context(|| "getting field builder for string array column")?;
        let string_builder = list_builder
            .values()
            .as_any_mut()
            .downcast_mut::<StringBuilder>()
            .unwrap();
        for v in strings {
            string_builder.append_value(v);
        }
        list_builder.append(true);
        Ok(())
    }

    fn field(&self) -> Field {
        self.field.clone()
    }
}

/// A `ColumnReader` for blob columns.
pub struct BlobColumnReader {
    pub field: Field,
    pub column_ordinal: usize,
}

impl ColumnReader for BlobColumnReader {
    fn extract_column_from_row(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
    ) -> Result<()> {
        let value: Vec<u8> = row
            .try_get(self.column_ordinal)
            .with_context(|| "try_get failed on row")?;
        let field_builder = struct_builder
            .field_builder::<BinaryBuilder>(self.column_ordinal)
            .with_context(|| "getting field builder for blob column")?;
        field_builder.append_value(value);
        Ok(())
    }

    fn extract_column_from_row_with_field_index(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
        field_index: usize,
    ) -> Result<()> {
        let value: Vec<u8> = row
            .try_get(self.column_ordinal)
            .with_context(|| "try_get failed on row")?;
        let field_builder = struct_builder
            .field_builder::<BinaryBuilder>(field_index)
            .with_context(|| "getting field builder for blob column")?;
        field_builder.append_value(value);
        Ok(())
    }

    fn field(&self) -> Field {
        self.field.clone()
    }
}

/// A `ColumnReader` for properties columns with dictionary encoding.
pub struct PropertiesColumnReader {
    pub field: Field,
    pub column_ordinal: usize,
}

impl ColumnReader for PropertiesColumnReader {
    fn extract_all_from_rows(&self, rows: &[PgRow]) -> Result<Option<Arc<dyn Array>>> {
        let mut builder = PropertiesDictionaryBuilder::new(rows.len());

        for row in rows {
            let props: Vec<Property> = row
                .try_get(self.column_ordinal)
                .with_context(|| "try_get failed on row")?;
            builder.append_properties_from_vec(props)?;
        }

        let dict_array = builder.finish()?;
        Ok(Some(Arc::new(dict_array)))
    }

    fn field(&self) -> Field {
        self.field.clone()
    }
}

/// Creates a `ColumnReader` for a given database column.
pub fn make_column_reader(column: &PgColumn) -> Result<Arc<dyn ColumnReader>> {
    match column.type_info().name() {
        "VARCHAR" => Ok(Arc::new(StringColumnReader {
            field: Field::new(column.name(), DataType::Utf8, true),
            column_ordinal: column.ordinal(),
        })),
        "UUID" => Ok(Arc::new(UuidColumnReader {
            field: Field::new(column.name(), DataType::Utf8, true),
            column_ordinal: column.ordinal(),
        })),
        "TIMESTAMPTZ" => Ok(Arc::new(TimestampColumnReader {
            field: Field::new(
                column.name(),
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())), //postgres only stores microseconds, but every event is in nanoseconds
                true,
            ),
            column_ordinal: column.ordinal(),
        })),
        "INT8" => Ok(Arc::new(Int64ColumnReader {
            field: Field::new(column.name(), DataType::Int64, true),
            column_ordinal: column.ordinal(),
        })),
        "INT4" => Ok(Arc::new(Int32ColumnReader {
            field: Field::new(column.name(), DataType::Int32, true),
            column_ordinal: column.ordinal(),
        })),
        "BYTEA" => Ok(Arc::new(BlobColumnReader {
            field: Field::new(column.name(), DataType::Binary, true),
            column_ordinal: column.ordinal(),
        })),
        "TEXT[]" => Ok(Arc::new(StringArrayColumnReader {
            field: Field::new(
                column.name(),
                DataType::List(Arc::new(Field::new("tag", DataType::Utf8, false))),
                true,
            ),
            column_ordinal: column.ordinal(),
        })),
        "micromegas_property[]" => Ok(Arc::new(PropertiesColumnReader {
            field: Field::new(
                column.name(),
                DataType::Dictionary(
                    Box::new(DataType::Int32),
                    Box::new(DataType::List(Arc::new(Field::new(
                        "Property",
                        DataType::Struct(Fields::from(vec![
                            Field::new("key", DataType::Utf8, false),
                            Field::new("value", DataType::Utf8, false),
                        ])),
                        false,
                    )))),
                ),
                true,
            ),
            column_ordinal: column.ordinal(),
        })),
        other => anyhow::bail!("unknown type {other}"),
    }
}

/// Converts a slice of database rows to an Arrow `RecordBatch`.
pub fn rows_to_record_batch(rows: &[PgRow]) -> Result<RecordBatch> {
    if rows.is_empty() {
        return Ok(make_empty_record_batch());
    }

    let mut field_readers = vec![];
    for column in rows[0].columns() {
        field_readers
            .push(make_column_reader(column).with_context(|| "error building column reader")?);
    }

    // Separate dictionary columns from regular columns
    let mut regular_fields = vec![];
    let mut regular_readers = vec![];
    let mut dictionary_arrays = vec![];
    let mut regular_column_indices = vec![];

    for (i, reader) in field_readers.iter().enumerate() {
        if let Ok(Some(dict_array)) = reader.extract_all_from_rows(rows) {
            // This is a dictionary column
            dictionary_arrays.push((i, dict_array));
        } else {
            // This is a regular column
            regular_fields.push(reader.field());
            regular_readers.push(reader);
            regular_column_indices.push(i);
        }
    }

    // Build regular columns using the existing struct builder approach
    let mut arrays: Vec<Option<Arc<dyn Array>>> = vec![None; field_readers.len()];

    if !regular_readers.is_empty() {
        let mut list_builder = ListBuilder::new(StructBuilder::from_fields(regular_fields, 1024));
        let struct_builder: &mut StructBuilder = list_builder.values();

        for r in rows {
            for (struct_field_idx, reader) in regular_readers.iter().enumerate() {
                reader.extract_column_from_row_with_field_index(
                    r,
                    struct_builder,
                    struct_field_idx,
                )?;
            }
            struct_builder.append(true);
        }
        list_builder.append(true);
        let array = list_builder.finish();
        let struct_array = as_struct_array(array.values())
            .with_context(|| "casting list values to struct array")?;

        // Place regular column arrays in their correct positions using the stored indices
        for (regular_col_idx, &original_idx) in regular_column_indices.iter().enumerate() {
            arrays[original_idx] = Some(struct_array.column(regular_col_idx).clone());
        }
    }

    // Place dictionary arrays in their correct positions
    for (i, dict_array) in dictionary_arrays {
        arrays[i] = Some(dict_array);
    }

    // Convert to required Vec<Arc<dyn Array>>
    let final_arrays: Vec<Arc<dyn Array>> = arrays
        .into_iter()
        .map(|opt| opt.expect("All arrays should be populated"))
        .collect();

    let fields: Vec<_> = field_readers.iter().map(|reader| reader.field()).collect();
    let schema = Arc::new(datafusion::arrow::datatypes::Schema::new(fields));
    RecordBatch::try_new(schema, final_arrays).with_context(|| "creating record batch")
}
