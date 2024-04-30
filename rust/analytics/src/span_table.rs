use std::sync::Arc;

use crate::call_tree::CallTree;
use crate::call_tree::CallTreeNode;
use anyhow::{Context, Result};
use datafusion::arrow::array::PrimitiveBuilder;
use datafusion::arrow::array::StringDictionaryBuilder;
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::datatypes::Field;
use datafusion::arrow::datatypes::Int16Type;
use datafusion::arrow::datatypes::Int64Type;
use datafusion::arrow::datatypes::Schema;
use datafusion::arrow::datatypes::TimeUnit;
use datafusion::arrow::datatypes::TimestampNanosecondType;
use datafusion::arrow::datatypes::UInt32Type;
use datafusion::arrow::datatypes::UInt64Type;
use datafusion::arrow::record_batch::RecordBatch;

#[derive(Debug)]
pub struct SpanRow {
    pub id: u64,
    pub parent: u64,
    pub depth: u32,
    pub begin: i64,
    pub end: i64,
    pub hash: u32,
    pub name: Arc<String>,
    pub filename: Arc<String>,
    pub line: u32,
}

pub struct SpanRecordBuilder {
    pub ids: PrimitiveBuilder<UInt64Type>,
    pub parents: PrimitiveBuilder<UInt64Type>,
    pub depths: PrimitiveBuilder<UInt32Type>,
    pub hashes: PrimitiveBuilder<UInt32Type>,
    pub begins: PrimitiveBuilder<TimestampNanosecondType>,
    pub ends: PrimitiveBuilder<TimestampNanosecondType>,
    pub durations: PrimitiveBuilder<Int64Type>,
    pub names: StringDictionaryBuilder<Int16Type>,
    pub filenames: StringDictionaryBuilder<Int16Type>,
    pub lines: PrimitiveBuilder<UInt32Type>,
}

impl SpanRecordBuilder {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            ids: PrimitiveBuilder::with_capacity(capacity),
            parents: PrimitiveBuilder::with_capacity(capacity),
            depths: PrimitiveBuilder::with_capacity(capacity),
            hashes: PrimitiveBuilder::with_capacity(capacity),
            begins: PrimitiveBuilder::with_capacity(capacity),
            ends: PrimitiveBuilder::with_capacity(capacity),
            durations: PrimitiveBuilder::with_capacity(capacity),
            names: StringDictionaryBuilder::new(), //we could estimate the number of different names and their size
            filenames: StringDictionaryBuilder::new(),
            lines: PrimitiveBuilder::with_capacity(capacity),
        }
    }

    pub fn append(&mut self, row: SpanRow) -> Result<()> {
        self.ids.append_value(row.id);
        self.parents.append_value(row.parent);
        self.depths.append_value(row.depth);
        self.hashes.append_value(row.hash);
        self.begins.append_value(row.begin);
        self.ends.append_value(row.end);
        self.durations.append_value(row.end - row.begin);
        self.names.append_value(&*row.name);
        self.filenames.append_value(&*row.filename);
        self.lines.append_value(row.line);
        Ok(())
    }

    pub fn finish(mut self) -> Result<RecordBatch> {
        let schema = Schema::new(vec![
            Field::new("id", DataType::UInt64, false),
            Field::new("parent", DataType::UInt64, false),
            Field::new("depth", DataType::UInt32, false),
            Field::new("hash", DataType::UInt32, false),
            Field::new(
                "begin",
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                false,
            ),
            Field::new(
                "end",
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                false,
            ),
            Field::new("duration", DataType::Int64, false), //DataType::Duration not supported by parquet
            Field::new(
                "name",
                DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
                false,
            ),
            Field::new(
                "filename",
                DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
                false,
            ),
            Field::new("line", DataType::UInt32, false),
        ]);
        RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(self.ids.finish()),
                Arc::new(self.parents.finish()),
                Arc::new(self.depths.finish()),
                Arc::new(self.hashes.finish()),
                Arc::new(self.begins.finish().with_timezone_utc()),
                Arc::new(self.ends.finish().with_timezone_utc()),
                Arc::new(self.durations.finish()),
                Arc::new(self.names.finish()),
                Arc::new(self.filenames.finish()),
                Arc::new(self.lines.finish()),
            ],
        )
        .with_context(|| "building record batch")
    }
}

fn for_each_node_in_tree<NodeFun>(
    node: &CallTreeNode,
    parent: u64,
    depth: u32,
    next_id: &mut u64,
    process_node: &mut NodeFun,
) -> Result<()>
where
    NodeFun: FnMut(&CallTreeNode, u64, u64, u32) -> Result<()>,
{
    let span_id = *next_id; //todo: use event sequence as span id
    *next_id += 1;
    process_node(node, span_id, parent, depth)?;
    for child in &node.children {
        for_each_node_in_tree(child, span_id, depth + 1, next_id, process_node)?;
    }
    Ok(())
}

pub fn call_tree_to_record_batch(tree: &CallTree) -> Result<RecordBatch> {
    let mut record_builder = SpanRecordBuilder::with_capacity(1024); //todo: replace with number of nodes
    if tree.call_tree_root.is_some() {
        let mut next_id = 1;
        for_each_node_in_tree(
            tree.call_tree_root.as_ref().unwrap(),
            0,
            0,
            &mut next_id,
            &mut |node, id, parent, depth| {
                let scope_desc = tree
                    .scopes
                    .get(&node.hash)
                    .with_context(|| "fetching scope_desc from hash")?;
                record_builder.append(SpanRow {
                    id,
                    parent,
                    depth,
                    begin: node.begin,
                    end: node.end,
                    hash: node.hash,
                    name: scope_desc.name.clone(),
                    filename: scope_desc.filename.clone(),
                    line: scope_desc.line,
                })
            },
        )?;
    }
    record_builder.finish()
}
