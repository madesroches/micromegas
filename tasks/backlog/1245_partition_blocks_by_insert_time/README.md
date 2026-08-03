# Partition `blocks` by `insert_time` (#1245)

Convert the Postgres `blocks` metadata table to a `PARTITION BY RANGE (insert_time)` table with
hourly UTC partitions, so retention becomes a partition `DROP` instead of batched row `DELETE`s —
eliminating the dead-tuple / autovacuum / WAL churn that spikes Aurora ACUs. Global `block_id` dedup
moves to the object store (conditional PUT as the uniqueness arbiter).

## Documents

- **[plan.md](./plan.md)** — the full design (schema, cutover, insert paths, forward-provisioning,
  daemon roll-off, monitoring, implementation steps).
- **[derisk_plantime_pruning.md](./derisk_plantime_pruning.md)** — de-risks the load-bearing
  assumption that Postgres prunes **and locks** only surviving partitions at plan time for inlined
  literal bounds. Includes a measurement harness and a hard pass/fail gate; if it fails, the hourly
  scheme cannot ship as designed. **Gate before Phase 1.**
- **[derisk_deploy_ordering.md](./derisk_deploy_ordering.md)** — de-risks the three-step ordered
  rollout (Deploy 1 → operator cutover → Deploy 2). Adds build-time invariant tests, a cutover
  preflight that refuses to run while old insert code is live, a staging rehearsal that injects each
  hazard, and the operator runbook.
- **[rollback.md](./rollback.md)** — containment and recovery for each way it can go wrong in
  production (duplicate rows, dropped blocks, lock storm, full un-partition, mid-cutover failure),
  anchored on "the blob is the durable source of truth."

## Hard gates before rollout

1. Plan-time pruning lock-count gate passes on the target Aurora version
   (derisk_plantime_pruning.md §"Pass / fail gate").
2. Cutover-timing measured on production-sized `blocks` in staging (plan *Decisions & Rollout* #2,
   also folded into the deploy-ordering rehearsal).
3. Blob-without-row reconciliation sweep live in monitoring before cutover (rollback.md Scenario 2).
