# Add `jsonb_array_elements` Table Function Plan

GitHub Issue: #977

## Overview

Add a `jsonb_array_elements(jsonb)` table-generating function (UDTF) that expands a JSONB array into a set of rows, one per element. This enables natural lateral joins with JSONB arrays, removing the need for subquery workarounds currently required with `jsonb_each`.

## Current State

The `datafusion-extensions` crate already has a `jsonb_each` UDTF (`jsonb/each.rs`) that expands both objects and arrays into `(key, value)` rows. For arrays, `jsonb_each` uses element indices as string keys (`"0"`, `"1"`, ...).

While `jsonb_each` works for arrays, it has a semantic mismatch — the `key` column is meaningless for array elements. More importantly, `jsonb_each` was designed around subquery/literal inputs. The new `jsonb_array_elements` should work naturally with expression arguments (like `jsonb_path_query(column, '$.path')`) to enable lateral join patterns:

```sql
SELECT jsonb_as_string(jsonb_get(elem.value, 'name')) as name
FROM events, jsonb_array_elements(jsonb_path_query(msg_jsonb, '$.items[*]')) as elem
```

The expression-argument pattern was recently added to `jsonb_each` in #982 via the `other =>` branch in `TableFunctionImpl::call()`, which wraps arbitrary expressions in a `LogicalPlan::project`. This same pattern will be reused.

### Relevant code paths

- `rust/datafusion-extensions/src/jsonb/each.rs` — existing UDTF to follow as template
- `rust/datafusion-extensions/src/jsonb/mod.rs` — module registry
- `rust/datafusion-extensions/src/lib.rs:58` — UDTF registration
- `rust/datafusion-extensions/tests/jsonb_each_tests.rs` — test patterns to mirror

## Design

### Approach

Create a new UDTF following the exact same architecture as `jsonb_each`:

1. **`JsonbArrayElementsTableFunction`** implements `TableFunctionImpl` — accepts one argument (literal, subquery, or expression), returns a `JsonbArrayElementsTableProvider`
2. **`JsonbArrayElementsTableProvider`** implements `TableProvider` — evaluates the source, calls `RawJsonb::array_values()`, builds a `RecordBatch` with a single `value` column

### Output Schema

```
value: Binary (not nullable) — JSONB bytes, composable with jsonb_as_string, jsonb_get, etc.
```

Only a `value` column — no `key`. This matches PostgreSQL's `jsonb_array_elements()` which returns a single `value` column.

### Data Flow

```
SQL: SELECT jsonb_as_string(elem.value) FROM jsonb_array_elements(jsonb_parse('[1,2,3]')) as elem
                                                          |
                                              TableFunctionImpl::call()
                                              parses Expr (Literal / Subquery / other)
                                                          |
                                              JsonbArrayElementsTableProvider::scan()
                                              evaluates source -> gets Binary column(s)
                                              iterates all rows across all batches
                                                          |
                                              RawJsonb::array_values() per row
                                              -> concatenated Vec<OwnedJsonb>
                                                          |
                                              RecordBatch { value: BinaryArray }
                                                          |
                                              MemorySourceConfig -> DataSourceExec
```

### Error Handling

- Wrong number of arguments → `DataFusionError::Plan`
- Input is not a JSONB array (object, scalar, null) → `DataFusionError::Execution` with message "jsonb_array_elements: input is not a JSONB array"
- Subquery returns no rows → `DataFusionError::Execution`
- Subquery returns non-Binary column → `DataFusionError::Execution`
- Dictionary<Int32, Binary> input → unwrap and process (reuse `extract_all_jsonb_bytes_from_column`)

### Code Reuse

The `extract_all_jsonb_bytes_from_column` function and the `JsonbSource` enum in `each.rs` handle column type dispatch and source parsing respectively. These should be extracted into a shared module or the new file can duplicate the small amount of necessary code. Given that `each.rs` is ~270 lines and most of the logic is specific to key-value extraction, the cleanest approach is to:

1. Move `JsonbSource`, `extract_all_jsonb_bytes_from_column`, and `scalar_to_jsonb_bytes` (new helper extracted from `scalar_to_entries`) into a shared utility in `jsonb/mod.rs` or a new `jsonb/common.rs`
2. Have both `each.rs` and `array_elements.rs` use the shared utilities

However, to minimize churn on existing code, an acceptable alternative is to duplicate the shared infrastructure in `array_elements.rs` — it's only ~60 lines of boilerplate (the `JsonbSource` enum, `extract_all_jsonb_bytes_from_column`, and scalar-to-bytes conversion).

