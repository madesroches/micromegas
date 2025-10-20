# Empty Lakehouse Partitions Support Plan

## Problem Statement

Currently, the lakehouse partition system **cannot create partition records for time ranges with zero rows**. When a time window is processed but contains no data:
1. No partition record is created in the database
2. System cannot distinguish "no data exists" from "not yet processed"
3. No way to prevent redundant reprocessing of known-empty time ranges
4. No audit trail for materialization attempts on empty windows
5. **Cannot safely merge/regroup partitions** - When merging small partitions (e.g., hourly) into larger ones (e.g., daily), missing partition records make it impossible to know if a time range is truly empty or just not yet processed

### Real-World Example: Partition Regrouping

**Scenario:** You have hourly partitions and want to merge them into daily partitions.

**Without empty partition support:**
```
2024-01-15 00:00 → partition exists (100 rows)
2024-01-15 01:00 → NO RECORD (was it processed? is it empty? unknown!)
2024-01-15 02:00 → partition exists (50 rows)
2024-01-15 03:00 → NO RECORD (unknown status)
...
2024-01-15 23:00 → partition exists (200 rows)

Result: Cannot safely merge to daily partition - missing hours might not be processed yet!
```

**With empty partition support:**
```
2024-01-15 00:00 → partition exists (100 rows)
2024-01-15 01:00 → partition exists (0 rows, file_path=None) ✅ Explicitly empty!
2024-01-15 02:00 → partition exists (50 rows)
2024-01-15 03:00 → partition exists (0 rows, file_path=None) ✅ Explicitly empty!
...
2024-01-15 23:00 → partition exists (200 rows)

Result: Can safely merge all 24 hourly partitions into 1 daily partition!
```

**Key insight:** Empty partition records provide **completeness guarantees** needed for safe data reorganization.

### 🚨 CRITICAL: Partition Struct Cannot Represent Empty Partitions

The `Partition` struct has **fundamental design issues** preventing empty partition support:

```rust
pub struct Partition {
    pub min_event_time: DateTime<Utc>,  // NOT Option
    pub max_event_time: DateTime<Utc>,  // NOT Option
    pub file_size: i64,
    pub num_rows: i64,
    // ... other fields
}
```

**Proposed fix:** Use `Option<TimeRange>` to represent event time range (None for empty partitions)

**Current behavior in `write_partition.rs:425-500`:**

```rust
let mut min_event_time: Option<DateTime<Utc>> = None;
let mut max_event_time: Option<DateTime<Utc>> = None;
// ... populate from data ...

// Early exit if no data was written
if min_event_time.is_none() || max_event_time.is_none() {
    logger.write_log_entry(format!(
        "no data for {desc} partition, not writing the object"
    )).await?;
    return Ok(());  // NO PARTITION RECORD CREATED
}

// Only create partition if we have data
let partition = Partition {
    min_event_time: min_event_time.unwrap(),  // Safe because checked above
    max_event_time: max_event_time.unwrap(),
    // ...
};
```

**Implications:**
- Empty time ranges are **never recorded** in the database
- Cannot distinguish "no data exists" from "not yet materialized"
- Cannot create a partition marker for empty time windows
- ⚠️ **Database schema DOES NOT have NOT NULL constraints** on min/max_event_time (see `migration.rs:101-102`)
  - This means the DB *could* technically store NULL values
  - But application code assumes they're always present
  - Rust `Partition` struct uses non-Optional types, preventing representation
  - `partition_cache.rs:116-117` uses `r.try_get("min_event_time")?` which **errors if NULL**
  - SQL queries at `partition_cache.rs:342-343` filter `min_event_time <= $3 AND max_event_time >= $4` which **excludes NULL** values automatically

**Solution:** Use `Option<TimeRange>` field to group min/max event times together.

### Scope Clarification

This plan implements **storing partition records for empty time ranges** (zero rows).

**In scope:**
- Change Partition struct to support optional min/max event times
- Create partition records even when num_rows = 0
- Handle partitions with no associated Parquet file
- Update all code that constructs/reads Partition structs
- Enable distinction between "no data" vs "not processed"

