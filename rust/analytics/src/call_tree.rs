use crate::metadata::{StreamMetadata, get_thread_name_from_stream_metadata};
use crate::scope::ScopeDesc;
use crate::scope::ScopeHashMap;
use crate::thread_block_processor::ThreadBlockProcessor;
use crate::thread_block_processor::parse_thread_block;
use crate::time::ConvertTicks;
use anyhow::Result;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_telemetry::types::block::BlockMetadata;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// A node in a call tree, representing a single scope instance.
#[derive(Debug)]
pub struct CallTreeNode {
    /// The unique identifier of the scope instance.
    pub id: Option<i64>,
    /// The hash of the scope description.
    pub hash: u32,
    /// The start time of the scope instance in nanoseconds.
    pub begin: i64, //absolute nanoseconds
    /// The end time of the scope instance in nanoseconds.
    pub end: i64,
    /// The children of this node in the call tree.
    pub children: Vec<CallTreeNode>,
}

/// A call tree, representing the execution of a single thread.
#[derive(Debug)]
pub struct CallTree {
    /// A map from scope hash to scope description.
    pub scopes: ScopeHashMap,
    /// The root node of the call tree.
    // the root node corresponds to the thread and has a span equal to the query range
    pub call_tree_root: Option<CallTreeNode>,
}

/// A builder for creating a `CallTree` from a stream of thread events.
pub struct CallTreeBuilder {
    begin_range_ns: i64,
    end_range_ns: i64,
    limit: Option<i64>,
    nb_spans: i64,
    stack: Vec<CallTreeNode>,
    scopes: ScopeHashMap,
    convert_ticks: ConvertTicks,
    root_hash: u32,
}

impl CallTreeBuilder {
    pub fn new(
        begin_range_ns: i64,
        end_range_ns: i64,
        limit: Option<i64>,
        convert_ticks: ConvertTicks,
        thread_name: String,
    ) -> Self {
        let thread_scope_desc = ScopeDesc::new(
            Arc::new(thread_name),
            Arc::new("".to_owned()),
            Arc::new("".to_owned()),
            0,
        );
        let mut scopes = ScopeHashMap::new();
        let root_hash = thread_scope_desc.hash;
        scopes.insert(root_hash, thread_scope_desc);
        Self {
            begin_range_ns,
            end_range_ns,
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

    fn add_child_to_top(&mut self, node: CallTreeNode) {
        if let Some(mut top) = self.stack.pop() {
            top.children.push(node);
            self.stack.push(top);
        } else {
            let new_root = CallTreeNode {
                id: None,
                hash: self.root_hash,
                begin: self.begin_range_ns,
                end: self.end_range_ns,
                children: vec![node],
            };
            self.stack.push(new_root);
            self.nb_spans += 1;
        }
    }

    fn record_scope_desc(&mut self, scope_desc: ScopeDesc) {
        self.scopes
            .entry(scope_desc.hash)
            .or_insert_with(|| scope_desc);
    }
}

impl ThreadBlockProcessor for CallTreeBuilder {
    fn on_begin_thread_scope(
        &mut self,
        _block_id: &str,
        event_id: i64,
        scope: ScopeDesc,
        ts: i64,
    ) -> Result<bool> {
        if self.limit.is_some() && self.nb_spans >= self.limit.unwrap() {
            return Ok(false);
        }
        let time = self.convert_ticks.ticks_to_nanoseconds(ts);
        if time < self.begin_range_ns {
            return Ok(true);
        }
        if time > self.end_range_ns {
            return Ok(false);
        }
        let hash = scope.hash;
        self.record_scope_desc(scope);
        let node = CallTreeNode {
            id: Some(event_id),
            hash,
            begin: time,
            end: self.end_range_ns,
            children: Vec::new(),
        };
        self.stack.push(node);
        self.nb_spans += 1;
        Ok(true) // continue even if we reached the limit to allow the opportunity to close than span
    }

    fn on_end_thread_scope(
        &mut self,
        _block_id: &str,
        event_id: i64,
        scope: ScopeDesc,
        ts: i64,
    ) -> Result<bool> {
        let time = self.convert_ticks.ticks_to_nanoseconds(ts);
        if time < self.begin_range_ns {
            return Ok(true);
        }
        if time > self.end_range_ns {
            return Ok(false);
        }
        let hash = scope.hash;
        self.record_scope_desc(scope);
        if let Some(mut old_top) = self.stack.pop() {
            if old_top.hash == hash {
                old_top.end = time;
                self.add_child_to_top(old_top);
            } else if old_top.hash == self.root_hash {
                old_top.id = Some(event_id);
                old_top.hash = hash;
                old_top.end = time;
                self.add_child_to_top(old_top);
            } else {
                anyhow::bail!("top scope mismatch parsing thread block");
            }
        } else {
            if self.limit.is_some() && self.nb_spans >= self.limit.unwrap() {
                return Ok(false);
            }
            let node = CallTreeNode {
                id: Some(event_id),
                hash,
                begin: self.begin_range_ns,
                end: time,
                children: Vec::new(),
            };
            self.add_child_to_top(node);
        }
        Ok(true)
    }
}

/// Creates a call tree from a set of thread event blocks.
#[allow(clippy::cast_precision_loss)]
#[span_fn]
pub async fn make_call_tree(
    blocks: &[BlockMetadata],
    begin_range_ns: i64,
    end_range_ns: i64,
    limit: Option<i64>,
    blob_storage: Arc<BlobStorage>,
    convert_ticks: ConvertTicks,
    stream: &StreamMetadata,
) -> Result<CallTree> {
    let mut builder = CallTreeBuilder::new(
        begin_range_ns,
        end_range_ns,
        limit,
        convert_ticks,
        get_thread_name_from_stream_metadata(stream)?,
    );
    for block in blocks {
        parse_thread_block(
            blob_storage.clone(),
            stream,
            block.block_id,
            block.object_offset,
            &mut builder,
        )
        .await?;
    }
    Ok(builder.finish())
}
