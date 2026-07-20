use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use bytes::Bytes;
use futures::future::join_all;
use futures::stream::{self, FuturesUnordered, StreamExt};
use micromegas_tracing::prelude::*;
use micromegas_tracing::property_set::PropertySet;
use object_store::{ObjectStoreExt, path::Path};

use crate::backend::FillHint;
use crate::blocks::{block_byte_range, coalesce_runs};
use crate::metric_tags;

use super::RangeCache;
use super::scheduler::{
    BatchState, FulfillGuard, InFlight, Ownership, Priority, acquire_run_permit,
    effective_priority, reconstruct_shared_error,
};

/// Concurrency for probing the cache backend for block hits before going to
/// origin. Backend probes do real disk I/O on a foyer RAM-tier miss, so a
/// large read probed sequentially would serialize hundreds of disk reads.
const BACKEND_PROBE_CONCURRENCY: usize = 16;

/// Result of phase 1 (`probe_blocks`): the backend-hit blocks (`Demand` only)
/// and the sorted, deduplicated set of blocks that must be fetched from
/// origin.
struct ProbeOutcome {
    hits: HashMap<u64, Bytes>,
    missing: Vec<u64>,
}

/// Result of phase 2 (`register_missing`): the single-flight registration of
/// every missing block.
struct RegisteredBlocks {
    /// The missing blocks this call owns (is responsible for fetching from
    /// origin); a sorted subsequence of the `missing` slice passed in.
    owned: Vec<u64>,
    /// The in-flight entry for every missing block, owned or joined, keyed by
    /// block index.
    entries: HashMap<u64, Arc<InFlight>>,
    /// The batch handle for a prefetch call (`None` on the demand path); lets a
    /// demand joiner promote every sibling of a scattered prefetch call.
    batch: Option<Arc<BatchState>>,
}

/// One coalesced run of contiguous missing blocks filled by a single origin
/// GET. Bundles the block-index `range` with the in-flight `entries` and cache
/// `keys` for those blocks; `entries[i]` and `keys[i]` both correspond to block
/// index `range.start + i`, an alignment the type keeps together rather than
/// leaving to three parallel `Vec`s.
struct CoalescedRun {
    range: Range<u64>,
    entries: Vec<Arc<InFlight>>,
    keys: Vec<String>,
}

impl RangeCache {
    /// Fetch `indices` (block indices within `key`, sized `file_size`) at the
    /// given priority, returning the bytes for every requested block on the
    /// `Demand` path. On the `Prefetch` path bytes are written to the backend
    /// and dropped as each owned run completes; the returned map is always
    /// empty, which is what keeps the prefetch peak bounded by
    /// `prefetch_concurrency * max_coalesced_get_bytes` rather than the full
    /// request size.
    pub(super) async fn fetch_blocks(
        &self,
        key: &str,
        file_size: u64,
        indices: &[u64],
        prio: Priority,
    ) -> Result<HashMap<u64, Bytes>> {
        if indices.is_empty() {
            return Ok(HashMap::new());
        }

        // Classify once per call (a longest-prefix string match), not once
        // per block probe: `prefix_tags` is reused across every probe and
        // every coalesced run below.
        let prefix_tags = self.classify_tags(key);
        let block_tag = prefix_tags.prefix;
        let (run_demand_tag, run_prefetch_tag) =
            (prefix_tags.prefix_demand, prefix_tags.prefix_prefetch);

        let ProbeOutcome { hits, missing } = self
            .probe_blocks(key, file_size, indices, prio, block_tag)
            .await;
        if missing.is_empty() {
            return Ok(hits);
        }

        let RegisteredBlocks {
            owned,
            entries,
            batch: _batch,
        } = self.register_missing(key, &missing, prio);

        let hint = match prio {
            Priority::Demand => FillHint::Demand,
            Priority::Prefetch => FillHint::Prefetch,
        };

        // `owned` is a sorted subsequence of `missing` (itself sorted), so no
        // extra sort is needed before coalescing.
        for range in coalesce_runs(&owned, self.block_size, self.max_coalesced_get_bytes) {
            let run_entries: Vec<Arc<InFlight>> = (range.start..range.end)
                .map(|idx| entries.get(&idx).expect("owned entry present").clone())
                .collect();
            let run_keys: Vec<String> = (range.start..range.end)
                .map(|idx| format!("blk:{}:{key}:{idx}", self.ns))
                .collect();
            let run = CoalescedRun {
                range,
                entries: run_entries,
                keys: run_keys,
            };
            self.spawn_run_fetch(key, file_size, run, hint, run_demand_tag, run_prefetch_tag);
        }

        if prio == Priority::Prefetch {
            join_prefetch(entries).await?;
            return Ok(HashMap::new());
        }

        join_demand(entries, &missing, hits).await
    }

