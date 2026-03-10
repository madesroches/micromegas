# JSONPath Query UDFs Plan

## Overview

Add two DataFusion scalar UDFs — `jsonb_path_query_first` and `jsonb_path_query` — that expose the JSONPath engine already shipped in the `jsonb` crate. This enables flexible extraction of values from deeply nested JSONB structures directly in SQL, without custom traversal logic.

GitHub issue: #920

## Current State

The `rust/datafusion-extensions/src/jsonb/` module provides these UDFs:

| UDF | Purpose | File |
|-----|---------|------|
| `jsonb_parse` | JSON string → JSONB | `parse.rs` |
| `jsonb_format_json` | JSONB → JSON string | `format_json.rs` |
| `jsonb_get` | Single-level key lookup | `get.rs` |
| `jsonb_as_string` / `jsonb_as_f64` / `jsonb_as_i64` | Type casts | `cast.rs` |
| `jsonb_object_keys` | Extract object keys | `keys.rs` |
| `jsonb_each` | Expand object to rows (UDTF) | `each.rs` |

There is no way to traverse nested structures in a single call. `jsonb_get` only supports one level of object key lookup. For nested access like `$[?(@.name=="Group")].attributes[?(@.key=="SomeKey")].value`, users have no SQL-level solution.

The `jsonb` crate (v0.5.3, workspace dep in `rust/Cargo.toml`) already provides:
- `jsonb::jsonpath::parse_json_path(input: &[u8]) -> Result<JsonPath<'_>, Error>` — parses a JSONPath string
- `RawJsonb::select_first_by_path(&self, path: &JsonPath) -> Result<Option<OwnedJsonb>, Error>` — first match
- `RawJsonb::select_array_by_path(&self, path: &JsonPath) -> Result<OwnedJsonb, Error>` — all matches as array

## Dependency Upgrade

The workspace currently pins `jsonb = "0.5.3"`. The latest is **0.5.5**. Changes since 0.5.3:
- **0.5.4**: Made `JsonbItemType` public; added `extract_scalar_key_values` and `to_value` functions
- **0.5.5**: Added JSON5 parsing support; fixed panic on infinite numbers from `serde_json`

None of these are breaking changes, and the infinite-number panic fix is a worthwhile safety improvement. The JSONPath API we need (`select_first_by_path`, `select_array_by_path`) exists in 0.5.3 already, so the upgrade is not required for this feature but is recommended as part of this PR.

**Recommendation**: Bump to `0.5.5` in `rust/Cargo.toml` workspace dependencies. Note: 0.5.5 adds `arbitrary_precision` to default features — our dep already uses `default-features = false` so this is safe.

## Design

### `jsonb_path_query_first(jsonb, path_string) → jsonb`

Returns the first match of a JSONPath expression, or NULL if no match.

**Behavior:**
1. Parse the path string argument (constant per batch) with `parse_json_path`
2. For each row, call `RawJsonb::select_first_by_path` with the parsed path
3. Return `OwnedJsonb` bytes via dictionary builder, or NULL on no match

### `jsonb_path_query(jsonb, path_string) → jsonb`

Returns all matches wrapped in a JSONB array.

**Behavior:**
1. Parse the path string argument (constant per batch) with `parse_json_path`
2. For each row, call `RawJsonb::select_array_by_path` with the parsed path
3. Return `OwnedJsonb` bytes via dictionary builder

### Input/Output types

Follow the established pattern from `get.rs`:
- **Input**: `Binary` or `Dictionary<Int32, Binary>` for JSONB; `Utf8` for path string
- **Output**: `Dictionary<Int32, Binary>` for memory efficiency
- Both UDFs use `Signature::any(2, Volatility::Immutable)`

### Path parsing with caching

The path string is typically a SQL literal (constant across all rows), but it could also be a column expression. Use a local `HashMap<String, JsonPath>` cache to parse each distinct path string at most once per batch:

```rust
fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
    let args = ColumnarValue::values_to_arrays(&args.args)?;
    let accessor = create_binary_accessor(&args[0])
        .map_err(|e| DataFusionError::External(e.into()))?;
    let paths = args[1].as_any().downcast_ref::<StringArray>()...;

    // Cache parsed paths: path_string → parsed JsonPath
    let mut path_cache: HashMap<String, JsonPath<'_>> = HashMap::new();

    let mut builder = BinaryDictionaryBuilder::<Int32Type>::new();
    for i in 0..accessor.len() {
        if accessor.is_null(i) || paths.is_null(i) {
            builder.append_null();
        } else {
            let path_str = paths.value(i);
            let json_path = match path_cache.get(path_str) {
                Some(cached) => cached,
                None => {
                    let parsed = parse_json_path(path_str.as_bytes())...;
                    path_cache.entry(path_str.to_string()).or_insert(parsed)
                }
            };
            let raw = RawJsonb::new(accessor.value(i));
            // select_first_by_path or select_array_by_path
        }
    }
    Ok(ColumnarValue::Array(Arc::new(builder.finish())))
}
```

