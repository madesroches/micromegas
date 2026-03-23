# Scope Merge Session Context to Insert Time Range

## Overview

`BatchPartitionMerger::execute_merge_query` and `QueryMerger::execute_merge_query` both create their session context with `query_range = None`, causing joined views (notably `processes`) to load their entire history instead of just the rows relevant to the merge window. This causes massive memory bloat during compaction — a daily merge that touches ~2,700 processes was loading all ~237K from 316 partitions.

The fix threads `insert_range: TimeRange` from the caller through the `View::merge_partitions` and `PartitionMerger::execute_merge_query` interfaces, then passes it to `make_session_context`.

GitHub issue: #963

## Current State

Both merger implementations pass `None` for `query_range`:

- `BatchPartitionMerger::execute_merge_query` at `batch_partition_merger.rs:126-133` — passes `None`
- `QueryMerger::execute_merge_query` at `merge.rs:71-78` — passes `None`

`make_session_context` (`query.rs:177-219`) uses `query_range` in two ways:
1. Adds a `TableScanRewrite` analyzer rule that injects time filters into all `MaterializedView` scans
2. Passes the range to `register_functions` and `register_table`, which filter `PartitionCache::fetch` by insert_time overlap

When `None` is passed, neither mechanism activates, so joined views load all partitions.

The caller `create_merged_partition` (`merge.rs:130`) already receives `insert_range: TimeRange` as a parameter but does not forward it to `view.merge_partitions`.

### Call chain today

```
create_merged_partition(insert_range: TimeRange, ...)
  └─ view.merge_partitions(lakehouse, partitions_to_merge, partitions_all_views)  // no range
       └─ merger.execute_merge_query(lakehouse, partitions_to_merge, partitions_all_views)  // no range
            └─ make_session_context(..., None, ...)  // loads all partitions for joined views
```

## Design

Thread `insert_range` from the caller through the trait interfaces so it reaches `make_session_context`:

```
create_merged_partition(insert_range: TimeRange, ...)
  └─ view.merge_partitions(lakehouse, partitions_to_merge, partitions_all_views, insert_range)
       └─ merger.execute_merge_query(lakehouse, partitions_to_merge, partitions_all_views, insert_range)
            └─ make_session_context(..., Some(insert_range), ...)
```

### Why this is safe

- The `processes` view is built from `blocks GROUP BY process_id`, where blocks are filtered by `insert_time`
- If partitions being merged have `insert_time` in [T1, T2], the underlying blocks were inserted in [T1, T2], so the processes view for that range contains every referenced `process_id`
- `PartitionCache::fetch` filters by insert_time overlap, matching the same time basis
- `TableScanRewrite` calls `view.make_time_filter(begin, end)` on `MaterializedView` scans. For the processes view (`processes_view.rs:69-70`), the time columns are `insert_time` and `last_update_time` (which is `max(insert_time)` from blocks), so passing an insert_time range is semantically correct
- The filter only narrows, never widens — worst case is identical to the current behavior

## Implementation Steps

### Step 1: Add `insert_range` to `PartitionMerger` trait (`merge.rs:27-35`)

```rust
#[async_trait]
pub trait PartitionMerger: Send + Sync + Debug {
    async fn execute_merge_query(
        &self,
        lakehouse: Arc<LakehouseContext>,
        partitions_to_merge: Arc<Vec<Partition>>,
        partitions_all_views: Arc<PartitionCache>,
        insert_range: TimeRange,
    ) -> Result<SendableRecordBatchStream>;
}
```

Add `use crate::time::TimeRange;` — already imported on line 13.

### Step 2: Update `QueryMerger::execute_merge_query` (`merge.rs:63-97`)

Accept the new parameter and pass `Some(insert_range)` to `make_session_context`:

```rust
async fn execute_merge_query(
    &self,
    lakehouse: Arc<LakehouseContext>,
    partitions_to_merge: Arc<Vec<Partition>>,
    partitions_all_views: Arc<PartitionCache>,
    insert_range: TimeRange,
) -> Result<SendableRecordBatchStream> {
    let reader_factory = lakehouse.reader_factory().clone();
    let ctx = make_session_context(
        lakehouse.clone(),
        partitions_all_views,
        Some(insert_range),
        self.view_factory.clone(),
        self.session_configurator.clone(),
    )
    .await?;
    // ... rest unchanged
```

