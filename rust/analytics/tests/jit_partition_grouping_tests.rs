//! Pure, no-DB unit tests for `group_blocks_into_partitions`
//! (tasks/1429_jit_event_time_block_ordering_plan.md, Testing Strategy #1-8): the suffix-min
//! insert-safe cut rule, the look-back cut, and the `BlockOrder::InsertTime` behavior-preserving
//! guarantee.

use chrono::{DateTime, TimeDelta, Utc};
use micromegas_analytics::lakehouse::jit_partitions::{
    BlockOrder, JitPartitionConfig, group_blocks_into_partitions,
};
use micromegas_analytics::lakehouse::partition_source_data::{
    PartitionSourceBlock, hash_to_object_count,
};
use micromegas_analytics::metadata::{ProcessMetadata, StreamMetadata};
use micromegas_telemetry::types::block::BlockMetadata;
use std::sync::Arc;

/// Builds a `PartitionSourceBlock` with full control over `begin_ticks`/`end_ticks`/`insert_time`/
/// `nb_objects`, sharing one process/stream id across every block a test builds (grouping doesn't
/// care about stream/process identity, only the block-level fields it cuts on).
fn make_block(
    process_id: uuid::Uuid,
    stream_id: uuid::Uuid,
    begin_ticks: i64,
    end_ticks: i64,
    insert_time: DateTime<Utc>,
    nb_objects: i32,
) -> Arc<PartitionSourceBlock> {
    let block = BlockMetadata {
        block_id: uuid::Uuid::new_v4(),
        stream_id,
        process_id,
        begin_time: insert_time,
        end_time: insert_time,
        begin_ticks,
        end_ticks,
        nb_objects,
        payload_size: 0,
        object_offset: 0,
        insert_time,
    };
    let stream = Arc::new(StreamMetadata {
        process_id,
        stream_id,
        dependencies_metadata: vec![],
        objects_metadata: vec![],
        tags: vec![],
        properties: Arc::new(vec![]),
    });
    let process = Arc::new(ProcessMetadata {
        process_id,
        exe: "test".to_owned(),
        username: "test".to_owned(),
        realname: "test".to_owned(),
        computer: "test".to_owned(),
        distro: "test".to_owned(),
        cpu_brand: "test".to_owned(),
        tsc_frequency: 1_000_000_000,
        start_time: insert_time,
        start_ticks: 0,
        parent_process_id: None,
        properties: Arc::new(vec![]),
    });
    Arc::new(PartitionSourceBlock {
        block,
        stream,
        process,
        format: "test".to_owned(),
    })
}

fn config(max_nb_objects: i64, block_order: BlockOrder) -> JitPartitionConfig {
    JitPartitionConfig {
        max_nb_objects,
        max_insert_time_slice: TimeDelta::hours(1),
        block_order,
    }
}

fn nb_objects(
    part: &micromegas_analytics::lakehouse::partition_source_data::SourceDataBlocksInMemory,
) -> i64 {
    hash_to_object_count(&part.block_ids_hash).expect("valid block_ids_hash")
}

fn base_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
        .unwrap()
        .to_utc()
}

/// Test #1: two consecutive blocks registered in reverse order, single partition: output blocks
/// are event-ordered.
#[test]
fn reproduces_the_bugs_shape() {
    let process_id = uuid::Uuid::new_v4();
    let stream_id = uuid::Uuid::new_v4();
    let t0 = base_time();

    // Block A is later in event time but registered (inserted) first.
    let block_a = make_block(process_id, stream_id, 1000, 2000, t0, 10);
    // Block B is earlier in event time but registered (inserted) second.
    let block_b = make_block(
        process_id,
        stream_id,
        0,
        1000,
        t0 + TimeDelta::seconds(1),
        10,
    );

    // SQL order is insert_time ascending: A, then B.
    let blocks = vec![block_a.clone(), block_b.clone()];
    let cfg = config(1_000_000, BlockOrder::EventTime);
    let parts = group_blocks_into_partitions(&cfg, blocks);

    assert_eq!(parts.len(), 1, "no cut should be needed");
    let part = &parts[0];
    assert_eq!(part.blocks.len(), 2);
    assert_eq!(
        part.blocks[0].block.block_id, block_b.block.block_id,
        "the event-earlier block (B) must come first"
    );
    assert_eq!(
        part.blocks[1].block.block_id, block_a.block.block_id,
        "the event-later block (A) must come second"
    );
}