In the common case (literal path), the cache has exactly one entry and parsing happens once. For column paths, each distinct string is parsed once per batch.

Note: `JsonPath` borrows from the input bytes (`JsonPath<'a>` where `'a` is tied to the input `&[u8]`). The cache keys own the strings, so the parsed paths need to borrow from the cache keys. If the borrow checker makes this awkward, an alternative is to simply parse per-row and rely on the fact that path strings are short and parsing is fast relative to the JSONB traversal.

### Code reuse

Use `BinaryColumnAccessor` from `binary_column_accessor.rs` to handle both `Binary` and `Dictionary<Int32, Binary>` inputs in a single code path, rather than duplicating the match arms like `get.rs` does. This keeps the new UDFs cleaner.

## Implementation Steps

1. **Bump jsonb dependency** — Update `rust/Cargo.toml` from `0.5.3` to `0.5.5`

2. **Create `rust/datafusion-extensions/src/jsonb/path_query.rs`**
   - Implement `JsonbPathQueryFirst` (struct + `ScalarUDFImpl`)
   - Implement `JsonbPathQuery` (struct + `ScalarUDFImpl`)
   - Add `make_jsonb_path_query_first_udf()` and `make_jsonb_path_query_udf()` factory functions
   - Use `BinaryColumnAccessor` for input handling

3. **Update `rust/datafusion-extensions/src/jsonb/mod.rs`**
   - Add `pub mod path_query;`
   - Add re-exports for the new structs

4. **Register in `rust/datafusion-extensions/src/lib.rs`**
   - Import factory functions
   - Call `ctx.register_udf(...)` for both new UDFs in `register_extension_udfs()`

5. **Add tests in `rust/datafusion-extensions/tests/jsonb_path_query_tests.rs`**
   - Test simple path: `$.name`
   - Test nested path: `$.a.b.c`
   - Test array access: `$[0]`, `$[*]`
   - Test filter expressions: `$[?(@.key=="foo")].value`
   - Test no match returns NULL (for `_first`) / empty array (for `_query`)
   - Test null JSONB input
   - Test invalid path string returns error
   - Test SQL integration with `SessionContext`
   - Test both Binary and Dictionary-encoded inputs

6. **Update documentation in `mkdocs/docs/query-guide/functions-reference.md`**
   - Add entries for `jsonb_path_query_first` and `jsonb_path_query` in the JSON/JSONB Functions section

## Files to Modify

| File | Change |
|------|--------|
| `rust/Cargo.toml` | Bump jsonb `0.5.3` → `0.5.5` |
| `rust/datafusion-extensions/src/jsonb/path_query.rs` | **New** — both UDF implementations |
| `rust/datafusion-extensions/src/jsonb/mod.rs` | Add module + re-exports |
| `rust/datafusion-extensions/src/lib.rs` | Register both UDFs |
| `rust/datafusion-extensions/tests/jsonb_path_query_tests.rs` | **New** — tests |
| `mkdocs/docs/query-guide/functions-reference.md` | Document new functions |

## Trade-offs

**Single file vs two files for the UDFs**: Both UDFs share nearly identical structure (parse path, iterate rows, call jsonb method, build dict output). Keeping them in one file (`path_query.rs`) reduces duplication and makes the shared pattern obvious. A helper function can handle the common accessor + path-parsing setup.

**BinaryColumnAccessor vs manual match arms**: The existing `get.rs` duplicates ~30 lines for Binary vs Dictionary handling. Using `BinaryColumnAccessor` is cleaner but introduces an `anyhow` → `DataFusionError` conversion at the boundary. The accessor already exists and is used by `format_json.rs`, so this is the preferred approach.

**Path caching strategy**: A `HashMap` cache ensures each distinct path string is parsed at most once per batch. In the common case (literal path), this means a single parse. For per-row path columns, each unique string is parsed once. If `JsonPath`'s lifetime constraints make the cache awkward, falling back to per-row parsing is acceptable — path strings are short and parsing is fast relative to JSONB traversal.

## Documentation

- **`mkdocs/docs/query-guide/functions-reference.md`**: Add `jsonb_path_query_first` and `jsonb_path_query` entries in the JSON/JSONB Functions section, with syntax, parameters, return types, and examples showing nested access patterns.

## Testing Strategy

1. **Unit tests** in `rust/datafusion-extensions/tests/jsonb_path_query_tests.rs` covering:
   - Basic path traversal (object keys, nested objects)
   - Array indexing and wildcards
   - Filter expressions (`?(@.key == "value")`)
   - Edge cases: null input, no match, invalid path, empty JSONB
   - Both Binary and Dictionary-encoded input types
   - Per-row path column (different path strings per row)
   - SQL integration via `SessionContext`

2. **Build validation**: `cargo clippy --workspace -- -D warnings` and `cargo fmt --check`

3. **Manual verification**: After services are running, test with real JSONB data using `micromegas-query`

## Open Questions

None.
