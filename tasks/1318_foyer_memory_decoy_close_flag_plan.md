# foyer-memory Decoy Close-Flag — Analysis & Decision

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1318

> **Status (2026-07-20): decision made — no code change (option 3).**
> The race is real in `foyer-memory` 0.22.3 but has **zero correctness impact** on
> object-cache (every consumer validates what it reads; all cached values are immutable
> per key). Every complete fix requires patching foyer itself (fork or vendor), which is
> disproportionate for what is at worst a rare, self-healing extra origin refetch.
> Remaining action: file the upstream bug (fix diff below), then bump foyer when a
> release containing the fix ships.

## The bug (root cause)

`foyer-memory` 0.22.3 has a bug in `InflightManager::enqueue`'s `Entry::Vacant` arm
(`src/inflight.rs`, ~lines 206-228): the fetch leader is handed a *different*
`Arc<AtomicBool>` "close" flag from the one stored in the inflight table.

```rust
Entry::Vacant(v) => {
    ...
    let entry = InflightEntry {
        ...
        inflight: Inflight {
            close: Arc::new(AtomicBool::new(false)),   // Arc A -- stored in the table
            ...
        },
    };
    v.insert(entry);
    let close = Arc::new(AtomicBool::new(false));       // Arc B -- a DIFFERENT Arc
    Enqueue::Lead { id, close, .. }                     // leader gets Arc B
}
```

Callers that cancel an inflight fetch (`take`, `fetch_or_take`) set the **table's** copy
(Arc A) via `inflight.close.store(true, ...)`; the leader's `RawFetch::poll` checks its
**own** copy (Arc B), which nothing ever writes. Cancellation is therefore inert.

The one-line upstream fix is to share a single `Arc`:

```rust
Entry::Vacant(v) => {
    let close = Arc::new(AtomicBool::new(false));
    let entry = InflightEntry { inflight: Inflight { close: close.clone(), .. }, .. };
    v.insert(entry);
    Enqueue::Lead { id, close, .. }
}
```

**Upstream status (verified 2026-07-20):** 0.22.3 (2026-01-23) is the newest release on
crates.io, and `foyer-rs/foyer` `main` still contains the identical two-Arc bug
(`foyer-memory/src/inflight.rs`: table Arc at ~L215, leader Arc at ~L221). No upstream
fix exists yet anywhere.

## How it reaches object-cache

`FoyerBackend` (`rust/object-cache/src/foyer_backend.rs`) drives a `HybridCache`:

- **Reads** — `get()` (line 318) calls `self.cache.get(key).await`. `HybridCache::get`
  routes through `memory.get_or_fetch_inner` → `InflightManager::enqueue`, i.e. **the
  inflight-coalescing path**. On a RAM miss this starts a disk-tier fetch (the "leader")
  and, on a disk hit, **promotes** the value into the RAM tier. This is the *only* call in
  object-cache that creates an inflight leader.
- **Demand writes** — `put(Demand)` (line 390) calls `self.cache.insert(...)`, which goes
  `raw.rs::emplace` → `InflightManager::take(hash, key, None)`. `emplace` (foyer-memory
  `src/raw.rs:141-150`) *always* calls `take`, which sets the table's close flag (Arc A)
  to cancel any in-flight fetch of that key.
- **Prefetch writes** — `put(Prefetch)` (line 372) uses `storage_writer().force().insert(...)`,
  which writes **disk-only** and does not touch the inflight path.

### The clobber

When a demand `insert()` (fresh bytes) races an in-flight disk read of the same key, the
`insert` sets Arc A to cancel the read; the leader checks Arc B, never sees it, finishes the
disk read, and **re-inserts the disk value over the fresh insert**.

### Verified severity: no correctness impact, ever

All values object-cache stores are immutable per key, and **both** consumers of
`backend.get` validate what they read before using it:

- **Blocks** (`blk:{ns}:{key}:{idx}`) — chunks of write-once, content-addressed object-store
  objects. For a given key the bytes never change, so in almost every case a clobbered
  value is **byte-identical** to what it replaced: wasted work, not corruption.
  The one differing-bytes case is the **poisoned-short-prefetch heal**: a prefetch stored a
  block under an undersized caller-supplied `size`; `probe_blocks`
  (`range_cache/fetch.rs:176`) detects the length mismatch, treats it as a miss, and
  refetches. A concurrent disk read racing the heal `put(Demand)` can re-poison the entry —
  but the length check runs on **every** hit, so a short block is *never served to a
  reader*; the next probe just heals it again. Worst case: one extra origin refetch.
- **Size metadata** (`meta:{ns}:{key}`) — `size()` (`range_cache/mod.rs:204`) only accepts
  a hit that is exactly 8 bytes and decodes to a plausible size; anything else falls
  through to an origin HEAD that repopulates the entry. Sizes are immutable per key
  (write-once keys), so clobbers here are always byte-identical anyway.

