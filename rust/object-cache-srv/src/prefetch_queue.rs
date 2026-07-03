use std::ops::Range;
use std::sync::Arc;

use micromegas_object_cache::blocks::blocks_for_range;
use micromegas_object_cache::prefetch::PrefetchItem;
use micromegas_object_cache::range_cache::RangeCache;
use micromegas_tracing::prelude::*;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::{JoinHandle, JoinSet};

/// Block indices covering a whole-object warm `[0, size)`. `size == 0` must
/// return an empty set rather than reach `blocks_for_range`: that function
/// computes `(end - 1) / block_size`, which underflows for `end == 0` and
/// trips its `debug_assert!(start < end)` in test builds.
fn block_indices_for(size: u64, block_size: u64) -> Vec<u64> {
    if size == 0 {
        return Vec::new();
    }
    blocks_for_range(0, size, block_size).collect()
}

/// Deduplicated block indices covering `ranges`. A range with `start >= end`
/// is skipped rather than passed to `blocks_for_range`, for the same reason
/// as `block_indices_for`.
fn block_indices_for_ranges(ranges: &[Range<u64>], block_size: u64) -> Vec<u64> {
    let mut set = std::collections::BTreeSet::new();
    for r in ranges {
        if r.start < r.end {
            let blk = blocks_for_range(r.start, r.end, block_size);
            set.extend(blk.start..blk.end);
        }
    }
    set.into_iter().collect()
}

/// Spawn the bounded-queue prefetch consumer. The returned `Sender` is what
/// `AppState` holds; the caller drives the pipeline to completion by dropping
/// every clone of the sender and awaiting the returned `JoinHandle`, which
/// resolves only once every spawned fill has finished (see the module docs on
/// `prefetch_handler` for why this matters to tests).
pub fn spawn_prefetch_worker(
    cache: RangeCache,
    queue_capacity: usize,
    worker_concurrency: usize,
) -> (mpsc::Sender<PrefetchItem>, JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<PrefetchItem>(queue_capacity);
    let worker_sem = Arc::new(Semaphore::new(worker_concurrency));

    let handle = tokio::spawn(async move {
        let block_size = cache.block_size();
        // Tracks spawned fills so the consumer can observe completion
        // deterministically instead of firing-and-forgetting: a `JoinSet`
        // retains a completed task until it is joined, so it must be reaped
        // every iteration or it grows unboundedly over the server's lifetime
        // (the production channel never closes).
        let mut fills = JoinSet::new();
        while let Some(item) = rx.recv().await {
            while fills.try_join_next().is_some() {}

            let permit = worker_sem
                .clone()
                .acquire_owned()
                .await
                .expect("worker_sem is never closed");
            let cache = cache.clone();
            fills.spawn(async move {
                let _permit = permit;
                // None or empty ranges = whole object [0, size); present =
                // only the supplied ranges. The handler already validated
                // ranges against item.size, so mapping [s, e] -> s..e here is
                // safe.
                let indices = match &item.ranges {
                    None => block_indices_for(item.size, block_size),
                    Some(rs) if rs.is_empty() => block_indices_for(item.size, block_size),
                    Some(rs) => {
                        let ranges: Vec<Range<u64>> = rs.iter().map(|[s, e]| *s..*e).collect();
                        block_indices_for_ranges(&ranges, block_size)
                    }
                };
                match cache.prefetch_blocks(&item.key, item.size, &indices).await {
                    Ok(()) => {
                        imetric!("object_cache_prefetch_keys_warmed", "count", 1_u64);
                    }
                    Err(e) => {
                        imetric!("object_cache_prefetch_fill_error", "count", 1_u64);
                        debug!("prefetch fill failed key={}: {e:?}", item.key);
                    }
                }
            });
        }
        // Channel closed (every `Sender` dropped): drain every outstanding
        // fill before this handle resolves.
        while fills.join_next().await.is_some() {}
    });

    (tx, handle)
}
