# Add jsonb_object_keys UDF

Add a new UDF to extract the keys from a JSONB object, following the pattern of existing JSONB UDFs.

## Background

The jsonb crate provides `RawJsonb::object_keys()` which returns an `OwnedJsonb` array containing the keys of a JSONB object as string values. This is useful for introspecting JSONB data without knowing the schema in advance.

## Implementation Steps

### 1. Create the UDF implementation

Create `rust/analytics/src/dfext/jsonb/keys.rs`:

- Define `JsonbObjectKeys` struct implementing `ScalarUDFImpl`
- Accept a single JSONB argument (Binary or Dictionary<Int32, Binary>)
- Return `List<Utf8>` (an array of strings) for the keys
- Handle null inputs by returning null
- Handle non-object inputs (arrays, scalars) by returning null (matching PostgreSQL behavior)
- Create `make_jsonb_object_keys_udf()` factory function

Key implementation details:
- Signature: `Signature::any(1, Volatility::Immutable)` (single argument, immutable)
- Use `jsonb::RawJsonb::object_keys()` to extract keys
- Convert the returned JSONB array to Arrow `List<Utf8>`

### 2. Update the jsonb module

Edit `rust/analytics/src/dfext/jsonb/mod.rs`:
- Add `pub mod keys;`
- Add re-export: `pub use keys::JsonbObjectKeys;`

### 3. Register the UDF

Edit `rust/analytics/src/lakehouse/query.rs`:
- Add import for `make_jsonb_object_keys_udf`
- Register the UDF alongside the other jsonb UDFs

### 4. Write tests

Create `rust/analytics/tests/jsonb_object_keys_tests.rs`:

Test cases:
- Extract keys from a simple object `{"a": 1, "b": 2}` → `["a", "b"]`
- Handle null input → null output
- Handle non-object input (array, string, number) → null output
- Handle empty object `{}` → empty array `[]`
- Handle nested object (keys are only top-level)
- Handle dictionary-encoded JSONB input
- Handle binary JSONB input

## Usage Example

```sql
SELECT jsonb_object_keys(properties) as keys FROM log_entries LIMIT 5;
-- Returns: ["key1", "key2", "key3"]
```

### 5. Update mkdocs documentation

Edit `mkdocs/docs/query-guide/functions-reference.md`:
- Add `jsonb_object_keys` documentation after `jsonb_as_i64` in the JSON/JSONB Functions section
- Follow the same format as other jsonb functions

Documentation content:
```markdown
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
```
```

## Files to Create/Modify

| File | Action |
|------|--------|
| `rust/analytics/src/dfext/jsonb/keys.rs` | Create |
| `rust/analytics/src/dfext/jsonb/mod.rs` | Modify |
| `rust/analytics/src/lakehouse/query.rs` | Modify |
| `rust/analytics/tests/jsonb_object_keys_tests.rs` | Create |
| `mkdocs/docs/query-guide/functions-reference.md` | Modify |