Net effect of the bug in this codebase: **a rare, transient, self-healing perf blip**
(extra origin refetch under a race that itself requires an already-rare short-poisoned
block). The `range_cache_block_len_mismatch` metric is the watchdog — sustained counts
would mean the short-block source needs investigating, independent of this race.

## Why no complete call-site fix exists

Two classes of workaround were evaluated; both are structurally incomplete:

1. **Two-step read** (`memory().get()` then `storage().load()`, bypassing the inflight
   path). Complete only if the read path never writes the RAM tier — but disk→RAM
   promotion on demand reads is an architectural requirement (prefetches are disk-only;
   demand access must promote them so repeat reads are fast). Adding promotion back
   (`memory().insert()` on disk hit) merely relocates the identical clobber into our code.
2. **Per-key serialization** (keyed async lock in `FoyerBackend` making `get` and
   `put(Demand)` mutually exclusive per key). **Ruled out by foyer's execution model**:
   the fetch leader is a *detached spawned task* (`spawner.spawn(fetch)`, foyer-memory
   `src/raw.rs:1034`), not polled inline by the `get()` future. If a query is cancelled
   and drops its `get` future, the lock guard drops but the zombie leader keeps running
   and can reinsert stale bytes *after* any lock scope we control. The close flag is
   precisely the mechanism for cancelling that detached task — and it is the thing that
   is broken. No lock we hold can bound the leader's lifetime.

Likewise, `remove`-on-length-mismatch hardening (via `HybridCache::remove`, which exists
at `foyer/src/hybrid/cache.rs:575`) shrinks but cannot close the window: a zombie leader
that already holds the bytes reinserts them after the remove. The only complete fixes are
patching foyer (share the Arc) — via fork+`[patch.crates-io]` git or an in-repo vendored
tree.

## Decision (option 3): accept the residual race; no code change

Rationale:

- **No correctness exposure** — verified above; no path can serve clobbered bytes.
- **Fork (+git patch)** would break the repo's deliberate supply-chain posture:
  `rust/deny.toml` `[sources]` denies git deps ("Verified: the tree has no git deps"),
  and the fork would need maintenance until upstream releases.
- **Vendor (+path patch)** carries a third-party tree plus fmt/clippy/machete/CI
  carve-outs — the cost this branch was opened to avoid.
- **`remove` hardening is skipped**: it adds trait surface (`RangeCacheBackend::remove`)
  on every backend to narrow a window that stays open regardless, in a race whose impact
  is already a self-healing perf blip. Not worth the complexity; reconsider only if
  `range_cache_block_len_mismatch` shows sustained counts in production.

Follow-ups:

1. **File the upstream bug** against `foyer-rs/foyer` — the issue body in #1318 already
   contains the fix diff, ready to paste. (Verified still unfixed on `main` as of
   2026-07-20.)
2. **Bump foyer** when a release containing the fix ships; that deletes the problem with
   a `cargo update`. Note the fix on the micromegas issue so the bump closes it.

## Key references

- Bug: `~/.cargo/registry/src/*/foyer-memory-0.22.3/src/inflight.rs`, `InflightManager::enqueue`,
  `Entry::Vacant` arm (~L206-228). `take`/`fetch_or_take` set the table's Arc; `RawFetch::poll`
  checks the leader's Arc (raw.rs ~L1299, L1316).
- Detached leader spawn: `foyer-memory .../src/raw.rs:1034` (`spawner.spawn(fetch)`).
- Always-called cancellation: `foyer-memory .../src/raw.rs:141-150` (`emplace` → `take`).
- object-cache read/write paths: `rust/object-cache/src/foyer_backend.rs` — `get()` L318,
  `put(Demand)` L390, `put(Prefetch)` L362-382.
- Length check / heal: `rust/object-cache/src/range_cache/fetch.rs:167-201` (`probe_blocks`).
- Size metadata guard: `rust/object-cache/src/range_cache/mod.rs:197-215` (`size()`).
- object-cache's own origin single-flight (coalesces origin GETs, **not** disk reads):
  `rust/object-cache/src/range_cache/scheduler.rs` (`FetchScheduler::own_or_join`).
- Dependency wiring: `rust/Cargo.toml` has `foyer = "0.22"` / `foyer-memory = "0.22"`, no
  `[patch]`; `Cargo.lock` resolves the foyer family at 0.22.3 from crates.io.
  `rust/deny.toml` `[sources]` allows only crates.io (git deps denied).

## Rejected alternatives (kept for the record)

1. **Git fork + `[patch.crates-io]`** — complete fix, no vendored tree, but requires an
   external fork to create and maintain, a `deny.toml` `[sources]` policy change to allow
   git deps, and a git rev pinned in `Cargo.lock` until upstream releases.
2. **Vendor + local patch** — complete and self-contained (`rust/vendor/foyer-memory-0.22.3/`
   + `[patch.crates-io]` path override + workspace exclusion + machete ignore + dedicated
   CI step, mirroring the `datafusion-wasm` precedent), but carries a vendored third-party
   tree.
3. **Two-step read / keyed per-key lock** — structurally incomplete; see "Why no complete
   call-site fix exists".