/// Test #2: several cuts, some local (non-cut-point) insert-time inversions: concatenating the
/// partitions yields non-decreasing `begin_ticks`, and each partition's event range is disjoint
/// from and ordered against the next.
#[test]
fn event_ordering_across_partitions() {
    let process_id = uuid::Uuid::new_v4();
    let stream_id = uuid::Uuid::new_v4();
    let t0 = base_time();

    // 12 blocks, strictly ascending begin_ticks/end_ticks (event order == index order).
    // insert_time ascending by 1s per index EXCEPT swapped within (1,2), (5,6), (9,10) -- local
    // inversions away from the expected cut points at indices 4 and 8 (max_nb_objects=4, 1
    // object/block).
    let mut nominal_insert: Vec<DateTime<Utc>> =
        (0..12).map(|i| t0 + TimeDelta::seconds(i)).collect();
    nominal_insert.swap(1, 2);
    nominal_insert.swap(5, 6);
    nominal_insert.swap(9, 10);

    let blocks: Vec<_> = (0..12i64)
        .map(|i| {
            make_block(
                process_id,
                stream_id,
                i * 100,
                (i + 1) * 100,
                nominal_insert[i as usize],
                1,
            )
        })
        .collect();

    let cfg = config(4, BlockOrder::EventTime);
    let parts = group_blocks_into_partitions(&cfg, blocks);

    assert_eq!(
        parts.len(),
        3,
        "expected exactly 3 partitions of 4 blocks each"
    );
    for part in &parts {
        assert_eq!(part.blocks.len(), 4);
    }

    // Concatenation is non-decreasing in begin_ticks.
    let mut previous_end = None;
    for part in &parts {
        for block in &part.blocks {
            if let Some(prev_end) = previous_end {
                assert!(block.block.begin_ticks >= prev_end);
            }
            previous_end = Some(block.block.begin_ticks);
        }
    }

    // Each partition's event range is disjoint from and ordered against the next.
    for pair in parts.windows(2) {
        let prev_max_end = pair[0]
            .blocks
            .iter()
            .map(|b| b.block.end_ticks)
            .max()
            .unwrap();
        let next_min_begin = pair[1]
            .blocks
            .iter()
            .map(|b| b.block.begin_ticks)
            .min()
            .unwrap();
        assert!(prev_max_end <= next_min_begin);
    }
}

/// Test #3: insert ranges never overlap across partitions, including a case with an inversion
/// placed exactly on the max_nb_objects cut point (the ~10% case that would otherwise fail the
/// write): the cut moves rather than overlapping.
#[test]
fn insert_ranges_never_overlap_even_at_the_cut_point() {
    let process_id = uuid::Uuid::new_v4();
    let stream_id = uuid::Uuid::new_v4();
    let t0 = base_time();

    // 8 blocks, ascending begin_ticks/end_ticks. Nominal insert_time ascending by 1s, except
    // indices 3 and 4 are swapped -- straddling the naive max_nb_objects=4 cut point exactly.
    let mut nominal_insert: Vec<DateTime<Utc>> =
        (0..8).map(|i| t0 + TimeDelta::seconds(i)).collect();
    nominal_insert.swap(3, 4);

    let blocks: Vec<_> = (0..8i64)
        .map(|i| {
            make_block(
                process_id,
                stream_id,
                i * 100,
                (i + 1) * 100,
                nominal_insert[i as usize],
                1,
            )
        })
        .collect();

    let cfg = config(4, BlockOrder::EventTime);
    let parts = group_blocks_into_partitions(&cfg, blocks);

    // The naive cut (uniform groups of 4) would put the inversion pair (index 3, 4) split across
    // two partitions and overlap; the cut must move instead.
    assert!(
        parts.iter().any(|p| p.blocks.len() != 4),
        "expected the cut to move away from the naive uniform split, got sizes {:?}",
        parts.iter().map(|p| p.blocks.len()).collect::<Vec<_>>()
    );

    // Every block appears exactly once, in event order overall.
    let total_blocks: usize = parts.iter().map(|p| p.blocks.len()).sum();
    assert_eq!(total_blocks, 8);

    // Insert ranges never overlap: for every adjacent pair, max_insert(P_k) <= min_insert(P_k+1).
    for pair in parts.windows(2) {
        let prev_max_insert = pair[0]
            .blocks
            .iter()
            .map(|b| b.block.insert_time)
            .max()
            .unwrap();
        let next_min_insert = pair[1]
            .blocks
            .iter()
            .map(|b| b.block.insert_time)
            .min()
            .unwrap();
        assert!(
            prev_max_insert <= next_min_insert,
            "insert ranges overlap: {prev_max_insert} > {next_min_insert}"
        );
    }
}

