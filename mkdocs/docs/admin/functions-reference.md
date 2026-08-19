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
| `partition_format_version` | Integer | Parquet/Arrow format version the partition file was written with |
| `sort_order` | List(String) | Recorded sort guarantee (e.g. `["insert_time"]`), or `NULL` if none is guaranteed |
| `max_sort_key_time` | Timestamp | Recorded true maximum of the view's leading sort column across the partition's rows (e.g. `begin` for `thread_spans`), or `NULL` if not recorded |

!!! note "Audience-filtered for a restricted caller"
    Under a `ReadScope::Audiences` session (auth enabled, non-admin), rows are filtered to the
    caller's own audiences: a `view_instance_id` row is kept only if its owning process/stream
    resolves to a readable audience, and a `'global'` row (no single audience) is kept only under
    `MICROMEGAS_UNSTAMPED_AUDIENCE`, a matching `MICROMEGAS_PUBLIC_VIEW_SETS` entry, or
    `ReadScope::All`. See [Authentication](authentication.md#audience-filtering-activation).

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

!!! note "Requires admin"
    Only callable by an authenticated admin, or by any authenticated caller when this deployment can never produce an admin principal at all -- see [Authentication](authentication.md#audience-filtering-activation). Otherwise callers, including API keys, get "function not found".

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

### `regenerate_partitions(view_set_name, begin, end, partition_delta_seconds)`

!!! note "Requires admin"
    Only callable by an authenticated admin, or by any authenticated caller when this deployment can never produce an admin principal at all -- see [Authentication](authentication.md#audience-filtering-activation). Otherwise callers, including API keys, get "function not found".

**Description**: Force-regenerates existing partition(s) directly from source data, bypassing the "already up to date" freshness check that `materialize_partitions()` stops at. Useful when a partition's content hash is unchanged but its internal row order or format needs to be refreshed -- for example, an existing merged `blocks` partition materialized before ordered merges were introduced (see `sort_order` in [`list_partitions()`](#list_partitions)), or a `SqlBatchView` (e.g. `log_stats`) whose live partitions predate it adopting a `with_merge_sort_order` declaration and so won't certify for ordered k-way merges until regenerated.

**Parameters**:
- `view_set_name` (String): Name of the view set to regenerate
- `begin` (Timestamp): Start time of the partition(s) being regenerated (inclusive)
- `end` (Timestamp): End time of the partition(s) being regenerated (exclusive)
- `partition_delta_seconds` (Integer): Size of each partition in seconds. Common values: 3600 (hourly), 86400 (daily)

**Usage**:
```sql
SELECT * FROM regenerate_partitions('blocks', '2024-01-01T00:00:00Z', '2024-01-02T00:00:00Z', 86400);
```

**Returns**: A stream of progress log rows (`time`, `msg`), same shape as `materialize_partitions()`.

!!! warning "Alignment invariant"
    `(begin, end, partition_delta_seconds)` must exactly cover the boundaries of the partition(s) being regenerated: `(end - begin)` must be an exact, non-zero multiple of `partition_delta_seconds`, and the range must exactly match existing partition boundaries. A misaligned range/delta fails the query loudly (an error, not a log row) instead of silently leaving a duplicate, overlapping partition behind.

    For a `SqlBatchView`, this means the regeneration bucket size is not a free choice: an already-merged, large partition can only be regenerated as one equally large bucket, whose extract query's `ORDER BY` (required by `with_merge_sort_order`) then sorts that whole bucket's aggregated output in one blocking pass -- there is no smaller `partition_delta_seconds` that avoids this once a partition has already grown large. The bounded alternative is to retire the oversized partition (`retire_partition_by_metadata`) and re-materialize it at a smaller delta via `materialize_partitions()` instead.

!!! note "Online, no downtime"
    Regeneration retires the old partition and inserts the new one atomically, in the same transaction, and streams the source data in bounded chunks -- safe to run against a busy, in-production lakehouse. If another writer (e.g. the maintenance daemon merging partitions) commits a conflicting partition concurrently, the database's overlap exclusion constraint rejects the regeneration with an error instead of leaving duplicate rows -- retry after checking `list_partitions()`. It is an admin/rollout tool, not a steady-state path: run calls serially, never with overlapping ranges in flight concurrently.

---

## Scalar Functions (UDFs)

### `retire_partition_by_metadata(view_set_name, view_instance_id, begin_insert_time, end_insert_time)`

!!! note "Requires admin"
    Only callable by an authenticated admin, or by any authenticated caller when this deployment can never produce an admin principal at all -- see [Authentication](authentication.md#audience-filtering-activation). Otherwise callers, including API keys, get an "Invalid function" error.

**Description**: Surgically retires a single partition by its metadata identifiers. This is the preferred method for retiring partitions as it works for both empty partitions (file_path=NULL) and non-empty partitions.

**Parameters**:
- `view_set_name` (String): Name of the view set
- `view_instance_id` (String): Instance ID (e.g., process_id or 'global')
- `begin_insert_time` (Timestamp): Begin insert time of the partition
- `end_insert_time` (Timestamp): End insert time of the partition

**Usage**:
```sql
SELECT retire_partition_by_metadata(
    'log_entries',
    'process-123',
    TIMESTAMP '2024-01-01 00:00:00',
    TIMESTAMP '2024-01-01 01:00:00'
) as result;
```

**Returns**: String message indicating success or failure:
- Success: `"SUCCESS: Retired partition <view_set>/<instance> [<begin>, <end>)"`
- Failure: `"ERROR: Partition not found: <view_set>/<instance> [<begin>, <end>)"`

**Safety**:
- Surgical precision - only targets the exact specified partition by its natural identifiers
- Works for both empty partitions (file_path=NULL) and non-empty partitions
- Uses database transactions with automatic rollback on batch errors
- Files are scheduled for cleanup rather than immediately deleted

**Example**:
```sql
-- Retire specific partition by metadata
SELECT retire_partition_by_metadata(
    'log_entries',
    'process-123',
    TIMESTAMP '2024-01-01 00:00:00',
    TIMESTAMP '2024-01-01 01:00:00'
);

-- Batch retire incompatible partitions
SELECT
    view_set_name,
    view_instance_id,
    retire_partition_by_metadata(
        view_set_name,
        view_instance_id,
        begin_insert_time,
        end_insert_time
    ) as result
FROM list_partitions() p
JOIN list_view_sets() vs ON p.view_set_name = vs.view_set_name
WHERE p.file_schema_hash != vs.current_schema_hash
LIMIT 10;
```

**Batch Behavior**: When called with multiple rows in a single query, all operations are executed within a single database transaction. If any retirement fails, all changes are rolled back and a `ROLLED_BACK` message is appended indicating the number of reverted changes.

**Performance**: Single partition operation, very fast with appropriate database indexes.

---

### `retire_partition_by_file(file_path)`

!!! note "Requires admin"
    Only callable by an authenticated admin, or by any authenticated caller when this deployment can never produce an admin principal at all -- see [Authentication](authentication.md#audience-filtering-activation). Otherwise callers, including API keys, get an "Invalid function" error.

**Description**: Retires a single partition by exact file path match.

!!! note "Prefer metadata-based retirement"
    For new code, prefer `retire_partition_by_metadata()` which works for both empty and non-empty partitions.

**Parameters**:
- `file_path` (String): Exact file path of partition to retire

**Usage**:
```sql
SELECT retire_partition_by_file('s3://bucket/data/log_entries/process-123/2024/01/01/file.parquet') as result;
```

**Returns**: String message indicating success or failure:
- Success: `"SUCCESS: Retired partition <file_path>"`
- Failure: `"ERROR: Partition not found: <file_path>"`

**Limitation**: Cannot retire empty partitions (where file_path is NULL). Use `retire_partition_by_metadata()` for empty partitions.

**Performance**: Single partition operation, very fast with appropriate database indexes.

---

## Query Deny List

The admin-managed query deny list (see `tasks/query_deny_list_plan.md`) is the manual valve an
on-call admin can pull, without a deploy, to stop a misbehaving query in flight — a dashboard on a
short refresh interval, an alert rule re-firing on failure, a notebook cell stuck in a retry loop.
A matching query is rejected at the front of the FlightSQL service, before any object-store reads
or memory-pool reservation. Rules stay in force until an admin removes them explicitly — there is
no expiry. See also the [Admin → Query Deny List](web-app.md) screen, which is a front end for
the three functions below, and the [query audit log](../query-guide/query-audit-log.md), where an
operator finds the fingerprint to paste into `deny_queries`.

### The match context

Every rule is a boolean SQL expression over this fixed set of columns — the attribution
`execute_query` has already resolved by the time the check runs. All columns are nullable
`Utf8`; standard SQL NULL semantics apply (`notebook = 'x'` is NULL, not true, for a query that
sent no notebook header, so such a rule does not fire on it).

| Column | NULL when | Client can change it? |
|---|---|---|
| `user_id` | never (`'unknown'` when no auth is configured) | Yes — client-asserted except for a non-delegating OIDC caller |
| `email` | never | Yes, same caveat as `user_id` |
| `service_account` | the caller is not a delegating service account (also NULL for an ordinary human caller) | No — server-derived |
| `client` | never (`'unknown'` if `x-client-type` is absent) | Yes |
| `agent` | never (`'unknown'` if `x-client-agent` is absent) | Yes |
| `entrypoint` | never (`'unknown'` if `x-client-entrypoint` is absent) | Yes |
| `session` | the caller sent no `x-client-session` | Yes |
| `notebook` | the query did not originate from a notebook cell | Yes |
| `cell` | the query did not originate from a notebook cell | Yes |
| `client_ip` | never | Partly — derived from `X-Forwarded-For` when present, so only trustworthy behind a proxy that overwrites it |
| `sql` | never | No — the raw statement text |
| `sql_hash` | never | No — the normalized fingerprint (see below) |

The identity column is named `user_id`, not `user`: a bare `user` parses as the zero-argument
function `user()` under DataFusion's default dialect, not a column reference, so `user = 'jean'`
fails with "Invalid function 'user'" — use `user_id`.

!!! warning "`client_ip` is only network-level truth behind a trusted proxy"
    `get_client_ip` trusts the rightmost entry of the last `X-Forwarded-For` header field line
    before falling back to the raw socket address, and nothing strips or overwrites that header.
    Deployed behind a proxy that appends its own observation (e.g. an AWS ALB in
    `xff_header_processing.mode = append`), that entry is genuinely non-forgeable. Without such a
    proxy in front of `flight-sql-srv`, a direct FlightSQL client can send its own
    `x-forwarded-for` header and fully control the value a `client_ip` rule matches against,
    evading the rule. Separately, every query proxied through `analytics-web-srv` reports *that
    server's* address (see the [query audit log](../query-guide/query-audit-log.md)), so a
    `client_ip` rule written against web traffic either matches nobody or matches every web user,
    never one specific browser client.

### The expression language

DataFusion itself parses and evaluates the expression — there is no grammar of this product's
own to learn. The useful subset is `AND`/`OR`/`NOT`, `=`/`!=`, `IN`, `LIKE`/`ILIKE`,
`IS [NOT] NULL`, and `regexp_like`, over the match-context columns and string literals — but
that's documentation, not an enforced grammar; anything DataFusion's planner accepts over this
schema works, including a top-level `OR` with no "anchor" equality (a blanket rule such as
`sql LIKE '%thread_spans%'` is a legitimate, powerful incident lever, and is accepted).

Exactly two shapes are rejected, both at `deny_queries` time, with DataFusion's own diagnostic
where there is one:

- The expression's result is not `Boolean`.
- The expression references **no column at all** (`true`, `1 = 1`) — such a rule would deny
  *every* query in the deployment.

Type mismatches (`client = 42`, `notebook = now()`) fail to compile rather than silently never
firing, since every match-context column is `Utf8` with no implicit cast to a different type.

```sql
-- deny one specific, fingerprinted query from one entrypoint
SELECT * FROM deny_queries(
  'sql_hash = ''9f2c41ab73de0155'' AND entrypoint = ''grafana-alert''',
  'alert rule re-firing on failure; owner notified');

-- deny everything from one service account, on one notebook
SELECT * FROM deny_queries(
  'user_id = ''dashboards-svc'' AND notebook = ''fleet-overview''',
  'dashboard stuck at a 1s refresh interval');

-- a blanket rule: stop anything scanning thread_spans, or anything from one host
-- (client_ip is only trustworthy here behind a proxy that overwrites X-Forwarded-For --
-- see the warning above)
SELECT * FROM deny_queries(
  'sql LIKE ''%thread_spans%'' OR client_ip = ''10.4.9.221''',
  'incident: thread_spans scan storm');
```

### `list_query_denials()`

!!! note "Requires admin"
    Same gate as `retire_partitions()` and friends — see [Authentication](authentication.md#audience-filtering-activation).

**Description**: Lists every query-deny-list rule currently in force.

**Usage**:
```sql
SELECT * FROM list_query_denials();
```

**Returns**: Table with columns:

| Column | Type | Description |
|---|---|---|
| `rule_id` | String | The rule's id — pass this to `remove_query_denial(rule_id)` |
| `created_at` | Timestamp | When the rule was created |
| `created_by` | String | The identity of the admin who created it |
| `reason` | String | The mandatory, free-text reason given at creation |
| `match_expr` | String | The expression exactly as written |
| `last_hit_at` | Timestamp, nullable | `NULL` until the rule first fires; otherwise the most recent match, accurate to within one refresh tick (`MICROMEGAS_QUERY_DENY_REFRESH_SECONDS`) — "4 s ago" means the offender is still calling in, "3 weeks ago" means the rule is probably safe to remove |

### `deny_queries(match_expr, reason)`

!!! note "Requires admin"
    Same gate as `retire_partitions()` and friends.

**Description**: Validates and inserts a new query-deny-list rule. Both arguments are SQL string
literals — double any inner `'` the same as anywhere else in SQL.

**Parameters**:
- `match_expr` (String): A boolean SQL expression over the match context above
- `reason` (String): Free-text, required — recorded alongside the rule and shown to a denied
  caller

**Usage**:
```sql
SELECT * FROM deny_queries(
  'sql_hash = ''9f2c41ab73de0155'' AND entrypoint = ''grafana-alert''',
  'alert rule re-firing on failure; owner notified');
```

**Returns**: A single row (the log-stream shape every mutating admin UDTF uses: `time`, `msg`) —
`msg` carries the new rule's id. Alias it for clarity: `SELECT msg AS rule_id FROM deny_queries(...)`.

**Validation** (fails loudly, before anything is written): the expression must compile against
the match context (see "The expression language" above); `reason` must not be empty; the caller
must carry an identity (always true for an authenticated admin); and the deployment must be under
`MICROMEGAS_QUERY_DENY_MAX_RULES` (default 100) rules already.

### `remove_query_denial(rule_id)`

!!! note "Requires admin"
    Same gate as `retire_partition_by_file()` and friends.

**Description**: Deletes a single query-deny-list rule by id, so callers it was rejecting can
reach the service again. Hard delete — no soft-revoke trail, since the [query audit
log](../query-guide/query-audit-log.md) already records every denial the rule ever caused, which
is the part worth keeping.

**Parameters**:
- `rule_id` (String): The rule's id, from `list_query_denials()`

**Usage**:
```sql
SELECT remove_query_denial('9f2c41ab-73de-4015-9d2e-000000000000') as result;
```

**Returns**: String message indicating success or failure:
- Success: `"SUCCESS: removed rule <rule_id>"`
- Failure: `"ERROR: no such rule: <rule_id>"`

**Escape hatch, no expiry to wait out**: since rules stand until removed, an admin (or, in a
deployment with no admin principal at all, any authenticated caller — the same
`admin_principal_possible` fallback `retire_partitions()` uses) can always call
`remove_query_denial`/`deny_queries`/`list_query_denials`, even from behind a rule that would
otherwise match every query they send — the check is skipped for a statement naming one of these
three functions, from a caller who could reach them anyway.

### Incident runbook

1. **Find the offender.** Query the [audit log](../query-guide/query-audit-log.md), grouping by
   `sql_hash`, to find the fingerprint of the query that's hurting the service. (A notebook that
   does this well is planned but out of scope here — see the audit-log doc's top-offenders query
   in the meantime.)
2. **Deny it.** `SELECT * FROM deny_queries('sql_hash = ''<fingerprint>''', '<why>')`.
3. **Confirm.** The offending client starts seeing `ResourceExhausted`; the audit log shows
   `error_class = "denied"` rows, and `measures` shows the `query_denied` metric ticking up,
   tagged with the rule's id.
4. **Lift it once the client is fixed.** `SELECT remove_query_denial('<rule_id>')`.

## Python API Functions

### `micromegas.admin.list_incompatible_partitions(client, view_set_name=None)`

**Description**: Identifies partitions with schemas incompatible with current schema versions. Returns one row per incompatible partition for precise targeting.

**Parameters**:
- `client` (FlightSQLClient): Connected Micromegas client
- `view_set_name` (str, optional): Filter to specific view set

**Returns**: pandas DataFrame with columns:

| Column | Type | Description |
|--------|------|-------------|
| `view_set_name` | str | View set name |
| `view_instance_id` | str | Instance ID |
| `begin_insert_time` | timestamp | Begin insert time of the partition |
| `end_insert_time` | timestamp | End insert time of the partition |
| `incompatible_schema_hash` | str | Old schema version in partition |
| `current_schema_hash` | str | Current schema version |
| `file_path` | str | File path for the partition (NULL for empty partitions) |
| `file_size` | int | Size in bytes of the partition file (0 for empty partitions) |

**Example**:
```python
import micromegas
import micromegas.admin

client = micromegas.connect()

# List all incompatible partitions
incompatible = micromegas.admin.list_incompatible_partitions(client)
print(f"Found {len(incompatible)} incompatible partitions")

# List for specific view set
log_incompatible = micromegas.admin.list_incompatible_partitions(client, 'log_entries')
print(f"Log entries: {len(log_incompatible)} incompatible partitions")
print(f"Total size: {log_incompatible['file_size'].sum()} bytes")

# Check for empty partitions (file_path is NULL)
empty_partitions = incompatible[incompatible['file_path'].isna()]
print(f"Empty partitions: {len(empty_partitions)}")
```

**Implementation**: Uses SQL JOIN between `list_partitions()` and `list_view_sets()` with server-side filtering.

**Performance**: Efficient server-side processing, minimal network overhead.

---

### `micromegas.admin.retire_incompatible_partitions(client, view_set_name=None)`

!!! note "Requires admin"
    Internally calls `retire_partition_by_metadata()`, which requires an authenticated admin, or any authenticated caller when this deployment can never produce an admin principal at all -- see [Authentication](authentication.md#audience-filtering-activation). Non-admin callers, including API keys, will otherwise see this fail with an "Invalid function" error.

**Description**: Safely retires partitions with incompatible schemas using metadata-based retirement. This handles both empty partitions (file_path=NULL) and non-empty partitions.

**Parameters**:
- `client` (FlightSQLClient): Connected Micromegas client
- `view_set_name` (str, optional): Filter to specific view set

**Returns**: pandas DataFrame with columns:

| Column | Type | Description |
|--------|------|-------------|
| `view_set_name` | str | View set processed |
| `view_instance_id` | str | Instance ID processed |
| `partitions_retired` | int | Count of successfully retired partitions |
| `partitions_failed` | int | Count of partitions that failed to retire |
| `storage_freed_bytes` | int | Total bytes freed from storage |
| `retirement_messages` | list | Detailed messages for each retirement attempt |

**Example**:
```python
import micromegas
import micromegas.admin

client = micromegas.connect()

# Preview what would be retired
preview = micromegas.admin.list_incompatible_partitions(client, 'log_entries')
print(f"Will retire {len(preview)} partitions")
print(f"Will free {preview['file_size'].sum() / (1024**3):.2f} GB")

# Retire incompatible partitions
result = micromegas.admin.retire_incompatible_partitions(client, 'log_entries')
for _, row in result.iterrows():
    print(f"Retired {row['partitions_retired']} partitions from {row['view_set_name']}")
    print(f"Failed {row['partitions_failed']} partitions")
    print(f"Freed {row['storage_freed_bytes']} bytes")
```

**Safety Features**:
- Uses `retire_partition_by_metadata()` for surgical precision
- Works for both empty partitions (file_path=NULL) and non-empty partitions
- Cannot accidentally retire compatible partitions
- Comprehensive error handling with detailed messages
- Continues processing even if individual partitions fail
- Results grouped by view_set_name and view_instance_id for clarity

**Implementation**:
1. Calls `list_incompatible_partitions()` to identify targets (one row per partition)
2. Groups partitions by view_set_name and view_instance_id
3. For each partition, calls `retire_partition_by_metadata()` with the partition's natural identifiers
4. Aggregates results and provides summary statistics per group
5. Includes detailed operation logs for auditing

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
