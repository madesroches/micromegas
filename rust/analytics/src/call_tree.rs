use crate::scope::ScopeDesc;
use crate::scope::ScopeHashMap;
use crate::thread_block_processor::parse_thread_block;
use crate::thread_block_processor::ThreadBlockProcessor;
use crate::time::ConvertTicks;
use anyhow::Result;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_telemetry::types::block::BlockMetadata;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

#[derive(Debug)]
pub struct CallTreeNode {
    pub hash: u32,
    pub begin: i64, //absolute nanoseconds
    pub end: i64,
    pub children: Vec<CallTreeNode>,
}

#[derive(Debug)]
pub struct CallTree {
    pub scopes: ScopeHashMap,
    // the root node corresponds to the thread and has a span equal to the query range
    pub call_tree_root: Option<CallTreeNode>,
}

pub struct CallTreeBuilder {
    begin_range_ns: i64,
    end_range_ns: i64,
    limit: i64,
    nb_spans: i64,
    stack: Vec<CallTreeNode>,
    scopes: ScopeHashMap,
    convert_ticks: ConvertTicks,
    root_hash: u32,
}

impl CallTreeBuilder {
    pub fn new(
        ts_begin_range: i64,
        ts_end_range: i64,
        limit: i64,
        convert_ticks: ConvertTicks,
        thread_name: String,
    ) -> Self {
        let thread_scope_desc = ScopeDesc::new(Arc::new(thread_name), Arc::new("".to_owned()), 0);
        let mut scopes = ScopeHashMap::new();
        let root_hash = thread_scope_desc.hash;
        scopes.insert(root_hash, thread_scope_desc);
        Self {
            begin_range_ns: convert_ticks.ticks_to_nanoseconds(ts_begin_range),
            end_range_ns: convert_ticks.ticks_to_nanoseconds(ts_end_range),
            limit,
            nb_spans: 0,
            stack: Vec::new(),
            scopes,
            convert_ticks,
            root_hash,
        }
    }

    #[span_fn]
    pub fn finish(mut self) -> CallTree {
        if self.stack.is_empty() {
            return CallTree {
                scopes: self.scopes,
                call_tree_root: None,
            };
        }
        while self.stack.len() > 1 {
            let top = self.stack.pop().unwrap();
            let last_index = self.stack.len() - 1;
            let parent = &mut self.stack[last_index];
            parent.children.push(top);
        }
        assert_eq!(1, self.stack.len());
        CallTree {
            scopes: self.scopes,
            call_tree_root: self.stack.pop(),
        }
    }

    fn add_child_to_top(&mut self, scope: CallTreeNode) {
        if let Some(mut top) = self.stack.pop() {
            top.children.push(scope);
            self.stack.push(top);
        } else {
            let new_root = CallTreeNode {
                hash: self.root_hash,
                begin: self.begin_range_ns,
                end: self.end_range_ns,
                children: vec![scope],
            };
            self.stack.push(new_root);
        }
    }

    fn record_scope_desc(&mut self, scope_desc: ScopeDesc) {
        self.scopes
            .entry(scope_desc.hash)
            .or_insert_with(|| scope_desc);
    }
}

impl ThreadBlockProcessor for CallTreeBuilder {
    fn on_begin_thread_scope(&mut self, scope: ScopeDesc, ts: i64) -> Result<bool> {
        if self.nb_spans >= self.limit {
            return Ok(false);
        }
        let time = self.convert_ticks.ticks_to_nanoseconds(ts);
        let hash = scope.hash;
        self.record_scope_desc(scope);
        let node = CallTreeNode {
            hash,
            begin: time,
            end: self.end_range_ns,
            children: Vec::new(),
        };
        self.stack.push(node);
        self.nb_spans += 1;
        Ok(true) // continue even if we reached the limit to allow the opportunity to close than span
    }

    fn on_end_thread_scope(&mut self, scope: ScopeDesc, ts: i64) -> Result<bool> {
        let time = self.convert_ticks.ticks_to_nanoseconds(ts);
        let hash = scope.hash;
        self.record_scope_desc(scope);
        if let Some(mut old_top) = self.stack.pop() {
            if old_top.hash == hash {
                old_top.end = time;
                self.add_child_to_top(old_top);
            } else if old_top.hash == self.root_hash {
                old_top.hash = hash;
                old_top.end = time;
                self.add_child_to_top(old_top);
            } else {
                anyhow::bail!("top scope mismatch parsing thread block");
            }
        } else {
            if self.nb_spans >= self.limit {
                return Ok(false);
            }
            let node = CallTreeNode {
                hash,
                begin: self.begin_range_ns,
                end: time,
                children: Vec::new(),
            };
            self.add_child_to_top(node);
            self.nb_spans += 1;
        }
        Ok(true)
    }
}

#[allow(clippy::cast_precision_loss)]
#[span_fn]
pub async fn make_call_tree(
    blocks: &[BlockMetadata],
    begin_ticks_query: i64,
    end_ticks_query: i64,
    limit: i64,
    blob_storage: Arc<BlobStorage>,
    convert_ticks: ConvertTicks,
    stream: &micromegas_telemetry::stream_info::StreamInfo,
) -> Result<CallTree> {
    let mut builder = CallTreeBuilder::new(
        begin_ticks_query,
        end_ticks_query,
        limit - 1, // the thread node eats one span
        convert_ticks,
        stream.get_thread_name(),
    );
    for block in blocks {
        parse_thread_block(blob_storage.clone(), stream, block.block_id, &mut builder).await?;
    }
    Ok(builder.finish())
}
