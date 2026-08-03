# Rollback & containment plan

**Parent plan**: [plan.md](./plan.md)
**Addresses the gap**: the parent plan is all forward-fix. This change alters the *correctness
foundation* of block ingestion (dedup moves from a Postgres unique index to an object-store
conditional PUT + probe) and does a hard-to-reverse in-place cutover of the metadata store. Before
shipping, there must be a written answer to "it's wrong in production — now what."

The organizing principle: **the blob is the durable source of truth.** The plan's hard rule "the
insert path never deletes blobs; only retention deletes blobs" is not just a race fix — it is what
makes every scenario below *recoverable*, because a blob without a row can always be replayed into a
row, and blobs are create-once so they are never silently mutated. Preserve that rule at all costs.

## Rollback floor (read first)

Once the operator cutover has run (schema at v5, `blocks` partitioned):

- Binaries may be rolled back **only to Deploy-1 code** (relaxed `>=` asserts, schema-agnostic
  no-target `ON CONFLICT`). Deploy-1 code runs correctly against both v4 and v5 schemas.
- Rolling back **past** Deploy 1 (to `==` asserts + targeted `ON CONFLICT (block_id)`) against a v5
  partitioned DB **crashes at startup and fails every insert**. Do not do it. If you think you need
  to, you actually need the full un-partition procedure (§4).

Deploy 1 is therefore the safe rollback anchor for the entire rollout window. Keep it deployable.

## Scenario 1 — dedup bug: duplicate `blocks` rows

**Symptom**: `BlocksView` double-counts; recovery-arm "recovered-insert" counter climbs; spot query
`SELECT block_id, count(*) FROM blocks GROUP BY 1 HAVING count(*) > 1` returns rows.

**Why it's bounded**: the per-partition local `UNIQUE (block_id)` guard means duplicates can only
exist **across** partitions (boundary-straddle or late-arrival), never within one. So detection can
focus on adjacent-partition pairs and recent hours rather than a full-table group-by.

**Contain**:
1. Confirm scope with the group-by scoped to recent partitions (cheap; plan-time pruned).
2. If duplicates are actively being created (not a one-off historical straddle), the advisory-lock
   serialization or the probe bound is broken. **Freeze the blast radius**: the fastest safe lever is
   to widen the hot-path probe's lower bound (e.g. `-1h` → `-25h`, matching stage 1) so more
   partitions are consulted before an insert — trades hot-path cost for a wider dedup net while the
   real bug is fixed. This is a config/const change, redeployable as a Deploy-1-compatible patch.
3. **Repair the data**: dedupe the affected rows with a one-time targeted DELETE keeping the earliest
   `insert_time` per `block_id`:
   ```sql
   DELETE FROM blocks a USING blocks b
    WHERE a.block_id = b.block_id
      AND a.insert_time > b.insert_time
      AND a.insert_time >= '<affected_window_start>'::timestamptz;  -- keep it partition-pruned
   ```
   This reintroduces row-DELETE churn, but as a **bounded one-time cleanup**, not the recurring churn
   the plan eliminates — acceptable. Re-materialize the affected `BlocksView` window afterward.

**No un-partitioning required** for this scenario.

## Scenario 2 — dedup bug: dropped blocks (recovery arm wrongly skips)

**Symptom**: data loss — a block that was ingested has no `blocks` row (the recovery arm's probe
found a false positive and skipped the INSERT). Harder to notice than duplicates; this is the more
dangerous failure and the reason the probe's correctness caveats (lower-bound-only, cache-bypassing
HEAD) matter.

**Detect**: because blobs are create-once and never deleted by the insert path, **a blob with no
matching row is a dropped block.** Run a reconciliation sweep: enumerate object-store keys under
`blobs/` for a window and left-join against `blocks` on `(process_id, stream_id, block_id)`; any blob
without a row within the retention window is a candidate loss. This sweep is the standing safety net
and should be run (sampled) as monitoring, not only during an incident.

**Recover**: re-insert the missing rows directly from blob metadata + the payload (the blob carries
everything needed; `insert_time` is stamped fresh on recovery, consistent with Decision 2). This is
exactly the recovery arm's intended job, so the fix is usually "correct the probe bound, then let a
re-ingest / a reconciliation job replay the orphaned blobs."

**Root-cause checklist** (the known ways stage 2 goes wrong):
- HEAD served by the object **cache** returned a synthesized `last_modified = now()` → collapsed the
  lower bound → old duplicate missed → spurious skip/insert. Confirm `head_origin` is bypassing the
  cache (Decision 1, caveat 2).
- Probe used a window *around* blob time instead of lower-bound-only → a recovery-inserted row
  (`insert_time` ≫ blob creation) fell outside the window. Confirm lower-bound-only.
- Object-store↔Postgres clock skew exceeded the 1h slack → widen slack.

## Scenario 3 — plan-time pruning fails in production (lock storm)

