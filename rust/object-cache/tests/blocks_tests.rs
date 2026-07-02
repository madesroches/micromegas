use bytes::Bytes;
use micromegas_object_cache::blocks::{
    assemble_range, block_byte_range, blocks_for_range, coalesce_runs,
};

#[test]
fn single_block_range() {
    let block_size = 1024_u64;
    let blks = blocks_for_range(0, 512, block_size);
    assert_eq!(blks, 0..1);
}

#[test]
fn multi_block_range() {
    let block_size = 1024_u64;
    let blks = blocks_for_range(512, 2048, block_size);
    assert_eq!(blks, 0..2);
}

#[test]
fn boundary_spanning_range() {
    let block_size = 1024_u64;
    let blks = blocks_for_range(1023, 1025, block_size);
    assert_eq!(blks, 0..2);
}

#[test]
fn last_block_is_short() {
    let block_size = 1024_u64;
    let file_size = 1500_u64;
    let blk = block_byte_range(1, block_size, file_size);
    assert_eq!(blk, 1024..1500);
}

#[test]
fn assemble_full_range() {
    let block_size = 4_u64;
    let block0 = Bytes::from(vec![0u8, 1, 2, 3]);
    let block1 = Bytes::from(vec![4u8, 5, 6, 7]);
    let assembled = assemble_range(&[(0, block0), (1, block1)], block_size, 2, 6);
    assert_eq!(&assembled[..], &[2u8, 3, 4, 5]);
}

#[test]
fn assemble_empty_range() {
    let block_size = 4_u64;
    let result = assemble_range(&[], block_size, 0, 0);
    assert!(result.is_empty());
}

#[test]
fn assemble_partial_last_block() {
    let block_size = 1024_u64;
    let block1 = Bytes::from(vec![0xABu8; 476]);
    let assembled = assemble_range(&[(1, block1)], block_size, 1024, 1500);
    assert_eq!(assembled.len(), 476);
}

#[test]
fn blocks_for_range_exact_block_boundary() {
    let block_size = 1024_u64;
    let blks = blocks_for_range(0, 1024, block_size);
    assert_eq!(blks, 0..1);
}

#[test]
fn blocks_for_range_starts_at_boundary() {
    let block_size = 1024_u64;
    let blks = blocks_for_range(1024, 2048, block_size);
    assert_eq!(blks, 1..2);
}

#[test]
fn coalesce_contiguous_merge() {
    let runs = coalesce_runs(&[0, 1, 2, 3], 1024, 8192);
    assert_eq!(runs, vec![0..4]);
}

#[test]
fn coalesce_gap_split() {
    let runs = coalesce_runs(&[0, 1, 5, 6, 7], 1024, 8192);
    assert_eq!(runs, vec![0..2, 5..8]);
}

#[test]
fn coalesce_oversize_split() {
    // block_size=1024, max_coalesced_get_bytes=4096 => 4 blocks per run max.
    let runs = coalesce_runs(&[0, 1, 2, 3, 4, 5, 6], 1024, 4096);
    assert_eq!(runs, vec![0..4, 4..7]);
}

#[test]
fn coalesce_single_block() {
    let runs = coalesce_runs(&[42], 1024, 8192);
    assert_eq!(runs, vec![42..43]);
}

#[test]
fn coalesce_scattered_no_merge() {
    let runs = coalesce_runs(&[0, 2, 4, 6], 1024, 8192);
    assert_eq!(runs, vec![0..1, 2..3, 4..5, 6..7]);
}

#[test]
fn coalesce_empty_input() {
    let runs = coalesce_runs(&[], 1024, 8192);
    assert!(runs.is_empty());
}

#[test]
fn coalesce_exact_boundary_run_not_split() {
    // 4 contiguous blocks at exactly the max span should stay in one run.
    let runs = coalesce_runs(&[0, 1, 2, 3], 1024, 4096);
    assert_eq!(runs, vec![0..4]);
}
