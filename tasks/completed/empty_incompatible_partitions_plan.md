# Plan: Handle Empty Incompatible Partitions

## Problem Statement

Empty partitions (partitions with `num_rows = 0`) currently cannot be retired by `retire_incompatible_partitions()` in the Python admin API because:

1. **Empty partitions have no file_path**: According to the partition invariants in `partition.rs`, empty partitions (num_rows = 0) MUST NOT have `file_path` or `event_time_range`. This is by design.

2. **Current retirement logic requires file_path**: The `retire_incompatible_partitions()` function in Python skips partitions without file paths:
   ```python
   for file_path in file_paths_list:
       if not file_path or pd.isna(file_path):
           continue  # Skips empty partitions!
   ```

3. **Result**: Empty incompatible partitions accumulate in the database, taking up metadata space and preventing clean schema evolution.

## Root Cause Analysis

### Why Empty Partitions Exist

Empty partitions are created when:
- A view set is registered/updated but no telemetry has been ingested yet for a time range
- Data is ingested but produces zero rows after filtering/transformation
- Partitions are explicitly created to mark schema boundaries

See `rust/analytics/src/lakehouse/write_partition.rs` lines 211-225 where empty partitions are created with `file_path: None`.

### Current Retirement Mechanisms

1. **retire_partitions() table function** (Rust): Retires by time range - works for both empty and non-empty partitions
   - Location: `rust/analytics/src/lakehouse/retire_partitions_table_function.rs`
   - Uses: `retire_partitions()` in `write_partition.rs` lines 115-179

2. **retire_partition_by_file() UDF** (Rust): Retires by exact file path - cannot handle empty partitions
   - Location: `rust/analytics/src/lakehouse/retire_partition_by_file_udf.rs`
   - Returns ERROR on null file_path (lines 158-161)

3. **retire_incompatible_partitions()** (Python): Uses retire_partition_by_file() - skips empty partitions
   - Location: `python/micromegas/micromegas/admin.py` lines 84-227
   - Skips null/empty file paths (lines 174-175)

### The Mismatch

- `list_incompatible_partitions()` correctly identifies ALL incompatible partitions (including empty ones)
- `retire_incompatible_partitions()` only retires non-empty partitions (those with file_path)
- This creates a disconnect: partitions are listed but cannot be retired

## Solution

Create a new `retire_partition_by_metadata()` UDF that:
1. Identifies partitions by their metadata (view_set_name, view_instance_id, insert time range)
2. Handles both empty (file_path=NULL) and non-empty partitions
3. Performs file cleanup when file_path exists
4. Becomes the primary retirement method, deprecating retire_partition_by_file()

## Implementation Plan

### Phase 1: Create retire_partition_by_metadata() UDF (Rust)

**File**: `rust/analytics/src/lakehouse/retire_partition_by_metadata_udf.rs` (new file)

**Function signature**: 
```rust
retire_partition_by_metadata(
    view_set_name: String,
    view_instance_id: String, 
    begin_insert_time: Timestamp,
    end_insert_time: Timestamp
) -> String
```

**Implementation details**:
1. Query lakehouse_partitions for exact match on all four metadata fields
2. Verify the partition exists
3. If file_path is NOT NULL, add file to temporary_files for cleanup
4. Delete the partition metadata
5. Return success/error message similar to retire_partition_by_file()

**Key invariants to maintain**:
- Use exact match on metadata to avoid accidental deletions
- Handle file cleanup if file_path exists (same as retire_partition_by_file)
- Single transaction for atomicity
- Works for both empty (file_path=NULL) and non-empty partitions

**Safety considerations**:
- Require exact match on all four metadata fields (no wildcards)
- Use same file cleanup mechanism as retire_partition_by_file (add_file_for_cleanup)
- Return descriptive error messages for debugging

### Phase 2: Update Python retire_incompatible_partitions()

**File**: `python/micromegas/micromegas/admin.py`

**Changes needed**:
1. Modify the loop in `retire_incompatible_partitions()` to handle both empty and non-empty partitions
2. For each incompatible partition group:
   - If file_paths contain valid paths → use `retire_partition_by_file()`
   - If file_paths are NULL/empty → use `retire_partition_by_metadata()`
3. Track retirement results separately for clarity in messages

**Implementation approach**:

Since retire_partition_by_metadata() can handle both empty and non-empty partitions, we should use metadata-based retirement exclusively and deprecate retire_partition_by_file().