    /// Phase 1: probe the backend for every requested block with bounded
    /// concurrency (a foyer RAM-tier miss falls through to async disk I/O,
    /// and probing a large read one block at a time would serialize those
    /// disk reads), healing a length-mismatched cached block by treating it
    /// as missing, and partitioning into hits (`Demand` only) and missing
    /// (sorted, deduplicated).
    async fn probe_blocks(
        &self,
        key: &str,
        file_size: u64,
        indices: &[u64],
        prio: Priority,
        block_tag: &'static PropertySet,
    ) -> ProbeOutcome {
        let mut hits: HashMap<u64, Bytes> = HashMap::new();
        let mut missing: Vec<u64> = Vec::new();
        {
            // The futures are collected eagerly (not mapped lazily inside the
            // stream) so the resulting future stays `Send`-inferable across
            // `tokio::spawn` (rustc's HRTB limitation with borrowed closures).
            let probe_futs: Vec<_> = indices
                .iter()
                .map(|&idx| {
                    let block_key = format!("blk:{}:{key}:{idx}", self.ns);
                    let backend = self.backend.clone();
                    // Computed eagerly (ahead of the `backend.get` call) so it
                    // can be passed in as the promotion-gate contract: a
                    // backend must not copy a length-mismatched value into a
                    // faster tier. See `RangeCacheBackend::get`'s doc.
                    let expected_range = block_byte_range(idx, self.block_size, file_size);
                    let expected_len = expected_range.end - expected_range.start;
                    async move {
                        imetric!("range_cache_block_request", "count", block_tag, 1_u64);
                        let probe = instrument_named!(
                            backend.get(&block_key, expected_len),
                            "range_cache_backend_read"
                        )
                        .await;
                        (idx, probe, expected_len)
                    }
                })
                .collect();
            let mut probes = stream::iter(probe_futs).buffer_unordered(BACKEND_PROBE_CONCURRENCY);
            while let Some((idx, probe, expected_len)) = probes.next().await {
                match probe {
                    Some(data) => {
                        // A hit's length must match the block's true byte span.
                        // A mismatch means a prior prefetch stored this block
                        // under an undersized caller-supplied `size` (or the
                        // origin object changed size): treat it as a miss so
                        // it gets refetched and overwritten with the correct
                        // bytes, healing the poisoned entry. Defense in depth:
                        // the backend itself must already refuse to promote a
                        // mismatched value (the `expected_len` passed above),
                        // so this should never trigger in practice.
                        if data.len() as u64 != expected_len {
                            imetric!("range_cache_block_len_mismatch", "count", 1_u64);
                            warn!(
                                "range_cache block length mismatch key={key} idx={idx} \
                                 expected={expected_len} got={}",
                                data.len()
                            );
                            missing.push(idx);
                            continue;
                        }
                        imetric!("range_cache_block_backend_hit", "count", block_tag, 1_u64);
                        // A prefetch has nothing to do with an already-cached
                        // block, so drop the bytes immediately instead of
                        // accumulating every hit for the whole call.
                        if prio == Priority::Demand {
                            hits.insert(idx, data);
                        }
                    }
                    None => missing.push(idx),
                }
            }
        }
        missing.sort_unstable();
        missing.dedup();
        ProbeOutcome { hits, missing }
    }

