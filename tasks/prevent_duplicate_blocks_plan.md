# Plan: Prevent Duplicate Insertion (Blocks, Streams, Processes)

## Status

| Table     | Status      | Location                                           |
|-----------|-------------|----------------------------------------------------|
| blocks    | IMPLEMENTED | `rust/ingestion/src/web_ingestion_service.rs:53-76` |
| streams   | PENDING     | `rust/ingestion/src/web_ingestion_service.rs:93-103` |
| processes | PENDING     | `rust/ingestion/src/web_ingestion_service.rs:113-129` |

## Background

The `delete_duplicate_blocks` UDF was added (commit `0bddf5be6`) to clean up existing duplicates in the blocks table. However, the root cause of duplicate insertion remains - new duplicates can still be added. The same issue affects streams and processes tables.

### Root Cause Analysis

1. **No unique constraint on primary keys**: Tables accept multiple rows with the same `block_id`/`stream_id`/`process_id`
2. **Same ID on retry**: When HTTP requests are retried due to network failures, the same encoded data (with same ID) is sent multiple times
3. **No idempotent INSERT logic**: The ingestion server doesn't check for existing records before inserting

### How Duplicates Are Created

1. Client encodes data with `uuid::Uuid::new_v4()` (generated once per entity)
2. Client sends HTTP POST to ingestion endpoint
3. Network timeout or server error occurs (but insert may have succeeded)
4. `tokio_retry2::Retry` retries with the **same encoded data** (same ID)
5. Server inserts another row with duplicate ID

## Solution

Use `INSERT ... WHERE NOT EXISTS` to prevent duplicate insertion without requiring schema changes.

### Blocks (IMPLEMENTED)

```rust
sqlx::query(
    "INSERT INTO blocks
     SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11
     WHERE NOT EXISTS (SELECT 1 FROM blocks WHERE block_id = $1);"
)
```

### Streams (PENDING)

Location: `rust/ingestion/src/web_ingestion_service.rs:93-103`

Change from:
```rust
sqlx::query("INSERT INTO streams VALUES($1,$2,$3,$4,$5,$6,$7);")
```

To:
```rust
sqlx::query(
    "INSERT INTO streams
     SELECT $1,$2,$3,$4,$5,$6,$7
     WHERE NOT EXISTS (SELECT 1 FROM streams WHERE stream_id = $1);"
)
```

Add duplicate detection logging:
```rust
if result.rows_affected() == 0 {
    debug!("duplicate stream_id={} skipped (already exists)", stream_info.stream_id);
}
```

### Processes (PENDING)

Location: `rust/ingestion/src/web_ingestion_service.rs:113-129`

Change from:
```rust
sqlx::query("INSERT INTO processes VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13);")
```

To:
```rust
sqlx::query(
    "INSERT INTO processes
     SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13
     WHERE NOT EXISTS (SELECT 1 FROM processes WHERE process_id = $1);"
)
```

Add duplicate detection logging:
```rust
if result.rows_affected() == 0 {
    debug!("duplicate process_id={} skipped (already exists)", process_info.process_id);
}
```

## Why This Approach

**Advantages:**
- No schema migration required
- No downtime - deploy immediately
- Tolerates existing duplicates
- Uses existing indices for fast lookups

**Performance:**
- Nearly identical to unique index approach
- Both do O(log n) B-tree lookup on ID
- Existing indices:
  - `CREATE INDEX block_id on blocks(block_id)`
  - `CREATE INDEX stream_id on streams(stream_id)`
  - `CREATE INDEX process_id on processes(process_id)`

**Tradeoff:**
- Small race condition window (two concurrent inserts with same ID could both succeed)
- Acceptable because duplicates come from retries seconds apart, not simultaneous requests

## Gradual Cleanup Path

1. **Deploy this change** - stops new duplicates immediately
2. **Run `delete_duplicate_blocks()` periodically** - cleans existing duplicates
3. **Add unique indices** - tracked in [#690](https://github.com/madesroches/micromegas/issues/690)

## Alternative Considered

**UNIQUE INDEX + ON CONFLICT DO NOTHING**: Deferred to [#690](https://github.com/madesroches/micromegas/issues/690) because:
- Requires schema migration
- Must clean all duplicates before adding constraint (downtime risk)

**Generate new ID on each retry**: Rejected because:
- Would create orphaned blobs in object storage for successful-but-retried inserts
- The current approach (same ID) is correct for idempotency

## Files to Modify

1. `rust/ingestion/src/web_ingestion_service.rs` - Idempotent INSERTs for all three tables
