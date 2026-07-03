use std::ops::Range;

use futures::stream::{self, StreamExt};
use micromegas_object_cache::blocks::blocks_for_range;
use micromegas_object_cache::prefetch::PrefetchItem;
use micromegas_object_cache::range_cache::RangeCache;
use micromegas_tracing::prelude::*;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;

/// Number of block indices per streamed window. Sized so a single window
/// already spans several coalesced runs and can saturate the scheduler's
/// `prefetch_permits` on its own, which is why `WINDOW_CONCURRENCY` can stay
/// low.
const WINDOW_BLOCKS: u64 = 64;

/// In-flight `prefetch_blocks` windows per item. Total in-flight fills across
/// the worker is `worker_concurrency * WINDOW_CONCURRENCY`, each bounded to
/// `WINDOW_BLOCKS` — independent of `item.size`.
const WINDOW_CONCURRENCY: usize = 1;

/// Lazily chunk a block-index range into `WINDOW_BLOCKS`-sized windows. Never
/// materializes the full range: representing `size == u64::MAX` costs
/// nothing until windows are actually pulled.
struct BlockWindows {
    remaining: Range<u64>,
}

impl Iterator for BlockWindows {
    type Item = Vec<u64>;

    fn next(&mut self) -> Option<Vec<u64>> {
        if self.remaining.is_empty() {
            return None;
        }
        let take = WINDOW_BLOCKS.min(self.remaining.end - self.remaining.start);
        let start = self.remaining.start;
        let end = start + take;
        self.remaining.start = end;
        Some((start..end).collect())
    }
}

/// Byte spans to warm for `item`: `[0, size)` for a whole-object warm (no or
/// empty `ranges`), or the caller-supplied ranges as-is. The handler already
/// validated each range against `item.size`.
// The single-element `vec![0..item.size]` is a `Vec<Range<u64>>`, not the
// `Vec<u64>` clippy's suggested `.collect()` fix would produce.
#[allow(clippy::single_range_in_vec_init)]
fn spans_for(item: &PrefetchItem) -> Vec<Range<u64>> {
    match &item.ranges {
        None => vec![0..item.size],
        Some(rs) if rs.is_empty() => vec![0..item.size],
        Some(rs) => rs.iter().map(|&[s, e]| s..e).collect(),
    }
}

/// Lazy iterator over the block-index windows covering `item`. Empty spans
/// (`size == 0`, or a degenerate `start >= end`) are skipped rather than
/// passed to `blocks_for_range`, which underflows on `end == 0` and trips its
/// `debug_assert!(start < end)` in test builds. No cross-window dedup is done
/// even when supplied ranges overlap: the scheduler's `own_or_join` and the
/// backend hit-path already dedup at the block level.
fn lazy_windows(item: &PrefetchItem, block_size: u64) -> impl Iterator<Item = Vec<u64>> {
    spans_for(item)
        .into_iter()
        .filter(|s| s.start < s.end)
        .flat_map(move |s| BlockWindows {
            remaining: blocks_for_range(s.start, s.end, block_size),
        })
}

/// Stream `item`'s block-index windows through `prefetch_blocks`, stopping at
/// the first error. This is the bound that replaces a per-item size cap: a
/// window fully past the real EOF (an over-claimed `size`) fails at the
/// origin, halting the stream within `WINDOW_CONCURRENCY` windows of the true
/// end — `buffered` only advances the lazy `windows` iterator as it needs the
/// next future, so on return the remaining windows are never generated.
async fn warm_item(cache: &RangeCache, item: PrefetchItem, block_size: u64) {
    let windows = lazy_windows(&item, block_size);
    let mut stream = stream::iter(windows)
        .map(|w| {
            let cache = cache.clone();
            let key = item.key.clone();
            let size = item.size;
            async move { cache.prefetch_blocks(&key, size, &w).await }
        })
        .buffered(WINDOW_CONCURRENCY);

    let mut warmed_any = false;
    while let Some(res) = stream.next().await {
        if let Err(e) = res {
            imetric!("object_cache_prefetch_fill_error", "count", 1_u64);
            debug!("prefetch fill failed key={}: {e:?}", item.key);
            return;
        }
        warmed_any = true;
    }
    // A no-op item (size == 0 or all-empty ranges) yields no windows; don't
    // count it as a warmed key.
    if warmed_any {
        imetric!("object_cache_prefetch_keys_warmed", "count", 1_u64);
    }
}

/// Spawn the bounded-queue prefetch consumer. The returned `Sender` is what
/// `AppState` holds; the caller drives the pipeline to completion by dropping
/// every clone of the sender and awaiting the returned `JoinHandle`, which
/// resolves only once the channel has closed and every in-flight `warm_item`
/// has finished (see the module docs on `prefetch_handler` for why this
/// matters to tests).
pub fn spawn_prefetch_worker(
    cache: RangeCache,
    queue_capacity: usize,
    worker_concurrency: usize,
) -> (mpsc::Sender<PrefetchItem>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<PrefetchItem>(queue_capacity);

    let handle = tokio::spawn(async move {
        let block_size = cache.block_size();
        ReceiverStream::new(rx)
            .for_each_concurrent(worker_concurrency, |item| {
                let cache = cache.clone();
                async move { warm_item(&cache, item, block_size).await }
            })
            .await;
    });

    (tx, handle)
}
