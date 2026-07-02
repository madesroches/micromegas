use bytes::Bytes;
use std::ops::Range;

pub fn blocks_for_range(start: u64, end: u64, block_size: u64) -> Range<u64> {
    debug_assert!(start < end);
    let first = start / block_size;
    let last = (end - 1) / block_size;
    first..last + 1
}

pub fn block_byte_range(block_idx: u64, block_size: u64, file_size: u64) -> Range<u64> {
    let start = block_idx * block_size;
    let end = (start + block_size).min(file_size);
    start..end
}

/// Group sorted, deduplicated, *owned* missing block indices into maximal
/// contiguous runs, splitting any run whose byte span would exceed
/// `max_coalesced_get_bytes` at block boundaries. Each returned block-index
/// range becomes one `origin.get_range` call.
pub fn coalesce_runs(
    sorted_missing_owned: &[u64],
    block_size: u64,
    max_coalesced_get_bytes: u64,
) -> Vec<Range<u64>> {
    let max_blocks_per_run = (max_coalesced_get_bytes / block_size).max(1);
    let mut runs = Vec::new();
    let mut i = 0;
    while i < sorted_missing_owned.len() {
        let mut j = i;
        while j + 1 < sorted_missing_owned.len()
            && sorted_missing_owned[j + 1] == sorted_missing_owned[j] + 1
            && sorted_missing_owned[j + 1] - sorted_missing_owned[i] < max_blocks_per_run
        {
            j += 1;
        }
        runs.push(sorted_missing_owned[i]..sorted_missing_owned[j] + 1);
        i = j + 1;
    }
    runs
}

pub fn assemble_range(
    blocks: &[(u64, Bytes)],
    block_size: u64,
    req_start: u64,
    req_end: u64,
) -> Bytes {
    if req_start >= req_end || blocks.is_empty() {
        return Bytes::new();
    }
    let mut result = Vec::with_capacity((req_end - req_start) as usize);
    for (block_idx, block_data) in blocks {
        let blk_start = block_idx * block_size;
        let blk_end = blk_start + block_data.len() as u64;
        let clip_start = req_start.max(blk_start);
        let clip_end = req_end.min(blk_end);
        if clip_start < clip_end {
            let local_start = (clip_start - blk_start) as usize;
            let local_end = (clip_end - blk_start) as usize;
            result.extend_from_slice(&block_data[local_start..local_end]);
        }
    }
    Bytes::from(result)
}
