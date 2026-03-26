# Add `jsonb_array_length` UDF Plan

GitHub Issue: #976

## Overview

Add a `jsonb_array_length(jsonb)` scalar UDF that returns the number of elements in a JSONB array. This provides a direct, efficient way to count array elements without fragile string-manipulation workarounds.

## Current State

The `rust/datafusion-extensions/src/jsonb/` module has scalar UDFs (`jsonb_parse`, `jsonb_format_json`, `jsonb_get`, `jsonb_as_string`, `jsonb_as_f64`, `jsonb_as_i64`, `jsonb_object_keys`, `jsonb_path_query_first`, `jsonb_path_query`) and two UDTFs (`jsonb_each`, `jsonb_array_elements`).

The `jsonb` crate already exposes `RawJsonb::array_length() -> Result<Option<usize>>`, used in `properties_udf.rs:37` to count object keys. This is exactly the method needed.

The established scalar UDF pattern (see `cast.rs`) is:
1. Struct with `Signature::any(1, Volatility::Immutable)`
2. Handle both `Binary` and `Dictionary<Int32, Binary>` inputs via match arms
3. An extraction function that takes `&[u8]` and returns `Result<Option<T>>`
4. A `make_*_udf()` factory function
5. Registration in `lib.rs::register_extension_udfs()`

For primitive return types (like `Int64`), the UDFs in `cast.rs` use `Int64Array::builder()` directly rather than dictionary encoding — appropriate since integer values don't benefit from deduplication.

## Design

### `jsonb_array_length(jsonb) → Int64`

**Behavior:**
1. For each row, call `RawJsonb::array_length()` on the JSONB bytes
2. Returns the element count as `Int64`
3. Returns NULL for non-array values (objects, scalars, null input)

**Extraction function:**
```rust
fn extract_array_length_from_jsonb(jsonb_bytes: &[u8]) -> Result<Option<i64>> {
    let jsonb = RawJsonb::new(jsonb_bytes);
    match jsonb.array_length() {
        Ok(Some(len)) => Ok(Some(len as i64)),
        Ok(None) => Ok(None), // Not an array
        Err(e) => Err(DataFusionError::External(e.into())),
    }
}
```

**Input/Output types:**
- **Input**: `Binary` or `Dictionary<Int32, Binary>` (standard JSONB input pattern)
- **Output**: `Int64` (same as `jsonb_as_i64`)

Follows the `JsonbAsI64` pattern from `cast.rs` exactly — same struct layout, same match arms for Binary vs Dictionary input, same `Int64Array::builder` for output.

## Implementation Steps

1. **Create `rust/datafusion-extensions/src/jsonb/array_length.rs`**
   - Define `JsonbArrayLength` struct implementing `ScalarUDFImpl`
   - `extract_array_length_from_jsonb()` helper calling `RawJsonb::array_length()`
   - `make_jsonb_array_length_udf()` factory function
   - Return type: `Int64`
   - Handle `Binary` and `Dictionary<Int32, Binary>` inputs

2. **Update `rust/datafusion-extensions/src/jsonb/mod.rs`**
   - Add `pub mod array_length;` (in alphabetical position — before `cast`)
   - Add re-export: `pub use array_length::JsonbArrayLength;`

3. **Register in `rust/datafusion-extensions/src/lib.rs`**
   - Import `make_jsonb_array_length_udf` from `jsonb::array_length`
   - Add `ctx.register_udf(make_jsonb_array_length_udf());` in `register_extension_udfs()`

4. **Add tests in `rust/datafusion-extensions/tests/jsonb_array_length_tests.rs`**
   - Test basic array: `jsonb_array_length(jsonb_parse('[1, 2, 3]'))` → 3
   - Test empty array: `jsonb_array_length(jsonb_parse('[]'))` → 0
   - Test non-array (object): returns NULL
   - Test non-array (scalar): returns NULL
   - Test null input: returns NULL
   - Test via SQL with `SessionContext`

5. **Update documentation in `mkdocs/docs/query-guide/functions-reference.md`**
   - Add `jsonb_array_length` entry in the JSON/JSONB Functions section, after `jsonb_object_keys` and before `jsonb_each`

## Files to Modify

| File | Change |
|------|--------|
| `rust/datafusion-extensions/src/jsonb/array_length.rs` | **New** — `JsonbArrayLength` UDF implementation |
| `rust/datafusion-extensions/src/jsonb/mod.rs` | Add module + re-export |
| `rust/datafusion-extensions/src/lib.rs` | Import and register the UDF |
| `rust/datafusion-extensions/tests/jsonb_array_length_tests.rs` | **New** — tests |
| `mkdocs/docs/query-guide/functions-reference.md` | Document new function |

## Trade-offs

**`BinaryColumnAccessor` vs manual match arms**: The existing `cast.rs` UDFs (`JsonbAsI64`, `JsonbAsF64`) use manual match arms for Binary vs Dictionary handling. While `BinaryColumnAccessor` exists and is cleaner, the manual approach is the established pattern for scalar UDFs returning primitive types. Following `JsonbAsI64` exactly keeps the code consistent with its neighbors.

**Return NULL vs error for non-array input**: The issue specifies returning NULL for non-array values, matching PostgreSQL's `jsonb_array_length` behavior. This is more SQL-friendly than erroring, since it composes well with COALESCE and WHERE filters.

## Documentation

- **`mkdocs/docs/query-guide/functions-reference.md`**: Add `jsonb_array_length` entry with syntax, parameters, return type, and examples.

## Testing Strategy

1. **Unit tests** in `rust/datafusion-extensions/tests/jsonb_array_length_tests.rs`:
   - Array with elements → correct count
   - Empty array → 0
   - Object input → NULL
   - Scalar input → NULL
   - Null input → NULL
   - SQL integration via `SessionContext`

2. **Build validation**: `cargo clippy --workspace -- -D warnings` and `cargo fmt --check`

## Open Questions

None — the issue is well-specified, the underlying API exists, and the implementation follows an established pattern exactly.