**Advantages of metadata-based retirement over file-based:**
1. Works for both empty (file_path=NULL) and non-empty partitions
2. More semantically correct - identifies partition by its primary key (metadata) not secondary attribute (file_path)
3. Simpler Python API - single retirement code path
4. More robust - file_path could theoretically change (e.g., file moved), but metadata is immutable

```python
# For each incompatible partition, use metadata-based retirement
# This works for both empty and non-empty partitions
for _, partition in incompatible.iterrows():
    retire_partition_by_metadata(
        partition['view_set_name'],
        partition['view_instance_id'], 
        partition['begin_insert_time'],
        partition['end_insert_time']
    )
```

**Deprecation of retire_partition_by_file()**:
- Mark retire_partition_by_file() UDF as deprecated (add deprecation notice to SQL function doc comment)
- Update Python code to use retire_partition_by_metadata() exclusively
- Keep retire_partition_by_file() UDF functional for backward compatibility (don't remove it yet)
- Add note in documentation recommending retire_partition_by_metadata() for new code

**Edge cases to handle**:
- Partitions with duplicate metadata (shouldn't happen - metadata should be unique)
- Transaction rollback if any retirement fails
- No partitions to retire (existing logic handles this)

### Phase 3: Update list_incompatible_partitions() Query

**File**: `python/micromegas/micromegas/admin.py`

**Consideration**: The current query aggregates by (view_set_name, view_instance_id, schema_hash). For metadata-based retirement, we need the exact insert time ranges for each partition.

**Options**:
1. **No aggregation** - Return one row per partition with full metadata
   - Simpler query, easier to understand
   - More rows but still manageable (one per partition)
   - Allows using metadata-based retirement for everything
   
2. **Keep aggregation** - Stay with current approach
   - Need to return ARRAY_AGG of (begin_insert_time, end_insert_time) tuples
   - More complex to iterate over in Python
   - Requires handling both file_paths and insert_time_ranges arrays

**Recommendation**: **Remove aggregation** - Query returns one row per incompatible partition with all metadata fields. This makes the Python code simpler and works naturally with metadata-based retirement.

**Updated query structure**:
```sql
SELECT 
    p.view_set_name,
    p.view_instance_id,
    p.begin_insert_time,
    p.end_insert_time,
    p.file_path,
    p.file_size,
    p.file_schema_hash as incompatible_schema_hash,
    vs.current_schema_hash,
    p.num_rows
FROM list_partitions() p
JOIN list_view_sets() vs ON p.view_set_name = vs.view_set_name
WHERE p.file_schema_hash != vs.current_schema_hash
ORDER BY p.view_set_name, p.view_instance_id, p.begin_insert_time
```

This gives us everything we need to retire each partition individually by metadata.

### Phase 4: Register New UDF and Deprecate Old One

**File**: `rust/analytics/src/lakehouse/query.rs`

Add registration in `register_lakehouse_functions()`:
```rust
ctx.register_udf(make_retire_partition_by_metadata_udf(lake.clone()).into_scalar_udf());
```

**File**: `rust/analytics/src/lakehouse/mod.rs`

Add module export:
```rust
pub mod retire_partition_by_metadata_udf;
```

**File**: `rust/analytics/src/lakehouse/retire_partition_by_file_udf.rs`

Add deprecation notice to module doc comment:
```rust
/// A scalar UDF that retires a single partition by its file path.
///
/// **DEPRECATED**: Use `retire_partition_by_metadata()` instead, which can handle
/// both empty partitions (file_path=NULL) and non-empty partitions, and identifies
/// partitions by their primary key (metadata) rather than file path.
///
/// This function remains available for backward compatibility but will be removed
/// in a future version.
```

### Phase 5: Testing

**Unit tests needed**:

1. **Rust UDF tests** (`rust/analytics/tests/retire_partition_by_metadata_test.rs`):
   - Retire single empty partition successfully (file_path=NULL, no file cleanup)
   - Retire single non-empty partition successfully (file_path!=NULL, file added to cleanup)
   - Handle partition not found error
   - Verify file cleanup is called when file_path is not NULL
   - Verify no file cleanup when file_path is NULL
   - Handle database transaction rollback on errors

2. **Python admin tests** (`python/micromegas/tests/test_admin.py`):
   - Update MockFlightSQLClient to handle retire_partition_by_metadata() calls
   - Update existing tests to use metadata-based retirement
   - Add test for retiring empty incompatible partitions
   - Add test for mixed empty/non-empty incompatible partitions
   - Verify retirement messages work with new approach
   - Remove or update tests that specifically test retire_partition_by_file() mocking

3. **Integration tests**:
   - Create view set, ingest data with schema v1
   - Update schema to v2 (creates empty partitions with new schema)
   - Verify list_incompatible_partitions shows empty v1 partitions
   - Retire incompatible partitions and verify empty ones are removed
   - Verify non-empty partitions can still coexist and be retired separately

## Migration Considerations

**Database schema**: No changes needed - using existing lakehouse_partitions table.

**Backward compatibility**: 
- New UDF is additive - existing code continues to work
- Old Python admin.py without changes will still work (just skip empty partitions as before)
- Users can upgrade Rust analytics service independently of Python client

**Documentation updates needed**:
1. Update docstrings in `admin.py` to mention that all partitions (empty and non-empty) are now retired
2. Add deprecation notice for retire_partition_by_file() in Rust doc comments
3. Update function docstring for `list_incompatible_partitions()` to clarify return value changes (no aggregation)
4. Add example showing new metadata-based retirement approach
5. **Update `mkdocs/docs/query-guide/functions-reference.md`**:
   - Add deprecation notice to retire_partition_by_file() section
   - Add new section for retire_partition_by_metadata()
6. **Update `mkdocs/docs/admin/functions-reference.md`**:
   - Add deprecation notice to retire_partition_by_file() section
   - Document that Python admin API now uses metadata-based retirement
   - Add migration guide for users calling retire_partition_by_file() directly

## Alternative Approaches Considered

### Alternative 1: Use retire_partitions() Table Function
Use the existing time-range-based retirement instead of creating new UDF.

**Rejected because**:
- Less precise - retires by time range, not by exact partition
- Harder to provide granular feedback on which partitions were retired
- Existing Python API is designed around partition-by-partition retirement

### Alternative 2: Allow retire_partition_by_file() to Accept NULL
Modify existing UDF to handle NULL file_path by falling back to metadata.

**Rejected because**:
- Violates single responsibility principle
- Makes the UDF interface confusing (sometimes takes file_path, sometimes metadata?)
- Harder to reason about which identification method is being used

### Alternative 3: Store Synthetic File Path for Empty Partitions
Store a sentinel value like "empty://partition/..." in file_path for empty partitions.

**Rejected because**:
- Violates the established partition invariants
- Would require extensive refactoring of partition creation logic
- Creates confusion about what file_path means
- Still requires special handling in file cleanup logic

## Success Criteria

1. **Functional**: `retire_incompatible_partitions()` successfully retires both empty and non-empty incompatible partitions
2. **Safe**: No accidental deletion of compatible partitions or wrong partitions
3. **Clear**: Return messages distinguish between empty and non-empty partition retirement
4. **Complete**: All incompatible partitions (empty and non-empty) are retired when function is called
5. **Tested**: Full test coverage for both Rust and Python layers
6. **Documented**: Clear documentation of behavior and examples

## Rollout Plan

1. **Phase 1**: Implement and test Rust UDF locally
2. **Phase 2**: Update Python admin API and add tests
3. **Phase 3**: Run integration tests against local test environment
4. **Phase 4**: Update documentation with examples
5. **Phase 5**: Deploy to staging and verify with real data
6. **Phase 6**: Deploy to production

## Risk Assessment

**Low risk** because:
- Additive change - doesn't modify existing behavior
- Empty partitions have no files to clean up (simpler than file-based retirement)
- Exact metadata matching prevents accidental deletions
- Existing partition validation ensures data integrity
- Can be rolled back by simply not calling the new UDF

**Potential issues**:
- Need to ensure metadata matching is exact enough (all four fields: view_set_name, view_instance_id, begin_insert_time, end_insert_time)
- Insert time precision might cause matching issues (nanosecond timestamps) - use exact equality, not ranges
- File cleanup logic must match retire_partition_by_file() behavior exactly

## Decisions Made

1. **Replace retire_partition_by_file() usage in Python completely**
   - Use retire_partition_by_metadata() exclusively in admin.py
   - Simpler Python code with single retirement code path
   - More semantically correct (retire by primary key, not file path)
   
2. **Deprecate but don't remove retire_partition_by_file()**
   - Keep UDF functional for backward compatibility
   - Add deprecation notice to documentation
   - Can be removed in future major version

3. **No aggregation in list_incompatible_partitions()**
   - Return one row per partition with full metadata
   - Simpler Python iteration
   - Makes metadata-based retirement natural
