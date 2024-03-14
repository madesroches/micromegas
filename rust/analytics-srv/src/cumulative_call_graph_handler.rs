use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset};
use sqlx::PgPool;
use std::sync::Arc;

use micromegas_analytics::prelude::*;
use micromegas_tracing::span_fn;

use crate::{
    cumulative_call_graph::span_overlaps,
    cumulative_call_graph_node::{CallGraphNode, CallNodeHashMap},
    lakehouse::{jit_lakehouse::JitLakehouse, span_table::TabularSpanTree},
    scope::compute_scope_hash,
};

pub struct CumulativeCallGraphHandler {
    pool: PgPool,
    jit_lakehouse: Arc<dyn JitLakehouse>,
}

impl CumulativeCallGraphHandler {
    pub fn new(pool: PgPool, jit_lakehouse: Arc<dyn JitLakehouse>) -> Self {
        Self {
            pool,
            jit_lakehouse,
        }
    }

    #[span_fn]
    #[allow(clippy::cast_precision_loss)]
    pub(crate) async fn get_process_call_graph_manifest(
        &self,
        process_id: String,
        begin_ms: f64,
        end_ms: f64,
    ) -> Result<CumulativeCallGraphManifest> {
        let mut connection = self.pool.acquire().await?;

        // For now child processes are not queried and as a result don't participate in cumulative call graph computations.
        let process = find_process(&mut connection, &process_id).await?;
        let time_range = get_process_time_range(&process, begin_ms, end_ms)?;
        let begin = time_range.0.to_rfc3339();
        let end = time_range.1.to_rfc3339();

        let mut block_desc: Vec<CumulativeCallGraphBlockDesc> = vec![];

        let inv_tsc_frequency = get_process_tick_length_ms(&process);

        let streams = find_process_thread_streams(&mut connection, &process_id).await?;
        for s in streams {
            let blocks =
                find_stream_blocks_in_range(&mut connection, &s.stream_id, &begin, &end).await?;
            let data: Vec<CumulativeCallGraphBlockDesc> = blocks
                .iter()
                .map(|b| CumulativeCallGraphBlockDesc {
                    full: ((b.begin_ticks - process.start_ticks) as f64 * inv_tsc_frequency)
                        >= begin_ms
                        && ((b.end_ticks - process.start_ticks) as f64 * inv_tsc_frequency)
                            <= end_ms,
                    id: b.block_id.clone(),
                })
                .collect();
            block_desc.extend_from_slice(&data);
        }
        Ok(CumulativeCallGraphManifest {
            blocks: block_desc,
            start_ticks: process.start_ticks,
            tsc_frequency: process.tsc_frequency,
        })
    }

    #[span_fn]
    pub(crate) async fn get_call_graph_computed_block(
        &self,
        block_id: String,
        _start_ticks: i64,
        _tsc_frequency: u64,
        begin_ms: f64,
        end_ms: f64,
    ) -> Result<CumulativeCallGraphComputedBlock> {
        let mut connection = self.pool.acquire().await?;
        let stream = find_block_stream(&mut connection, &block_id).await?;
        let thread_name = stream
            .properties
            .get("thread-name")
            .map_or_else(|| String::from("Unknown"), std::borrow::ToOwned::to_owned);

        let process = find_block_process(&mut connection, &block_id).await?;
        let (scopes, tree) = self
            .jit_lakehouse
            .get_call_tree(&process, &stream, &block_id)
            .await?;

        let mut result = CallNodeHashMap::new();
        let full = tree.get_begin() >= begin_ms && tree.get_end() <= end_ms;
        for root_id in tree.get_roots() {
            build_graph(&tree, *root_id, begin_ms, end_ms, &mut result)?;
        }

        let nodes = result
            .iter()
            .map(|(_, node)| node.to_proto_node())
            .collect();

        Ok(CumulativeCallGraphComputedBlock {
            full,
            scopes,
            nodes,
            stream_hash: compute_scope_hash(&stream.stream_id),
            stream_name: thread_name,
        })
    }
}

#[span_fn]
fn build_graph(
    tree: &TabularSpanTree,
    spanid: i64,
    begin_ms: f64,
    end_ms: f64,
    result: &mut CallNodeHashMap,
) -> Result<()> {
    let row = tree.get_span(spanid)?;
    if !span_overlaps(row, begin_ms, end_ms) {
        return Ok(());
    }

    let node = result
        .entry(row.hash)
        .or_insert_with(|| CallGraphNode::new(row.hash, begin_ms, end_ms));
    node.add_call(tree, row)?;

    if let Some(children) = tree.span_children.get(&spanid) {
        for childid in children {
            build_graph(tree, *childid, begin_ms, end_ms, result)?;
        }
    }
    Ok(())
}

#[span_fn]
fn get_process_time_range(
    process: &Process,
    begin_ms: f64,
    end_ms: f64,
) -> Result<(DateTime<FixedOffset>, DateTime<FixedOffset>)> {
    let start_time = chrono::DateTime::parse_from_rfc3339(&process.start_time)
        .with_context(|| String::from("parsing process start time"))?;
    let begin_offset_ns = begin_ms * 1_000_000.0;
    let begin_time = start_time + chrono::Duration::nanoseconds(begin_offset_ns as i64);
    let end_offset_ns = end_ms * 1_000_000.0;
    let end_time = start_time + chrono::Duration::nanoseconds(end_offset_ns as i64);
    Ok((begin_time, end_time))
}
