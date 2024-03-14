use anyhow::{Context, Result};
use parquet::file::reader::ChunkReader;
use parquet::file::reader::FileReader;
use parquet::file::serialized_reader::SerializedFileReader;
use parquet::record::RowAccessor;
use std::collections::HashMap;
use std::path::Path;

use super::column::Column;
use super::column::TableColumn;
use super::parquet_buffer::write_to_file;
use super::parquet_buffer::ParquetBufferWriter;

pub fn make_spans_table_writer() -> Result<ParquetBufferWriter> {
    let schema = "message schema {
    REQUIRED INT32 hash;
    REQUIRED INT32 depth;
    REQUIRED DOUBLE begin_ms;
    REQUIRED DOUBLE end_ms;
    REQUIRED INT64 id;
    REQUIRED INT64 parent;
  }
";
    ParquetBufferWriter::create(schema)
}

pub async fn write_spans_parquet(rows: &SpanRowGroup, parquet_full_path: &Path) -> Result<()> {
    let mut writer = make_spans_table_writer()?;
    writer.write_row_group(&rows.get_columns())?;
    write_to_file(writer, parquet_full_path).await?;
    Ok(())
}

#[derive(Debug)]
pub struct SpanRowGroup {
    //todo: unify with TabularSpanTree, represent using arrow arrays
    pub hashes: Column<i32>,
    pub depths: Column<i32>,
    pub begins: Column<f64>,
    pub ends: Column<f64>,
    pub ids: Column<i64>,
    pub parents: Column<i64>,
}

impl Default for SpanRowGroup {
    fn default() -> Self {
        Self::new()
    }
}

impl SpanRowGroup {
    pub fn new() -> Self {
        Self {
            hashes: Column::new(),
            depths: Column::new(),
            begins: Column::new(),
            ends: Column::new(),
            ids: Column::new(),
            parents: Column::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.hashes.len()
    }

    pub fn get(&self, i: usize) -> Result<SpanRow> {
        Ok(SpanRow {
            hash: *self.hashes.get(i)? as u32,
            depth: *self.depths.get(i)? as u32,
            begin_ms: *self.begins.get(i)?,
            end_ms: *self.ends.get(i)?,
            id: *self.ids.get(i)?,
            parent: *self.parents.get(i)?,
        })
    }

    #[allow(clippy::cast_possible_wrap)]
    pub fn append(&mut self, row: &SpanRow) {
        self.hashes.append(row.hash as i32);
        self.depths.append(row.depth as i32);
        self.begins.append(row.begin_ms);
        self.ends.append(row.end_ms);
        self.ids.append(row.id as i64);
        self.parents.append(row.parent as i64);
    }

    pub fn get_columns(&self) -> Vec<&dyn TableColumn> {
        vec![
            &self.hashes,
            &self.depths,
            &self.begins,
            &self.ends,
            &self.ids,
            &self.parents,
        ]
    }
}

#[derive(Debug)]
pub struct SpanRow {
    pub hash: u32,
    pub depth: u32,
    pub begin_ms: f64,
    pub end_ms: f64,
    pub id: i64,
    pub parent: i64,
}

fn make_rows_from_tree_impl<RowFun>(
    tree: &CallTreeNode,
    parent: i64,
    depth: u32,
    next_id: &mut i64,
    process_row: &mut RowFun,
) where
    RowFun: FnMut(SpanRow),
{
    assert!(tree.hash != 0);
    let span_id = *next_id;
    *next_id += 1;
    let span = SpanRow {
        hash: tree.hash,
        depth,
        begin_ms: tree.begin_ms,
        end_ms: tree.end_ms,
        id: span_id,
        parent,
    };
    process_row(span);
    for child in &tree.children {
        make_rows_from_tree_impl(child, span_id, depth + 1, next_id, process_row);
    }
}

pub fn make_rows_from_tree(tree: &CallTreeNode, next_id: &mut i64, table: &mut SpanRowGroup) {
    if tree.hash == 0 {
        for child in &tree.children {
            make_rows_from_tree_impl(child, 0, 0, next_id, &mut |row| table.append(&row));
        }
    } else {
        make_rows_from_tree_impl(tree, 0, 0, next_id, &mut |row| table.append(&row));
    }
}

//todo: delete this version
pub fn build_spans_lod0<R: 'static + ChunkReader>(
    file_reader: &SerializedFileReader<R>,
) -> Result<SpanBlockLod> {
    let mut lod = SpanBlockLod {
        lod_id: 0,
        tracks: vec![],
    };
    for row in file_reader.get_row_iter(None)? {
        let hash = row.get_int(0)?;
        let depth = row.get_int(1)?;
        let begin = row.get_double(2)?;
        let end = row.get_double(3)?;
        if lod.tracks.len() <= depth as usize {
            lod.tracks.push(SpanTrack { spans: vec![] });
        }
        let span = Span {
            scope_hash: hash as u32,
            begin_ms: begin,
            end_ms: end,
            alpha: 255,
        };
        lod.tracks[depth as usize].spans.push(span);
    }
    Ok(lod)
}

