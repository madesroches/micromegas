# LIMIT Pushdown for Table Functions

## Problem

When executing queries like:
```sql
SELECT file_path, updated FROM list_partitions() LIMIT 5;
```

The query returns all rows (~14,000+) instead of the expected 5 rows.

## Investigation Status

### What was tried (did not fix the issue)
1. Changed from `MemorySourceConfig::try_new_exec()` to `MemorySourceConfig::try_new().with_limit(limit)` pattern
2. Unit tests pass with this approach
3. EXPLAIN plan shows `fetch=5` correctly in both logical and physical plans

### Current findings
- Unit tests with `df.collect()` work correctly - returning exactly 5 rows
- The SQL server reports `fetch=5` in EXPLAIN output
- However, FlightSQL client receives ALL rows (~15,000+)
- Time range parameter makes no difference
- `SELECT COUNT(*) FROM (SELECT * FROM list_partitions() LIMIT 5)` returns 5 (correct!)
- Direct client query returns 15,000+ rows (incorrect!)

### Root cause hypothesis
The issue is NOT in the `TableProvider::scan` limit parameter. The `.with_limit(limit)` is being called and the EXPLAIN shows it. The problem seems to be:
1. Either MemorySourceConfig's `with_limit()` doesn't actually limit execution (just sets metadata)
2. Or there's something in the FlightSQL streaming layer that ignores the limit

### Latest findings (Nov 21, 2025)

**Critical discovery: The bug is projection-dependent!**

| Query | Result |
|-------|--------|
| `SELECT file_path FROM list_partitions() LIMIT 5` | **5 rows** (correct) |
| `SELECT updated FROM list_partitions() LIMIT 5` | **5 rows** (correct) |
| `SELECT file_path, updated FROM list_partitions() LIMIT 5` | **16,000+ rows** (wrong!) |
| `SELECT * FROM list_partitions() LIMIT 5` | **16,000+ rows** (wrong!) |

All queries show `limit=Some(5)` in the server logs, confirming the limit parameter IS being passed to `scan()`.

**Root cause confirmed**: DataFusion's `MemorySourceConfig::with_limit()` doesn't work correctly with FlightSQL streaming when multiple columns are projected. The limit is set correctly in the execution plan metadata but not enforced during actual execution via `execute_stream()`.

### Time Range Rewrite Investigation

The system uses a custom `TableScanRewrite` analyzer rule (`rust/analytics/src/lakehouse/table_scan_rewrite.rs`) that rewrites the logical plan to add time-based filters. However, this rule:
- Uses `transform_up_with_subqueries` to traverse the plan
- Only affects `MaterializedView` tables (line 38-44)
- **Explicitly skips table functions** like `list_partitions()` (returns `Transformed::no(plan)`)

Therefore, the `TableScanRewrite` is NOT the cause of the LIMIT pushdown bug for table functions.

## Solution (Nov 21, 2025)

**Workaround implemented**: Instead of relying on `MemorySourceConfig::with_limit()`, we now manually slice the `RecordBatch` before creating the `MemorySourceConfig`:

```rust
// Apply limit by slicing the RecordBatch before creating MemorySourceConfig
// This is a workaround for a DataFusion bug where MemorySourceConfig's with_limit()
// doesn't work correctly with certain projections
let limited_rb = if let Some(n) = limit {
    rb.slice(0, n.min(rb.num_rows()))
} else {
    rb
};

let source = MemorySourceConfig::try_new(
    &[vec![limited_rb]],
    self.schema(),
    projection.map(|v| v.to_owned()),
)?;
```

This ensures the data is limited at the source level, regardless of DataFusion's internal handling of limits with projections.

## Affected Files

- `rust/analytics/src/lakehouse/list_partitions_table_function.rs`
- `rust/analytics/src/lakehouse/list_view_sets_table_function.rs`
- Any other TableProvider implementations using MemorySourceConfig

## Testing Instructions

When testing changes to services, use the service management scripts:
1. Stop services: `python3 local_test_env/ai_scripts/stop_services.py`
2. Build with changes: `cargo build` (from `rust/` directory)
3. Start services: `python3 local_test_env/ai_scripts/start_services.py`
4. Test with query CLI: `cd python/micromegas/cli && poetry run python query.py "SELECT * FROM list_partitions() LIMIT 5" --begin 1h`

## TODO

- [x] Add unit tests in the analytics crate to verify LIMIT pushdown works correctly for table functions
- [x] Fix actual LIMIT pushdown to work end-to-end with FlightSQL (via RecordBatch slicing workaround)
- [x] Verify fix works end-to-end via FlightSQL (use start/stop_services.py scripts) - **Verified Nov 21, 2025**
- [ ] Consider filing a DataFusion bug report for `MemorySourceConfig::with_limit()` not working with projections in streaming context