    /// Phase 2: register every missing block in the single-flight scheduler,
    /// splitting into `owned` (this call is responsible for fetching it from
    /// origin) and joined (some other call already owns it). `missing` is
    /// taken by reference so `fetch_blocks` retains ownership of the `Vec`
    /// for the `join_demand` call in phase 4.
    fn register_missing(&self, key: &str, missing: &[u64], prio: Priority) -> RegisteredBlocks {
        // Only prefetch calls carry a batch handle: it's what lets a demand
        // joiner promote every sibling of a scattered prefetch call, not
        // just the one block it happened to touch (see `promote_whole_batch`).
        let batch = if prio == Priority::Prefetch {
            Some(Arc::new(BatchState::new(missing.len())))
        } else {
            None
        };

        let mut owned: Vec<u64> = Vec::new();
        let mut entries: HashMap<u64, Arc<InFlight>> = HashMap::with_capacity(missing.len());
        for &idx in missing {
            let block_key = format!("blk:{}:{key}:{idx}", self.ns);
            match self.scheduler.own_or_join(block_key, prio, batch.clone()) {
                Ownership::Owner(entry) => {
                    owned.push(idx);
                    entries.insert(idx, entry);
                }
                Ownership::Joiner(entry) => {
                    entries.insert(idx, entry);
                }
            }
        }
        // Every member of this batch (owned or joined) was recorded as a
        // promotion sibling inside `own_or_join`, under the inflight lock,
        // so no entry became joinable before the batch list held it.
        RegisteredBlocks {
            owned,
            entries,
            batch,
        }
    }

    /// Phase 3: spawn the detached task that owns one coalesced run of
    /// `run_entries`/`run_keys`: acquire a run permit, issue one
    /// `origin.get_range`, validate the run length, split the buffer into
    /// per-block chunks, write each to the backend, fulfill each entry, and
    /// remove the run's keys from the scheduler. Spawned as a detached task
    /// so a cancelled caller (e.g. an axum request future dropped on client
    /// disconnect) can never strand the other owned blocks or any joiners
    /// waiting on this run. `FulfillGuard` covers the panic path.
    fn spawn_run_fetch(
        &self,
        key: &str,
        file_size: u64,
        run: CoalescedRun,
        hint: FillHint,
        run_demand_tag: &'static PropertySet,
        run_prefetch_tag: &'static PropertySet,
    ) {
        // Capture a `RangeCache` clone (cheap: all fields are `Arc`s or `Copy`)
        // rather than cloning each field the task needs individually, mirroring
        // the pattern in `stream_ranges_inner`. It also lets the success tail be
        // a `RangeCache` method that reads `block_size`/`backend` from `self`.
        let cache = self.clone();
        let block_size = self.block_size;
        let key_owned = key.to_string();

        tokio::spawn(async move {
            let guard = FulfillGuard::new(
                cache.scheduler.clone(),
                run.keys
                    .iter()
                    .cloned()
                    .zip(run.entries.iter().cloned())
                    .collect(),
            );
            let permit_wait_start = Instant::now();
            let permit = instrument_named!(
                acquire_run_permit(&cache.scheduler, &run.entries),
                "range_cache_fetch_permit_wait"
            )
            .await;
            // The run's effective priority, resolved once the permit is
            // acquired so a promotion racing the wait is reflected: used
            // to tag every latency/hit-rate signal this run emits.
            let class = effective_priority(&run.entries).class_label();
            let run_class_tag = if class == metric_tags::CLASS_DEMAND {
                run_demand_tag
            } else {
                run_prefetch_tag
            };
            fmetric!(
                "range_cache_fetch_permit_wait_ms",
                "ms",
                metric_tags::class_tags(class),
                permit_wait_start.elapsed().as_secs_f64() * 1000.0
            );

            let byte_start = run.range.start * block_size;
            let byte_end = block_byte_range(run.range.end - 1, block_size, file_size).end;
            let path = Path::from(key_owned.as_str());
            let origin_get_start = Instant::now();
            let outcome = instrument_named!(
                cache.origin.get_range(&path, byte_start..byte_end),
                "range_cache_origin_get"
            )
            .await;
            fmetric!(
                "range_cache_origin_get_ms",
                "ms",
                metric_tags::class_tags(class),
                origin_get_start.elapsed().as_secs_f64() * 1000.0
            );
            drop(permit);

            match outcome {
                Ok(data) => {
                    cache
                        .fulfill_run_success(
                            &run,
                            data,
                            byte_start..byte_end,
                            &key_owned,
                            run_class_tag,
                            hint,
                        )
                        .await;
                }
                Err(e) => {
                    let shared: Arc<anyhow::Error> = Arc::new(anyhow::Error::from(e));
                    for entry in &run.entries {
                        entry.fulfill(Err(shared.clone()));
                    }
                }
            }
            for k in &run.keys {
                cache.scheduler.remove_entry(k);
            }
            guard.disarm();
        });
    }