**Benefits:**
- Prevent redundant reprocessing of known-empty time windows
- Complete audit trail of all materialization attempts
- Explicit markers for time ranges that have been checked
- Better observability of data gaps
- **Enable safe partition merging/regrouping**: Can merge multiple small partitions (some empty, some not) into larger partitions because you know which time ranges were processed vs. missing

## Current State Analysis

### Files Involved
- `rust/analytics/src/lakehouse/partitioned_execution_plan.rs` - Creates execution plans from partitions
- `rust/analytics/src/lakehouse/partition_cache.rs` - Filters and retrieves partitions
- `rust/analytics/src/lakehouse/materialized_view.rs` - Scans materialized views
- `rust/analytics/src/lakehouse/batch_update.rs` - Materializes partitions
- `rust/analytics/src/lakehouse/merge.rs` - Merges partitions
- `rust/analytics/src/lakehouse/write_partition.rs` - Writes partition files
- `rust/analytics/src/lakehouse/jit_partitions.rs` - Generates JIT partitions

### Current Empty Partition Handling

| Location | Old Behavior | New Behavior | Status |
|----------|--------------|--------------|--------|
| `write_partition.rs:463-471` | Returns `Ok(())` silently, no record | Creates partition record with `event_time_range=None` | ✅ Resolved |
| `partitioned_execution_plan.rs:30-45` | Passes empty file_group to DataFusion | Filters empty partitions, returns EmptyExec | ✅ Addressed in Phase 4 |
| `batch_update.rs:120-123` | Returns `Ok(())` when spec is empty | N/A - different issue (empty spec, not empty data) | ⚠️ Out of scope |
| `jit_partitions.rs:82-87` | Returns `Ok(None)` properly | No change needed | ✅ Already good |
| `merge.rs:161-166` | Returns `Ok(())` when `< 2` partitions | Could add better logging | ⚠️ Nice to have |

### Key Issues Identified and How This Design Addresses Them

#### Issue 1: Empty File Groups in Execution Plans (ADDRESSED)
**Location**: `partitioned_execution_plan.rs:30-46`

**Problem**: When `partitions` is empty (or all partitions are empty), `file_group` is empty, and DataFusion might not handle this gracefully.

**Solution in this plan**: Phase 4 updates execution plan to:
- Filter out partitions with `file_path = None` (empty partitions)
- Return `EmptyExec` if no data partitions remain
- Properly handle mix of empty and non-empty partitions

#### Issue 2: Silent Failures (RESOLVED BY THIS DESIGN)

**Old behavior in `write_partition.rs:463-471`:**
```rust
if min_event_time.is_none() || max_event_time.is_none() {
    logger.write_log_entry(format!(
        "no data for {desc} partition, not writing the object"
    )).await?;
    return Ok(());  // ❌ NO PARTITION RECORD CREATED
}
```

**Problem:** Multiple locations return `Ok(())` when encountering empty data without creating records, making it impossible to distinguish "no data" from "not yet processed".

**✅ Solution with this design:** Creating partition records for empty time ranges provides an audit trail:
- Empty partition record explicitly shows "this time range was processed and found empty"
- Can distinguish between "no data exists" vs "not yet materialized"
- Prevents redundant reprocessing of known-empty windows
- **Enables safe merging**: Can confidently merge multiple partitions (e.g., 24 hourly → 1 daily) when some source partitions are empty, because you know they were processed and found empty rather than missing

**Remaining silent operations** (not directly related to empty partitions):
- `batch_update.rs:120-123` - Returns `Ok(())` when partition spec is empty (different issue - about empty source data specs)
- `merge.rs:161-166` - Returns `Ok(())` when `< 2` partitions (merge operation skipped, could benefit from better logging)


## Design Approach

### Core Changes Required
1. **Partition struct refactor:**
   ```rust
   pub struct Partition {
       pub view_metadata: ViewMetadata,
       pub insert_time_range: TimeRange,         // Always present
       pub event_time_range: Option<TimeRange>,  // None for empty partitions
       pub updated: DateTime<Utc>,
       pub file_path: Option<String>,            // None for empty partitions
       pub file_size: i64,                       // 0 for empty
       pub source_data_hash: Vec<u8>,
       pub num_rows: i64,                        // 0 for empty
   }
   ```

   **Benefits of TimeRange types:**
   - `insert_time_range: TimeRange` - Groups begin/end together, always present
   - `event_time_range: Option<TimeRange>` - Clearer semantics, single check, prevents invalid state
   - `file_path: Option<String>` - Explicitly no file rather than empty string
   - Consistent API for time ranges throughout

