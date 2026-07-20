use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use bytes::Bytes;
use foyer::{
    BlockEngineConfig, Code, DeviceBuilder, Event, EventListener, FsDeviceBuilder, HybridCache,
    HybridCacheBuilder, LruConfig, Source,
};
use micromegas_tracing::prelude::*;

use super::backend::{BackendDiskStats, FillHint, RangeCacheBackend};
use super::metric_tags::{
    EvictionTagTable, REASON_CLEAR, REASON_EVICT, REASON_REMOVE, REASON_REPLACE,
};

/// RAM-tier cache value carrying the timing needed for eviction/age
/// telemetry.
/// - `ram_inserted_at`: when the entry (re-)entered the RAM tier. Set on
///   `new()` and refreshed on `Code::decode` (a disk->RAM promotion is a
///   *new* RAM residency), so RAM age always measures time resident in RAM.
///   Not serialized.
/// - `disk_write_ms`: wall-clock ms (epoch) when the entry was written to
///   disk. Stamped by `Code::encode` (which records "now" -- under the
///   default `WriteOnEviction` policy, encode runs at disk-write time) and
///   preserved verbatim through disk reclaim (raw-byte reinsertion, no
///   re-encode). `DISK_WRITE_NONE` for a RAM-only entry that has never been
///   persisted.
/// - `is_prefetch`: true only for the ephemeral phantom record created by the
///   prefetch `put` arm. Not serialized (always `false` on decode). foyer
///   0.22.3 fires `on_leave` *twice* for that phantom record -- `Event::Remove`
///   synchronously during `insert`, then `Event::Evict` when the ephemeral
///   handle is dropped (the disk-write dispatch) -- both at age ~= 0 ms. The
///   listener uses this marker to exclude that noise from both signals.
#[derive(Clone)]
struct CachedBlock {
    bytes: Bytes,
    ram_inserted_at: Instant,
    disk_write_ms: i64,
    is_prefetch: bool,
}

const DISK_WRITE_NONE: i64 = i64::MIN;

impl CachedBlock {
    fn new(bytes: Bytes) -> Self {
        Self {
            bytes,
            ram_inserted_at: Instant::now(),
            disk_write_ms: DISK_WRITE_NONE,
            is_prefetch: false,
        }
    }

    /// Ephemeral disk-only phantom record for the prefetch path (see field
    /// doc on `is_prefetch`).
    fn new_prefetch(bytes: Bytes) -> Self {
        Self {
            is_prefetch: true,
            ..Self::new(bytes)
        }
    }
}

impl Code for CachedBlock {
    fn encode(&self, writer: &mut impl std::io::Write) -> foyer::Result<()> {
        // Stamp the disk-write instant here: encode == disk write under the
        // default WriteOnEviction hybrid policy. Leading i64 LE, then the
        // payload.
        let now_ms = chrono::Utc::now().timestamp_millis();
        now_ms.encode(writer)?;
        self.bytes.encode(writer)
    }

    fn decode(reader: &mut impl std::io::Read) -> foyer::Result<Self> {
        let disk_write_ms = i64::decode(reader)?;
        let bytes = Bytes::decode(reader)?;
        Ok(Self {
            bytes,
            ram_inserted_at: Instant::now(),
            disk_write_ms,
            is_prefetch: false,
        })
    }

    fn estimated_size(&self) -> usize {
        std::mem::size_of::<i64>() + self.bytes.estimated_size()
    }
}

/// On-disk format version for the foyer disk tier. The serialized value layout
/// (`CachedBlock`'s `Code` impl) carries no self-describing version, so a layout
/// change would otherwise misdecode entries recovered from a persisted store on
/// restart (see #1287, #1283). On startup the store directory is wiped iff the
/// persisted marker does not match this constant.
///
/// BUMP THIS whenever `CachedBlock`'s `Code` encode/decode (or any on-disk
/// layout foyer persists for us) changes.
///
/// History:
/// - v1: `CachedBlock` = `[i64 LE disk_write_ms][length-prefixed Bytes]` (#1283).
///   (The pre-#1283 `Bytes`-only layout was unversioned; upgrading onto a store
///   it wrote is the crash this guard prevents.)
pub const DISK_FORMAT_VERSION: u32 = 1;

