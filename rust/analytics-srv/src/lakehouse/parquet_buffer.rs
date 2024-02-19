use super::column::TableColumn;
use anyhow::{Context, Result};
use tracing::prelude::*;
use parquet::file::properties::WriterProperties;
use parquet::file::writer::FileWriter;
use parquet::file::writer::SerializedFileWriter;
use parquet::file::writer::TryClone;
use parquet::schema::parser::parse_message_type;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

#[derive(Clone)]
pub struct InMemStream {
    _cursor: Arc<Cursor<Vec<u8>>>,
    cursor_ptr: *mut Cursor<Vec<u8>>, // until we can use get_mut_unchecked
}

#[allow(unsafe_code)]
unsafe impl Send for InMemStream {}

impl InMemStream {
    pub fn new(cursor: Arc<Cursor<Vec<u8>>>) -> Self {
        let cursor_ptr = Arc::as_ptr(&cursor) as *mut std::io::Cursor<Vec<u8>>;
        Self {
            _cursor: cursor,
            cursor_ptr,
        }
    }
}

impl std::io::Write for InMemStream {
    #[allow(unsafe_code)]
    fn write(&mut self, buf: &[u8]) -> Result<usize, std::io::Error> {
        unsafe { std::io::Write::write(&mut *self.cursor_ptr, buf) }
    }

    #[allow(unsafe_code)]
    fn flush(&mut self) -> Result<(), std::io::Error> {
        unsafe { std::io::Write::flush(&mut *self.cursor_ptr) }
    }
}

impl std::io::Seek for InMemStream {
    #[allow(unsafe_code)]
    fn seek(&mut self, pos: std::io::SeekFrom) -> Result<u64, std::io::Error> {
        unsafe { (&mut *self.cursor_ptr).seek(pos) }
    }
}

impl TryClone for InMemStream {
    fn try_clone(&self) -> Result<Self, std::io::Error> {
        Ok(self.clone())
    }
}

//
// ParquetBufferWriter
//
pub struct ParquetBufferWriter {
    buffer: Arc<Cursor<Vec<u8>>>,
    file_writer: SerializedFileWriter<InMemStream>,
}

impl ParquetBufferWriter {
    #[span_fn]
    pub fn create(message_type: &str) -> Result<Self> {
        let schema =
            Arc::new(parse_message_type(message_type).with_context(|| "parsing parquet schema")?);
        let props = Arc::new(WriterProperties::builder().build());
        let buffer = Arc::new(Cursor::new(Vec::new()));
        let file_writer =
            SerializedFileWriter::new(InMemStream::new(buffer.clone()), schema, props)
                .with_context(|| "creating parquet writer")?;
        Ok(Self {
            buffer,
            file_writer,
        })
    }

    #[span_fn]
    pub fn close(mut self) -> Result<Arc<Cursor<Vec<u8>>>> {
        self.file_writer.close()?;
        Ok(self.buffer)
    }
    #[span_fn]
    pub fn write_row_group(&mut self, columns: &[&dyn TableColumn]) -> Result<()> {
        let mut row_group_writer = self
            .file_writer
            .next_row_group()
            .with_context(|| "creating row group writer")?;
        for c in columns {
            c.write_batch(&mut *row_group_writer)
                .with_context(|| "writing column")?;
        }
        self.file_writer
            .close_row_group(row_group_writer)
            .with_context(|| "closing row group")?;
        Ok(())
    }
}

//
// file output
//
pub async fn write_to_file(writer: ParquetBufferWriter, filepath: &Path) -> Result<()> {
    if let Some(parent) = filepath.parent() {
        tokio::fs::create_dir_all(&parent)
            .await
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }
    let buffer = writer.close()?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .open(filepath)
        .await
        .with_context(|| format!("creating file {}", filepath.display()))?;
    file.write_all(buffer.as_ref().get_ref()).await?;
    Ok(())
}