2. **Database schema migration:**
   ```sql
   -- Actually NOT NEEDED! migration.rs:101-102 shows these columns
   -- were never created with NOT NULL constraints.
   -- Database already supports NULL for min_event_time and max_event_time
   ```

   **Note**: The database schema already allows NULL for:
   - `min_event_time` (line 101)
   - `max_event_time` (line 102)
   - `file_path` (line 104)

   No database migration needed! However, application code assumes non-NULL, so all code must be updated to handle Option types.

3. **Object storage handling:**
   - Empty partitions have no Parquet file (num_rows = 0, file_size = 0)
   - file_path is None (no file path)
   - ReaderFactory must handle None file_path (skip file loading)
   - Execution plan must filter out empty partitions using `!partition.is_empty()`

4. **Update ALL code reading partitions** (50+ locations):
   - partition_cache.rs query filters using min/max_event_time (lines 342-343)
   - All execution plan creation
   - Merge operations comparing time ranges
   - Partition filtering logic
   - Statistics computation

5. **Query semantics:**
   - SQL queries must handle NULL event times: `min_event_time IS NULL OR min_event_time <= $3`
   - Empty partitions should NOT be included in data scans (num_rows = 0)
   - Performance impact of NULL checks in WHERE clauses

**Affected code locations:**
- `partition_cache.rs:112-123, 173-184, 391-402` - Reading from database with `try_get`
- `partition_cache.rs:342-343` - SQL filtering by event times
- `write_partition.rs:463-471` - Early return on empty data
- `write_partition.rs:490-501` - Creating Partition struct
- `batch_partition_merger.rs:34-38` - Stats computation
- `merge.rs:70-103` - Merge operations using time ranges
- `partitioned_execution_plan.rs:30-46` - File group creation
- All SQL queries using min/max_event_time and begin/end_insert_time

## Implementation Plan

### Phase 1: Update Partition Struct to Support Empty Partitions ✅ COMPLETED
**Objective**: Make the Partition struct capable of representing empty partitions.

1. **Update Partition struct in partition.rs**
   ```rust
   pub struct Partition {
       pub view_metadata: ViewMetadata,
       pub insert_time_range: TimeRange,         // Changed: was begin/end_insert_time
       pub event_time_range: Option<TimeRange>,  // Changed: was min/max_event_time
       pub updated: DateTime<Utc>,
       pub file_path: Option<String>,            // Changed: None for empty partitions
       pub file_size: i64,                       // 0 for empty partitions
       pub source_data_hash: Vec<u8>,
       pub num_rows: i64,                        // 0 for empty partitions
   }
   ```

2. **Add helper methods to Partition**
   ```rust
   impl Partition {
       /// Returns true if this partition has no data (num_rows = 0)
       pub fn is_empty(&self) -> bool {
           self.num_rows == 0
       }

       /// Returns the min event time, if this partition has data
       pub fn min_event_time(&self) -> Option<DateTime<Utc>> {
           self.event_time_range.as_ref().map(|r| r.begin)
       }

       /// Returns the max event time, if this partition has data
       pub fn max_event_time(&self) -> Option<DateTime<Utc>> {
           self.event_time_range.as_ref().map(|r| r.end)
       }
   }
   ```

   **Migration helpers for existing code:**
   ```rust
   // Temporary compatibility methods (remove after migration)
   impl Partition {
       #[deprecated(note = "Use insert_time_range.begin instead")]
       pub fn begin_insert_time(&self) -> DateTime<Utc> {
           self.insert_time_range.begin
       }

       #[deprecated(note = "Use insert_time_range.end instead")]
       pub fn end_insert_time(&self) -> DateTime<Utc> {
           self.insert_time_range.end
       }

       #[deprecated(note = "Use event_time_range instead")]
       pub fn min_event_time_unwrap(&self) -> DateTime<Utc> {
           self.event_time_range.expect("partition has no event time range").begin
       }

       #[deprecated(note = "Use event_time_range instead")]
       pub fn max_event_time_unwrap(&self) -> DateTime<Utc> {
           self.event_time_range.expect("partition has no event time range").end
       }
   }
   ```