/// Test #4: a straggler (one block whose insert_time is far later than its event-time neighbors,
/// simulating a slow/retried upload) forces a look-back cut: the partition emitted *before* the
/// straggler's window stays <= max_nb_objects; the re-seeded window containing the straggler is
/// not bounded by the same rule, but has no split/duplicated blocks and non-overlapping insert
/// ranges against its neighbor.
#[test]
fn look_back_cut_isolates_a_straggler() {
    let process_id = uuid::Uuid::new_v4();
    let stream_id = uuid::Uuid::new_v4();
    let t0 = base_time();

    // 9 blocks, ascending begin_ticks/end_ticks. insert_time ascending except index 2, which gets
    // a huge insert_time (the straggler) -- every forward index from there on stays unsafe for
    // the rest of this (finite) test array.
    let mut nominal_insert: Vec<DateTime<Utc>> =
        (0..9).map(|i| t0 + TimeDelta::seconds(i)).collect();
    nominal_insert[2] = t0 + TimeDelta::seconds(1000);

    let blocks: Vec<_> = (0..9i64)
        .map(|i| {
            make_block(
                process_id,
                stream_id,
                i * 100,
                (i + 1) * 100,
                nominal_insert[i as usize],
                1,
            )
        })
        .collect();

    let cfg = config(3, BlockOrder::EventTime);
    let parts = group_blocks_into_partitions(&cfg, blocks);

    assert_eq!(
        parts.len(),
        2,
        "expected the prefix before the straggler, then the straggler's tail"
    );
    assert!(
        nb_objects(&parts[0]) <= 3,
        "the partition preceding the straggler must stay <= max_nb_objects, got {}",
        nb_objects(&parts[0])
    );
    assert!(
        nb_objects(&parts[1]) > 3,
        "the straggler's re-seeded window is expected to grow past the soft limit here, got {}",
        nb_objects(&parts[1])
    );

    // No split or duplicated blocks: every block appears exactly once.
    let total_blocks: usize = parts.iter().map(|p| p.blocks.len()).sum();
    assert_eq!(total_blocks, 9);
    let mut seen = std::collections::HashSet::new();
    for part in &parts {
        for block in &part.blocks {
            assert!(seen.insert(block.block.block_id), "duplicated block");
        }
    }

    // Non-overlapping insert ranges against its neighbor.
    let prev_max_insert = parts[0]
        .blocks
        .iter()
        .map(|b| b.block.insert_time)
        .max()
        .unwrap();
    let next_min_insert = parts[1]
        .blocks
        .iter()
        .map(|b| b.block.insert_time)
        .min()
        .unwrap();
    assert!(prev_max_insert <= next_min_insert);
}

/// Test #5: a continuous inversion chain (strictly decreasing insert_time) has no safe index
/// anywhere: grouping still emits a single partition past max_nb_objects (soft limit, matching
/// today's oversized-block behavior), and safety trivially holds (only one partition).
#[test]
fn no_safe_point_soft_limit_grows_safety_holds() {
    let process_id = uuid::Uuid::new_v4();
    let stream_id = uuid::Uuid::new_v4();
    let t0 = base_time();

    // 6 blocks, ascending begin_ticks/end_ticks, strictly *decreasing* insert_time.
    let blocks: Vec<_> = (0..6i64)
        .map(|i| {
            make_block(
                process_id,
                stream_id,
                i * 100,
                (i + 1) * 100,
                t0 + TimeDelta::seconds(5 - i),
                1,
            )
        })
        .collect();

    let cfg = config(2, BlockOrder::EventTime);
    let parts = group_blocks_into_partitions(&cfg, blocks);

    assert_eq!(
        parts.len(),
        1,
        "no safe cut point exists anywhere in this window"
    );
    assert_eq!(parts[0].blocks.len(), 6);
    assert!(
        nb_objects(&parts[0]) > 2,
        "the single partition should grow past the soft max_nb_objects limit"
    );
}

/// Test #6: under `BlockOrder::InsertTime`, grouping is bit-identical to a naive greedy cut over
/// the incoming (SQL) order -- including partition boundaries and `block_ids_hash`.
#[test]
fn insert_time_order_is_unchanged() {
    let process_id = uuid::Uuid::new_v4();
    let stream_id = uuid::Uuid::new_v4();
    let t0 = base_time();

    // 8 blocks already in insert-time (and event-time) ascending order.
    let blocks: Vec<_> = (0..8i64)
        .map(|i| {
            make_block(
                process_id,
                stream_id,
                i * 100,
                (i + 1) * 100,
                t0 + TimeDelta::seconds(i),
                1,
            )
        })
        .collect();

    let cfg = config(3, BlockOrder::InsertTime);
    let parts = group_blocks_into_partitions(&cfg, blocks);

    // Naive greedy cut at max_nb_objects=3, 1 object/block: sizes 3, 3, 2.
    assert_eq!(
        parts.iter().map(|p| p.blocks.len()).collect::<Vec<_>>(),
        vec![3, 3, 2]
    );
    assert_eq!(
        parts.iter().map(nb_objects).collect::<Vec<_>>(),
        vec![3, 3, 2]
    );
}

