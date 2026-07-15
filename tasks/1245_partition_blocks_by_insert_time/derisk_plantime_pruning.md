# De-risk: plan-time partition pruning is the load-bearing assumption

**Parent plan**: [plan.md](./plan.md)
**Addresses risk #2**: the entire hourly-partition scheme rests on Postgres pruning partitions at
*plan time* — and, critically, locking only the surviving partitions — when a query's `insert_time`
bound is an inlined literal `timestamptz` constant.

## Why this is the single most dangerous assumption

Every gated-insert probe (hot path and recovery stages 1–2) and the `BlocksView` materialization
filter on `insert_time`. The plan renders each bound as an **inlined literal** specifically so that:

1. Postgres prunes the non-matching partitions **at plan time**, and
2. therefore acquires an `AccessShareLock` on **only the surviving** partitions and their indexes.

If assumption (2) does **not** hold on the target Aurora version — i.e. if the planner locks the
whole partition hierarchy before pruning, or if the bound only prunes at executor-startup
(runtime) after locks are already taken — then the **hot path takes `AccessShareLock` on all
~2,160 partitions plus their block_id indexes (4,000+ locks) on every single block insert**. That
is past the per-backend fast-path lock slots, so every ingest contends on the shared lock manager.
Under production block-insert rate this is not a slowdown; it is a lock-manager collapse that takes
ingestion down. The failure is silent in staging at low volume and catastrophic at production
volume — exactly the profile that must be proven before shipping, not discovered after.

The plan asserts this is "deterministic Postgres behavior for a literal constant." That is the
documented behavior for PG ≥ 12, but "documented for upstream PG" and "true on this Aurora
version under our exact query shapes" are different claims. This document turns the claim into a
measured gate.

## The distinction that must be verified

There are three prunings, and only the first gives lock reduction:

| Pruning kind | Trigger | Removes scans? | Removes **locks**? |
|---|---|---|---|
| Plan-time | literal constant / constant-folded expr | yes | **yes** — pruned partitions never enter the plan |
| Executor-startup ("init") | `$1` bind param, stable fn (`now()`) | yes | **no** — partitions are in the plan and locked, then skipped |
| Per-scan (runtime) | values from a join/subquery | partially | no |

The plan's whole safety argument is that literals get **plan-time** pruning, not executor-startup
pruning. `now() - '1 hour'::interval` is a *stable* expression → executor-startup at best → locks
everything. This is why the helper must inline a server-computed literal and quantize it to the hour
(so the SQL text — and the cached plan — repeats within the hour). All of that reasoning is only
worth anything if plan-time pruning actually drops the locks. Verify it directly.

## Verification harness (the gate)

Run against a Postgres whose **major version matches the target Aurora version** — ideally a
throwaway Aurora Serverless v2 instance, otherwise a local PG of the same major. Scale the partition
count to production (90 days × hourly ≈ 2,160 child partitions + `DEFAULT`).

Script it in Python (per project convention) under
`tasks/1245_partition_blocks_by_insert_time/scripts/` (or a temp harness); steps:

1. **Build a production-shaped hierarchy.** Create `blocks` `PARTITION BY RANGE (insert_time)`,
   2,160 hourly partitions each with a local `UNIQUE (block_id)` index, plus the `DEFAULT`. Populate
   a few partitions with representative rows so the planner has real relpages/reltuples (run
   `ANALYZE`).

2. **Prove plan-time pruning of scans.** `EXPLAIN` the exact hot-path probe with the inlined literal
   bound the helper will emit:
   ```sql
   EXPLAIN SELECT EXISTS(SELECT 1 FROM blocks
     WHERE block_id = $1
       AND insert_time >  '<hour-1h>'::timestamptz
       AND insert_time <= '<hour+2h>'::timestamptz);
   ```
   **Pass criterion**: the plan's `Append` lists only ~2–3 named hourly partitions **plus**
   `blocks_default` — never an `Append` over all ~2,160, and never a `Seq Scan`/`Index Scan` node
   naming a partition outside the bound.

3. **Prove plan-time pruning of _locks_ (the part that actually matters).** Scans in `EXPLAIN`
   output are necessary but not sufficient — measure locks:
   ```sql
   BEGIN;
   -- run the probe (EXECUTE of the prepared statement, or the raw SELECT)
   SELECT relation::regclass, mode
     FROM pg_locks
    WHERE locktype = 'relation' AND pid = pg_backend_pid() AND mode = 'AccessShareLock';
   COMMIT;
   ```
   **Pass criterion**: the count of `AccessShareLock`s is on the order of *(surviving partitions +
   their indexes + parent)* — i.e. low tens — **not** ~4,000. This is the definitive test; if this
   passes, the design is safe. If the count is ~4,000 with the literal bound, the assumption is
   **false on this version** and hourly partitioning cannot ship as designed — go to Fallbacks.

