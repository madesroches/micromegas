use anyhow::{Context, Result};
use parquet::data_type::ByteArray;
use parquet::{column::writer::ColumnWriter, file::writer::RowGroupWriter};
use micromegas_tracing::prelude::*;

pub trait TableColumn {
    fn write_batch(&self, row_group_writer: &mut dyn RowGroupWriter) -> Result<()>;
}

#[derive(Debug)]
pub struct Column<T> {
    pub values: Vec<T>,
}

impl<T> Column<T> {
    pub fn new() -> Self {
        Self { values: vec![] }
    }

    pub fn append(&mut self, v: T) {
        self.values.push(v);
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn get(&self, i: usize) -> Result<&T> {
        self.values
            .get(i)
            .with_context(|| "out of bounds reading column")
    }
}

impl<T> Default for Column<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl TableColumn for Column<i32> {
    #[span_fn]
    fn write_batch(&self, row_group_writer: &mut dyn RowGroupWriter) -> Result<()> {
        if let Some(mut col_writer) = row_group_writer
            .next_column()
            .with_context(|| "creating column writer")?
        {
            if let ColumnWriter::Int32ColumnWriter(writer_impl) = &mut col_writer {
                writer_impl
                    .write_batch(&self.values, None, None)
                    .with_context(|| "writing i32 batch")?;
            }
            row_group_writer
                .close_column(col_writer)
                .with_context(|| "closing column")?;
        }
        Ok(())
    }
}

impl TableColumn for Column<i64> {
    #[span_fn]
    fn write_batch(&self, row_group_writer: &mut dyn RowGroupWriter) -> Result<()> {
        if let Some(mut col_writer) = row_group_writer
            .next_column()
            .with_context(|| "creating column writer")?
        {
            if let ColumnWriter::Int64ColumnWriter(writer_impl) = &mut col_writer {
                writer_impl
                    .write_batch(&self.values, None, None)
                    .with_context(|| "writing i64 batch")?;
            }
            row_group_writer
                .close_column(col_writer)
                .with_context(|| "closing column")?;
        }
        Ok(())
    }
}

impl TableColumn for Column<f64> {
    #[span_fn]
    fn write_batch(&self, row_group_writer: &mut dyn RowGroupWriter) -> Result<()> {
        if let Some(mut col_writer) = row_group_writer
            .next_column()
            .with_context(|| "creating column writer")?
        {
            if let ColumnWriter::DoubleColumnWriter(writer_impl) = &mut col_writer {
                writer_impl
                    .write_batch(&self.values, None, None)
                    .with_context(|| "writing f64 batch")?;
            }
            row_group_writer
                .close_column(col_writer)
                .with_context(|| "closing column")?;
        }
        Ok(())
    }
}

impl TableColumn for Column<ByteArray> {
    #[span_fn]
    fn write_batch(&self, row_group_writer: &mut dyn RowGroupWriter) -> Result<()> {
        if let Some(mut col_writer) = row_group_writer
            .next_column()
            .with_context(|| "creating column writer")?
        {
            if let ColumnWriter::ByteArrayColumnWriter(writer_impl) = &mut col_writer {
                writer_impl
                    .write_batch(&self.values, None, None)
                    .with_context(|| "writing string batch")?;
            }
            row_group_writer
                .close_column(col_writer)
                .with_context(|| "closing column")?;
        }
        Ok(())
    }
}