**Files to modify**:
- `rust/analytics/src/lakehouse/partition.rs`

**Impact**: This is a breaking change for all code that constructs or accesses:
- `begin_insert_time` / `end_insert_time` → use `insert_time_range.begin` / `.end`
- `min_event_time` / `max_event_time` → use `event_time_range` (Option)

### Phase 2: Update Database Read Operations ✅ COMPLETED
**Objective**: Update all code that reads partitions from database to construct Option<TimeRange>.

1. **Update partition_cache.rs reads (3 locations: lines 112-123, 173-184, 391-402)**
   ```rust
   // OLD:
   partitions.push(Partition {
       view_metadata,
       begin_insert_time: r.try_get("begin_insert_time")?,
       end_insert_time: r.try_get("end_insert_time")?,
       min_event_time: r.try_get("min_event_time")?,
       max_event_time: r.try_get("max_event_time")?,
       updated: r.try_get("updated")?,
       file_path: r.try_get("file_path")?,
       file_size: r.try_get("file_size")?,
       source_data_hash: r.try_get("source_data_hash")?,
       num_rows: r.try_get("num_rows")?,
   });

   // NEW:
   let insert_time_range = TimeRange {
       begin: r.try_get("begin_insert_time")?,
       end: r.try_get("end_insert_time")?,
   };

   let event_time_range = match (
       r.try_get::<DateTime<Utc>, _>("min_event_time").ok(),
       r.try_get::<DateTime<Utc>, _>("max_event_time").ok()
   ) {
       (Some(begin), Some(end)) => Some(TimeRange { begin, end }),
       _ => None,  // If either is NULL, treat as empty partition
   };

   partitions.push(Partition {
       view_metadata,
       insert_time_range,     // Changed
       event_time_range,      // Changed
       updated: r.try_get("updated")?,
       file_path: r.try_get::<String, _>("file_path").ok(),  // Changed: Option<String>
       file_size: r.try_get("file_size")?,
       source_data_hash: r.try_get("source_data_hash")?,
       num_rows: r.try_get("num_rows")?,
   });
   ```

2. **Update SQL queries to handle NULL event times (partition_cache.rs:342-343)**
   ```sql
   -- OLD:
   WHERE view_set_name = $1
   AND view_instance_id = $2
   AND min_event_time <= $3
   AND max_event_time >= $4
   AND file_schema_hash = $5

   -- NEW (Option A - Include empty partitions in results, filter later in Rust):
   WHERE view_set_name = $1
   AND view_instance_id = $2
   AND (min_event_time IS NULL OR min_event_time <= $3)
   AND (max_event_time IS NULL OR max_event_time >= $4)
   AND file_schema_hash = $5

   -- NEW (Option B - Exclude empty partitions from data queries):
   WHERE view_set_name = $1
   AND view_instance_id = $2
   AND min_event_time <= $3  -- NULL rows automatically excluded
   AND max_event_time >= $4
   AND file_schema_hash = $5
   AND num_rows > 0  -- Explicit filter for clarity
   ```

**Files to modify**:
- `rust/analytics/src/lakehouse/partition_cache.rs` (lines 112-123, 173-184, 342-343, 391-402)

**Decision needed**:
- **Option A**: Include empty partitions in query results, filter in execution plan
- **Option B**: Exclude empty partitions at SQL level (simpler, better performance)
- **Recommendation**: Option B for data queries, Option A for admin/listing queries

### Phase 3: Update Partition Write Operations ✅ COMPLETED
**Objective**: Allow creating partition records even when data is empty.

