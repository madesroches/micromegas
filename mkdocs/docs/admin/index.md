# Administrative Functions

The Micromegas admin module provides powerful tools for managing partitions and schemas in your data lake. These functions are designed for system administrators and advanced users who need to maintain data consistency and optimize storage usage.

!!! warning "Administrative Functions"
    The functions in this module can permanently delete data. Always preview operations using the list functions before executing retirement operations.

## Overview

The admin module includes:

- **Schema Discovery**: View current schema versions across all view sets
- **Partition Analysis**: Identify partitions with incompatible schemas
- **Safe Retirement**: Remove partitions that cannot be queried due to schema mismatches
- **Bulk Retirement**: Remove non-global partitions for decommissioned processes or maintenance

## Quick Start

```python
import micromegas
import micromegas.admin

# Connect to analytics service
client = micromegas.connect()

# View current schema versions
schemas = client.query("SELECT * FROM list_view_sets()")
print(schemas)

# Find incompatible partitions
incompatible = micromegas.admin.list_incompatible_partitions(client)
print(f"Found {incompatible['partition_count'].sum()} incompatible partitions")

# Preview what would be retired
print("Partitions to be retired:")
print(incompatible[['view_set_name', 'view_instance_id', 'partition_count']])

# Retire incompatible partitions (after careful review)
# result = micromegas.admin.retire_incompatible_partitions(client)
```

## Key Concepts

### Schema Evolution

As your application evolves, the schema of telemetry data may change. New fields are added, existing fields might change types, or data structures could be reorganized. Micromegas tracks these changes using schema versions.

### Incompatible Partitions

When schemas evolve, older partitions may become incompatible with the current schema version. These partitions:

- Are ignored during queries (do not cause failures)
- Take up storage space unnecessarily
- Should be retired to free storage and maintain clean data lake hygiene

### Safe Retirement

The admin module provides surgical retirement capabilities that:

- Target specific incompatible partitions by file path
- Cannot accidentally delete compatible data
- Include comprehensive error handling and rollback safety
- Provide detailed operation logs for auditing

## Safety Guidelines

### Always Preview First

```python
# ALWAYS do this first
incompatible = micromegas.admin.list_incompatible_partitions(client, 'log_entries')
print(f"Will retire {incompatible['partition_count'].sum()} partitions")
print(f"Total size: {incompatible['total_size_bytes'].sum() / (1024**3):.2f} GB")

# Review the specific partitions
print(incompatible[['view_set_name', 'view_instance_id', 'incompatible_schema_hash', 'partition_count']])

# Only then proceed with retirement
result = micromegas.admin.retire_incompatible_partitions(client, 'log_entries')
```

### Start with Specific View Sets

```python
# RECOMMENDED: Start with a specific view set
result = micromegas.admin.retire_incompatible_partitions(client, 'log_entries')

# CAUTION: Only use this after testing with specific view sets
# result = micromegas.admin.retire_incompatible_partitions(client)  # All view sets
```

### Verify Results

```python
# Check what was actually retired
result = micromegas.admin.retire_incompatible_partitions(client, 'log_entries')
print(f"Successfully retired: {result['partitions_retired'].sum()} partitions")
print(f"Storage freed: {result['storage_freed_bytes'].sum() / (1024**3):.2f} GB")

# Check for any errors
for _, row in result.iterrows():
    if row['partitions_retired'] == 0:
        print(f"Warning: No partitions retired for {row['view_set_name']}")
```

## Common Workflows

### Schema Maintenance Workflow

```python
import micromegas
import micromegas.admin

client = micromegas.connect()

# 1. Check current schema versions
print("=== Current Schema Versions ===")
schemas = client.query("SELECT * FROM list_view_sets()")
for _, schema in schemas.iterrows():
    print(f"{schema['view_set_name']}: {schema['current_schema_hash']}")

# 2. Find incompatible partitions across all view sets
print("\n=== Incompatible Partitions Summary ===")
all_incompatible = micromegas.admin.list_incompatible_partitions(client)
if len(all_incompatible) == 0:
    print("No incompatible partitions found")
else:
    for view_set in all_incompatible['view_set_name'].unique():
        vs_data = all_incompatible[all_incompatible['view_set_name'] == view_set]
        total_partitions = vs_data['partition_count'].sum()
        total_size_gb = vs_data['total_size_bytes'].sum() / (1024**3)
        print(f"{view_set}: {total_partitions} partitions ({total_size_gb:.2f} GB)")

# 3. Process each view set individually for safety
for view_set in all_incompatible['view_set_name'].unique():
    print(f"\n=== Processing {view_set} ===")
    
    # Preview
    vs_incompatible = micromegas.admin.list_incompatible_partitions(client, view_set)
    print(f"Found {vs_incompatible['partition_count'].sum()} incompatible partitions")
    
    # Confirm before proceeding
    confirm = input(f"Retire incompatible partitions for {view_set}? (yes/no): ")
    if confirm.lower() == 'yes':
        result = micromegas.admin.retire_incompatible_partitions(client, view_set)
        retired_count = result['partitions_retired'].sum()
        freed_gb = result['storage_freed_bytes'].sum() / (1024**3)
        print(f"Retired {retired_count} partitions, freed {freed_gb:.2f} GB")
    else:
        print(f"Skipped {view_set}")
```

### Bulk Retirement of Non-Global Partitions

Remove partitions for decommissioned processes:

```python
import micromegas

client = micromegas.connect()

# List non-global partitions for a specific process
process_partitions = client.query("""
    SELECT view_set_name, view_instance_id, COUNT(*) as partition_count, SUM(file_size) as total_size
    FROM list_partitions() 
    WHERE view_instance_id = 'process-123'
    GROUP BY view_set_name, view_instance_id
""")

# Retire all partitions for the process
for _, row in process_partitions.iterrows():
    client.query(f"""
        SELECT * FROM retire_partitions(
            '{row['view_set_name']}', 
            '{row['view_instance_id']}',
            '1970-01-01T00:00:00Z',
            '2099-12-31T23:59:59Z'
        )
    """)
```

