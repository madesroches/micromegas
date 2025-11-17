# Administrative Functions Reference

This page provides detailed reference documentation for all administrative functions available in the Micromegas admin module.

## Table Functions (UDTFs)

### `list_view_sets()`

**Description**: Lists all available view sets with their current schema information.

**Usage**:
```sql
SELECT * FROM list_view_sets();
```

**Returns**: Table with columns:

| Column | Type | Description |
|--------|------|-------------|
| `view_set_name` | String | Name of the view set (e.g., "log_entries", "measures") |
| `current_schema_hash` | Binary | Version identifier for current schema (e.g., `[4]`) |
| `schema` | String | Full schema as formatted string |
| `has_view_maker` | Boolean | Whether view set supports non-global instances |
| `global_instance_available` | Boolean | Whether a global instance exists |

**Example**:
```sql
-- List all view sets with schema versions
SELECT view_set_name, current_schema_hash, has_view_maker 
FROM list_view_sets()
ORDER BY view_set_name;

-- Find view sets with specific schema version
SELECT * FROM list_view_sets() 
WHERE current_schema_hash = '[4]';
```

**Performance**: Fast operation, queries in-memory view registry.

---

### `list_partitions()`

**Description**: Lists all partitions in the lakehouse with metadata.

**Usage**:
```sql
SELECT * FROM list_partitions();
```

**Returns**: Table with columns including:

| Column | Type | Description |
|--------|------|-------------|
| `view_set_name` | String | View set name |
| `view_instance_id` | String | Instance ID or 'global' |
| `begin_insert_time` | Timestamp | Partition start time |
| `end_insert_time` | Timestamp | Partition end time |
| `min_event_time` | Timestamp | Earliest event time in partition |
| `max_event_time` | Timestamp | Latest event time in partition |
| `updated` | Timestamp | Last update time |
| `file_path` | String | Object storage file path |
| `file_size` | Integer | File size in bytes |
| `file_schema_hash` | Binary | Schema version when partition was created |
| `source_data_hash` | Binary | Hash of the source data |
| `num_rows` | Integer | Number of rows in the partition |

**Example**:
```sql
-- List partitions for specific view set
SELECT file_path, file_size, file_schema_hash 
FROM list_partitions() 
WHERE view_set_name = 'log_entries';

-- Find partitions by schema version
SELECT view_set_name, COUNT(*) as partition_count
FROM list_partitions()
WHERE file_schema_hash = '[3]'
GROUP BY view_set_name;
```

**Performance**: Queries database metadata table, indexed by view_set_name.

---

### `retire_partitions(view_set, view_instance, start_time, end_time)`

**Description**: Retires partitions within a specified time range.

**Parameters**:
- `view_set` (String): Target view set name
- `view_instance` (String): Target view instance ID  
- `start_time` (Timestamp): Start of time range (inclusive)
- `end_time` (Timestamp): End of time range (inclusive)

**Usage**:
```sql
SELECT * FROM retire_partitions('log_entries', 'process-123', '2024-01-01T00:00:00Z', '2024-01-02T00:00:00Z');
```

**Returns**: Table with retirement operation results.

**Safety**: Uses database transactions for atomicity. All partitions in time range are retired.

!!! warning "Time-Based Retirement"
    This function retires ALL partitions in the specified time range, regardless of schema compatibility. Use with caution.

---

## Scalar Functions (UDFs)

### `retire_partition_by_file(file_path)`

**Description**: Surgically retires a single partition by exact file path match.

**Parameters**:
- `file_path` (String): Exact file path of partition to retire

**Usage**:
```sql
SELECT retire_partition_by_file('s3://bucket/data/log_entries/process-123/2024/01/01/file.parquet') as result;
```

**Returns**: String message indicating success or failure:
- Success: `"SUCCESS: Retired partition <file_path>"`
- Failure: `"ERROR: Partition not found: <file_path>"`

**Safety**: Surgical precision - only targets the exact specified partition. Cannot accidentally retire other partitions.

**Example**:
```sql
-- Retire specific partition
SELECT retire_partition_by_file('s3://bucket/data/log_entries/proc1/partition.parquet');

-- Batch retire multiple partitions
SELECT file_path, retire_partition_by_file(file_path) as result
FROM (
    SELECT file_path FROM list_partitions() 
    WHERE view_set_name = 'log_entries' 
    AND file_schema_hash = '[3]'
    LIMIT 10
);
```

**Performance**: Single partition operation, very fast with appropriate database indexes.

---

## Python API Functions

### `micromegas.admin.list_incompatible_partitions(client, view_set_name=None)`

**Description**: Identifies partitions with schemas incompatible with current schema versions.

**Parameters**:
- `client` (FlightSQLClient): Connected Micromegas client
- `view_set_name` (str, optional): Filter to specific view set

**Returns**: pandas DataFrame with columns:

| Column | Type | Description |
|--------|------|-------------|
| `view_set_name` | str | View set name |
| `view_instance_id` | str | Instance ID |
| `incompatible_schema_hash` | str | Old schema version in partition |
| `current_schema_hash` | str | Current schema version |
| `partition_count` | int | Number of incompatible partitions |
| `total_size_bytes` | int | Total size of incompatible partitions |
| `file_paths` | list | Array of file paths for each partition |

