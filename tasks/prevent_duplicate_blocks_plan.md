# Plan: Prevent Duplicate Block Insertion

## Background

The `delete_duplicate_blocks` UDF was added (commit `0bddf5be6`) to clean up existing duplicates in the blocks table. However, the root cause of duplicate insertion remains - new duplicates can still be added.

### Root Cause Analysis

1. **No unique constraint on `block_id`**: The blocks table accepts multiple rows with the same `block_id`
2. **Same `block_id` on retry**: When HTTP requests are retried due to network failures, the same encoded block (with same `block_id`) is sent multiple times
3. **No idempotent INSERT logic**: The ingestion server doesn't check for existing blocks before inserting

### How Duplicates Are Created

1. Client encodes a block with `block_id = uuid::Uuid::new_v4()` (generated once)
2. Client sends HTTP POST to `/ingestion/insert_block`
3. Network timeout or server error occurs (but insert may have succeeded)
4. `tokio_retry2::Retry` retries with the **same encoded block** (same `block_id`)
5. Server inserts another row with duplicate `block_id`

## Proposed Solution

Use `INSERT ... WHERE NOT EXISTS` to prevent duplicate insertion without requiring schema changes.

### Step 1: Modify INSERT to check for existing block_id

Location: `rust/ingestion/src/web_ingestion_service.rs`

Change from:
```rust
sqlx::query("INSERT INTO blocks VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11);")
```

To:
```rust
sqlx::query(
    "INSERT INTO blocks
     SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11
     WHERE NOT EXISTS (SELECT 1 FROM blocks WHERE block_id = $1);"
)
```

### Step 2: Handle return value

The INSERT returns 0 rows affected if block already exists. We should:
- Log at DEBUG level when a duplicate is skipped (for observability)
- Return success to client (idempotent behavior)

## Why This Approach

**Advantages:**
- No schema migration required
- No downtime - deploy immediately
- Tolerates existing duplicates
- Uses existing `block_id` index for fast lookups

**Performance:**
- Nearly identical to unique index approach
- Both do O(log n) B-tree lookup on `block_id`
- Existing index: `CREATE INDEX block_id on blocks(block_id)`

**Tradeoff:**
- Small race condition window (two concurrent inserts with same `block_id` could both succeed)
- Acceptable because duplicates come from retries seconds apart, not simultaneous requests

## Gradual Cleanup Path

1. **Deploy this change** - stops new duplicates immediately
2. **Run `delete_duplicate_blocks()` periodically** - cleans existing duplicates
3. **Add unique index** - tracked in [#690](https://github.com/madesroches/micromegas/issues/690)

## Alternative Considered

**UNIQUE INDEX + ON CONFLICT DO NOTHING**: Deferred to [#690](https://github.com/madesroches/micromegas/issues/690) because:
- Requires schema migration
- Must clean all duplicates before adding constraint (downtime risk)

**Generate new `block_id` on each retry**: Rejected because:
- Would create orphaned blobs in object storage for successful-but-retried inserts
- The current approach (same `block_id`) is correct for idempotency

## Testing

Manual testing with local services:
1. Start services with `python3 local_test_env/ai_scripts/start_services.py`
2. Insert a block, note the `block_id`
3. Replay the same request, verify no duplicate row created
4. Check rows affected indicates duplicate was skipped

## Files to Modify

1. `rust/ingestion/src/web_ingestion_service.rs` - Idempotent INSERT
