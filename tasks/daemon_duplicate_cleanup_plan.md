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

## Future Improvement: Unify UDF and Standalone Function

The `DeleteDuplicateBlocks` UDF and the standalone `delete_duplicate_blocks()` function currently have duplicated SQL logic. Refactor the UDF to call the standalone function internally:

```rust
#[async_trait]
impl AsyncScalarUDFImpl for DeleteDuplicateBlocks {
    async fn invoke_async_with_args(&self, args: ScalarFunctionArgs) -> datafusion::error::Result<ColumnarValue> {
        // ... validation ...

        let deleted_count = delete_duplicate_blocks(&self.lake.db_pool, range.clone())
            .await
            .map_err(|e| DataFusionError::Execution(format!("Failed to delete duplicates: {e}")))?;

        // ... build result array ...
    }
}
```

This eliminates code duplication and ensures both paths use identical cleanup logic.

## Temporary Nature

This is explicitly a stopgap measure. The permanent solution is:
1. Clean all existing duplicates
2. Add unique constraints to blocks, streams, and processes tables
3. Remove this daemon task

Tracked in [#690](https://github.com/madesroches/micromegas/issues/690).