    /// Success-path tail of `spawn_run_fetch`'s per-run task: validate the
    /// origin GET's length against the run's expected byte span (a short read
    /// is an explicit error, never silently clipped — see the inline comment
    /// below), then split the buffer into per-block chunks, writing each to the
    /// backend and fulfilling its entry.
    async fn fulfill_run_success(
        &self,
        run: &CoalescedRun,
        data: Bytes,
        byte_range: Range<u64>,
        key_owned: &str,
        run_class_tag: &'static PropertySet,
        hint: FillHint,
    ) {
        let block_size = self.block_size;
        let range = &run.range;
        let run_len = (range.end - range.start) as usize;
        // The backend-hit path above heals a length mismatch (an undersized
        // cached entry, or the origin object having changed size) by treating
        // it as a miss and refetching. A true origin fetch has nowhere further
        // to fall back to, so a short read here — which `object_store`'s
        // `GetRange::Bounded` returns without error when the object is shorter
        // than requested — must surface as an explicit error instead of being
        // silently clipped by `assemble_range`, which would otherwise
        // under-yield bytes and either corrupt a `Content-Length`-declared
        // response or trip `frame_ranges_stream`'s under-yield `.expect()`.
        let expected_run_bytes = byte_range.end - byte_range.start;
        if data.len() as u64 != expected_run_bytes {
            imetric!("range_cache_origin_run_len_mismatch", "count", 1_u64);
            warn!(
                "range_cache origin fetch length mismatch key={key_owned} \
                 run={range:?} expected={expected_run_bytes} got={}",
                data.len()
            );
            let shared: Arc<anyhow::Error> = Arc::new(anyhow!(
                "origin object changed size mid-fetch: key={key_owned} \
                 run={range:?} expected {expected_run_bytes} bytes, got {} bytes",
                data.len()
            ));
            for entry in &run.entries {
                entry.fulfill(Err(shared.clone()));
            }
        } else {
            imetric!(
                "range_cache_origin_block_fetch",
                "count",
                run_class_tag,
                run_len as u64
            );
            imetric!(
                "range_cache_origin_block_bytes",
                "bytes",
                run_class_tag,
                data.len() as u64
            );
            debug!(
                "range_cache origin fetch key={key_owned} run={range:?} bytes={}",
                data.len()
            );
            for i in 0..run_len {
                let offset = i as u64 * block_size;
                let local_start = offset as usize;
                let local_end = (offset + block_size).min(data.len() as u64) as usize;
                let chunk = data.slice(local_start..local_end);
                self.backend
                    .put(run.keys[i].clone(), chunk.clone(), hint)
                    .await;
                run.entries[i].fulfill(Ok(chunk));
            }
        }
    }
}

/// Phase 4, demand path: join every entry in index order, returning the
/// populated hits map (backend hits from phase 1 plus the freshly-fetched
/// missing blocks).
async fn join_demand(
    entries: HashMap<u64, Arc<InFlight>>,
    missing: &[u64],
    mut hits: HashMap<u64, Bytes>,
) -> Result<HashMap<u64, Bytes>> {
    let joined = join_all(missing.iter().map(|idx| {
        let entry = entries
            .get(idx)
            .expect("entry present for every missing index")
            .clone();
        let idx = *idx;
        async move { (idx, entry.join().await) }
    }))
    .await;

    for (idx, r) in joined {
        let data = r.map_err(|e| reconstruct_shared_error(&e))?;
        hits.insert(idx, data);
    }
    Ok(hits)
}

/// Phase 4, prefetch path: join entries as each completes (not in index
/// order), dropping the joined bytes and the `Arc<InFlight>` right away. The
/// watch channel inside an entry retains the fulfilled `Bytes` — a slice
/// sharing its whole run's GET buffer — for as long as the entry is alive,
/// so holding every entry until all runs finished (as the demand path's
/// `join_all` does) would let the prefetch peak reach the full request size
/// instead of the documented `prefetch_concurrency * max_coalesced_get_bytes`
/// bound. Any demand joiner that registered before completion still gets the
/// bytes: it holds its own `Arc<InFlight>` and reads the channel
/// independently of this early drop.
async fn join_prefetch(entries: HashMap<u64, Arc<InFlight>>) -> Result<()> {
    let mut joins: FuturesUnordered<_> = entries
        .into_values()
        .map(|entry| async move { entry.join().await.map(|_bytes| ()) })
        .collect();
    while let Some(r) = joins.next().await {
        r.map_err(|e| reconstruct_shared_error(&e))?;
    }
    Ok(())
}