**Example**:
```python
import micromegas
import micromegas.admin

client = micromegas.connect()

# List all incompatible partitions
incompatible = micromegas.admin.list_incompatible_partitions(client)
print(f"Found {incompatible['partition_count'].sum()} incompatible partitions")

# List for specific view set
log_incompatible = micromegas.admin.list_incompatible_partitions(client, 'log_entries')
for _, row in log_incompatible.iterrows():
    print(f"{row['view_set_name']}: {row['partition_count']} partitions, {row['total_size_bytes']} bytes")
```

**Implementation**: Uses SQL JOIN between `list_partitions()` and `list_view_sets()` with server-side aggregation.

**Performance**: Efficient server-side processing, minimal network overhead.

---

### `micromegas.admin.retire_incompatible_partitions(client, view_set_name=None)`

**Description**: Safely retires partitions with incompatible schemas using surgical file-path-based retirement.

**Parameters**:
- `client` (FlightSQLClient): Connected Micromegas client  
- `view_set_name` (str, optional): Filter to specific view set

**Returns**: pandas DataFrame with columns:

| Column | Type | Description |
|--------|------|-------------|
| `view_set_name` | str | View set processed |
| `view_instance_id` | str | Instance ID processed |
| `partitions_retired` | int | Count of successfully retired partitions |
| `storage_freed_bytes` | int | Total bytes freed from storage |
| `retirement_messages` | list | Detailed messages for each retirement attempt |

**Example**:
```python
import micromegas
import micromegas.admin

client = micromegas.connect()

# Preview what would be retired
preview = micromegas.admin.list_incompatible_partitions(client, 'log_entries')
print(f"Will retire {preview['partition_count'].sum()} partitions")

# Retire incompatible partitions
result = micromegas.admin.retire_incompatible_partitions(client, 'log_entries')
for _, row in result.iterrows():
    print(f"Retired {row['partitions_retired']} partitions from {row['view_set_name']}")
    print(f"Freed {row['storage_freed_bytes']} bytes")
```

**Safety Features**:
- Uses `retire_partition_by_file()` for surgical precision
- Cannot accidentally retire compatible partitions
- Comprehensive error handling with detailed messages
- Continues processing even if individual partitions fail
- Full transaction safety and rollback protection

**Implementation**: 
1. Calls `list_incompatible_partitions()` to identify targets
2. For each incompatible partition, calls `retire_partition_by_file()` 
3. Aggregates results and provides summary statistics
4. Includes detailed operation logs for auditing

**Performance**: Processes partitions individually for safety, efficient for typical partition counts.

---

## Complex Query Examples

### Find Schema Migration Candidates

```sql
-- Identify view sets with the most incompatible partitions
SELECT 
    vs.view_set_name,
    vs.current_schema_hash,
    COUNT(DISTINCT p.file_schema_hash) as schema_versions_count,
    SUM(CASE WHEN p.file_schema_hash != vs.current_schema_hash THEN 1 ELSE 0 END) as incompatible_count,
    SUM(CASE WHEN p.file_schema_hash != vs.current_schema_hash THEN p.file_size ELSE 0 END) as incompatible_size_bytes
FROM list_view_sets() vs
LEFT JOIN list_partitions() p ON vs.view_set_name = p.view_set_name
GROUP BY vs.view_set_name, vs.current_schema_hash
HAVING incompatible_count > 0
ORDER BY incompatible_size_bytes DESC;
```

### Analyze Partition Age Distribution

```sql
-- Find old incompatible partitions that are candidates for retirement
SELECT 
    p.view_set_name,
    p.file_schema_hash as old_schema,
    vs.current_schema_hash,
    COUNT(*) as partition_count,
    MIN(p.end_insert_time) as oldest_partition,
    MAX(p.end_insert_time) as newest_partition,
    SUM(p.file_size) as total_size_bytes
FROM list_partitions() p
JOIN list_view_sets() vs ON p.view_set_name = vs.view_set_name
WHERE p.file_schema_hash != vs.current_schema_hash
    AND p.end_insert_time < NOW() - INTERVAL '30 days'
GROUP BY p.view_set_name, p.file_schema_hash, vs.current_schema_hash
ORDER BY oldest_partition ASC;
```

### Storage Impact Analysis

```sql
-- Calculate storage savings from retiring incompatible partitions
WITH incompatible_summary AS (
    SELECT 
        p.view_set_name,
        COUNT(*) as incompatible_partitions,
        SUM(p.file_size) as incompatible_size_bytes
    FROM list_partitions() p
    JOIN list_view_sets() vs ON p.view_set_name = vs.view_set_name
    WHERE p.file_schema_hash != vs.current_schema_hash
    GROUP BY p.view_set_name
),
total_summary AS (
    SELECT 
        view_set_name,
        COUNT(*) as total_partitions,
        SUM(file_size) as total_size_bytes
    FROM list_partitions()
    GROUP BY view_set_name
)
SELECT 
    t.view_set_name,
    COALESCE(i.incompatible_partitions, 0) as incompatible_partitions,
    t.total_partitions,
    ROUND(100.0 * COALESCE(i.incompatible_partitions, 0) / t.total_partitions, 2) as incompatible_percentage,
    COALESCE(i.incompatible_size_bytes, 0) as incompatible_size_bytes,
    t.total_size_bytes,
    ROUND(100.0 * COALESCE(i.incompatible_size_bytes, 0) / t.total_size_bytes, 2) as size_percentage
FROM total_summary t
LEFT JOIN incompatible_summary i ON t.view_set_name = i.view_set_name
ORDER BY size_percentage DESC;
```