/// Marker filename holding the decimal `DISK_FORMAT_VERSION`, stored alongside
/// foyer's own `foyer-storage-direct-fs-*` region files inside `--disk-path`.
/// The name does not collide with foyer's prefix, so foyer's recovery ignores it.
pub const DISK_FORMAT_MARKER: &str = "micromegas-object-cache-format-version";

/// Reuses a single fixed directory across restarts, wiping its contents in
/// place only when the persisted format marker does not match `version`. See
/// `DISK_FORMAT_VERSION` for why this exists.
fn prepare_disk_dir(dir: &str, version: u32) -> Result<()> {
    let dir_path = std::path::Path::new(dir);
    let marker = dir_path.join(DISK_FORMAT_MARKER);
    let current = std::fs::read_to_string(&marker)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    if current == Some(version) {
        return Ok(()); // match: let foyer recover the store untouched (warm reuse)
    }
    // Missing marker (first boot, or a pre-versioning old-format store) or a
    // mismatch: reclaim the space and start clean on the SAME directory.
    if dir_path.exists() {
        warn!(
            "object-cache disk format {current:?} != {version}; wiping {dir} to avoid \
             misdecoding old-format entries (#1287)"
        );
        imetric!("object_cache_disk_format_wiped", "count", 1_u64);
        // Remove directory CONTENTS, not the directory itself, so a mounted
        // volume root is preserved.
        for entry in
            std::fs::read_dir(dir_path).with_context(|| format!("reading disk dir {dir}"))?
        {
            let path = entry?.path();
            if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            }
            .with_context(|| format!("removing {}", path.display()))?;
        }
    } else {
        std::fs::create_dir_all(dir_path).with_context(|| format!("creating disk dir {dir}"))?;
    }
    std::fs::write(&marker, version.to_string())
        .with_context(|| format!("writing disk format marker {}", marker.display()))?;
    Ok(())
}

/// `reason` label for a RAM-tier `on_leave` event.
fn reason_str(reason: Event) -> &'static str {
    match reason {
        Event::Evict => REASON_EVICT,
        Event::Replace => REASON_REPLACE,
        Event::Remove => REASON_REMOVE,
        Event::Clear => REASON_CLEAR,
    }
}

/// RAM-tier eviction listener: emits `object_cache_ram_tier_eviction_count`
/// (all reasons) and `object_cache_ram_tier_eviction_age_ms`
/// (capacity-driven `Event::Evict` only -- the thrashing signal). Runs
/// synchronously inside foyer's insert path, possibly on a foyer-internal
/// thread; see the hot-path note on `dispatch`'s global metrics mutex in the
/// design doc for why this is safe from any thread.
struct RamEvictionListener {
    tags: Arc<EvictionTagTable>,
}

impl EventListener for RamEvictionListener {
    type Key = String;
    type Value = CachedBlock;

    fn on_leave(&self, reason: Event, key: &String, value: &CachedBlock) {
        if value.is_prefetch {
            // Phantom prefetch record: foyer fires Remove (synchronously
            // during insert) then Evict (when the ephemeral handle is
            // dropped, i.e. the disk-write dispatch) for the *same*
            // disk-only write, both at age ~= 0 ms -- indistinguishable from
            // real thrashing if counted. Exclude from both signals.
            return;
        }
        let t = self.tags.classify(key);
        imetric!(
            "object_cache_ram_tier_eviction_count",
            "count",
            t.count_for(reason_str(reason)),
            1_u64
        );
        if reason == Event::Evict {
            // Capacity-driven -- the thrashing signal. Replace/Remove/Clear
            // don't speak to capacity pressure.
            let age_ms = value.ram_inserted_at.elapsed().as_secs_f64() * 1000.0;
            fmetric!(
                "object_cache_ram_tier_eviction_age_ms",
                "ms",
                t.prefix,
                age_ms
            );
        }
    }
}

