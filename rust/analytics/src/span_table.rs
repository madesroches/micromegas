use std::sync::Arc;

use crate::call_tree::CallTree;
use crate::call_tree::CallTreeNode;
use anyhow::{Context, Result};
use datafusion::arrow::array::ArrayBuilder;
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
use datafusion::arrow::record_batch::RecordBatch;

#[derive(Debug)]
pub struct SpanRow {
    pub id: i64,
    pub parent: i64,
    pub depth: u32,
    pub begin: i64,
    pub end: i64,
    pub hash: u32,
    pub name: Arc<String>,
    pub target: Arc<String>,
    pub filename: Arc<String>,
    pub line: u32,
}

pub struct SpanRecordBuilder {
    pub ids: PrimitiveBuilder<Int64Type>,
    pub parents: PrimitiveBuilder<Int64Type>,
    pub depths: PrimitiveBuilder<UInt32Type>,
    pub hashes: PrimitiveBuilder<UInt32Type>,
    pub begins: PrimitiveBuilder<TimestampNanosecondType>,
    pub ends: PrimitiveBuilder<TimestampNanosecondType>,
    pub durations: PrimitiveBuilder<Int64Type>,
    pub names: StringDictionaryBuilder<Int16Type>,
    pub targets: StringDictionaryBuilder<Int16Type>,
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
            targets: StringDictionaryBuilder::new(),
            filenames: StringDictionaryBuilder::new(),
            lines: PrimitiveBuilder::with_capacity(capacity),
        }
    }

    pub fn len(&self) -> i64 {
        self.ids.len() as i64
    }

    pub fn is_empty(&self) -> bool {
        self.ids.len() == 0
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
        self.targets.append_value(&*row.target);
        self.filenames.append_value(&*row.filename);
        self.lines.append_value(row.line);
        Ok(())
    }

    pub fn append_call_tree(&mut self, tree: &CallTree) -> Result<()> {
        if tree.call_tree_root.is_some() {
            for_each_node_in_tree(
                tree.call_tree_root.as_ref().unwrap(),
                0,
                0,
                &mut |node, parent, depth| {
                    let scope_desc = tree
                        .scopes
                        .get(&node.hash)
                        .with_context(|| "fetching scope_desc from hash")?;
                    self.append(SpanRow {
                        id: node.id.unwrap_or(-1),
                        parent,
                        depth,
                        begin: node.begin,
                        end: node.end,
                        hash: node.hash,
                        name: scope_desc.name.clone(),
                        target: scope_desc.target.clone(),
                        filename: scope_desc.filename.clone(),
                        line: scope_desc.line,
                    })
                },
            )?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<RecordBatch> {
        let schema = Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("parent", DataType::Int64, false),
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
                "target",
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
                Arc::new(self.targets.finish()),
                Arc::new(self.filenames.finish()),
                Arc::new(self.lines.finish()),
            ],
        )
        .with_context(|| "building record batch")
    }
}

fn for_each_node_in_tree<NodeFun>(
    node: &CallTreeNode,
    parent: i64,
    depth: u32,
    process_node: &mut NodeFun,
) -> Result<()>
where
    NodeFun: FnMut(&CallTreeNode, i64, u32) -> Result<()>,
{
    process_node(node, parent, depth)?;
    let span_id = node.id.unwrap_or(-1);
    for child in &node.children {
        for_each_node_in_tree(child, span_id, depth + 1, process_node)?;
    }
    Ok(())
}
