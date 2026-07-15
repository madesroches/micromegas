# De-risk: the multi-deploy cutover ordering

**Parent plan**: [plan.md](./plan.md) — see "Rolling-deploy hazard" under *Schema (#1240)*.
**Addresses risk #3**: correctness of the change depends on a human executing a three-step, ordered
deploy sequence exactly. Getting the order wrong crashes every instance at startup or fails every
insert. This document converts that fragile sequence into a checklist with automated guardrails and
a rehearsal, so no single human error produces a fleet-wide outage.

## The sequence and why order is load-bearing

```
Deploy 1  ──►  Operator cutover  ──►  Deploy 2
(code)         (out-of-band DDL)      (LATEST bump)
```

- **Deploy 1** — ships to *every* ingestion/replication/monolith-ingestion instance:
  - new no-target `ON CONFLICT DO NOTHING` insert code (schema-agnostic: works on both the plain and
    the partitioned table),
  - conditional-PUT + gated-insert helper,
  - `migrate_db`'s and `execute_migration`'s post-migration asserts **relaxed from `==` to
    `current_version >= LATEST_DATA_LAKE_SCHEMA_VERSION`** (warn when DB is ahead),
  - `LATEST_DATA_LAKE_SCHEMA_VERSION` **still 4**,
  - `upgrade_data_lake_schema_v5` present but reachable **only** via the standalone CLI subcommand —
    never wired into `execute_migration`'s auto chain.
- **Operator cutover** — after confirming *every* live instance runs Deploy 1 code, an operator runs
  the standalone subcommand once. It performs the attach-legacy DDL and stamps `current_version = 5`.
- **Deploy 2** — bumps `LATEST_DATA_LAKE_SCHEMA_VERSION` to 5. Ships only after the operator confirms
  the DB is already at 5.

### The three ways it goes wrong

1. **LATEST bumped to 5 in Deploy 1 (skibbing the split).** `execute_migration`'s chain has no step
   that advances a DB past 4, so a binary declaring LATEST=5 against a still-v4 DB hits
   `assert_eq!(current_version, LATEST)` and **crash-loops at startup — fleet-wide**, before any
   cutover can run.
2. **Old instance restarts in the gap** (after cutover stamped 5, before Deploy 2). A still-LATEST=4
   binary re-enters `migrate_db`, sees `current(5) != LATEST(4)`, and crashes — **unless** the `>=`
   assert relaxation from Deploy 1 is present. The relaxation is what makes the gap survivable.
3. **Cutover runs while old insert code is still live.** Old targeted `ON CONFLICT (block_id)` code
   against the now-partitioned parent fails immediately ("no unique or exclusion constraint matching
   the ON CONFLICT specification") because a partitioned parent can't carry a `block_id`-only unique
   index — **every insert on old instances starts erroring.**

## Guardrails (make the failure modes unreachable, not just documented)

### G1 — Build-time invariant test: LATEST bump and cutover-wiring can never co-ship

Add a unit test (in `rust/ingestion/tests/`) asserting the Deploy-1 shape is internally consistent:

- If `LATEST_DATA_LAKE_SCHEMA_VERSION == 4`: assert `execute_migration` has **no** `4 == current`
  branch (v5 is standalone-only), and assert both post-migration asserts are the `>=` form.
- A dedicated test asserts the asserts are `>=`, not `==`, by exercising `migrate_db`/
  `execute_migration` against a **stub DB reporting version 5 while the binary's LATEST is 4** and
  confirming it returns Ok with a warning rather than panicking. This is the exact hazard-2 scenario
  and it must be a standing regression test, not a manual check.
- When Deploy 2 bumps LATEST to 5, the same test file is updated in the same commit; the test makes
  the two edits move together and fails CI if someone bumps LATEST while leaving the asserts `==`.

The point: hazard 1 and hazard 2 become **CI failures**, not production crashes.

### G2 — Cutover preflight: refuse to run unless the fleet is on new code

The plan's ordering relies on the operator "confirming every instance runs the new insert code."
Make that a machine check instead of a belief. Two options, in preference order:

- **Instance version registry (recommended).** Deploy-1 instances upsert a heartbeat row into a
  small unpartitioned table on startup and periodically:
  `instance_registry(instance_id text pk, insert_code_version int, last_seen timestamptz)`.
  `insert_code_version` is a monotonic constant baked into the binary (bump it whenever the insert
  path's schema-compatibility changes). The standalone cutover subcommand's **preflight** does:
  ```sql
  SELECT count(*) FROM instance_registry
   WHERE last_seen > now() - interval '2 minutes'
     AND insert_code_version < <REQUIRED_VERSION>;
  ```
  Non-zero → **abort with the offending instances listed.** This directly closes hazard 3: the
  cutover cannot proceed while any recently-alive instance still runs old insert code. (The registry
  table is unpartitioned and tiny; its churn is negligible and it is useful beyond this migration.)
- **Manual confirmation gate (minimum).** If the registry is deemed too much for this change, the
  subcommand at least requires an explicit `--i-have-confirmed-all-instances-run-deploy-1` flag and
  prints the current fleet's deploy expectations, so the confirmation is a deliberate act with an
  audit trail, not a silent default.

### G3 — Cutover idempotency / retry-safety guard

The subcommand begins by inspecting `blocks`:
- `relkind = 'p'` (already partitioned — e.g. a fresh install during the interim window, or a
  re-run) → perform **no DDL**, only ensure `current_version = 5`, exit success. This is what makes
  a re-run and the fresh-install case both no-ops.
- unpartitioned `blocks` at `current_version = 4` → perform the attach-legacy DDL, stamp 5.
- any other state (e.g. `current_version = 5` but unpartitioned — should be impossible) → **abort
  loudly**, do not guess.

Guard each DDL step on `to_regclass(...)` so a mid-way failure + re-run resumes cleanly (mirror the
existing v2→v3 non-transactional-then-transactional structure).

## Rehearsal (staging, before touching production)

Run the **entire** three-phase sequence on a staging DB seeded to production shape, and deliberately
inject each hazard to prove the guardrails catch it:

1. Deploy-1 code to a multi-instance staging fleet against a v4 populated `blocks`. Confirm ingestion
   continues, asserts are `>=`, instances register in `instance_registry`.
2. **Inject hazard 3**: leave one instance on pre-Deploy-1 code; run the cutover preflight; confirm
   it **aborts** and names that instance. Kill the old instance, re-run, confirm it proceeds.
3. Run the cutover; measure the exclusive-lock windows and `VALIDATE` durations (this doubles as the
   cutover-timing gate from the parent plan's *Decisions & Rollout* item 2, feeding `<cutover_hour>`
   headroom).
4. **Inject hazard 2**: with the DB now at v5, restart a still-LATEST=4 Deploy-1 instance; confirm it
   starts, logs the "DB ahead of binary" warning, and serves inserts (proves the `>=` relaxation).
5. **Inject hazard 1 in CI, not staging**: confirm the G1 test fails if LATEST is bumped to 5 without
   the standalone wiring / with `==` asserts.
6. Simulate a **fresh install during the interim** (Deploy-1 live, LATEST still 4): confirm
   `create_tables` builds a partitioned `blocks` and stamps `migration` to 4; then run the cutover
   subcommand and confirm G3's `relkind='p'` guard makes it a stamp-only no-op to 5.
7. Deploy 2 (LATEST→5) to the fleet; confirm clean startup against the v5 DB and that the G1 test was
   updated in the same commit.

## Operator runbook (the checklist that ships with the change)

Each step has an explicit **verify-before-proceed** so the sequence can't skip ahead:

1. ☐ Deploy 1 to **all** ingestion/replication/monolith-ingestion instances.
   **Verify**: `SELECT min(insert_code_version), count(*) FROM instance_registry WHERE last_seen >
   now() - interval '2 minutes';` shows `min >= REQUIRED` and the count matches expected fleet size.
2. ☐ Confirm current schema is still v4 and unpartitioned:
   `SELECT relkind FROM pg_class WHERE relname='blocks';` → `r`; migration version = 4.
3. ☐ Choose `<cutover_hour>` = ceil(now + measured VALIDATE duration + 1–2h margin) from the staging
   rehearsal (step 3 above).
4. ☐ Run the standalone cutover subcommand. Preflight (G2) must pass; it performs the DDL and stamps
   5. **Verify**: `blocks` `relkind='p'`, `blocks_legacy` attached with the right bound, `DEFAULT`
   present, forward buffer provisioned, migration version = 5, `BlocksView` still queries correctly.
5. ☐ Watch for one full retention/materialization cycle: DEFAULT stays empty, forward-buffer alarm
   green, recovery-arm counters sane, no duplicate rows (spot-check
   `SELECT block_id,count(*) FROM blocks GROUP BY 1 HAVING count(*)>1` scoped to recent hours).
6. ☐ Only then: Deploy 2 (LATEST→5) to the fleet. **Verify**: clean startup fleet-wide, no assert
   warnings about DB-ahead-of-binary.

**Rollback floor**: once step 4 has run, binaries may be rolled back **only to Deploy-1 code**, never
older — pre-Deploy-1 binaries have `==` asserts and targeted `ON CONFLICT` and will crash/fail
against the v5 partitioned DB. See [rollback.md](./rollback.md).