/// foyer disk-engine write-path tuning. Defaults reproduce foyer's own
/// `BlockEngineConfig` defaults (`flushers=1`, `buffer_pool_size=16 MiB`),
/// with the submit-queue threshold pinned to 2x the buffer pool -- which
/// foyer's doc comment describes as its intended default but which 0.22 no
/// longer applies automatically (its actual default is 1x, see
/// `BlockEngineConfig::new`) -- so existing callers/tests are unaffected
/// unless they opt into a different tuning.
#[derive(Clone, Copy, Debug)]
pub struct WriteTuning {
    /// `BlockEngineConfig::with_flushers`.
    pub flushers: usize,
    /// `BlockEngineConfig::with_buffer_pool_size`, in bytes.
    pub buffer_pool_bytes: usize,
    /// `BlockEngineConfig::with_submit_queue_size_threshold`, in bytes.
    pub submit_queue_threshold_bytes: usize,
}

impl Default for WriteTuning {
    fn default() -> Self {
        let buffer = 16 * 1024 * 1024;
        Self {
            flushers: 1,
            buffer_pool_bytes: buffer,
            submit_queue_threshold_bytes: buffer * 2,
        }
    }
}

pub struct FoyerBackend {
    cache: HybridCache<String, CachedBlock>,
    tags: Arc<EvictionTagTable>,
}