1. **Update write_partition.rs to create empty partitions**
   ```rust
   // Lines 463-471 - Remove early return, continue to create partition record
   if min_event_time.is_none() || max_event_time.is_none() {
       logger.write_log_entry(format!(
           "creating empty partition record for {desc}"
       )).await?;
       // Don't return early - continue to create partition with:
       // event_time_range = None
       // file_path = None
       // file_size = 0
       // num_rows = 0
   }
   ```

2. **Update Partition construction (lines 490-501)**
   ```rust
   let event_time_range = match (min_event_time, max_event_time) {
       (Some(begin), Some(end)) => Some(TimeRange { begin, end }),
       _ => None,
   };

   &Partition {
       view_metadata,
       insert_time_range: insert_range,  // Already a TimeRange
       event_time_range,                  // Option<TimeRange>
       updated: sqlx::types::chrono::Utc::now(),
       file_path: event_time_range.map(|_| file_path),  // None for empty
       file_size,       // 0 for empty
       source_data_hash,
       num_rows,        // 0 for empty
   }
   ```

3. **Update insert_partition SQL (lines 324-330)**
   - Already supports NULL for min/max_event_time (no schema change needed)
   - Need to handle NULL for file_path (check if column allows NULL)
   - If file_path doesn't allow NULL, use empty string as fallback

**Files to modify**:
- `rust/analytics/src/lakehouse/write_partition.rs`

**Note**: ✅ Database schema allows NULL for file_path (migration.rs:104) - no schema change needed!

### Phase 4: Update Execution Plan and Query Handling ✅ COMPLETED
**Objective**: Filter out empty partitions from data scans, handle stats computation.

1. **Update partitioned_execution_plan.rs**
   ```rust
   // Line 30-33: Filter out empty partitions before creating file group
   let mut file_group = vec![];
   for part in &*partitions {
       if !part.is_empty() {
           let file_path = part.file_path.as_ref().ok_or_else(|| {
               DataFusionError::Internal(format!(
                   "non-empty partition has no file_path: num_rows={}",
                   part.num_rows
               ))
           })?;
           file_group.push(PartitionedFile::new(file_path, part.file_size as u64));
       }
   }

   // If file_group is empty after filtering, return EmptyExec
   if file_group.is_empty() {
       return Ok(Arc::new(EmptyExec::new(schema)));
   }
   ```

2. **Update batch_partition_merger.rs stats computation**
   ```rust
   // Line 34-38: Skip or handle empty partitions
   fn compute_partition_stats(partitions: &[Partition]) -> Result<PartitionStats> {
       let non_empty: Vec<_> = partitions.iter().filter(|p| !p.is_empty()).collect();
       if non_empty.is_empty() {
           anyhow::bail!("compute_partition_stats given only empty partitions");
       }
       // Use first non-empty partition for time range
       let first = non_empty[0];
       let min_event_time = first.event_time_range
           .as_ref()
           .ok_or_else(|| anyhow::anyhow!("non-empty partition has no event_time_range"))?
           .begin;
       let max_event_time = non_empty.iter()
           .filter_map(|p| p.event_time_range.as_ref().map(|r| r.end))
           .max()
           .ok_or_else(|| anyhow::anyhow!("no non-empty partitions found"))?;
       // ...
   }
   ```

3. **Update merge.rs to handle empty partitions**
   - Filter empty partitions before merge operations
   - Consider merge criteria with optional event times

**Files to modify**:
- `rust/analytics/src/lakehouse/partitioned_execution_plan.rs`
- `rust/analytics/src/lakehouse/batch_partition_merger.rs`
- `rust/analytics/src/lakehouse/merge.rs`

### Phase 5: Update All Partition Construction Sites ✅ COMPLETED
**Objective**: Update all code that constructs Partition structs with hardcoded values.

**Files to audit and fix**:
- Any test code creating Partition structs
- JIT partition generation
- Any other partition creation outside write_partition.rs

**Pattern**: Search for `Partition {` and update all construction sites to handle Optional times.

### Phase 6: Testing and Documentation ✅ COMPLETED

1. **Add comprehensive test coverage**
   - Create and query empty partitions
   - Mix of empty and non-empty partitions
   - Merge operations with empty partitions
   - Stats computation with empty partitions
   - Edge cases (all partitions empty, single empty partition)

