# Parse Block Table UDF Plan

## Overview

Add a `parse_block` table UDF that takes a `block_id`, fetches the block payload from blob storage, parses its transit-serialized objects, and returns each object as a row with its type name and full content as JSONB. This provides a general-purpose block inspection tool, independent of any specific view (logs, metrics, spans).

## Current State

Blocks contain transit-serialized objects. Today, the only way to inspect block contents is through materialized views (`log_entries`, `thread_spans`, `measures`, `async_events`), each of which has a specialized block processor that extracts only the fields it cares about. There is no generic "show me everything in this block" capability.

Key existing pieces:
- **`parse_block()`** (`rust/analytics/src/payload.rs:35`) — iterates transit objects in a block, calling a closure for each `Value`
- **`fetch_block_payload()`** (`rust/analytics/src/payload.rs:11`) — downloads and CBOR-decodes payload from blob storage
- **`transit::Value`** (`rust/transit/src/value.rs:117`) — enum with variants: `String`, `Object`, `U8`, `U32`, `U64`, `I64`, `F64`, `None`
- **`transit::Object`** (`rust/transit/src/value.rs:5`) — `type_name: Arc<String>`, `members: Vec<(Arc<String>, Value)>`
- **`find_stream()`** (`rust/analytics/src/metadata.rs:152`) — fetches `StreamMetadata` (with `dependencies_metadata` and `objects_metadata` needed for parsing) by stream_id
- **`blocks` view** — materialized view with `block_id`, `stream_id`, `process_id`, and stream metadata columns
- **`jsonb::Value`** — used throughout for JSONB serialization via `value.write_to_vec(&mut buf)`
- **JSONB table UDF pattern** (`rust/datafusion-extensions/src/jsonb/each.rs`) — `TableFunctionImpl` + `TableProvider` pattern
- **Lakehouse table UDF pattern** (`rust/analytics/src/lakehouse/query.rs:95`) — UDFs that need `DataLakeConnection` are registered in `register_lakehouse_functions()`

## Design

### Arguments

```sql
SELECT * FROM parse_block('block_id_uuid')
```

Single argument: `block_id` as a string UUID. The function queries the global `blocks` materialized view for the block's metadata and stream info internally.

### Output Schema

```
object_index: Int64     — ordinal position within the block (object_offset + local index)
type_name:    Utf8      — transit type name (e.g., "BeginThreadSpanEvent", "LogStringEvent")
value:        Binary    — JSONB encoding of the full object (not dict-encoded)
```

### Transit Value → JSONB Conversion

Recursive conversion from `transit::Value` to `jsonb::Value`:

| transit::Value         | jsonb::Value                          |
|------------------------|---------------------------------------|
| `String(s)`            | `Value::String(Cow::Owned(s))`        |
| `Object(obj)`          | `Value::Object(BTreeMap)` (recursive) |
| `U8(v)`                | `Value::Number(Number::UInt64(v))`    |
| `U32(v)`               | `Value::Number(Number::UInt64(v))`    |
| `U64(v)`               | `Value::Number(Number::UInt64(v))`    |
| `I64(v)`               | `Value::Number(Number::Int64(v))`     |
| `F64(v)`               | `Value::Number(Number::Float64(v))`   |
| `None`                 | `Value::Null`                         |

For `Object` values, the type_name is added as a `"__type"` field in the JSONB object so it's visible when inspecting nested objects.

### Data Flow

```
SQL: SELECT * FROM parse_block('block-uuid')
                              ↓
              TableFunctionImpl::call()
              parses block_id from Expr
                              ↓
              ParseBlockProvider::scan()
              1. query global blocks view via lakehouse:
                 SELECT block_id, stream_id, process_id, object_offset,
                        "streams.dependencies_metadata", "streams.objects_metadata"
                 FROM blocks WHERE block_id = '...'
              2. extract StreamMetadata from the result row
              3. fetch_block_payload(blob_storage, process_id, stream_id, block_id)
              4. parse_block(stream_metadata, payload, callback)
                              ↓
              For each transit::Value::Object:
              - convert to jsonb::Value recursively
              - serialize via write_to_vec()
                              ↓
              RecordBatch { object_index: Int64, type_name: Utf8, value: Binary }
                              ↓
              MemorySourceConfig → DataSourceExec
```

### Registration

Registered in `register_lakehouse_functions()` alongside other lake-aware UDFs, since it needs `LakehouseContext`, `ViewFactory`, and `QueryPartitionProvider` to query the global blocks view.

## Implementation Steps

### 1. Create `rust/analytics/src/lakehouse/parse_block_table_function.rs`

New file with:
- `ParseBlockTableFunction` implementing `TableFunctionImpl`
  - Holds `Arc<LakehouseContext>`, `Arc<ViewFactory>`, `Arc<dyn QueryPartitionProvider>`, `Option<TimeRange>` (same pattern as `ProcessSpansTableFunction`)
  - `call()` extracts `block_id` string from the expression argument using `exp_to_string()`
  - Returns `ParseBlockProvider`