impl FoyerBackend {
    pub async fn new(dir: &str, ram_bytes: usize, disk_bytes: usize) -> Result<Self> {
        Self::new_with_shards(
            dir,
            ram_bytes,
            disk_bytes,
            8,
            WriteTuning::default(),
            Arc::from(Vec::new()),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn new_with_shards(
        dir: &str,
        ram_bytes: usize,
        disk_bytes: usize,
        shards: usize,
        tuning: WriteTuning,
        prefix_labels: Arc<[&'static str]>,
    ) -> Result<Self> {
        ensure!(shards > 0, "shards must be > 0");

        prepare_disk_dir(dir, DISK_FORMAT_VERSION)?;

        // Direct I/O (bypassing the page cache) matches the old `DirectFs`
        // engine's behavior; the flag only exists on Linux.
        #[cfg(target_os = "linux")]
        let device = FsDeviceBuilder::new(dir)
            .with_capacity(disk_bytes)
            .with_direct(true)
            .build()?;
        #[cfg(not(target_os = "linux"))]
        let device = FsDeviceBuilder::new(dir)
            .with_capacity(disk_bytes)
            .build()?;

        let tags = Arc::new(EvictionTagTable::new(prefix_labels));
        let listener = Arc::new(RamEvictionListener { tags: tags.clone() });

        let cache = HybridCacheBuilder::new()
            .with_event_listener(listener)
            .memory(ram_bytes)
            .with_weighter(|_key: &String, value: &CachedBlock| value.bytes.len())
            .with_shards(shards)
            // Pin the RAM tier to LRU explicitly: LRU is the crate's current
            // default eviction policy; pinning it here guards against a
            // future foyer default change silently altering RAM-tier
            // eviction behavior for demand fills.
            .with_eviction_config(LruConfig::default())
            .storage()
            .with_engine_config(
                BlockEngineConfig::new(device)
                    .with_flushers(tuning.flushers)
                    .with_buffer_pool_size(tuning.buffer_pool_bytes)
                    .with_submit_queue_size_threshold(tuning.submit_queue_threshold_bytes),
            )
            .build()
            .await?;
        Ok(Self { cache, tags })
    }

    pub async fn close(&self) -> Result<()> {
        self.cache.close().await?;
        Ok(())
    }

    /// Current RAM-tier byte usage. Exposed so integration tests (which
    /// compile as a separate crate and cannot reach the private `cache`
    /// field) can assert prefetch fills do not grow RAM-tier residency.
    pub fn ram_usage(&self) -> usize {
        self.cache.memory().usage()
    }
}

#[async_trait]
impl RangeCacheBackend for FoyerBackend {
    async fn get(&self, key: &str) -> Option<Bytes> {
        match self.cache.get(key).await {
            Ok(Some(entry)) => {
                if entry.source() == Source::Disk && entry.value().disk_write_ms != DISK_WRITE_NONE
                {
                    // Source::Disk fires exactly once per disk read (the
                    // now-promoted entry reports Source::Memory on a
                    // subsequent hit), so this never double-counts. See the
                    // disk-tier limitation note in the design doc: this
                    // per-read age is the observable disk-exit signal foyer
                    // 0.22 exposes, and its max/high-quantiles estimate the
                    // (unobservable) reclaim age.
                    let age_ms = (chrono::Utc::now().timestamp_millis()
                        - entry.value().disk_write_ms) as f64;
                    let t = self.tags.classify(key);
                    fmetric!(
                        "object_cache_disk_tier_read_age_ms",
                        "ms",
                        t.prefix,
                        age_ms.max(0.0)
                    );
                }
                Some(entry.value().bytes.clone())
            }
            Ok(None) => None,
            // A backend (disk/IO) error must not fail the read: treat it as a
            // miss so the caller falls back to origin, but surface it as a
            // metric + log so a degraded SSD volume is observable rather than
            // silently inflating origin traffic.
            Err(e) => {
                imetric!("range_cache_backend_error", "count", 1_u64);
                warn!("range_cache backend get error key={key}: {e}");
                None
            }
        }
    }

    async fn put(&self, key: String, value: Bytes, hint: FillHint) {
        match hint {
            // SSD-only admission: `.force()` bypasses the disk admission
            // picker so the block is always admitted deterministically (no
            // silent decline). The write holds only an ephemeral RAM record
            // that is dropped immediately (no eviction-structure residency),
            // so a prefetch fill never retains RAM residency.
            FillHint::Prefetch => {
                // Copy so the phantom prefetch record does not retain its whole
                // coalesced-GET parent buffer for the duration it lives in foyer's
                // write pipeline (submit queue, io buffer encode, pending piece_refs) --
                // see the demand arm's identical rationale below.
                let owned = Bytes::copy_from_slice(&value);
                let entry = self
                    .cache
                    .storage_writer(key)
                    .force()
                    .insert(CachedBlock::new_prefetch(owned));
                if entry.is_none() {
                    // Should not occur under `.force()`, which always admits.
                    imetric!(
                        "range_cache_prefetch_admission_unexpected_none",
                        "count",
                        1_u64
                    );
                    warn!("prefetch storage_writer().force().insert() unexpectedly returned None");
                }
            }
            FillHint::Demand => {
                // Copy so the cached block does not retain its whole coalesced-GET
                // parent buffer; otherwise RAM-tier RSS runs up to
                // (max_coalesced_get_bytes / block_size)x its accounted weight while the
                // weigher (value.len()) believes the tier is under budget. One memcpy per
                // admitted block is negligible against the origin GET.
                let owned = Bytes::copy_from_slice(&value);
                self.cache.insert(key, CachedBlock::new(owned));
            }
        }
    }

    fn disk_stats(&self) -> Option<BackendDiskStats> {
        let stats = self.cache.statistics();
        Some(BackendDiskStats {
            write_bytes: stats.disk_write_bytes() as u64,
            read_bytes: stats.disk_read_bytes() as u64,
            write_ios: stats.disk_write_ios() as u64,
            read_ios: stats.disk_read_ios() as u64,
        })
    }

    fn ram_usage_bytes(&self) -> Option<usize> {
        Some(self.ram_usage())
    }
}