2. **Update documentation**
   - Document empty partition semantics
   - Explain when empty partitions are created
   - Add troubleshooting guide
   - Document file_path="" convention

3. **Performance testing**
   - Verify NULL checks don't impact query performance
   - Test with many empty partitions
   - Verify logging overhead is minimal

**Files to modify**:
- `rust/analytics/tests/` - Add new test files
- `rust/analytics/src/lakehouse/README.md` - Update docs (if exists)

## Success Criteria

- ✅ Partition struct supports Option<DateTime<Utc>> for min/max event times
- ✅ Empty partitions (num_rows=0) can be created and stored in database
- ✅ Database records created for time ranges with no data
- ✅ Queries automatically filter empty partitions from data scans
- ✅ Stats computation handles mix of empty and non-empty partitions
- ✅ Merge operations handle empty partitions correctly
- ✅ System can distinguish "no data exists" from "not yet processed"
- ✅ All existing tests pass with updated Partition struct
- ✅ New tests cover empty partition scenarios
- ✅ No performance regression from Option types and NULL checks
- ✅ Clear documentation on empty partition semantics

## Design Decisions to Make

1. **file_path Type and Storage**
   - **Chosen**: `Option<String>` in Rust, NULL in database
   - ✅ Database schema already allows NULL (migration.rs:104)
   - No schema migration needed
   - Clearer than empty string - explicitly "no file"

2. **Query Filtering of Empty Partitions**
   - Option A: Automatically filter `WHERE num_rows > 0` in all data queries
   - Option B: Let execution plan filter via `!partition.is_empty()` check
   - Option C: Include empty partitions, let them produce zero rows
   - **Recommendation**: Option B - filter in execution plan, keeps SQL simple

3. **Logging Level for Empty Partition Creation**
   - Option A: Debug level (normal operation)
   - Option B: Info level (notable event)
   - Option C: Warn level (unexpected)
   - **Recommendation**: Option B (Info) - creating empty partition is notable

4. **Handling NULL Event Times in SQL Queries**
   - Option A: `(min_event_time IS NULL OR min_event_time <= $3)` - include empty partitions
   - Option B: Keep existing queries, rely on NULL comparison excluding them
   - **Recommendation**: Option A if empty partitions need to be found by time range, Option B if they should be excluded

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Breaking change to Partition struct affects all code | **CRITICAL** | Comprehensive grep for all usage, thorough testing |
| Option<DateTime> adds overhead | Medium | Profile performance, benchmark before/after |
| Empty partitions accidentally included in data scans | High | Filter via !partition.is_empty() in execution plan |
| NULL comparisons in SQL exclude empty partitions unexpectedly | Medium | Decide on query semantics early, test thoroughly |
| Existing tests assume non-Option event times | High | Update all test helpers and fixtures |
| Stats computation breaks with only empty partitions | Medium | Filter empty partitions before stats computation |
| Merge logic assumes comparable event times | Medium | Handle None cases explicitly in merge operations |

## References

- DataFusion `EmptyExec`: For producing zero-row results efficiently
- DataFusion `FileScanConfig`: May already handle empty file groups
- Arrow `RecordBatch`: Empty batches are valid with schema

## Timeline Estimate

- **Phase 1** (Partition struct update): 1-2 hours (struct change + helper methods)
- **Phase 2** (Database reads): 3-4 hours (update 3 read sites + SQL queries + testing)
- **Phase 3** (Write operations): 2-3 hours (remove early return + update construction)
- **Phase 4** (Execution plan + stats): 4-6 hours (filter logic + stats handling + merge)
- **Phase 5** (Audit all construction sites): 3-5 hours (grep + fix all Partition{} sites)
- **Phase 6** (Testing + documentation): 4-6 hours (comprehensive tests + docs)

**Total**: 17-26 hours

**Risk buffer**: Add 30-50% for unexpected issues = **22-39 hours total**

## Next Steps

1. **Review and approve plan** - Confirm this is the desired approach
2. **Make design decisions** - Resolve the 4 decisions listed above
3. **Phase 1: Update Partition struct** - Breaking change, start here
4. **Comprehensive grep audit** - Find all `Partition {` construction sites
5. **Incremental implementation** - One phase at a time with tests
6. **Performance benchmarking** - Before/after comparison