**Symptom**: ingestion latency collapses; `pg_locks` shows backends holding thousands of
`AccessShareLock`s on `blocks` children; shared-lock-manager contention. This is the risk-#2 failure
arriving despite the gate (e.g. an Aurora engine upgrade changed planner behaviour).

**Contain immediately**:
1. The standing guardrail alarm (see [derisk_plantime_pruning.md](./derisk_plantime_pruning.md),
   "Production guardrail") should fire first. 
2. Fastest lever: switch the hot-path insert to **direct-child targeting** (app computes the
   partition name and inserts into the child directly) — no parent-level pruning needed on the write
   path. This is Fallback 1 in the pruning de-risk doc and should be kept behind a config flag so it
   can be flipped without a code deploy if pre-staged, or shipped as a Deploy-1-compatible hotfix.
3. If direct-child targeting is not staged, the next lever is coarsening to **daily** partitions
   (Fallback 2) — a larger change, treat as a planned follow-up, not an incident hotfix.

This scenario does **not** require un-partitioning; it requires changing how rows are routed/probed.

## Scenario 4 — full revert to an unpartitioned `blocks`

The escape hatch of last resort, if partitioning itself must be abandoned (e.g. an unfixable
interaction with Aurora). This is a **maintenance-window, full-table-copy** operation — expensive but
clean, and it dedupes as it goes:

1. Stop the forward provisioner and the daemon roll-off (disable the tasks / scale maintenance to 0).
2. Build the target plain table:
   ```sql
   CREATE TABLE blocks_unpart (LIKE blocks INCLUDING DEFAULTS);  -- no PARTITION BY
   ```
3. Copy with cross-partition dedup (keep earliest `insert_time` per `block_id`):
   ```sql
   INSERT INTO blocks_unpart
   SELECT DISTINCT ON (block_id) *
     FROM blocks
    ORDER BY block_id, insert_time;
   ```
   (For a large table, do this in `insert_time` ranges to bound transaction size.)
4. Swap in one transaction and rebuild the global unique index:
   ```sql
   BEGIN;
     ALTER TABLE blocks RENAME TO blocks_partitioned_old;
     ALTER TABLE blocks_unpart RENAME TO blocks;
   COMMIT;
   CREATE UNIQUE INDEX CONCURRENTLY blocks_block_id_unique ON blocks (block_id);
   CREATE INDEX CONCURRENTLY ... (stream_id / begin_time / end_time / insert_time);
   ```
5. Roll insert code back to **Deploy-1** (schema-agnostic) or, once the global unique index exists
   again, to targeted `ON CONFLICT (block_id)`. Restore `delete_expired_blocks` for retention (revert
   the roll-off change) — this reintroduces DELETE churn, which is the pre-project status quo.
6. Drop `blocks_partitioned_old` once reads are confirmed against the new plain `blocks`.

**Data is not lost** in this procedure: every row is copied, dedup is applied, and any blob without a
row can still be reconciled per Scenario 2 before dropping the old table.

## Scenario 5 — cutover subcommand fails midway

The subcommand mirrors the v2→v3 structure: non-transactional idempotent DDL (index creation with
`IF NOT EXISTS` / `to_regclass` guards) first, then the **atomic** rename+attach+default in one
transaction (plan Decision 3, step 4).

- Failure **before** the transactional swap → `blocks` is untouched and still plain; ingestion
  unaffected. Fix the cause, re-run — the `to_regclass` guards make completed steps no-ops.
- Failure **during** the swap → the transaction rolls back atomically; `blocks` remains the original
  plain table. Re-run.
- Failure **after** the swap but before forward-buffer provisioning → `DEFAULT` backstops all inserts
  (no outage); re-run the provisioner / subcommand to fill the buffer.

The one hard deadline: if the swap hasn't completed by wall-clock `<cutover_hour>`, inserts stamping
`>= <cutover_hour>` fail the legacy CHECK (ingestion outage). Mitigation is choosing `<cutover_hour>`
with measured headroom (deploy-ordering rehearsal, step 3); if the deadline is at risk mid-cutover,
the fix is to `ALTER ... DROP CONSTRAINT blocks_legacy_bound` on the not-yet-swapped table to relieve
the deadline, then reschedule.

## Pre-ship checklist for this doc

- ☐ Reconciliation sweep (blob-without-row detector, Scenario 2) implemented and running sampled in
  monitoring **before** cutover — it is the primary detector for silent data loss.
- ☐ Duplicate-detection spot query (Scenario 1) added to the operator runbook post-cutover watch.
- ☐ Direct-child-insert fallback (Scenario 3) staged behind a flag, or its hotfix path rehearsed.
- ☐ Deploy-1 kept as the deployable rollback anchor for the whole rollout window.
- ☐ Un-partition procedure (Scenario 4) dry-run once on staging to confirm timing and that
  `BlocksView` reads correctly against the reverted plain table.
