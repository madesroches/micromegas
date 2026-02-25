# Add `jsonb_each` Table Function Plan

GitHub Issue: #860

## Overview

Add a `jsonb_each` table-generating function (UDTF) to the `datafusion-extensions` crate that expands a JSONB object into rows of `(key Utf8, value Binary/JSONB)`. This enables dynamic expansion of JSONB objects in SQL queries without knowing keys ahead of time — needed for displaying process properties in transposed table cells in notebooks.

## Current State

The crate has 7 JSONB scalar functions registered in `register_extension_udfs()` (`lib.rs:47-53`):
- `jsonb_parse`, `jsonb_format_json`, `jsonb_get`, `jsonb_as_string`, `jsonb_as_f64`, `jsonb_as_i64`, `jsonb_object_keys`

Key extraction logic already exists in `jsonb/keys.rs` via `extract_keys_from_jsonb()` which calls `RawJsonb::object_keys()`. For `jsonb_each`, the jsonb crate provides `RawJsonb::object_each()` which returns `Result<Option<Vec<(String, OwnedJsonb)>>>` — both keys and values in one call.

One UDTF already exists in the crate: `expand_histogram` (`histogram/expand.rs`), which follows a pattern of `TableFunctionImpl` + `TableProvider` that takes a scalar subquery, executes it during `scan()`, and returns an in-memory `RecordBatch`.

## Design

### Approach

Follow the `expand_histogram` pattern exactly:

1. **`JsonbEachTableFunction`** implements `TableFunctionImpl` — accepts one argument (literal or subquery), returns a `JsonbEachTableProvider`
2. **`JsonbEachTableProvider`** implements `TableProvider` — executes the subquery during `scan()`, calls `RawJsonb::object_each()` on the result, builds a `RecordBatch` with `(key, value)` columns

### Output Schema

```
key:   Utf8   (not nullable)
value: Binary (not nullable) — JSONB bytes, composable with jsonb_as_string, jsonb_format_json, etc.
```

### Data Flow

```
SQL: SELECT key, jsonb_as_string(value) FROM jsonb_each((SELECT properties FROM processes WHERE ...))
                                                         ↓
                                              TableFunctionImpl::call()
                                              parses Expr::ScalarSubquery
                                                         ↓
                                              JsonbEachTableProvider::scan()
                                              executes subquery → gets Binary column(s)
                                              iterates all rows across all batches
                                                         ↓
                                              RawJsonb::object_each() per row
                                              → concatenated Vec<(String, OwnedJsonb)>
                                                         ↓
                                              RecordBatch { key: StringArray, value: BinaryArray }
                                                         ↓
                                              MemorySourceConfig → DataSourceExec
```

### Error Handling

- Wrong number of arguments → `DataFusionError::Plan`
- Subquery returns no rows → `DataFusionError::Execution`
- Subquery returns non-Binary column → `DataFusionError::Execution`
- Input is not a JSONB object (array, scalar, null) → `DataFusionError::Execution` (matching issue spec: "return an error")
- Also handle `Dictionary<Int32, Binary>` input by unwrapping the dictionary value

## Implementation Steps

### 1. Create `rust/datafusion-extensions/src/jsonb/each.rs`

New file implementing both `JsonbEachTableFunction` and `JsonbEachTableProvider`.

**Core extraction function:**
```rust
fn extract_entries_from_jsonb(jsonb_bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let jsonb = RawJsonb::new(jsonb_bytes);
    match jsonb.object_each() {
        Ok(Some(entries)) => Ok(entries
            .into_iter()
            .map(|(k, v)| (k, v.as_ref().to_vec()))
            .collect()),
        Ok(None) => Err(DataFusionError::Execution(
            "jsonb_each: input is not a JSONB object".into(),
        )),
        Err(e) => Err(DataFusionError::External(e.into())),
    }
}
```

**TableFunctionImpl::call()** — same pattern as `expand_histogram`: match on `Expr::Literal` and `Expr::ScalarSubquery`.

**TableProvider::scan()** — execute subquery, iterate all rows across all batches extracting Binary JSONB bytes (handling both plain Binary and Dictionary-encoded Binary), call `extract_entries_from_jsonb` per row and concatenate results, build a `RecordBatch` with `StringArray` and `BinaryArray`, wrap in `MemorySourceConfig` + `DataSourceExec`. Apply limit if specified.

### 2. Update `rust/datafusion-extensions/src/jsonb/mod.rs`

Add `pub mod each;` and re-export `JsonbEachTableFunction`.

### 3. Update `rust/datafusion-extensions/src/lib.rs`

Import `JsonbEachTableFunction` and register it:
```rust
ctx.register_udtf("jsonb_each", Arc::new(JsonbEachTableFunction::new()));
```

## Files to Modify

| File | Change |
|------|--------|
| `rust/datafusion-extensions/src/jsonb/each.rs` | **New** — `JsonbEachTableFunction` + `JsonbEachTableProvider` |
| `rust/datafusion-extensions/src/jsonb/mod.rs` | Add `pub mod each;` and re-export |
| `rust/datafusion-extensions/src/lib.rs` | Import and register the UDTF |

## Trade-offs

**Why UDTF instead of a scalar function returning `List<Struct<key, value>>`?**
A UDTF naturally fits the SQL pattern `FROM jsonb_each(...)` and produces rows that can be filtered, joined, and aggregated with standard SQL. A scalar function returning a list would require `UNNEST` which is less ergonomic and would produce a different query pattern than PostgreSQL's `jsonb_each`.

**Why use `object_each()` instead of combining `object_keys()` + `get_by_name()`?**
`object_each()` iterates the object once, extracting both keys and values. Using `object_keys()` + `get_by_name()` for each key would iterate the object N+1 times.

**Why plain arrays instead of dictionary encoding for output?**
The output rows represent a single object's key-value pairs (typically <100 entries). Dictionary encoding adds complexity with no benefit at this scale, unlike the scalar functions that process thousands of rows where deduplication matters.

## Testing Strategy

1. `cargo build` — verify compilation
2. `cargo clippy --workspace -- -D warnings` — no warnings
3. `cargo test` — run existing tests to ensure no regressions
4. Manual integration test with running services:
   ```sql
   SELECT key, jsonb_as_string(value) as value
   FROM jsonb_each(
     (SELECT properties FROM processes WHERE process_id = '<some_id>')
   )
   ```
5. Edge cases to verify:
   - Empty object → returns 0 rows
   - Non-object input → returns error
   - Null input → returns error
   - Nested values (object/array as values) → returned as JSONB Binary, composable with `jsonb_format_json`
   - Multi-row subquery → concatenates key-value entries from all rows

## Open Questions

None — the issue is well-specified and the implementation pattern is established by `expand_histogram`.