// really need to cleanup those variations and unify the data structures
pub fn lod0_from_span_tree(tree: &TabularSpanTree) -> Result<SpanBlockLod> {
    let mut lod = SpanBlockLod {
        lod_id: 0,
        tracks: vec![],
    };
    for index in 0..tree.len() {
        let row = tree.get_row(index)?;
        let hash = row.hash;
        let depth = row.depth;
        let begin = row.begin_ms;
        let end = row.end_ms;
        if lod.tracks.len() <= depth as usize {
            lod.tracks.push(SpanTrack { spans: vec![] });
        }
        let span = Span {
            scope_hash: hash as u32,
            begin_ms: begin,
            end_ms: end,
            alpha: 255,
        };
        lod.tracks[depth as usize].spans.push(span);
    }
    Ok(lod)
}

#[derive(Debug)]
pub struct TabularSpanTree {
    //todo: represent using arrow arrays
    pub span_rows: HashMap<i64, SpanRow>,
    pub span_children: HashMap<i64, Vec<i64>>,
    roots: Vec<i64>,
    ids: Vec<i64>,
    begin_ms: f64,
    end_ms: f64,
}

impl Default for TabularSpanTree {
    fn default() -> Self {
        Self::new()
    }
}

impl TabularSpanTree {
    pub fn new() -> Self {
        Self {
            span_rows: HashMap::new(),
            span_children: HashMap::new(),
            roots: Vec::new(),
            ids: Vec::new(),
            begin_ms: f64::MAX,
            end_ms: f64::MIN,
        }
    }

    pub fn get_begin(&self) -> f64 {
        self.begin_ms
    }

    pub fn get_end(&self) -> f64 {
        self.end_ms
    }

    pub fn get_roots(&self) -> &Vec<i64> {
        &self.roots
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn get_row(&self, index: usize) -> Result<&SpanRow> {
        let id = self
            .ids
            .get(index)
            .with_context(|| "out of bounds reading id in TabularSpanTree")?;
        self.get_span(*id)
    }

    pub fn get_span(&self, id: i64) -> Result<&SpanRow> {
        self.span_rows
            .get(&id)
            .with_context(|| "accessing span in TabularSpanTree")
    }

    pub fn from_rows(rows: &SpanRowGroup) -> Result<Self> {
        let mut tree = Self::new();
        for i in 0..rows.len() {
            let row = rows.get(i)?;
            let id = row.id;
            let parent = row.parent;
            let depth = row.depth;
            let row = SpanRow {
                hash: row.hash,
                depth,
                begin_ms: row.begin_ms,
                end_ms: row.end_ms,
                id,
                parent,
            };
            tree.begin_ms = tree.begin_ms.min(row.begin_ms);
            tree.end_ms = tree.begin_ms.max(row.end_ms);
            if depth == 0 {
                tree.roots.push(id);
            }
            tree.span_rows.insert(id, row);
            tree.span_children.entry(parent).or_default().push(id);
            tree.ids.push(id);
        }
        Ok(tree)
    }
}

pub fn build_span_tree<R: 'static + ChunkReader>(
    file_reader: &SerializedFileReader<R>,
) -> Result<TabularSpanTree> {
    let mut tree = TabularSpanTree::new();
    for row in file_reader.get_row_iter(None)? {
        let id = row.get_long(4)?;
        let parent = row.get_long(5)?;
        let depth = row.get_int(1)? as u32;
        let row = SpanRow {
            hash: row.get_int(0)? as u32,
            depth,
            begin_ms: row.get_double(2)?,
            end_ms: row.get_double(3)?,
            id,
            parent,
        };
        tree.begin_ms = tree.begin_ms.min(row.begin_ms);
        tree.end_ms = tree.begin_ms.max(row.end_ms);
        if depth == 0 {
            tree.roots.push(id);
        }
        tree.span_rows.insert(id, row);
        tree.span_children.entry(parent).or_default().push(id);
        tree.ids.push(id);
    }
    Ok(tree)
}