- `ParseBlockProvider` implementing `TableProvider`
  - `schema()` returns `(object_index: Int64, type_name: Utf8, value: Binary)`
  - `scan()` does the actual work:
    1. Query the global blocks view via `make_session_context()`:
       ```sql
       SELECT block_id, stream_id, process_id, object_offset,
              "streams.dependencies_metadata", "streams.objects_metadata"
       FROM blocks WHERE block_id = '...'
       ```
    2. Extract `StreamMetadata` from the result row — `stream_metadata_from_row` cannot be reused (it expects `sqlx::postgres::PgRow`), so construct manually:
       - Read `stream_id`, `process_id`, `block_id` as Utf8 strings and parse to `sqlx::types::Uuid` via `Uuid::parse_str()`
       - Read `object_offset` as Int64
       - CBOR-decode `dependencies_metadata` and `objects_metadata` Binary columns to `Vec<UserDefinedType>` (same as `stream_metadata_from_row` does)
       - Build `StreamMetadata` with empty `tags` and dummy `properties` (only `dependencies_metadata` and `objects_metadata` are needed for parsing)
    3. Call `fetch_block_payload(blob_storage, process_id, stream_id, block_id)` with the parsed UUIDs
    4. Call `parse_block()` to iterate transit objects
    5. Convert each `Value::Object` to JSONB, collect into vectors (log a warning and skip any non-Object values)
    6. Build `RecordBatch` and return via `MemorySourceConfig`

Filters are not pushed down — DataFusion applies them on top. For limits: if `filters` is empty and a `limit` is provided, stop collecting objects early once the limit is reached (avoids materializing hundreds of thousands of objects). If filters are present, materialize all objects and let DataFusion apply filter+limit on top (since the limit hint is pre-filter and can't be used to short-circuit).
- `fn transit_value_to_jsonb(value: &transit::Value) -> jsonb::Value` — recursive conversion function

### 2. Update `rust/analytics/src/lakehouse/mod.rs`

Add `pub mod parse_block_table_function;`

### 3. Update `rust/analytics/src/lakehouse/query.rs`

Import and register the new UDTF in `register_lakehouse_functions()`:
```rust
ctx.register_udtf(
    "parse_block",
    Arc::new(ParseBlockTableFunction::new(
        lakehouse.clone(),
        view_factory.clone(),
        part_provider.clone(),
        query_range,
    )),
);
```

## Files to Modify

| File | Change |
|------|--------|
| `rust/analytics/src/lakehouse/parse_block_table_function.rs` | **New** — table function + provider + transit→jsonb conversion |
| `rust/analytics/src/lakehouse/mod.rs` | Add `pub mod` |
| `rust/analytics/src/lakehouse/query.rs` | Import and register UDTF |

## Trade-offs

**Why a single `block_id` argument instead of `(process_id, stream_id, block_id)`?**
The block_id is a UUID that uniquely identifies a block. Requiring the user to also pass `process_id` and `stream_id` adds friction for no benefit — the function can look them up from the blocks view. The lakehouse query is negligible compared to the blob fetch and parse.

**Why query the global blocks view instead of the database directly?**
The blocks view is a materialized view that joins blocks with stream and process metadata. Querying through the lakehouse keeps the UDF consistent with the rest of the system and avoids direct DB access from the UDF layer.

**Why put this in `analytics` instead of `datafusion-extensions`?**
It needs `LakehouseContext` for the blocks view query and blob storage access, which are analytics-layer concerns. The `datafusion-extensions` crate is for pure data-processing UDFs.

**Why plain Binary instead of Dictionary<Int32, Binary> for the value column?**
As requested — dict-encoding is unnecessary here since each object will likely have unique JSONB content.

**Why `__type` in the JSONB for nested objects?**
Transit objects have a `type_name` field that isn't a regular member. Embedding it in the JSONB as `__type` preserves this information for nested objects while keeping the output flat.

## Testing Strategy

1. `cargo build` — verify compilation
2. `cargo clippy --workspace -- -D warnings` — no warnings
3. `cargo test` — run existing tests for regressions
4. Unit tests for `transit_value_to_jsonb`:
   - Scalar values (String, U8, U32, U64, I64, F64, None)
   - Object with mixed member types
   - Nested objects (Object containing Object members)
5. Manual integration tests with running services:
   ```sql
   -- Find a block to inspect
   SELECT block_id, nb_objects, "streams.tags"
   FROM view_instance('blocks', 'global')
   LIMIT 5;

   -- All objects in a block
   SELECT object_index, type_name, jsonb_format_json(value)
   FROM parse_block('some-block-uuid');

   -- Limit without filter (should stop early, not materialize all objects)
   SELECT object_index, type_name
   FROM parse_block('some-block-uuid')
   LIMIT 5;

   -- Filter without limit (materializes all, filters on top)
   SELECT object_index, type_name, jsonb_as_string(jsonb_get(value, 'msg'))
   FROM parse_block('some-block-uuid')
   WHERE type_name LIKE 'Log%';

   -- Filter + limit (materializes all, DataFusion filters then limits)
   SELECT object_index, type_name
   FROM parse_block('some-block-uuid')
   WHERE type_name = 'BeginThreadSpanEvent'
   LIMIT 3;
   ```

## Open Questions

None.