## Implementation Steps

### 1. Create `rust/datafusion-extensions/src/jsonb/array_elements.rs`

New file implementing `JsonbArrayElementsTableFunction` and `JsonbArrayElementsTableProvider`.

**Core extraction function:**
```rust
fn extract_elements_from_jsonb(jsonb_bytes: &[u8]) -> Result<Vec<Vec<u8>>, DataFusionError> {
    let jsonb = RawJsonb::new(jsonb_bytes);
    match jsonb.array_values() {
        Ok(Some(values)) => Ok(values.into_iter().map(|v| v.as_ref().to_vec()).collect()),
        Ok(None) => Err(DataFusionError::Execution(
            "jsonb_array_elements: input is not a JSONB array".into(),
        )),
        Err(e) => Err(DataFusionError::External(e.into())),
    }
}
```

**Output schema:** Single column `value: Binary (not nullable)`.

**`TableFunctionImpl::call()`:** Same three-branch pattern as `jsonb_each` (Literal, ScalarSubquery, other expression).

**`TableProvider::scan()`:** Same execution flow as `jsonb_each` but producing only a `BinaryArray` of values.

### 2. Update `rust/datafusion-extensions/src/jsonb/mod.rs`

Add `pub mod array_elements;` and re-export `JsonbArrayElementsTableFunction`.

### 3. Update `rust/datafusion-extensions/src/lib.rs`

Import and register the UDTF:
```rust
ctx.register_udtf("jsonb_array_elements", Arc::new(JsonbArrayElementsTableFunction::new()));
```

### 4. Create `rust/datafusion-extensions/tests/jsonb_array_elements_tests.rs`

Tests mirroring `jsonb_each_tests.rs`:
- Simple array `[1, 2, 3]` → 3 rows
- Array of objects `[{"name": "Alice"}, {"name": "Bob"}]` → 2 rows with composable JSONB values
- Empty array `[]` → 0 rows
- Non-array input (object, scalar) → error
- Limit support
- Schema validation (single `value` column)
- SQL integration with `jsonb_parse`
- Composability with `jsonb_as_string`, `jsonb_get`
- Composition with `jsonb_path_query` expression argument

### 5. Update SQL documentation

Add `jsonb_array_elements` to `mkdocs/docs/query-guide/functions-reference.md` in the JSONB section, alongside `jsonb_each`.

## Files to Modify

| File | Change |
|------|--------|
| `rust/datafusion-extensions/src/jsonb/array_elements.rs` | **New** — `JsonbArrayElementsTableFunction` + `JsonbArrayElementsTableProvider` |
| `rust/datafusion-extensions/src/jsonb/mod.rs` | Add `pub mod array_elements;` and re-export |
| `rust/datafusion-extensions/src/lib.rs` | Import and register the UDTF |
| `rust/datafusion-extensions/tests/jsonb_array_elements_tests.rs` | **New** — unit and integration tests |
| `mkdocs/docs/query-guide/functions-reference.md` | Add documentation for the new function |

## Trade-offs

**Why a separate UDTF instead of using `jsonb_each` for arrays?**
`jsonb_each` already handles arrays, but returns a meaningless `key` column (index as string). A dedicated `jsonb_array_elements` provides a cleaner single-column output matching PostgreSQL semantics, and makes SQL queries more readable and self-documenting. The implementation cost is low since it follows the established pattern.

**Why not extract shared code into a common module?**
The shared code between `jsonb_each` and `jsonb_array_elements` is ~60 lines (source enum, column extraction, scalar conversion). Extracting it would touch `each.rs` which is stable and tested. Duplicating it keeps the change isolated. Either approach is acceptable — prefer extraction if more JSONB UDTFs are planned.

## Documentation

- `mkdocs/docs/query-guide/functions-reference.md` — add `jsonb_array_elements` entry with syntax, parameters, and examples

## Testing Strategy

1. `cargo build` — verify compilation
2. `cargo clippy --workspace -- -D warnings` — no warnings
3. `cargo test` — run all tests including new `jsonb_array_elements_tests`
4. Manual integration test with running services:
   ```sql
   SELECT jsonb_as_string(elem.value) as val
   FROM jsonb_array_elements(jsonb_parse('[1, 2, 3]')) as elem

   SELECT jsonb_as_string(jsonb_get(elem.value, 'name')) as name
   FROM jsonb_array_elements(
     jsonb_parse('[{"name": "Alice"}, {"name": "Bob"}]')
   ) as elem
   ```

## Open Questions

None — the issue is well-specified and the implementation pattern is established by `jsonb_each`.
