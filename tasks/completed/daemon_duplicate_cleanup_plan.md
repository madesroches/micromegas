# Plan: Daemon Periodic Duplicate Block Cleanup

**Status: Implemented**

## Background

Despite implementing idempotent INSERTs (see `prevent_duplicate_blocks_plan.md`), duplicate blocks can still accumulate due to:
- Race conditions in concurrent inserts
- Historical duplicates created before the prevention fix
- Edge cases in retry logic

The `delete_duplicate_blocks()` UDF exists but must be invoked manually. Until we can deploy unique constraints (tracked in [#690](https://github.com/madesroches/micromegas/issues/690)), we need an automated cleanup mechanism.

## Goal

Modify the maintenance daemon to automatically clean up duplicate blocks every minute as a temporary measure until unique constraints can be deployed.

## Implementation

Added duplicate block cleanup to the `EveryMinuteTask` in the maintenance daemon:
- Runs every minute with a 5-minute lookback window
- Logs only when duplicates are found
- Failures warn but don't block materialization

### Files Modified

| File | Change |
|------|--------|
| `rust/analytics/src/lakehouse/delete_duplicate_blocks_udf.rs` | Added standalone `delete_duplicate_blocks()` async function |
| `rust/public/src/servers/maintenance.rs` | Call cleanup in `EveryMinuteTask::run()` |

## UDF Unified with Standalone Function

**Status: Complete**

The `DeleteDuplicateBlocks` UDF now calls the standalone `delete_duplicate_blocks()` function internally, eliminating code duplication. Both the daemon task and SQL UDF use identical cleanup logic.

Benefits:
- Single source of truth for the SQL query
- Consistent logging behavior (only logs when duplicates found)
- Easier maintenance - changes to cleanup logic only need to be made in one place

## Temporary Nature

This is explicitly a stopgap measure. The permanent solution is:
1. Clean all existing duplicates
2. Add unique constraints to blocks, streams, and processes tables
3. Remove this daemon task

Tracked in [#690](https://github.com/madesroches/micromegas/issues/690).
