//! Pure, no-DB unit tests for `group_contiguous_block_chains`: the rule deciding which source
//! blocks are decoded into one cross-block tree (a call tree for `thread_spans`, a net span tree
//! for `net_spans`).
//!
//! The case that matters most is `overlapping_seam_stays_one_chain`. Both views previously tested
//! `begin_ticks == previous end_ticks` for contiguity, which never holds for
//! `micromegas_tracing`-produced streams -- `dispatch`'s flush paths stamp the replacement block's
//! `begin` before closing the outgoing block, so consecutive blocks overlap by the cost of the
//! buffer swap -- so every block became its own tree and trees never spanned a block boundary.

use chrono::{DateTime, Utc};
use micromegas_analytics::lakehouse::jit_partitions::group_contiguous_block_chains;
use micromegas_analytics::lakehouse::partition_source_data::PartitionSourceBlock;
use micromegas_analytics::metadata::{ProcessMetadata, StreamMetadata};
use micromegas_telemetry::types::block::BlockMetadata;
use std::sync::Arc;

fn base_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
        .unwrap()
        .to_utc()
}

/// Builds a `PartitionSourceBlock` carrying only the fields chain grouping reads
/// (`begin_ticks`/`end_ticks`); `object_offset` doubles as a stable identity for assertions.
fn block(begin_ticks: i64, end_ticks: i64, object_offset: i64) -> Arc<PartitionSourceBlock> {
    let process_id = uuid::Uuid::nil();
    let stream_id = uuid::Uuid::nil();
    let time = base_time();
    Arc::new(PartitionSourceBlock {
        block: BlockMetadata {
            block_id: uuid::Uuid::new_v4(),
            stream_id,
            process_id,
            begin_time: time,
            end_time: time,
            begin_ticks,
            end_ticks,
            nb_objects: 1,
            payload_size: 0,
            object_offset,
            insert_time: time,
        },
        stream: Arc::new(StreamMetadata {
            process_id,
            stream_id,
            dependencies_metadata: vec![],
            objects_metadata: vec![],
            tags: vec![],
            properties: Arc::new(vec![]),
        }),
        process: Arc::new(ProcessMetadata {
            process_id,
            exe: "test".to_owned(),
            username: "test".to_owned(),
            realname: "test".to_owned(),
            computer: "test".to_owned(),
            distro: "test".to_owned(),
            cpu_brand: "test".to_owned(),
            tsc_frequency: 1_000_000_000,
            start_time: time,
            start_ticks: 0,
            parent_process_id: None,
            properties: Arc::new(vec![]),
        }),
        format: "test".to_owned(),
    })
}

/// The chains, described by each block's `object_offset`, so assertions read as shapes.
fn chain_shapes(blocks: &[Arc<PartitionSourceBlock>]) -> Vec<Vec<i64>> {
    group_contiguous_block_chains(blocks)
        .iter()
        .map(|chain| chain.iter().map(|b| b.object_offset).collect())
        .collect()
}

/// The `micromegas_tracing` (Rust) producer shape: each block's `begin_ticks` lands *before* the
/// previous block's `end_ticks`, because the replacement block's `begin` is stamped before the
/// outgoing block is closed. Coverage is unbroken, so this must stay a single chain -- this is the
/// case the old `==` test got wrong.
#[test]
fn overlapping_seam_stays_one_chain() {
    let blocks = vec![block(0, 105, 0), block(100, 205, 1), block(200, 305, 2)];
    assert_eq!(chain_shapes(&blocks), vec![vec![0, 1, 2]]);
}

/// The Unreal (C++) producer shape: one `DualTime::Now()` is used for both the new block's `begin`
/// and the outgoing block's `Close`, so seams touch exactly. Also a single chain -- the behavior the
/// old `==` test already had, which must not regress.
#[test]
fn touching_seam_stays_one_chain() {
    let blocks = vec![block(0, 100, 0), block(100, 200, 1), block(200, 300, 2)];
    assert_eq!(chain_shapes(&blocks), vec![vec![0, 1, 2]]);
}

/// A real gap -- this block begins strictly after the running end, so blocks are missing in
/// between and a tree built across the seam would be nonsense.
#[test]
fn gap_splits_the_chain() {
    let blocks = vec![block(0, 100, 0), block(500, 600, 1)];
    assert_eq!(chain_shapes(&blocks), vec![vec![0], vec![1]]);
}

/// Gaps and overlaps interleaved: chains break only at the gaps.
#[test]
fn only_gaps_split_a_mixed_run() {
    let blocks = vec![
        block(0, 105, 0),   // chain A
        block(100, 205, 1), // overlaps -> same chain
        block(400, 505, 2), // gap -> chain B
        block(500, 600, 3), // overlaps -> same chain
        block(900, 950, 4), // gap -> chain C
    ];
    assert_eq!(chain_shapes(&blocks), vec![vec![0, 1], vec![2, 3], vec![4]]);
}

/// A short block fully contained in a longer earlier one must not break the chain: the running end
/// is a max over the chain, not just the previous block's `end_ticks`.
#[test]
fn contained_block_does_not_split_the_chain() {
    let blocks = vec![
        block(0, 1000, 0),
        block(10, 20, 1),     // entirely inside block 0
        block(1000, 1100, 2), // resumes at block 0's end, not block 1's
    ];
    assert_eq!(chain_shapes(&blocks), vec![vec![0, 1, 2]]);
}

#[test]
fn degenerate_inputs() {
    assert!(group_contiguous_block_chains(&[]).is_empty());
    assert_eq!(chain_shapes(&[block(0, 100, 0)]), vec![vec![0]]);
    // A zero-length block at the seam is still contiguous.
    assert_eq!(
        chain_shapes(&[block(0, 100, 0), block(100, 100, 1), block(100, 200, 2)]),
        vec![vec![0, 1, 2]]
    );
}