4. **Negative control — prove the literal is doing the work.** Repeat step 3 with the *stable-expr*
   form `insert_time > now() - '1 hour'::interval AND insert_time <= now() + '2 hours'::interval`.
   **Expected**: this form locks all ~2,160 partitions (executor-startup pruning). Seeing the
   contrast confirms the harness measures the right thing and that the inlined-literal rendering is
   not accidentally equivalent to the stable-expr form. If the literal and the `now()` forms lock
   the *same* (large) number, the "literal" is being constant-folded to something non-prunable —
   investigate the exact rendering.

5. **Recovery-arm stages.** Repeat steps 2–3 for:
   - stage 1: two-sided `> '<hour-25h>' AND <= '<hour+2h>'` → expect ~26 partitions + `DEFAULT`.
   - stage 2: lower-bound-only `>= '<blob_last_modified - 1h>'` → expect [blob-age → now] partitions
     + forward buffer + `DEFAULT`. Confirm a weeks-old bound locks a few hundred, not all 2,160, and
     that `EXISTS` short-circuits.
   - Explicitly exercise the acknowledged pathological case (full-range not-found: bound at the
     90-day edge, no matching row) and **record** its lock count and latency, so the "essentially
     never, still correct" claim has a measured worst case attached rather than an assertion.

6. **Prepared-statement cache behaviour.** The helper quantizes bounds to the hour so SQL text
   repeats. Verify:
   - Two probes in the *same* hour produce byte-identical SQL text (so sqlx's per-connection prepared
     cache hits — no re-parse/re-plan per insert).
   - `ATTACH`ing a new partition mid-session invalidates the cached plan and the next execution
     replans without error (relcache invalidation). Do this by preparing the statement, attaching a
     new hourly partition, then re-executing — confirm no stale-plan/"partition not found" error and
     that the new partition is correctly considered.

7. **Concurrency sanity.** Run N concurrent backends each firing the hot-path probe+insert under the
   advisory lock; confirm `pg_locks` does not accumulate a shared-lock-manager backlog and p99
   insert latency stays flat. This catches the "fast-path slots exhausted → shared lock manager
   contention" regression even if per-query lock counts look fine in isolation.

## Pass / fail gate

- **Steps 3 and 4 are the go/no-go.** Literal bound locks low-tens of partitions **and** the
  `now()` control locks ~all → **PASS**, hourly partitioning ships as designed.
- Literal bound locks ~all partitions → **FAIL** → Fallbacks below; do not ship hourly.
- Record the actual `EXPLAIN` output, lock counts, and latencies in a results file committed
  alongside this plan (`derisk_plantime_pruning_results.md`) so the decision is auditable and can be
  re-checked after any Aurora major-version upgrade.

This gate must pass **before Phase 1 ships** (it validates the schema choice itself), and should be
re-run as part of the pre-cutover checklist and after any Aurora engine-version bump.

## Fallbacks if the gate fails

In rough order of preference — each is a real escape, none requires abandoning drop-based retention:

1. **Direct-child insert, app-computed partition.** The insert path already knows `insert_time`
   (`Utc::now()`), so it knows the exact target partition name (shared boundary fn). Insert directly
   into the child (`INSERT INTO blocks_2026071409 ...`) instead of the parent — no pruning needed on
   the write, and the per-partition local unique index still guards. The cross-partition *probe*
   still needs pruning, but the probe is on the rare/recovery path far more than the hot path if the
   hot path can be made to trust the local unique index + advisory lock alone. Evaluate whether the
   boundary-straddle race can be closed without a cross-partition hot-path probe (e.g. the advisory
   lock already serializes; the residual is only the adjacent-partition window).
2. **Coarser partitions (daily).** ~90 partitions instead of ~2,160. Even executor-startup pruning
   over 90 partitions is tolerable, and plan-time-pruning lock counts shrink 24×. This is the
   fallback the earlier plan draft carried and later removed; reinstate it if the gate fails. Cost:
   coarser retention granularity (drop a day at a time) and wider merge-window pruning sets — both
   acceptable.
3. **Shorter effective partition span via sub-partitioning or reduced retention on the metadata
   axis.** Lower priority; only if daily is somehow insufficient.

Whichever fallback is taken, the rollback/rollout docs and monitoring (low-buffer, DEFAULT-nonempty)
carry over unchanged — only width/targeting changes.

## Production guardrail (ship regardless of gate outcome)

Even after the gate passes, add a cheap standing check so a regression (Aurora upgrade changing
planner behaviour, a query accidentally reverting to a `now()` bound) is caught before it melts
ingestion:

- A low-frequency probe that runs a representative hot-path `EXPLAIN` and alarms if the plan's
  partition count exceeds a small threshold, **or**
- A sampled `pg_locks` gauge on ingestion backends alarming when a single backend holds an
  anomalously high `AccessShareLock` count on `blocks` children.

Wire it into the #1243 monitoring surface alongside the DEFAULT/low-buffer alarms.
