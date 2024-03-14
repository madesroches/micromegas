use std::path::Path;

use super::{
    column::{Column, TableColumn},
    parquet_buffer::{write_to_file, ParquetBufferWriter},
};
use crate::scope::ScopeHashMap;
use anyhow::Result;
use parquet::data_type::ByteArray;

#[derive(Debug)]
pub struct ScopeRowGroup {
    pub hashes: Column<i32>,
    pub names: Column<ByteArray>,
    pub filenames: Column<ByteArray>,
    pub lines: Column<i32>,
}

impl ScopeRowGroup {
    pub fn new() -> Self {
        Self {
            hashes: Column::new(),
            names: Column::new(),
            filenames: Column::new(),
            lines: Column::new(),
        }
    }

    #[allow(clippy::cast_possible_wrap)]
    pub fn append(&mut self, row: &ScopeDesc) {
        self.hashes.append(row.hash as i32);
        self.names.append(ByteArray::from(row.name.as_str()));
        self.filenames
            .append(ByteArray::from(row.filename.as_str()));
        self.lines.append(row.line as i32);
    }

    pub fn get_columns(&self) -> Vec<&dyn TableColumn> {
        vec![&self.hashes, &self.names, &self.filenames, &self.lines]
    }
}

pub fn make_scopes_table_writer() -> Result<ParquetBufferWriter> {
    let schema = "message schema {
    REQUIRED INT32 hash;
    REQUIRED BYTE_ARRAY name;
    REQUIRED BYTE_ARRAY filename;
    REQUIRED INT32 line;
  }
";
    ParquetBufferWriter::create(schema)
}

fn make_scope_rows(scopes: &ScopeHashMap) -> ScopeRowGroup {
    let mut rows = ScopeRowGroup::new();
    for (_k, v) in scopes.iter() {
        rows.append(v);
    }
    rows
}

pub async fn write_scopes_parquet(scopes: &ScopeHashMap, parquet_path: &Path) -> Result<()> {
    let mut writer = make_scopes_table_writer()?;
    let rows = make_scope_rows(scopes);
    writer.write_row_group(&rows.get_columns())?;
    write_to_file(writer, parquet_path).await?;
    Ok(())
}