### Step 3: Update `BatchPartitionMerger::execute_merge_query` (`batch_partition_merger.rs:103-181`)

Accept the new parameter, add `use crate::time::TimeRange;` to imports, and pass `Some(insert_range)` to `make_session_context`:

```rust
async fn execute_merge_query(
    &self,
    lakehouse: Arc<LakehouseContext>,
    partitions_to_merge: Arc<Vec<Partition>>,
    partitions_all_views: Arc<PartitionCache>,
    insert_range: TimeRange,
) -> Result<SendableRecordBatchStream> {
    // ... early return for empty partitions unchanged ...
    let ctx = make_session_context(
        lakehouse.clone(),
        partitions_all_views,
        Some(insert_range),
        self.view_factory.clone(),
        self.session_configurator.clone(),
    )
    .await?;
    // ... rest unchanged
```

### Step 4: Add `insert_range` to `View::merge_partitions` (`view.rs:94-111`)

Update the trait method signature and its default implementation:

```rust
async fn merge_partitions(
    &self,
    lakehouse: Arc<LakehouseContext>,
    partitions_to_merge: Arc<Vec<Partition>>,
    partitions_all_views: Arc<PartitionCache>,
    insert_range: TimeRange,
) -> Result<SendableRecordBatchStream> {
    let merge_query = Arc::new(String::from("SELECT * FROM source;"));
    let empty_view_factory = Arc::new(ViewFactory::new(vec![]));
    let merger = QueryMerger::new(
        empty_view_factory,
        Arc::new(NoOpSessionConfigurator),
        self.get_file_schema(),
        merge_query,
    );
    merger
        .execute_merge_query(lakehouse, partitions_to_merge, partitions_all_views, insert_range)
        .await
}
```

The default implementation has no joins (`SELECT * FROM source`), so the range is a no-op there — but the API stays consistent.

### Step 5: Update `SqlBatchView::merge_partitions` (`sql_batch_view.rs:249-263`)

Accept and forward the new parameter:

```rust
async fn merge_partitions(
    &self,
    lakehouse: Arc<LakehouseContext>,
    partitions_to_merge: Arc<Vec<Partition>>,
    partitions_all_views: Arc<PartitionCache>,
    insert_range: TimeRange,
) -> Result<SendableRecordBatchStream> {
    let res = self
        .merger
        .execute_merge_query(lakehouse, partitions_to_merge, partitions_all_views, insert_range)
        .await;
    if let Err(e) = &res {
        error!("{e:?}");
    }
    res
}
```

### Step 6: Update the caller `create_merged_partition` (`merge.rs:170-177`)

Pass the `insert_range` it already has:

```rust
let mut merged_stream = view
    .merge_partitions(
        lakehouse.clone(),
        Arc::new(filtered_partitions),
        partitions_all_views,
        insert_range,
    )
    .await
    .with_context(|| "view.merge_partitions")?;
```

## Files to Modify

- `rust/analytics/src/lakehouse/merge.rs` — `PartitionMerger` trait, `QueryMerger` impl, `create_merged_partition` caller
- `rust/analytics/src/lakehouse/batch_partition_merger.rs` — add import, accept and use `insert_range`
- `rust/analytics/src/lakehouse/view.rs` — `View::merge_partitions` signature and default impl
- `rust/analytics/src/lakehouse/sql_batch_view.rs` — `SqlBatchView::merge_partitions` override

## Trade-offs

**Why thread through traits instead of computing from partitions:** Computing the range from `partitions_to_merge` inside each merger would also work, but the caller already has the authoritative `insert_range`. Threading it makes the contract explicit, avoids redundant computation, and avoids needing `expect()` on potentially-empty iterators.

**Alternative: Fix only `BatchPartitionMerger`** — The issue description highlights batched merges, but `QueryMerger` has the same problem. Since we're changing the trait anyway, both get fixed naturally.

## Testing Strategy

- Run existing tests: `cargo test` from `rust/` directory
- The existing merge tests exercise both merger implementations — they will need to pass the new `insert_range` parameter
- Memory impact can be validated in production by observing peak RSS during daily merges (should drop from ~6.5 GB to ~2.2 GB for the scenario described in the issue)
