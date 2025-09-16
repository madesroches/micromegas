# Admin Feature: Incompatible Partition Retirement for Schema Evolution

## Overview

This document outlines the completed implementation of admin functionality for managing incompatible partitions during schema evolution. The feature enables safe cleanup of old schema versions while maintaining data integrity.

## Key Concepts

**Incompatible Partitions**: Partitions with schema versions different from the current schema version for their view set. These partitions are ignored during queries but consume storage space unnecessarily.

**Schema Hash**: Version identifier (stored as arrays like `[4]`) that uniquely identifies the schema version used when a partition was created. Found in `lakehouse_partitions.file_schema_hash` column.

**Retirement**: Process of removing partition metadata from the database and deleting associated data files from object storage. This operation is irreversible.

## Implementation Summary

### Phase 1: Schema Discovery ✅
- **`list_view_sets()` UDTF**: Provides current schema versions across all view sets
- **Catalog infrastructure**: Added `ViewMaker` trait methods for schema access
- **Unit tests**: Comprehensive validation of schema consistency

### Phase 2: Partition Analysis ✅  
- **Enhanced `list_partitions()`**: Leveraged existing partition discovery functionality
- **Test coverage**: Unit tests for partition discovery workflows

### Phase 3: Incompatible Partition Detection ✅
- **`micromegas.admin` module**: Python API for administrative functions
- **`list_incompatible_partitions()`**: Server-side SQL JOIN for optimal performance
- **Returns**: Partition counts, sizes, and file paths for precise targeting

### Phase 4: Safe Retirement Implementation ✅
- **`retire_partition_by_file()` UDF**: AsyncScalarUDF for targeted partition removal
- **File-path-based retirement**: Ensures only exact incompatible partitions are retired
- **`retire_incompatible_partitions()`**: Python orchestration with comprehensive error handling
- **Zero risk**: Cannot accidentally retire compatible partitions

### Phase 5: Documentation ✅
- **Admin functions reference**: Complete mkdocs documentation
- **Python API documentation**: Integration with existing Python guide
- **Safety guidelines**: Best practices and usage examples

## Technical Architecture

### Core Functions

**SQL Functions (UDTFs/UDFs)**:
- `list_view_sets()` - Schema discovery
- `list_partitions()` - Partition enumeration (existing)
- `retire_partition_by_file(file_path)` - Targeted retirement

**Python API** (`micromegas.admin`):
- `list_incompatible_partitions(client, view_set_name=None)` - Detection
- `retire_incompatible_partitions(client, view_set_name=None)` - Retirement

### Safety Features

✅ **File-path precision**: Only exact incompatible partitions are targeted  
✅ **Preview functionality**: Always list incompatible partitions before retirement  
✅ **Comprehensive error handling**: Detailed messages for each operation  
✅ **Transaction safety**: Full rollback protection for failed operations  
✅ **Optional filtering**: View-set-specific operations prevent bulk mistakes

## Usage Examples

### Basic Workflow
```python
import micromegas
import micromegas.admin

client = micromegas.connect()

# 1. Discover schema versions
schemas = client.query("SELECT * FROM list_view_sets()")

# 2. Find incompatible partitions
incompatible = micromegas.admin.list_incompatible_partitions(client, 'log_entries')
print(f"Found {incompatible['partition_count'].sum()} incompatible partitions")

# 3. Preview storage impact
total_size_gb = incompatible['total_size_bytes'].sum() / (1024**3)
print(f"Would free {total_size_gb:.2f} GB")

# 4. Retire incompatible partitions
result = micromegas.admin.retire_incompatible_partitions(client, 'log_entries')
print(f"Retired {result['partitions_retired'].sum()} partitions")
```

### Bulk Retirement Workflow
```python
# Process all view sets individually for safety
all_incompatible = micromegas.admin.list_incompatible_partitions(client)
for view_set in all_incompatible['view_set_name'].unique():
    print(f"Processing {view_set}...")
    result = micromegas.admin.retire_incompatible_partitions(client, view_set)
    print(f"Retired {result['partitions_retired'].sum()} partitions")
```

## Key Design Decisions

### Python API Over Additional Rust UDTFs
- **Faster development**: Leverages existing `list_partitions()` and `list_view_sets()` infrastructure
- **More intuitive**: Familiar Python API for administrative scripts
- **Easier to extend**: Additional logic and safety features in Python
- **Better error handling**: Rich exception handling and user feedback

### File-Path-Based Retirement
- **Precision targeting**: Each partition retired individually by exact file path
- **Safety**: Impossible to accidentally retire compatible partitions
- **Auditability**: Clear one-to-one mapping of file path to retirement action
- **Error resilience**: Continues processing even if individual partitions fail

### Server-Side Processing
- **Performance**: SQL JOIN and aggregation performed efficiently by DataFusion
- **Network efficiency**: Minimal data transfer with aggregated results
- **Scalability**: Handles large partition counts without client-side memory issues

## Benefits

1. **Storage optimization**: Remove partitions that consume space but are never queried
2. **Schema evolution enablement**: Clean up old schema versions safely
3. **Operational efficiency**: Automate identification and cleanup workflows
4. **Risk mitigation**: Targeted retirement eliminates accidental data loss
5. **Comprehensive tooling**: Both SQL and Python interfaces for different use cases

## Future Considerations

- **Automated scheduling**: Consider cron-based cleanup for regular maintenance
- **Metrics integration**: Track storage freed and partition retirement statistics
- **Backup integration**: Optional backup before retirement for extra safety
- **Performance monitoring**: Track query performance improvements after cleanup

## Files Modified/Added

### Rust Implementation
- `rust/analytics/src/lakehouse/catalog.rs` - Schema discovery
- `rust/analytics/src/lakehouse/list_view_sets_table_function.rs` - UDTF implementation
- `rust/analytics/src/lakehouse/retire_partition_by_file_udf.rs` - Retirement UDF
- `rust/analytics/src/lakehouse/view_factory.rs` - ViewMaker trait enhancements
- `rust/analytics/tests/catalog_tests.rs` - Unit test coverage

### Python Implementation  
- `python/micromegas/micromegas/admin.py` - Admin module
- `python/micromegas/tests/test_admin.py` - Admin API tests

### Documentation
- `mkdocs/docs/admin/functions-reference.md` - Complete admin reference
- `mkdocs/docs/query-guide/python-api.md` - Python API integration
- `mkdocs/docs/query-guide/functions-reference.md` - SQL function docs

This implementation provides a solid foundation for schema evolution management in Micromegas lakehouse environments.