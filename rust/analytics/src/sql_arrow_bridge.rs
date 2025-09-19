use anyhow::{Context, Result};
use datafusion::arrow::array::ArrayBuilder;
use datafusion::arrow::array::BinaryBuilder;
use datafusion::arrow::array::BinaryDictionaryBuilder;
use datafusion::arrow::array::ListBuilder;
use datafusion::arrow::array::PrimitiveBuilder;
use datafusion::arrow::array::StringBuilder;
use datafusion::arrow::array::StructBuilder;
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::datatypes::Field;
use datafusion::arrow::datatypes::Int32Type;
use datafusion::arrow::datatypes::Int64Type;
use datafusion::arrow::datatypes::TimeUnit;
use datafusion::arrow::datatypes::TimestampNanosecondType;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::common::cast::as_struct_array;
use jsonb::Value;
use micromegas_telemetry::property::Property;
use sqlx::Column;
use sqlx::Row;
use sqlx::TypeInfo;
use sqlx::postgres::{PgColumn, PgRow};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::arrow_utils::make_empty_record_batch;

/// A trait for reading a column from a database row and writing it to an Arrow `StructBuilder`.
pub trait ColumnReader {
    fn extract_column_from_row(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
    ) -> Result<()>;
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

    fn field(&self) -> Field {
        self.field.clone()
    }
}

/// A `ColumnReader` for properties columns.
pub struct PropertiesColumnReader {
    pub field: Field,
    pub column_ordinal: usize,
}

impl ColumnReader for PropertiesColumnReader {
    fn extract_column_from_row(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
    ) -> Result<()> {
        let props: Vec<Property> = row
            .try_get(self.column_ordinal)
            .with_context(|| "try_get failed on row")?;

        // Get the dictionary builder for JSONB format
        let dict_builder = struct_builder
            .field_builder::<BinaryDictionaryBuilder<Int32Type>>(self.column_ordinal)
            .with_context(|| "getting dictionary field builder for properties")?;

        // Convert properties to JSONB format
        if props.is_empty() {
            // For empty properties, append an empty JSONB object
            let empty_map = Value::Object(BTreeMap::new());
            let mut buffer = Vec::new();
            empty_map.write_to_vec(&mut buffer);
            dict_builder.append_value(&buffer);
        } else {
            // Build a BTreeMap from properties
            let mut map = BTreeMap::new();
            for p in props {
                map.insert(
                    p.key_str().to_string(),
                    Value::String(Cow::Owned(p.value_str().to_string())),
                );
            }

            // Convert to JSONB bytes
            let jsonb_object = Value::Object(map);
            let mut buffer = Vec::new();
            jsonb_object.write_to_vec(&mut buffer);
            dict_builder.append_value(&buffer);
        }

        Ok(())
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
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Binary)),
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

    let fields: Vec<_> = field_readers.iter().map(|reader| reader.field()).collect();
    let mut list_builder = ListBuilder::new(StructBuilder::from_fields(fields, 1024));
    let struct_builder: &mut StructBuilder = list_builder.values();
    for r in rows {
        for reader in &field_readers {
            reader.extract_column_from_row(r, struct_builder)?;
        }
        struct_builder.append(true);
    }
    list_builder.append(true);
    let array = list_builder.finish();
    Ok(as_struct_array(array.values())
        .with_context(|| "casting list values to struct srray")?
        .into())
}