/// Test #7: with no inversions, `EventTime` grouping matches `InsertTime` grouping block-for-block.
#[test]
fn size_respected_when_it_can_be() {
    let process_id = uuid::Uuid::new_v4();
    let stream_id = uuid::Uuid::new_v4();
    let t0 = base_time();

    let blocks: Vec<_> = (0..8i64)
        .map(|i| {
            make_block(
                process_id,
                stream_id,
                i * 100,
                (i + 1) * 100,
                t0 + TimeDelta::seconds(i),
                1,
            )
        })
        .collect();

    // Clone the Vec (Arc clones, same block identity) so both runs see identical blocks.
    let insert_time_parts =
        group_blocks_into_partitions(&config(3, BlockOrder::InsertTime), blocks.clone());
    let event_time_parts = group_blocks_into_partitions(&config(3, BlockOrder::EventTime), blocks);

    assert_eq!(insert_time_parts.len(), event_time_parts.len());
    for (a, b) in insert_time_parts.iter().zip(event_time_parts.iter()) {
        assert_eq!(a.block_ids_hash, b.block_ids_hash);
        let a_ids: Vec<_> = a.blocks.iter().map(|blk| blk.block.block_id).collect();
        let b_ids: Vec<_> = b.blocks.iter().map(|blk| blk.block.block_id).collect();
        assert_eq!(a_ids, b_ids);
    }
}

/// Test #8: degenerate inputs -- empty list; one block; all blocks with identical insert_time;
/// identical begin_ticks (tiebreak determinism: same input order in, same grouping out).
#[test]
fn degenerate_inputs() {
    let process_id = uuid::Uuid::new_v4();
    let stream_id = uuid::Uuid::new_v4();
    let t0 = base_time();

    // Empty list.
    for order in [BlockOrder::InsertTime, BlockOrder::EventTime] {
        let parts = group_blocks_into_partitions(&config(10, order), vec![]);
        assert!(parts.is_empty());
    }

    // One block.
    for order in [BlockOrder::InsertTime, BlockOrder::EventTime] {
        let block = make_block(process_id, stream_id, 0, 100, t0, 5);
        let parts = group_blocks_into_partitions(&config(10, order), vec![block.clone()]);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].blocks.len(), 1);
        assert_eq!(parts[0].blocks[0].block.block_id, block.block.block_id);
    }

    // All blocks with identical insert_time: cuts still happen at max_nb_objects (equality is a
    // safe cut point, tstzrange is [)).
    {
        let blocks: Vec<_> = (0..5i64)
            .map(|i| make_block(process_id, stream_id, i * 100, (i + 1) * 100, t0, 1))
            .collect();
        let parts = group_blocks_into_partitions(&config(2, BlockOrder::EventTime), blocks);
        assert_eq!(
            parts.iter().map(|p| p.blocks.len()).collect::<Vec<_>>(),
            vec![2, 2, 1]
        );
    }

    // Identical begin_ticks (tiebreak determinism): a stable sort must preserve the tied group's
    // input relative order, and running the same input twice must produce the same output.
    {
        let a = make_block(process_id, stream_id, 0, 100, t0 + TimeDelta::seconds(2), 1);
        let b = make_block(process_id, stream_id, 0, 100, t0 + TimeDelta::seconds(0), 1);
        let c = make_block(process_id, stream_id, 0, 100, t0 + TimeDelta::seconds(1), 1);
        let input = vec![b.clone(), a.clone(), c.clone()];

        let parts1 =
            group_blocks_into_partitions(&config(10, BlockOrder::EventTime), input.clone());
        let parts2 = group_blocks_into_partitions(&config(10, BlockOrder::EventTime), input);

        assert_eq!(parts1.len(), 1);
        let ids1: Vec<_> = parts1[0]
            .blocks
            .iter()
            .map(|blk| blk.block.block_id)
            .collect();
        let ids2: Vec<_> = parts2[0]
            .blocks
            .iter()
            .map(|blk| blk.block.block_id)
            .collect();
        assert_eq!(
            ids1,
            vec![b.block.block_id, a.block.block_id, c.block.block_id],
            "stable sort must preserve the tied group's input relative order"
        );
        assert_eq!(
            ids1, ids2,
            "grouping must be deterministic for the same input"
        );
    }
}
