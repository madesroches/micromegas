use anyhow::{Context, Result};
use datafusion::arrow::array::PrimitiveBuilder;
use datafusion::arrow::array::StringBuilder;
use datafusion::arrow::array::StructBuilder;
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::datatypes::Field;
use datafusion::arrow::datatypes::Int64Type;
use sqlx::postgres::{PgColumn, PgRow};
use sqlx::Column;
use sqlx::Row;
use sqlx::TypeInfo;
use std::sync::Arc;

pub trait ColumnReader {
    fn extract_column_from_row(
        &self,
        row: &PgRow,
        struct_builder: &mut StructBuilder,
    ) -> Result<()>;
    fn field(&self) -> Field;
}

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
            .with_context(|| "getting field builder")?;
        field_builder.append_value(value);
        Ok(())
    }

    fn field(&self) -> Field {
        self.field.clone()
    }
}

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
            .with_context(|| "getting field builder")?;
        field_builder.append_value(value);
        Ok(())
    }

    fn field(&self) -> Field {
        self.field.clone()
    }
}

pub fn make_column_reader(column: &PgColumn) -> Result<Arc<dyn ColumnReader>> {
    match column.type_info().name() {
        "VARCHAR" => Ok(Arc::new(StringColumnReader {
            field: Field::new(column.name(), DataType::Utf8, true),
            column_ordinal: column.ordinal(),
        })),
        "INT8" => Ok(Arc::new(Int64ColumnReader {
            field: Field::new(column.name(), DataType::Int64, true),
            column_ordinal: column.ordinal(),
        })),
        other => anyhow::bail!("unknown type {other}"),
    }
}
