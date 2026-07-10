# DataFusion Target Partitions Configuration

## Problem

The `streams` and `processes` views perform `GROUP BY` aggregations that run in parallel across all CPU cores. Each parallel `GroupedHashAggregateStream` consumes ~500-700 MB of memory for large datasets. With 20 CPU cores, this requires ~11 GB of memory to query the `streams` view on a dataset with 250k streams.

The existing `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` environment variable sets a hard limit but doesn't reduce actual memory consumption - queries simply fail if they exceed the budget.

## Solution

Add a `MICROMEGAS_DATAFUSION_TARGET_PARTITIONS` environment variable to control the number of parallel partitions DataFusion uses. Reducing this from the default (CPU count) to a smaller value (e.g., 4) would proportionally reduce memory consumption.

**Example:** With 4 partitions instead of 20, memory requirement drops from ~11 GB to ~2-3 GB.

## Implementation Plan

### 1. Create helper function for SessionConfig

Add a new function in `rust/analytics/src/lakehouse/runtime.rs`:

```rust
use datafusion::prelude::SessionConfig;

/// Creates a SessionConfig with settings from environment variables.
pub fn make_session_config() -> SessionConfig {
    let mut config = SessionConfig::default()
        .set_bool("datafusion.execution.parquet.enable_page_index", false);

    if let Ok(partitions_str) = std::env::var("MICROMEGAS_DATAFUSION_TARGET_PARTITIONS") {
        if let Ok(partitions) = partitions_str.parse::<usize>() {
            config = config.with_target_partitions(partitions);
        }
    }

    config
}
```

### 2. Update all SessionConfig::default() usages

Replace `SessionConfig::default()` with `make_session_config()` in:

- `rust/analytics/src/lakehouse/query.rs:89` - `query_record_batches`
- `rust/analytics/src/lakehouse/query.rs:227` - `make_session_context` (main query path)
- `rust/analytics/src/lakehouse/merge.rs:206` - partition merging

### 3. Update documentation

Add the new environment variable to:
- `CLAUDE.md` or relevant documentation
- Any deployment/configuration docs

### 4. Testing

1. Start services with `MICROMEGAS_DATAFUSION_TARGET_PARTITIONS=4` and `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB=3000`
2. Run `poetry run pytest tests/test_streams.py` - should pass
3. Verify query results are correct (same data, potentially different row order)
4. Benchmark query performance impact (reduced parallelism = slower queries)

## Files to Modify

- `rust/analytics/src/lakehouse/runtime.rs` - add `make_session_config()`
- `rust/analytics/src/lakehouse/query.rs` - use `make_session_config()`
- `rust/analytics/src/lakehouse/merge.rs` - use `make_session_config()`

## Trade-offs

- **Lower memory:** Fewer parallel aggregations = less memory
- **Slower queries:** Less parallelism = longer query times
- **User control:** Operators can tune for their hardware constraints

## Verification Data

From testing on dataset with 37M blocks and 250k streams:

| Target Partitions | Approx Memory Needed |
|-------------------|---------------------|
| 20 (default)      | ~11 GB              |
| 10                | ~5-6 GB             |
| 4                 | ~2-3 GB             |
| 2                 | ~1-1.5 GB           |
