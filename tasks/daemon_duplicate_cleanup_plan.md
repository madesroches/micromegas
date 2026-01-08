# Plan: Daemon Periodic Duplicate Block Cleanup

## Background

Despite implementing idempotent INSERTs (see `prevent_duplicate_blocks_plan.md`), duplicate blocks can still accumulate due to:
- Race conditions in concurrent inserts
- Historical duplicates created before the prevention fix
- Edge cases in retry logic

The `delete_duplicate_blocks()` UDF exists but must be invoked manually. Until we can deploy unique constraints (tracked in [#690](https://github.com/madesroches/micromegas/issues/690)), we need an automated cleanup mechanism.

## Goal

Modify the maintenance daemon to automatically clean up duplicate blocks every minute as a temporary measure until unique constraints can be deployed.

## Approach

Add duplicate block cleanup to the `EveryMinuteTask` in the maintenance daemon. This provides:
- Frequent cleanup (every minute) to prevent duplicate accumulation
- Minimal impact on existing daemon architecture
- Easy removal once unique constraints are in place

## Implementation Steps

### Step 1: Add delete_duplicate_blocks function to analytics crate

Create a new public async function in `rust/analytics/src/lakehouse/` that wraps the SQL logic from `delete_duplicate_blocks_udf.rs` for direct invocation (without going through DataFusion UDF machinery).

```rust
pub async fn delete_duplicate_blocks(
    pool: &sqlx::PgPool,
    time_range: TimeRange,
) -> Result<u64>
```

### Step 2: Export the function from analytics crate

Add the new function to the appropriate module exports so it can be called from the public crate.

### Step 3: Modify EveryMinuteTask

In `rust/public/src/servers/maintenance.rs`, update `EveryMinuteTask::run()` to call the duplicate cleanup before materialization:

```rust
async fn run(&self, task_scheduled_time: DateTime<Utc>) -> Result<()> {
    // Clean up any duplicate blocks from the last few minutes
    let cleanup_range = TimeRange::new(
        task_scheduled_time - TimeDelta::minutes(5),
        task_scheduled_time,
    );
    match delete_duplicate_blocks(&self.lakehouse.lake().db_pool, cleanup_range).await {
        Ok(count) if count > 0 => info!("deleted {count} duplicate blocks"),
        Ok(_) => {}
        Err(e) => warn!("duplicate cleanup failed: {e:?}"),
    }

    // Existing materialization logic...
    let partition_time_delta = TimeDelta::minutes(1);
    // ...
}
```

### Step 4: Add logging/metrics

Add span instrumentation and optionally a metric counter for deleted duplicates to track cleanup effectiveness.

## Files to Modify

| File | Action |
|------|--------|
| `rust/analytics/src/lakehouse/delete_duplicate_blocks_udf.rs` | Add standalone async function |
| `rust/analytics/src/lakehouse/mod.rs` | Export new function |
| `rust/public/src/servers/maintenance.rs` | Call cleanup in EveryMinuteTask |

## Considerations

1. **Error handling**: Duplicate cleanup failures should log warnings but not fail the entire minute task - materialization should still proceed
2. **Time range**: Use a 5-minute lookback window to catch any recent duplicates without scanning too much data
3. **Performance**: The cleanup query uses existing indices on `block_id` and `insert_time`
4. **Removal path**: Once unique constraints are deployed (#690), remove this code from the daemon

## Alternative Considered

**Run cleanup in EveryHourTask instead**: Rejected because hourly cleanup allows duplicates to accumulate, potentially causing issues with queries and materialization.

## Temporary Nature

This is explicitly a stopgap measure. The permanent solution is:
1. Clean all existing duplicates
2. Add unique constraints to blocks, streams, and processes tables
3. Remove this daemon task

Tracked in [#690](https://github.com/madesroches/micromegas/issues/690).