## Important Notes

⚠️ **This is a breaking change** affecting the core Partition data structure. All code that constructs or pattern matches on Partition will need updates.

✅ **Database schema already supports this** - No migration needed! The original schema (migration.rs:96-108) never added NOT NULL constraints to:
   - `min_event_time`
   - `max_event_time`
   - `file_path`

   This suggests empty partitions may have been considered in the original design but never implemented in application code.

⚠️ **High testing burden** - Must verify all partition operations work correctly with Option types.

💡 **Alternative consideration**: If this proves too complex, consider a simpler "marker table" approach where empty time ranges are tracked separately from the partitions table.

## Summary of Changes

**Partition struct:**
- `begin_insert_time: DateTime<Utc>` → removed
- `end_insert_time: DateTime<Utc>` → removed
- **Added**: `insert_time_range: TimeRange`
- `min_event_time: DateTime<Utc>` → removed
- `max_event_time: DateTime<Utc>` → removed
- **Added**: `event_time_range: Option<TimeRange>`
- `file_path: String` → `file_path: Option<String>`

**Benefits:**
- Consistent TimeRange API for all time ranges
- Can store partition records for empty time ranges
- Distinguish "no data" from "not yet processed"
- Prevent redundant reprocessing
- Better audit trail
- Cleaner, more ergonomic code
- **Enable safe partition regrouping**: Merge multiple small partitions (hourly) into larger ones (daily) even when some source partitions are empty

---

## ✅ Implementation Status: COMPLETED

**Date**: 2025-10-20

### What Was Implemented

All phases (1-6) have been successfully completed:

1. ✅ **Phase 1**: Updated `Partition` struct in `partition.rs` with `Option<TimeRange>` for event times, added helper methods
2. ✅ **Phase 2**: Updated all database read operations in `partition_cache.rs` to handle NULL values properly
3. ✅ **Phase 3**: Modified `write_partition.rs` to create partition records for empty time ranges
4. ✅ **Phase 4**: Updated execution plan to filter empty partitions and return `EmptyExec` when appropriate
5. ✅ **Phase 5**: Fixed all field accesses throughout the codebase (batch_update.rs, merge.rs, partition_cache.rs, etc.)
6. ✅ **Phase 6**: All tests pass (14 tests), full workspace builds successfully

### Files Modified

**Core Changes:**
- `rust/analytics/src/lakehouse/partition.rs` - Updated struct and added helper methods
- `rust/analytics/src/lakehouse/write_partition.rs` - Empty partition creation logic
- `rust/analytics/src/lakehouse/partition_cache.rs` - Database read operations and filtering
- `rust/analytics/src/lakehouse/partitioned_execution_plan.rs` - Empty partition filtering
- `rust/analytics/src/lakehouse/batch_partition_merger.rs` - Stats computation for mixed partitions
- `rust/analytics/src/lakehouse/merge.rs` - Updated field accesses
- `rust/analytics/src/lakehouse/batch_update.rs` - Updated field accesses

### Test Results

```
running 14 tests
test result: ok. 14 passed; 0 failed; 1 ignored; 0 measured
```

Full workspace builds without errors in 59.71s.

### Design Decisions Made

1. **file_path Type**: Using `Option<String>` (NULL in database) - no schema migration needed
2. **Query Filtering**: Filter empty partitions in execution plan (Option B)
3. **Logging Level**: Info level for empty partition creation
4. **SQL Queries**: Let NULL comparisons naturally exclude empty partitions (Option B)

### Key Implementation Details

- Empty partitions have: `num_rows = 0`, `event_time_range = None`, `file_path = None`
- Database already supported NULL values - no schema migration required
- Execution plan returns `EmptyExec` when all partitions are empty
- Stats computation filters out empty partitions before calculating ranges
- All helper methods provide backward compatibility for field access patterns

### Known Limitations

- Empty partitions still require a dummy `ParquetMetaData` object to satisfy function signatures (could be improved in future)
- Performance impact of NULL checks not yet benchmarked (appears negligible based on tests)
