# Functions Reference

This page provides a complete reference to all SQL functions available in Micromegas queries, including both standard DataFusion functions and Micromegas-specific extensions.

**Legend:** 🔧 administrative-flavored function (lakehouse-management, not typical query use) · 🔒 requires an authenticated admin — non-admin callers, including API keys, get a "not found"/"Invalid function" error, unless this deployment can never produce an admin principal at all, in which case any authenticated caller may call it (deployment-wide, not per-audience — see [Authentication](../admin/authentication.md#audience-filtering-activation)). See [Authentication](../admin/flight-sql.md#authentication).

## Micromegas Extensions

### Table Functions

Table functions return tables that can be used in FROM clauses.

#### `view_instance(view_name, identifier)`

Creates a process or stream-scoped view instance for better performance.

**Syntax:**
```sql
view_instance(view_name, identifier)
```

**Parameters:**

- `view_name` (`Utf8`): Name of the view ('log_entries', 'measures', 'thread_spans', 'async_events')

- `identifier` (`Utf8`): Process ID (for most views) or Stream ID (for thread_spans)

**Returns:** Schema depends on the view type (see [Schema Reference](schema-reference.md))

!!! note "Audience-filtered deployments"
    On a deployment with audience filtering active (see
    [Authentication](../admin/authentication.md#audience-filtering-activation)), naming a
    process or stream id outside the caller's own audiences fails with a not-found-shaped error
    instead of returning an empty result — the same check `process_spans`,
    `perfetto_trace_chunks`, `parse_block`, and `get_payload` already apply to their own id
    arguments. The `'global'` identifier is exempt from this check and stays readable for any
    scoped caller.

**Examples:**
```sql
-- Get logs for a specific process
SELECT time, level, msg
FROM view_instance('log_entries', 'my_process_123')
WHERE level <= 3;

-- Get spans for a specific stream
SELECT name, duration
FROM view_instance('thread_spans', 'stream_456')
WHERE duration > 1000000;  -- > 1ms
```

#### `list_partitions()` 🔧

**Administrative Function** - Lists available data partitions in the lakehouse with metadata including file paths, sizes, and schema hashes.

See [Admin Functions Reference](../admin/functions-reference.md#list_partitions) for details.

#### `retire_partitions(view_set_name, view_instance_id, begin_insert_time, end_insert_time)` 🔧🔒

**Administrative Function** - Retires data partitions from the lakehouse for a specified time range. Admin-only, unless this deployment has no admin principal at all (see [Authentication](../admin/authentication.md#audience-filtering-activation)).

**⚠️ DESTRUCTIVE OPERATION:** See [Admin Functions Reference](../admin/functions-reference.md#retire_partitionsview_set-view_instance-start_time-end_time) for details.

#### `materialize_partitions(view_name, begin_insert_time, end_insert_time, partition_delta_seconds)` 🔧🔒

**Administrative Function** - Materializes data partitions for a view over a specified time range. Admin-only, unless this deployment has no admin principal at all (see [Authentication](../admin/authentication.md#audience-filtering-activation)).

See [Admin Functions Reference](../admin/functions-reference.md) for details.

#### `regenerate_partitions(view_name, begin_insert_time, end_insert_time, partition_delta_seconds)` 🔧🔒

**Administrative Function** - Force-regenerates existing partition(s) directly from source data, bypassing the freshness check `materialize_partitions()` stops at. Admin-only, unless this deployment has no admin principal at all (see [Authentication](../admin/authentication.md#audience-filtering-activation)).

See [Admin Functions Reference](../admin/functions-reference.md) for details.

#### `list_view_sets()` 🔧

**Administrative Function** - Lists all available view sets with their current schema information.

See [Admin Functions Reference](../admin/functions-reference.md#list_view_sets) for details.

#### `list_audience_grants()` 🔧

**Administrative Function** - Lists rows of the DB-backed audience grant store, scoped to what
the calling principal may see: an admin sees every row, a non-admin sees every grant on each
`(audience, axis)` pair they hold a matching grant on. Not admin-gated — every authenticated
caller can call it, which is why it's the way to list your own audience grants from
`micromegas-query` (`micromegas-query --all "SELECT * FROM list_audience_grants()"`) or a
notebook.

See [Admin Functions Reference](../admin/functions-reference.md#list_audience_grants) for details.

#### `retire_partition_by_metadata(view_set_name, view_instance_id, begin_insert_time, end_insert_time, file_schema_hash)` 🔧🔒

**Administrative Function** - Retires a single partition by its metadata identifiers. Admin-only, unless this deployment has no admin principal at all (see [Authentication](../admin/authentication.md#audience-filtering-activation)).

**⚠️ DESTRUCTIVE OPERATION:** See [Admin Functions Reference](../admin/functions-reference.md#retire_partition_by_metadataview_set_name-view_instance_id-begin_insert_time-end_insert_time-file_schema_hash) for details.

#### `retire_partition_by_file(file_path)` 🔧🔒

**Administrative Function** - Retires a single partition by file path. Prefer `retire_partition_by_metadata()` for new code. Admin-only, unless this deployment has no admin principal at all (see [Authentication](../admin/authentication.md#audience-filtering-activation)).

**⚠️ DESTRUCTIVE OPERATION:** See [Admin Functions Reference](../admin/functions-reference.md#retire_partition_by_filefile_path) for details.

#### `perfetto_trace_chunks(process_id, span_types, start_time, end_time)`

Generates Perfetto trace chunks from process telemetry data for visualization and performance analysis.

**Syntax:**
```sql
SELECT chunk_id, chunk_data
FROM perfetto_trace_chunks(process_id, span_types, start_time, end_time)
ORDER BY chunk_id
```

**Parameters:**

- `process_id` (`Utf8`): Process UUID to generate trace for

- `span_types` (`Utf8`): Type of spans to include: `'thread'`, `'async'`, or `'both'`

- `start_time` (`Timestamp`): Start time for trace data (UTC timestamp)

- `end_time` (`Timestamp`): End time for trace data (UTC timestamp)

**Returns:**

| Column | Type | Description |
|--------|------|-------------|
| chunk_id | Int32 | Sequential chunk identifier |
| chunk_data | Binary | Binary protobuf TracePacket data |

**Examples:**
```sql
-- Generate trace for thread spans only
SELECT chunk_id, chunk_data
FROM perfetto_trace_chunks(
    'process-uuid-123',
    'thread',
    TIMESTAMP '2024-01-01T00:00:00Z',
    TIMESTAMP '2024-01-01T01:00:00Z'
)
ORDER BY chunk_id;

-- Generate trace for both thread and async spans
SELECT chunk_id, chunk_data
FROM perfetto_trace_chunks(
    'my-process-id',
    'both',
    NOW() - INTERVAL '1 hour',
    NOW()
)
ORDER BY chunk_id;
```

**Note:** The returned binary data is in Perfetto protobuf format and can be loaded directly into the [Perfetto UI](https://ui.perfetto.dev/) for visualization and analysis.

**Note:** `process_id` must name a process in an audience the caller can read (under a restricted, authenticated session); otherwise the call fails with a not-found-shaped error, indistinguishable from naming a process that doesn't exist.

#### `process_spans(process_id, types)`

Returns thread spans, async spans, or both from a process, with `stream_id` and `thread_name` columns prepended. For async spans, `stream_id` is empty and `thread_name` is `'async'`.

**Syntax:**
```sql
-- Thread spans only
SELECT * FROM process_spans('process-uuid', 'thread')

-- Async spans only
SELECT * FROM process_spans('process-uuid', 'async')

-- Both combined
SELECT name, begin, end, depth, thread_name as lane
FROM process_spans('process-uuid', 'both')
ORDER BY lane, begin
```

**Parameters:**

- `process_id` (`Utf8`): Process UUID to query
- `types` (`Utf8`): `'thread'`, `'async'`, or `'both'`

**Note:** The time range is provided out of band via the query's begin/end parameters, not as function arguments.

**Note:** `process_id` must name a process in an audience the caller can read (under a restricted, authenticated session); otherwise the call fails with a not-found-shaped error, indistinguishable from naming a process that doesn't exist.

**Returns:** Same schema as `thread_spans` with two additional leading columns:

| Column | Type | Description |
|--------|------|-------------|
| stream_id | Dictionary(Int16, Utf8) | Stream identifier (empty for async) |
| thread_name | Dictionary(Int16, Utf8) | Thread display name (`'async'` for async spans) |
| id | Int64 | Span identifier |
| parent | Int64 | Parent span identifier |
| depth | UInt32 | Nesting depth |
| hash | UInt32 | Span hash |
| begin | Timestamp(Nanosecond) | Span start time |
| end | Timestamp(Nanosecond) | Span end time |
| duration | Int64 | Duration in nanoseconds |
| name | Dictionary(Int32, Utf8) | Span name (function) |
| target | Dictionary(Int32, Utf8) | Module/target |
| filename | Dictionary(Int32, Utf8) | Source file |
| line | UInt32 | Line number |

**Examples:**
```sql
-- Get all spans across threads for a process
SELECT stream_id, thread_name, name, duration
FROM process_spans('process-uuid-123', 'thread')
ORDER BY begin;

-- Analyze frame time per thread
SELECT thread_name, name, AVG(duration) / 1000000.0 as avg_ms
FROM process_spans('my-process-id', 'both')
WHERE depth = 0
GROUP BY thread_name, name
ORDER BY avg_ms DESC;
```

#### `parse_block(block_id)`

Parses a block's payload and returns each object as a row with its type name and full content as JSONB. This provides a generic block inspection tool, independent of any specific view (logs, metrics, spans) — useful for telling "the JIT/daemon pipeline hasn't caught up yet" from "the payload doesn't contain what we think it does" when a view looks empty.

`parse_block` understands every wire format shipped in-tree, dispatching on the block's `streams.format` (visible in the `blocks` view):

| `streams.format` | Payload | `type_name` values |
|---|---|---|
| `micromegas-transit` | Native CBOR/transit (logs, spans, metrics, images) | The transit type name, e.g. `LogStringEvent`, `BeginThreadSpanEvent` |
| `otlp/v1/logs` | One OTLP `ResourceLogs` proto | `otlp.LogRecord` |
| `otlp/v1/traces` | One OTLP `ResourceSpans` proto | `otlp.Span` |
| `otlp/v1/metrics` | One OTLP `ResourceMetrics` proto | `otlp.NumberDataPoint`, `otlp.HistogramDataPoint`, `otlp.ExponentialHistogramDataPoint`, `otlp.SummaryDataPoint` |

A `streams.format` with no registered decoder returns an error naming the known formats, rather than silently returning nothing.

`block_id` must name a block belonging to a process in an audience the caller can read (under a restricted, authenticated session); otherwise the call fails with a not-found-shaped error, indistinguishable from naming a block that doesn't exist.

**Syntax:**
```sql
SELECT object_index, type_name, jsonb_format_json(value)
FROM parse_block(block_id)
```

**Parameters:**

- `block_id` (`Utf8`): UUID of the block to parse. Block IDs can be found in the `blocks` view.

**Returns:**

| Column | Type | Description |
|--------|------|-------------|
| object_index | Int64 | Ordinal position within the block (global, starting from the block's object_offset for transit blocks; purely positional, `object_offset` is always 0, for OTLP blocks) |
| type_name | Utf8 | Type name — see the format table above |
| value | Binary | Full object content as JSONB binary data |

**Examples:**
```sql
-- Find a block to inspect
SELECT block_id, nb_objects, "streams.format"
FROM blocks
LIMIT 5;

-- Parse all objects in a block
SELECT object_index, type_name, jsonb_format_json(value)
FROM parse_block('550e8400-e29b-41d4-a716-446655440000');

-- Filter by object type
SELECT object_index, jsonb_format_json(value)
FROM parse_block('550e8400-e29b-41d4-a716-446655440000')
WHERE type_name LIKE 'Log%';

-- Extract a specific field from a transit object
SELECT object_index, type_name,
       jsonb_as_string(jsonb_get(value, 'msg')) as msg
FROM parse_block('550e8400-e29b-41d4-a716-446655440000')
WHERE type_name = 'LogStringInteropEvent'
LIMIT 10;

-- Extract one flattened attribute from an OTLP block
SELECT jsonb_as_string(jsonb_get(jsonb_get(value, '__attributes'), 'my.event.id'))
FROM parse_block('550e8400-e29b-41d4-a716-446655440000');
```

**Notes:**

- The `value` column contains JSONB-encoded objects. For transit blocks, each object includes a `__type` field with the transit type name, which is especially useful for inspecting nested objects.
- For OTLP blocks, `value` is a faithful OTLP/JSON dump of the leaf record (log record / span / metric data point) — same field names, camelCase, 64-bit nanos as quoted strings — plus a synthesized `__`-prefixed envelope: `__type` (the `type_name` above), `__attributes` (the leaf's own attributes, flattened, bare keys — same shape as `log_entries.properties`'s record-attribute portion), `__resource` (the resource's attributes, flattened, bare keys), `__scope` (`otel.scope.name`/`otel.scope.version`/`otel.scope.attr.*`/`otel.scope.schema_url`, matching `scope_extras`), and — metrics only — `__metric` (the parent `Metric`'s `name`/`unit`/`description`, `otel.metric.kind`, and, where applicable, `otel.metric.aggregation_temporality`, `otel.metric.is_monotonic`, `otel.metric.metadata` — none of which is reachable from the data point itself).
- For OTLP blocks, `object_index` is a positional index only and is **not** guaranteed to match `nb_objects` from `list_partitions()`/`blocks`, since Summary data points are over-counted there (`SUMMARY_MAX_ROWS_PER_POINT = 4`).
- Non-finite `f64` values (NaN/±Infinity) in an OTLP source payload render as JSON `null` in `value`, indistinguishable from an absent field — the `serde_json` conversion path doesn't preserve OTLP/JSON's `"NaN"`/`"Infinity"` string encoding.
- `asInt` (metrics `NumberDataPoint`) and `Exemplar.as_int` render as bare JSON numbers rather than OTLP/JSON's quoted-string int64 form.
- A block absent from `blocks` for the queried time range errors instead of returning zero rows — widen `--begin` or pass `--all`. A `streams.format` with no registered decoder also errors, listing the known formats.
- When a `LIMIT` is used without filters, the function stops parsing early for efficiency (for OTLP blocks, this still avoids the JSONB conversion cost even though the whole proto message is decoded up front). When filters are present, all objects are materialized first so DataFusion can apply the filter.
- Use with JSONB functions like `jsonb_get`, `jsonb_format_json`, and `jsonb_as_string` to extract and display object contents.

### Scalar Functions

#### JSON/JSONB Functions

Micromegas provides functions for working with JSON data stored in binary JSONB format for efficient storage and querying.

##### `jsonb_parse(json_string)`

Parses a JSON string into binary JSONB format.

**Syntax:**
```sql
jsonb_parse(json_string)
```

**Parameters:**

- `json_string` (Multiple formats supported): JSON string to parse:

   * `Utf8` - Plain string
   * `Dictionary<Int32, Utf8>` - Dictionary-encoded string

**Returns:** `Dictionary<Int32, Binary>` - Dictionary-encoded JSONB data

**Example:**
```sql
-- Parse JSON string into JSONB
SELECT jsonb_parse('{"name": "web_server", "port": 8080}') as parsed_json
FROM processes;
```

##### `jsonb_path_query_first(jsonb, path)`

Returns the first match of a JSONPath expression on a JSONB value, or NULL if no match is found.

**Syntax:**
```sql
jsonb_path_query_first(jsonb, path)
```

**Parameters:**

- `jsonb` (Multiple formats supported): JSONB value in any of these formats:

   * `Binary` - Plain JSONB binary
   * `Dictionary<Int32, Binary>` - Dictionary-encoded JSONB

- `path` (`Utf8`): A JSONPath expression string (e.g., `$.store.book[0].title`)

**Returns:** `Dictionary<Int32, Binary>` - Dictionary-encoded JSONB value of the first match, or NULL if no match

**Examples:**
```sql
-- Extract a nested value
SELECT jsonb_path_query_first(jsonb_parse('{"user": {"name": "Alice"}}'), '$.user.name') as name;
-- Returns: "Alice" (as JSONB)

-- Array index access
SELECT jsonb_path_query_first(jsonb_parse('{"items": [10, 20, 30]}'), '$.items[1]') as second;
-- Returns: 20

-- First wildcard match
SELECT jsonb_as_string(jsonb_path_query_first(data, '$.tags[0]')) as first_tag
FROM processes;
```

##### `jsonb_path_query(jsonb, path)`

Returns all matches of a JSONPath expression on a JSONB value as a JSONB array.

**Syntax:**
```sql
jsonb_path_query(jsonb, path)
```

**Parameters:**

- `jsonb` (Multiple formats supported): JSONB value in any of these formats:

   * `Binary` - Plain JSONB binary
   * `Dictionary<Int32, Binary>` - Dictionary-encoded JSONB

- `path` (`Utf8`): A JSONPath expression string (e.g., `$.store.book[*].title`)

**Returns:** `Dictionary<Int32, Binary>` - Dictionary-encoded JSONB array containing all matched values, or an empty array if no match

**Examples:**
```sql
-- Extract all names from an array of objects
SELECT jsonb_path_query(jsonb_parse('{"users": [{"name": "Alice"}, {"name": "Bob"}]}'), '$.users[*].name') as names;
-- Returns: ["Alice", "Bob"]

-- All array elements
SELECT jsonb_path_query(jsonb_parse('[1, 2, 3]'), '$[*]') as all_items;
-- Returns: [1, 2, 3]

-- No match returns empty array
SELECT jsonb_path_query(jsonb_parse('{"a": 1}'), '$.missing') as result;
-- Returns: []
```

##### Filter Predicates in JSONPath

`jsonb_path_query` and `jsonb_path_query_first` support **SQL/JSON path syntax** for filter predicates. This differs from the JavaScript-style JSONPath syntax commonly found in online tutorials.

**Key difference:** Filters use `? ()` after a wildcard step, not `[?()]` inside brackets.

| Feature | JavaScript JSONPath (NOT supported) | SQL/JSON path (supported) |
|---------|--------------------------------------|--------------------------|
| Filter in brackets | `$.items[?(@.price < 10)]` | `$.items[*] ? (@.price < 10)` |
| String equality | `[?(@.type=="human")]` | `[*] ? (@.type == "human")` |
| Logical AND | `[?(@.a > 1 && @.b < 5)]` | `[*] ? (@.a > 1 && @.b < 5)` |

**Examples:**
```sql
-- Filter array elements by field value
SELECT jsonb_path_query(
  jsonb_parse('{"items": [{"type": "active", "id": 1}, {"type": "inactive", "id": 2}]}'),
  '$.items[*] ? (@.type == "active")'
) as active_items;
-- Returns: [{"id": 1, "type": "active"}]

-- Numeric comparison
SELECT jsonb_path_query(
  jsonb_parse('{"scores": [{"name": "Alice", "val": 85}, {"name": "Bob", "val": 42}]}'),
  '$.scores[*] ? (@.val > 50)'
) as high_scores;
-- Returns: [{"name": "Alice", "val": 85}]

-- Get first match with filter
SELECT jsonb_path_query_first(
  jsonb_parse('{"users": [{"role": "admin", "name": "Alice"}, {"role": "user", "name": "Bob"}]}'),
  '$.users[*] ? (@.role == "admin")'
) as first_admin;
-- Returns: {"name": "Alice", "role": "admin"}

-- Combined with jsonb_path_elements for per-row expansion (see jsonb_path_elements below —
-- jsonb_array_elements can't take a column reference like msg_jsonb as its argument)
SELECT jsonb_as_string(jsonb_get(e, 'name')) as player_name
FROM (
  SELECT unnest(jsonb_path_elements(msg_jsonb, '$.teams[*].players[*] ? (@.type == "human")')) as e
  FROM events
)
```

!!! warning "Common mistake"
    Using JavaScript-style filter syntax like `$.items[?(@.type=="active")]` will result in a parse error. Always use the SQL/JSON style: `$.items[*] ? (@.type == "active")`.

##### `jsonb_entries(jsonb)`

Expands a JSONB object or array into a list of `{key, value}` entries. Unlike `jsonb_each`, this is a plain scalar function, so it can take a column reference and be expanded **per source row** with `unnest()` — the per-row equivalent of `FROM t, jsonb_each(t.col)`, which DataFusion cannot evaluate as a correlated join.

For objects, `key` is the field name. For arrays, `key` is the element index as a string (`"0"`, `"1"`, ...). Returns NULL for a JSON scalar (number, string, boolean) or a NULL input.

**Syntax:**
```sql
jsonb_entries(jsonb)
```

**Parameters:**

- `jsonb` (Multiple formats supported): JSONB value in any of these formats:

   * `Binary` - Plain JSONB binary
   * `Dictionary<Int32, Binary>` - Dictionary-encoded JSONB

**Returns:** `List<Struct<key: Utf8, value: Binary>>` - one entry per object field or array element, or NULL if the input is a JSON scalar or NULL; errors if the JSONB bytes cannot be decoded

**Examples:**
```sql
-- One row per property, per source row
SELECT time, kv['key'] as key, jsonb_as_string(kv['value']) as value
FROM (SELECT time, unnest(jsonb_entries(properties)) as kv FROM log_entries)

-- Count occurrences of each property key
SELECT kv['key'] as key, count(*) as n
FROM (SELECT unnest(jsonb_entries(properties)) as kv FROM log_entries)
GROUP BY key
ORDER BY n DESC;
```

!!! note "Accessing struct fields after unnest()"
    Use `kv['key']` / `kv['value']` (as above), or `kv.key` / `kv.value` in a subquery that aliases the unnested column. `(unnest(jsonb_entries(...))).key` — dot access directly on the `unnest()` call — is not supported by DataFusion.

##### `jsonb_elements(jsonb)`

Expands a JSONB array into a list of its elements, for **per-row** expansion with `unnest()` — the per-row equivalent of `FROM t, jsonb_array_elements(t.col)`. Unlike `jsonb_entries`, the result has no `key`, just the element values. Returns NULL for a JSON object or scalar, or a NULL input.

**Syntax:**
```sql
jsonb_elements(jsonb)
```

**Parameters:**

- `jsonb` (Multiple formats supported): JSONB value in any of these formats:

   * `Binary` - Plain JSONB binary
   * `Dictionary<Int32, Binary>` - Dictionary-encoded JSONB

**Returns:** `List<Binary (JSONB)>` - one entry per array element, or NULL if the input is not a JSONB array; errors if the JSONB bytes cannot be decoded

**Examples:**
```sql
-- One row per array element, per source row
SELECT jsonb_as_string(jsonb_get(e, 'name')) as player_name
FROM (SELECT unnest(jsonb_elements(jsonb_get(jsonb_parse(msg), 'players'))) as e FROM events)
```

##### `jsonb_path_elements(jsonb, path)`

Expands all matches of a JSONPath expression on a JSONB value into a list, for **per-row** expansion with `unnest()`. This is the per-row equivalent of `unnest(jsonb_path_query(jsonb, path))`, which doesn't work directly — `jsonb_path_query` returns a JSONB scalar (a JSONB array), not an Arrow list, so DataFusion's `unnest()` can't expand it.

**Syntax:**
```sql
jsonb_path_elements(jsonb, path)
```

**Parameters:**

- `jsonb` (Multiple formats supported): JSONB value in any of these formats:

   * `Binary` - Plain JSONB binary
   * `Dictionary<Int32, Binary>` - Dictionary-encoded JSONB

- `path` (`Utf8`): A JSONPath expression string, including filter predicates (see [Filter Predicates in JSONPath](#filter-predicates-in-jsonpath) above). A NULL `path` produces a NULL list for that row.

**Returns:** `List<Binary (JSONB)>` - one entry per path match, an empty list if the path has no matches, or NULL if the JSONB input or the path is NULL; errors if the JSONB bytes cannot be decoded

**Examples:**
```sql
-- One row per matched commit, per source row
SELECT jsonb_as_string(jsonb_get(c, 'id')) as commit_id
FROM (SELECT unnest(jsonb_path_elements(jsonb_parse(msg), '$.commits[*]')) as c FROM log_entries)

-- Combined with a filter predicate
SELECT jsonb_as_string(jsonb_get(e, 'name')) as player_name
FROM (
  SELECT unnest(jsonb_path_elements(msg_jsonb, '$.teams[*].players[*] ? (@.type == "human")')) as e
  FROM events
)
```

##### `jsonb_get(jsonb, key)`

Extracts a value from a JSONB object by key name.

**Syntax:**
```sql
jsonb_get(jsonb, key)
```

**Parameters:**

- `jsonb` (Multiple formats supported): JSONB object in any of these formats:

   * `Binary` - Plain JSONB binary
   * `Dictionary<Int32, Binary>` - Dictionary-encoded JSONB

- `key` (`Utf8`): Key name to extract

**Returns:** `Dictionary<Int32, Binary>` - Dictionary-encoded JSONB value or NULL if key not found

**Example:**
```sql
-- Extract name field from JSON data
SELECT jsonb_get(jsonb_parse('{"name": "web_server", "port": 8080}'), 'name') as name_value
FROM processes;
```

##### `jsonb_format_json(jsonb)`

Converts a JSONB value back to a human-readable JSON string.

**Syntax:**
```sql
jsonb_format_json(jsonb)
```

**Parameters:**

- `jsonb` (Multiple formats supported): JSONB value in any of these formats:

   * `Dictionary<Int32, Binary>` - **Dictionary-encoded JSONB (default)**
   * `Binary` - Non-dictionary JSONB

**Returns:** `Dictionary<Int32, Utf8>` - Dictionary-encoded JSON string representation

**Examples:**
```sql
-- Format JSONB back to JSON string
SELECT jsonb_format_json(jsonb_parse('{"name": "web_server"}')) as json_string
FROM processes;

-- Works directly with dictionary-encoded properties
SELECT jsonb_format_json(properties_to_jsonb(properties)) as json_props
FROM log_entries;

-- Format property values as JSON
SELECT jsonb_format_json(properties) as json_string
FROM processes
WHERE properties IS NOT NULL;
```

##### `jsonb_as_string(jsonb)`

Casts a JSONB value to a string.

**Syntax:**
```sql
jsonb_as_string(jsonb)
```

**Parameters:**

- `jsonb` (Multiple formats supported): JSONB value to convert:

   * `Binary` - Plain JSONB binary
   * `Dictionary<Int32, Binary>` - Dictionary-encoded JSONB

**Returns:** `Dictionary<Int32, Utf8>` - Dictionary-encoded string value or NULL if not a string

**Example:**
```sql
-- Extract string value from JSONB
SELECT jsonb_as_string(jsonb_get(jsonb_parse('{"service": "web_server"}'), 'service')) as service_name
FROM processes;
```

##### `jsonb_as_f64(jsonb)`

Casts a JSONB value to a 64-bit float.

**Syntax:**
```sql
jsonb_as_f64(jsonb)
```

**Parameters:**

- `jsonb` (Multiple formats supported): JSONB value to convert:

   * `Binary` - Plain JSONB binary
   * `Dictionary<Int32, Binary>` - Dictionary-encoded JSONB

**Returns:** `Float64` - Numeric value or NULL if not a number

**Example:**
```sql
-- Extract numeric value from JSONB
SELECT jsonb_as_f64(jsonb_get(jsonb_parse('{"cpu_usage": 75.5}'), 'cpu_usage')) as cpu_usage
FROM processes;
```

##### `jsonb_as_i64(jsonb)`

Casts a JSONB value to a 64-bit integer.

**Syntax:**
```sql
jsonb_as_i64(jsonb)
```

**Parameters:**

- `jsonb` (Multiple formats supported): JSONB value to convert:

   * `Binary` - Plain JSONB binary
   * `Dictionary<Int32, Binary>` - Dictionary-encoded JSONB

**Returns:** `Int64` - Integer value or NULL if not an integer

**Example:**
```sql
-- Extract integer value from JSONB
SELECT jsonb_as_i64(jsonb_get(jsonb_parse('{"port": 8080}'), 'port')) as port_number
FROM processes;
```

##### `jsonb_object_keys(jsonb)`

Returns the keys of a JSONB object as an array of strings.

**Syntax:**
```sql
jsonb_object_keys(jsonb)
```

**Parameters:**

- `jsonb` (Multiple formats supported): JSONB object:

   * `Binary` - Plain JSONB binary
   * `Dictionary<Int32, Binary>` - Dictionary-encoded JSONB

**Returns:** `List<Utf8>` - Array of key names, or NULL if input is not an object

**Examples:**
```sql
-- Get keys from a JSONB object
SELECT jsonb_object_keys(jsonb_parse('{"name": "server", "port": 8080}')) as keys;
-- Returns: ["name", "port"]

-- Get keys from process properties
SELECT jsonb_object_keys(properties) as prop_keys
FROM processes
LIMIT 5;

-- One row per key, across all processes
SELECT DISTINCT unnest(jsonb_object_keys(properties)) as prop_key
FROM processes;
```

##### `jsonb_array_length(jsonb)`

Returns the number of elements in a JSONB array.

**Syntax:**
```sql
jsonb_array_length(jsonb)
```

**Parameters:**

- `jsonb` (Multiple formats supported): JSONB value:

   * `Binary` - Plain JSONB binary
   * `Dictionary<Int32, Binary>` - Dictionary-encoded JSONB

**Returns:** `Int64` - The number of elements in the array, or NULL if the input is not an array

**Examples:**
```sql
-- Count elements in an array
SELECT jsonb_array_length(jsonb_parse('[1, 2, 3]')) as len;
-- Returns: 3

-- Empty array returns 0
SELECT jsonb_array_length(jsonb_parse('[]')) as len;
-- Returns: 0

-- Non-array input returns NULL
SELECT jsonb_array_length(jsonb_parse('{"key": "value"}')) as len;
-- Returns: NULL

-- Filter by array size
SELECT *
FROM events
WHERE jsonb_array_length(jsonb_get(msg_jsonb, 'items')) > 5;
```

##### `jsonb_each(jsonb_value)`

Expands a JSONB object or array into rows of key-value pairs. This is a table-returning function (UDTF) that produces one row per entry.

For objects, `key` is the field name. For arrays, `key` is the element index as a string (`"0"`, `"1"`, ...).

**Syntax:**
```sql
SELECT key, value
FROM jsonb_each(jsonb_subquery)
```

**Parameters:**

- `jsonb_value` (Binary/JSONB): A JSONB object or array value, provided as a literal or an **uncorrelated** subquery returning a single JSONB column — `jsonb_each(t.col)` with a bare column reference to a table in the outer `FROM` (e.g. `FROM t, jsonb_each(t.col)`) is not supported (DataFusion has no correlated table-function evaluation) and fails to plan with a "no field named" schema error. Use `jsonb_entries` above for per-row expansion instead. If the subquery returns multiple rows, the entries from all rows are concatenated. Null values are skipped. Returns an error if the input is a scalar (e.g., number or string).

**Returns:**

| Column | Type | Description |
|--------|------|-------------|
| key | Utf8 | Object field name, or array index as a string |
| value | Binary (JSONB) | Value as JSONB bytes, composable with `jsonb_as_string`, `jsonb_format_json`, etc. |

**Examples:**
```sql
-- Expand process properties into rows
SELECT key, jsonb_as_string(value) as value
FROM jsonb_each(
  (SELECT properties FROM processes WHERE process_id = 'my_process_123')
)

-- Use with other JSONB functions for nested values
SELECT key, jsonb_format_json(value) as json_value
FROM jsonb_each(
  (SELECT jsonb_parse('{"name": "server", "port": 8080, "tags": ["prod", "us-east"]}'))
)

-- Expand a JSONB array into rows
SELECT key as index, jsonb_format_json(value) as element
FROM jsonb_each(
  (SELECT jsonb_parse('[10, 20, 30]'))
)
-- Returns: ("0", 10), ("1", 20), ("2", 30)
```

##### `jsonb_array_elements(jsonb_value)`

Expands a JSONB array into a set of rows, one per element. This is a table-returning function (UDTF) that produces one row per array element with a single `value` column.

Unlike `jsonb_each`, this function only accepts arrays (not objects) and does not produce a `key` column, making it more natural for array unnesting.

**Syntax:**
```sql
SELECT value
FROM jsonb_array_elements(jsonb_array)
```

**Parameters:**

- `jsonb_value` (Binary/JSONB): A JSONB array value, provided as a literal, subquery, or expression (e.g., `jsonb_path_query(...)`) — but the expression must contain **no column reference** to a table in the outer `FROM` (`jsonb_array_elements(jsonb_path_query(jsonb_parse('...'), path))` is fine; `jsonb_array_elements(jsonb_path_query(msg_jsonb, path))` is not, since `msg_jsonb` is a bare column reference, and fails to plan with a "no field named" schema error). Use `jsonb_path_elements`/`jsonb_elements` above for per-row expansion of a column instead. If a subquery returns multiple rows, the elements from all arrays are concatenated. Returns an error if the input is not a JSONB array.

**Returns:**

| Column | Type | Description |
|--------|------|-------------|
| value | Binary (JSONB) | Array element as JSONB bytes, composable with `jsonb_as_string`, `jsonb_get`, `jsonb_format_json`, etc. |

**Examples:**
```sql
-- Unnest a simple array
SELECT jsonb_as_string(value) as val
FROM jsonb_array_elements(jsonb_parse('[1, 2, 3]'))

-- Unnest array of objects and extract a field
SELECT jsonb_as_string(jsonb_get(value, 'name')) as name
FROM jsonb_array_elements(jsonb_parse('[{"name": "Alice"}, {"name": "Bob"}]'))

-- Unnest from a subquery
SELECT jsonb_as_string(jsonb_get(value, 'profile_id')) as profile_id
FROM jsonb_array_elements((SELECT jsonb_path_query(msg_jsonb, '$.teams[*].players[*]') FROM events LIMIT 1))
WHERE jsonb_as_string(jsonb_get(value, 'type')) = 'human'
```

#### Data Access Functions

##### `get_payload(process_id, stream_id, block_id)`

Retrieves the raw binary payload of a telemetry block from data lake storage.

**Syntax:**
```sql
get_payload(process_id, stream_id, block_id)
```

**Parameters:**

- `process_id` (`Utf8`): Process identifier

- `stream_id` (`Utf8`): Stream identifier

- `block_id` (`Utf8`): Block identifier

**Returns:** `Binary` - Raw block payload data

**Example:**
```sql
-- Get raw payload data for specific blocks
SELECT process_id, stream_id, block_id, get_payload(process_id, stream_id, block_id) as payload
FROM blocks
WHERE insert_time >= NOW() - INTERVAL '1 hour'
LIMIT 10;
```

**Note:** This is an async function that fetches data from object storage. Use sparingly in queries as it can impact performance.

**Note:** `process_id` must name a process in an audience the caller can read (under a restricted, authenticated session); otherwise the whole call fails with a not-found-shaped error, indistinguishable from naming a process that doesn't exist. A batch mixing a readable and an unreadable `process_id` fails entirely, not just the unreadable rows.

#### Property Functions

Micromegas provides specialized functions for working with property data, including efficient dictionary encoding for memory optimization.

##### `property_get(properties, key)`

Extracts a value from a properties map with automatic format detection and optimized performance for JSONB data.

**Syntax:**
```sql
property_get(properties, key)
```

**Parameters:**

 - `properties` (Multiple formats supported): Properties data in any of these formats:

    * `Dictionary<Int32, Binary>` - **JSONB format (default, optimized)**
    * `List<Struct<key, value>>` - Legacy format (automatic conversion)
    * `Dictionary<Int32, List<Struct>>` - Dictionary-encoded legacy
    * `Binary` - Non-dictionary JSONB

 - `key` (`Utf8`): Property key to extract

**Returns:** `Dictionary<Int32, Utf8>` - Property value or NULL if not found

**Performance:** Optimized for the new JSONB format. Legacy formats are automatically converted for backward compatibility.

**Examples:**
```sql
-- Get thread name from process properties (works with all formats)
SELECT time, msg, property_get(process_properties, 'thread-name') as thread
FROM log_entries
WHERE property_get(process_properties, 'thread-name') IS NOT NULL;

-- Filter by custom property
SELECT time, name, value
FROM measures
WHERE property_get(properties, 'source') = 'system_monitor';

-- Direct JSONB property access (default)
SELECT time, msg, property_get(properties, 'service') as service
FROM log_entries
WHERE property_get(properties, 'env') = 'production';
```

##### `properties_length(properties)`

Returns the number of properties in a properties map with support for multiple storage formats.

**Syntax:**
```sql
properties_length(properties)
```

**Parameters:**

 - `properties` (Multiple formats supported): Properties data in any of these formats:

    * `List<Struct<key, value>>` - Legacy format
    * `Dictionary<Int32, Binary>` - JSONB format (optimized)
    * `Dictionary<Int32, List<Struct>>` - Dictionary-encoded legacy
    * `Binary` - Non-dictionary JSONB

**Returns:** `Int32` - Number of properties

**Examples:**
```sql
-- Works with regular properties
SELECT properties_length(properties) as prop_count
FROM measures;

-- Works with dictionary-encoded properties
SELECT properties_length(properties_to_dict(properties)) as prop_count
FROM measures;

-- JSONB property counting
SELECT properties_length(properties_to_jsonb(properties)) as prop_count
FROM measures;

```

##### `properties_to_dict(properties)`

Converts a properties list to a dictionary-encoded array for memory efficiency.

**Syntax:**
```sql
properties_to_dict(properties)
```

**Parameters:**

- `properties` (`List<Struct<key: Utf8, value: Utf8>>`): Properties list to encode

**Returns:** `Dictionary<Int32, List<Struct<key: Utf8, value: Utf8>>>` - Dictionary-encoded properties

**Examples:**
```sql
-- Convert properties to dictionary encoding for memory efficiency
SELECT properties_to_dict(properties) as dict_props
FROM measures;

-- Use with other functions via properties_to_array
SELECT array_length(properties_to_array(properties_to_dict(properties))) as prop_count
FROM measures;
```

**Note:** Dictionary encoding can reduce memory usage by 50-80% for datasets with repeated property patterns.

##### `properties_to_jsonb(properties)`

Converts a properties list to binary JSONB format with dictionary encoding for efficient storage and querying.

**Syntax:**
```sql
properties_to_jsonb(properties)
```

**Parameters:**

 - `properties` (Multiple formats supported): Properties in any of these formats:

    * `List<Struct<key: Utf8, value: Utf8>>` - Regular properties list
    * `Dictionary<Int32, List<Struct>>` - Dictionary-encoded properties
    * `Binary` - Non-dictionary JSONB
    * `Dictionary<Int32, Binary>` - JSONB format

**Returns:** `Dictionary<Int32, Binary>` - Dictionary-encoded JSONB object containing the properties as key-value pairs

**Examples:**
```sql
-- Convert properties to JSONB format
SELECT properties_to_jsonb(properties) as jsonb_props
FROM log_entries;

-- Use with other JSONB functions
SELECT jsonb_get(properties_to_jsonb(properties), 'hostname') as hostname
FROM log_entries;

-- Convert dictionary-encoded properties to JSONB
SELECT properties_to_jsonb(properties_to_dict(properties)) as jsonb_props
FROM measures;
```

**Note:** This function returns `Dictionary<Int32, Binary>` format for optimal memory usage with Arrow's built-in dictionary encoding.

##### `properties_to_array(dict_properties)`

Converts dictionary-encoded properties back to a regular array for compatibility with standard functions.

**Syntax:**
```sql
properties_to_array(dict_properties)
```

**Parameters:**

- `dict_properties` (`Dictionary<Int32, List<Struct>>`): Dictionary-encoded properties

**Returns:** `List<Struct<key: Utf8, value: Utf8>>` - Regular properties array

**Examples:**
```sql
-- Convert dictionary-encoded properties back to array
SELECT properties_to_array(properties_to_dict(properties)) as props
FROM measures;

-- Use with array functions
SELECT array_length(properties_to_array(properties_to_dict(properties))) as count
FROM measures;
```

#### Histogram Functions

Micromegas provides a comprehensive set of functions for creating and analyzing histograms, enabling efficient statistical analysis of large datasets.

##### `make_histogram(start, end, bins, values)`

Creates histogram data from numeric values with specified range and bin count.

**Syntax:**
```sql
make_histogram(start, end, bins, values)
```

**Parameters:**

- `start` (`Float64`): Histogram minimum value — accepts a literal or any runtime scalar expression (e.g. `MIN(value)`)

- `end` (`Float64`): Histogram maximum value — accepts a literal or any runtime scalar expression (e.g. `MAX(value)`)

- `bins` (`Int64`): Number of histogram bins

- `values` (`Float64`): Column of numeric values to histogram

**Returns:** Histogram structure with buckets and counts

**Example:**
```sql
-- Create histogram of response times (0-50ms, 20 bins)
SELECT make_histogram(0.0, 50.0, 20, CAST(duration AS FLOAT64) / 1000000.0) as duration_histogram
FROM view_instance('thread_spans', 'web_server_123')
WHERE name = 'handle_request';
```

##### `sum_histograms(histogram_column)`

Aggregates multiple histograms by summing their bins.

**Syntax:**
```sql
sum_histograms(histogram_column)
```

**Parameters:**

- `histogram_column` (Histogram): Column containing histogram values

**Returns:** Combined histogram with summed bins

**Example:**
```sql
-- Combine histograms across processes
SELECT sum_histograms(duration_histogram) as combined_histogram
FROM cpu_usage_per_process_per_minute
WHERE time_bin >= NOW() - INTERVAL '1 hour';
```

##### `expand_histogram(histogram)`

Expands a histogram struct into rows of (bin_center, count) for visualization as a bar chart.

**Syntax:**
```sql
SELECT bin_center, count
FROM expand_histogram(histogram_subquery)
```

**Parameters:**

- `histogram` (Histogram struct): A histogram value from `make_histogram()` or a subquery returning one

**Returns:**

| Column | Type | Description |
|--------|------|-------------|
| bin_center | Float64 | Center value of each bin |
| count | UInt64 | Number of values in the bin |

**Examples:**
```sql
-- Expand a CPU usage histogram into chartable rows
SELECT bin_center, count
FROM expand_histogram(
  (SELECT make_histogram(0.0, 100.0, 100, value)
   FROM measures
   WHERE name = 'cpu_usage')
)

-- Histogram for a specific process
SELECT bin_center, count
FROM expand_histogram(
  (SELECT make_histogram(0.0, 50.0, 50, value)
   FROM view_instance('measures', 'my_process_123')
   WHERE name = 'frame_time')
)
```

**Note:** This function is designed for visualization. Use with a bar chart to display distribution data.

##### `quantile_from_histogram(histogram, quantile)`

Estimates a quantile value from a histogram.

**Syntax:**
```sql
quantile_from_histogram(histogram, quantile)
```

**Parameters:**

- `histogram` (Histogram): Histogram to analyze

- `quantile` (`Float64`): Quantile to estimate (0.0 to 1.0)

**Returns:** `Float64` - Estimated quantile value

**Examples:**
```sql
-- Get median (50th percentile) response time
SELECT quantile_from_histogram(duration_histogram, 0.5) as median_duration
FROM performance_histograms;

-- Get 95th percentile response time
SELECT quantile_from_histogram(duration_histogram, 0.95) as p95_duration
FROM performance_histograms;
```

##### `variance_from_histogram(histogram)`

Calculates variance from histogram data.

**Syntax:**
```sql
variance_from_histogram(histogram)
```

**Parameters:**

- `histogram` (Histogram): Histogram to analyze

**Returns:** `Float64` - Variance of the histogram data

**Example:**
```sql
-- Calculate response time variance
SELECT variance_from_histogram(duration_histogram) as duration_variance
FROM performance_histograms;
```

##### `count_from_histogram(histogram)`

Extracts the total count of values from a histogram.

**Syntax:**
```sql
count_from_histogram(histogram)
```

**Parameters:**

- `histogram` (Histogram): Histogram to analyze

**Returns:** `UInt64` - Total number of values in the histogram

**Example:**
```sql
-- Get total sample count from histogram
SELECT count_from_histogram(duration_histogram) as total_samples
FROM performance_histograms;
```

##### `sum_from_histogram(histogram)`

Extracts the sum of all values from a histogram.

**Syntax:**
```sql
sum_from_histogram(histogram)
```

**Parameters:**

- `histogram` (Histogram): Histogram to analyze

**Returns:** `Float64` - Sum of all values in the histogram

**Example:**
```sql
-- Get total duration from histogram
SELECT sum_from_histogram(duration_histogram) as total_duration
FROM performance_histograms;
```

#### Color Functions

Color functions build packed RGBA `u32` values suitable for the map cell's `color` channel (and any other consumer that decodes colors in `0xRRGGBBAA` byte order). Component floats are in `[0.0, 1.0]`; out-of-range values are clamped at the byte boundary. Alpha is straight (not premultiplied), and operations act directly on sRGB-encoded 8-bit channels.

##### `rgba(r, g, b, a)`

Packs four `[0.0, 1.0]` floats into a `UInt32` color in `0xRRGGBBAA` byte order.

**Syntax:**
```sql
rgba(r, g, b, a)
```

**Parameters:**

- `r` (`Float64`): Red channel, `[0.0, 1.0]` (clamped)

- `g` (`Float64`): Green channel, `[0.0, 1.0]` (clamped)

- `b` (`Float64`): Blue channel, `[0.0, 1.0]` (clamped)

- `a` (`Float64`): Alpha channel, `[0.0, 1.0]` (clamped). Straight alpha — not premultiplied.

**Returns:** `UInt32` — packed color where byte 0 (high) is red and byte 3 (low) is alpha. `NULL` if any input is `NULL`. Integer literals (e.g. `rgba(1, 0, 0, 1)`) are accepted via DataFusion's implicit numeric coercion to `Float64`.

**Examples:**
```sql
-- Opaque red.
SELECT rgba(1, 0, 0, 1) AS color;          -- 0xff0000ff

-- 50% grey, fully opaque (round-half-up: 0.5 -> 128).
SELECT rgba(0.5, 0.5, 0.5, 1) AS color;    -- 0x808080ff

-- Out-of-range values clamp safely (useful for normalized metrics).
SELECT rgba(value / max_value, 0.0, 1.0 - value / max_value, 1.0) AS color
FROM measures;
```

##### `lerp_color(c1, c2, t)`

Component-wise linear interpolation between two packed RGBA colors.

**Syntax:**
```sql
lerp_color(c1, c2, t)
```

**Parameters:**

- `c1` (`UInt32`): Start color in `0xRRGGBBAA` packing.

- `c2` (`UInt32`): End color in `0xRRGGBBAA` packing.

- `t` (`Float64`): Interpolation factor, clamped to `[0.0, 1.0]`. Alpha is interpolated alongside RGB.

**Returns:** `UInt32` — packed color. `NULL` if any input is `NULL`.

> **Note on literal colors.** `c1`/`c2` must be `UInt32`. Bare integer or hex literals do not coerce to `UInt32` under this signature and will fail at planning time with a coercion error. Either construct colors via `rgba(...)` (which returns `UInt32` natively) or wrap literals with `CAST(<literal> AS INT UNSIGNED)`. Existing `UInt32` columns work without ceremony.

**Examples:**
```sql
-- Hot/cold gradient over a metric, with full alpha.
-- `t` is clamped internally, so out-of-range ratios safely saturate.
SELECT x, y, z,
       lerp_color(rgba(0, 0.5, 1, 1),       -- cool
                  rgba(1, 0.2, 0, 1),       -- hot
                  value / 100.0) AS color
FROM my_events;

-- Equivalent endpoint construction via CAST.
SELECT lerp_color(CAST(4278190080 AS INT UNSIGNED),  -- 0xff000000
                  CAST(16711680   AS INT UNSIGNED),  -- 0x00ff0000
                  0.5) AS color;                     -- 0x80800000
```

##### `color_scale(name, t, alpha)`

Samples a built-in perceptually-uniform color scale at position `t` and returns a packed RGBA `UInt32` in `0xRRGGBBAA` byte order. One function call replaces the `lerp_color(rgba(0,0,1,a), rgba(1,0,0,a), t)` pattern, which has a muddy purple mid-band, flat luminance, and poor accessibility.

**Syntax:**
```sql
color_scale(name, t, alpha)
```

**Parameters:**

- `name` (`Utf8`): Color scale identifier (case-insensitive). The recognized scales:

<table>
  <thead>
    <tr><th>Name</th><th style="text-align:center;">Gradient</th><th>Notes</th></tr>
  </thead>
  <tbody>
    <tr>
      <td><code>viridis</code></td>
      <td><span style="display:inline-block;width:160px;height:14px;border-radius:2px;vertical-align:middle;background:linear-gradient(to right,#440154,#482475,#404387,#345f8d,#29788e,#20908c,#22a884,#42be70,#79d151,#bbde27,#fde725);"></span></td>
      <td>Sequential blue → green → yellow. Default heatmap; monotonic luminance, color-vision safe.</td>
    </tr>
    <tr>
      <td><code>magma</code></td>
      <td><span style="display:inline-block;width:160px;height:14px;border-radius:2px;vertical-align:middle;background:linear-gradient(to right,#000004,#140e37,#3b0f6f,#641a80,#8c2981,#b53679,#dd4968,#f66e5b,#fe9f6d,#fece91,#fcfdbf);"></span></td>
      <td>Sequential black → red → yellow. Reads well on dark backdrops.</td>
    </tr>
    <tr>
      <td><code>plasma</code></td>
      <td><span style="display:inline-block;width:160px;height:14px;border-radius:2px;vertical-align:middle;background:linear-gradient(to right,#0d0887,#41039d,#6a00a8,#900da3,#b12a90,#cb4678,#e16462,#f1834b,#fca636,#fccd25,#f0f921);"></span></td>
      <td>Sequential purple → orange → yellow. High contrast.</td>
    </tr>
    <tr>
      <td><code>inferno</code></td>
      <td><span style="display:inline-block;width:160px;height:14px;border-radius:2px;vertical-align:middle;background:linear-gradient(to right,#000004,#170b3a,#420a67,#6b176e,#932567,#bb3654,#dc5139,#f3761a,#fca40a,#f6d644,#fcffa4);"></span></td>
      <td>Sequential black → red → yellow. Dark backdrops, hotter mid-band than magma.</td>
    </tr>
    <tr>
      <td><code>cividis</code></td>
      <td><span style="display:inline-block;width:160px;height:14px;border-radius:2px;vertical-align:middle;background:linear-gradient(to right,#002051,#0a316a,#2a436d,#4c566d,#68686f,#7f7b74,#948f78,#aca375,#cab969,#e9d156,#fde945);"></span></td>
      <td>Sequential blue → yellow. Maximum color-vision-deficiency safety.</td>
    </tr>
    <tr>
      <td><code>turbo</code></td>
      <td><span style="display:inline-block;width:160px;height:14px;border-radius:2px;vertical-align:middle;background:linear-gradient(to right,#22171b,#4957dc,#2f9df4,#27d6c3,#4cf883,#94fa50,#dedc32,#ffa422,#f55e17,#ba2108,#900c00);"></span></td>
      <td>Rainbow-style but perceptually corrected. Use when categorical-looking contrast is wanted.</td>
    </tr>
  </tbody>
</table>

- `t` (`Float64`): Position along the scale, clamped to `[0.0, 1.0]`.

- `alpha` (`Float64`): Output alpha channel, `[0.0, 1.0]` (clamped). Straight (not premultiplied), and independent of the scale's RGB output.

**Returns:** `UInt32` — packed color. `NULL` if any input is `NULL`. An unrecognized `name` raises an error that lists the recognized set.

**Examples:**
```sql
-- Density overlay with a perceptual scale; replaces the blue → red lerp.
SELECT x, y,
       color_scale('viridis', value / max_value, 0.7) AS color
FROM density_grid;

-- Dark-mode map cell: magma keeps the hottest cell bright yellow.
SELECT x, y,
       color_scale('magma', t, 1.0) AS color
FROM heatmap;

-- Pure turbo lookup (alpha = 1).
SELECT color_scale('turbo', 0.5, 1.0);  -- mid-band turbo color
```

#### Binning Functions

Binning functions snap continuous coordinates onto a discrete grid. Bins are centered on zero with width `cell_size`, so callers building a 2D heatmap or density grid can `GROUP BY bin_center(x, cs), bin_center(y, cs)` and feed the result straight into a map cell (or any other consumer that expects continuous `(x, y)` coordinates) without grid-aware code.

##### `bin_center(coord, cell_size)`

Snaps a coordinate to the center of its enclosing 1D bin. Bins are centered on zero (`bin_center(0, cs) = 0`) with width `cell_size`; the bin containing `coord` spans the half-open interval `[c - cs/2, c + cs/2)` where `c` is the returned center. Call once per axis to build a 2D grid; the result is a continuous coordinate pair that map cells (and other position-aware consumers) render the same way they render raw points.

**Syntax:**
```sql
bin_center(coord, cell_size)
```

**Parameters:**

- `coord` (`Float64`): Coordinate to snap.

- `cell_size` (`Float64`): Bin width. Must be positive; behaviour is undefined for non-positive values.

**Returns:** `Float64` — the bin center. `NULL` if either input is `NULL`; `NaN`/`±∞` inputs propagate. Integer literals (e.g. `bin_center(3, 10)`) are accepted via DataFusion's implicit numeric coercion to `Float64`.

**Examples:**
```sql
-- 2D density grid over map events. Renderer sees (x, y, cnt) the same
-- way it sees raw points — no awareness of "cells" required.
SELECT bin_center(x, 50.0) AS x,
       bin_center(y, 50.0) AS y,
       COUNT(*) AS cnt
FROM events
GROUP BY 1, 2;
```

#### Math Functions

Scalar math helpers. `lerp` and `unlerp` are the canonical pair for normalize-then-remap pipelines: `lerp(c, d, unlerp(a, b, x))` maps the input range `[a, b]` to the output range `[c, d]`. Neither clamps; callers who want clamping wrap the result (e.g. `LEAST(GREATEST(t, 0.0), 1.0)`) or use the existing `nanvl(...)` to provide a fallback for degenerate `unlerp(a, a, x)` cases.

##### `lerp(a, b, t)`

Linear interpolation between `a` and `b`. Computes `a + (b - a) * t`. No clamping — `t` outside `[0, 1]` extrapolates past the endpoints.

**Syntax:**
```sql
lerp(a, b, t)
```

**Parameters:**

- `a` (`Float64`): Start of the output range.

- `b` (`Float64`): End of the output range.

- `t` (`Float64`): Interpolation parameter. `0.0` returns `a`, `1.0` returns `b`; values outside `[0, 1]` extrapolate.

**Returns:** `Float64` — the interpolated value. `NULL` if any input is `NULL`; `NaN`/`±∞` propagate. Integer literals are accepted via DataFusion's implicit numeric coercion to `Float64`.

**Examples:**
```sql
-- Alpha ramp from 0.5 to 1.0 as t goes 0 → 1. Swap the second
-- argument for whatever maximum alpha the caller wants.
SELECT color_scale('inferno', t, lerp(0.5, 1.0, t)) AS color
FROM scaled;
```

##### `unlerp(a, b, x)`

Inverse linear interpolation. Computes `(x - a) / (b - a)` — i.e. the `t` such that `lerp(a, b, t) == x`. No clamping; `x` outside `[a, b]` returns a value outside `[0, 1]`.

`unlerp(a, a, x)` divides by zero and returns IEEE `NaN` (when `x == a`) or `±Inf` (when `x != a`). Wrap with `nanvl(unlerp(...), 0.0)` if a fallback is required.

**Syntax:**
```sql
unlerp(a, b, x)
```

**Parameters:**

- `a` (`Float64`): Start of the input range.

- `b` (`Float64`): End of the input range.

- `x` (`Float64`): Value to normalize.

**Returns:** `Float64` — the normalized position. `NULL` if any input is `NULL`; `NaN`/`±∞` propagate. Integer literals are accepted via DataFusion's implicit numeric coercion to `Float64`.

**Examples:**
```sql
-- Density normalization for a heatmap: t goes 0 → 1 across the visible range.
WITH scaled AS (
  SELECT cnt, unlerp(0.0, MAX(cnt) OVER (), CAST(cnt AS DOUBLE)) AS t
  FROM cells
)
SELECT cnt, t, color_scale('inferno', t, lerp(0.5, 1.0, t)) AS color
FROM scaled;
```

## Standard SQL Functions

Micromegas supports all standard DataFusion SQL functions including math, string, date/time, conditional, and array functions. For a complete list with examples, see the [DataFusion Scalar Functions documentation](https://datafusion.apache.org/user-guide/sql/scalar_functions.html).

For example queries combining these functions (histograms, property extraction, JSON processing, time-based aggregation), see [Query Patterns](query-patterns.md).

## DataFusion Reference

Micromegas supports all standard DataFusion SQL syntax, functions, and operators. For complete documentation including functions, operators, data types, and SQL syntax, see the [Apache DataFusion SQL Reference](https://datafusion.apache.org/user-guide/sql/).

## Next Steps

- **[Query Patterns](query-patterns.md)** - Common observability query patterns
- **[Performance Guide](performance.md)** - Optimize your queries for best performance
- **[Schema Reference](schema-reference.md)** - Complete view and field reference
