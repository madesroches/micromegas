# List-Returning JSONB Scalar UDFs Plan

GitHub Issue: #1475 (step 2 of the revised scope in
[this comment](https://github.com/madesroches/micromegas/issues/1475#issuecomment-5440347616))

## Overview

Add scalar JSONB UDFs that return Arrow `List` values instead of an opaque JSONB scalar, so
`unnest()` can expand a JSONB object or array into one row per entry **per source row**. This
closes the only remaining hole in the JSONB story — per-row value/element expansion — without
touching correlated table functions or `LATERAL`, which remain an upstream DataFusion
limitation.

Three new UDFs, all additive:

| Function | Returns | Replaces the broken form |
|---|---|---|
| `jsonb_entries(jsonb)` | `List<Struct<key: Utf8, value: Binary>>` | `FROM t, jsonb_each(t.col)` |
| `jsonb_elements(jsonb)` | `List<Binary>` | `FROM t, jsonb_array_elements(t.col)` |
| `jsonb_path_elements(jsonb, path)` | `List<Binary>` | `unnest(jsonb_path_query(col, path))` |

Target usage:

```sql
-- one row per property, per log entry
SELECT time, kv['key'] AS key, jsonb_as_string(kv['value']) AS value
FROM (SELECT time, unnest(jsonb_entries(properties)) AS kv FROM log_entries);

-- one row per array element selected by a path
SELECT jsonb_as_string(jsonb_get(e, 'id')) AS commit_id
FROM (SELECT unnest(jsonb_path_elements(jsonb_parse(msg), '$.commits[*]')) AS e
      FROM log_entries);
```

## Current State

### What exists

Scalar JSONB UDFs live in `rust/datafusion-extensions/src/jsonb/` and are registered in
`rust/datafusion-extensions/src/lib.rs:60-77`:

- `jsonb_parse`, `jsonb_format_json`, `jsonb_get`, `jsonb_as_string` / `_f64` / `_i64`,
  `jsonb_array_length`, `jsonb_object_keys`, `jsonb_path_query`, `jsonb_path_query_first`
- Two UDTFs: `jsonb_each` (`jsonb/each.rs`), `jsonb_array_elements` (`jsonb/array_elements.rs`)

Input dispatch over `Binary` / `Dictionary<Int32, Binary>` is already abstracted by
`create_binary_accessor` in `rust/datafusion-extensions/src/binary_column_accessor.rs`;
`path_query.rs:31` uses it, and it is the intended reuse point for the new UDFs.

### The gap (verified empirically, DataFusion 54.1)

Probed against a `Binary` JSONB column holding `{"a":1,"b":"x","items":[1,2,3]}` and
`{"c":true,"items":[4,5]}`:

| Query | Result |
|---|---|
| `unnest(arrow_cast(jsonb_object_keys(j), 'List(Utf8)'))` | works — per-row **key** expansion, groupable, `DISTINCT`-able |
| `unnest(jsonb_object_keys(j))` | fails — return type is `Dictionary(Int32, List(Utf8))`; `unnest() can only be applied to array, struct and null` |
| `FROM t, jsonb_each(t.j)` | fails — `Schema error: No field named t.j` |
| `unnest(jsonb_path_query(j, '$.items[*]'))` | fails — return type is `Dictionary(Int32, Binary)`, a JSONB scalar, not a list |

So per-row *key* expansion is already reachable behind an `arrow_cast`. Per-row **value /
element** expansion has no route at all.

Two causes:

1. Both UDTFs handle only `Expr::Literal` and `Expr::ScalarSubquery`. Any other expression
   falls into the `other =>` branch (`each.rs:65-71`, `array_elements.rs:62-68`), which builds
   `LogicalPlanBuilder::empty(true).project(...)` — an empty relation with no columns, so a
   column reference cannot resolve at plan time. The `tasks/completed/jsonb_array_elements_udtf_plan.md`
   design aimed explicitly at "lateral join patterns"; this branch is why it never delivered them.
2. Even the working uncorrelated subquery form concatenates entries from all rows with no
   source-row column (`each.rs` `JsonbSource::Subquery` scan path), so results cannot be joined
   back to the row they came from.

`properties_to_array` does not help: it only unwraps the dictionary via `take`
(`properties/properties_udf.rs:83-121`), so on a JSONB `properties` column it returns `Binary`.
`properties_to_dict` (`rust/analytics/src/properties/properties_to_dict_udf.rs`) converts the
*other* direction (`List<Struct>` → `Dictionary<List<Struct>>`). Nothing in the codebase converts
JSONB into an Arrow list.

This matters because every `properties` column in the core views is `Dictionary<Int32, Binary>`
JSONB (`log_entries_table.rs:75,80`, `metrics_table.rs:78,83`, `blocks_view.rs:285,320`,
`otel/spans_table.rs:36,71`), and webhook / OTLP JSON bodies land in `msg` with nested arrays
(see the `commits` array assertion in `python/micromegas/tests/test_otlp_e2e.py:607`).

### Verified target behavior

Probed by registering hand-built `List<Binary>` and `List<Struct<key,value>>` columns and running
the intended queries — all of the following work in DataFusion 54.1:

| Query shape | Result |
|---|---|
| `unnest(list_binary_col)` | one row per element |
| `jsonb_format_json(unnest(elems))`, `jsonb_as_i64(e)` over the unnested column | composes — the `Binary` element feeds the existing scalar UDFs |
| `unnest(entries)` on `List<Struct<key,value>>` | one struct row per entry, printed as `{key: a, value: …}` |
| `kv['key']`, `kv.value` in a subquery over `unnest(entries) AS kv` | works (`kv.value` normalizes to `kv[value]`) |
| `unnest(entries)['key']` inline, then `GROUP BY` | works |
| `unnest` of a NULL list | zero rows |
| `(unnest(entries)).key` / `unnest(entries).key` | **not supported** — `Dot access not supported for non-string expr`; docs must show the subquery or `['key']` spelling |

## Design

### New module layout

```
rust/datafusion-extensions/src/jsonb/
  extract.rs      NEW — shared JSONB extraction, no DataFusion plumbing
  list_udfs.rs    NEW — the three ScalarUDFImpls
  each.rs         MODIFIED — extraction moved to extract.rs
  array_elements.rs MODIFIED — extraction moved to extract.rs
```

`extract.rs` holds the pure extraction functions, lifted verbatim from the two UDTFs so the UDTF
and UDF families cannot drift apart (this is the DRY debt flagged as an open trade-off in
`tasks/completed/jsonb_array_elements_udtf_plan.md`):

```rust
/// Object → (field name, value); array → (index as string, value). None if scalar.
pub fn object_or_array_entries(bytes: &[u8]) -> Result<Option<Vec<(String, Vec<u8>)>>>;

/// Array → elements. None if not an array.
pub fn array_elements(bytes: &[u8]) -> Result<Option<Vec<Vec<u8>>>>;

/// All matches of a parsed JSONPath, as separate values (not wrapped in a JSONB array).
pub fn path_select_all(bytes: &[u8], path: &JsonPath) -> Result<Vec<Vec<u8>>>;
```

The `Option` return is the one shape change against the current UDTF helpers, which fold
"not an object/array" into a `DataFusionError::Execution` inside the extraction function
(`each.rs:106-109`, `array_elements.rs:88-91`). Pushing that decision to the caller lets the
UDTFs keep erroring while the UDFs return NULL (see Semantics). The UDTFs' error text and
behavior stay byte-identical.

### Return types

| Function | Return type |
|---|---|
| `jsonb_entries(jsonb)` | `List<Struct<key: Utf8 (non-null), value: Binary (nullable)>>` |
| `jsonb_elements(jsonb)` | `List<Binary (nullable)>` |
| `jsonb_path_elements(jsonb, path)` | `List<Binary (nullable)>` |

Plain `List`, deliberately **not** `Dictionary<Int32, List<…>>`. Dictionary-wrapping is what makes
`jsonb_object_keys` unusable with `unnest` today, and it is the arrow-row encoding gap behind the
`SELECT DISTINCT` failure noted in the issue.

The struct field names `key` / `value` match both `jsonb_each`'s output columns (`each.rs:80-82`)
and the legacy `List<Struct<key, value>>` properties layout still used by `properties_to_dict`, so
`unnest(jsonb_entries(properties))` reads the same as the pre-JSONB `unnest(properties)` did. The
`value` field is JSONB `Binary` rather than `Utf8`, since a JSONB value can be a nested object or
array; `jsonb_as_string` recovers the string case.

Both argument positions accept `Binary` and `Dictionary<Int32, Binary>` via
`create_binary_accessor`; `path` is `Utf8`, matching `jsonb_path_query`.

### Semantics

| Input per row | `jsonb_entries` | `jsonb_elements` | `jsonb_path_elements` |
|---|---|---|---|
| NULL | NULL list → 0 rows under `unnest` | same | same |
| object | one entry per field | NULL (not an array) | matches of the path |
| array | one entry per element, `key` = index as string | one element per item | matches of the path |
| JSON scalar (number, string, bool) | NULL | NULL | 0 matches → empty list |
| empty object / array | empty list → 0 rows | empty list → 0 rows | empty list → 0 rows |
| invalid JSONPath | — | — | `DataFusionError::Execution`, same text as `jsonb_path_query` |

**NULL rather than an error for a shape mismatch** is the deliberate divergence from the UDTFs,
which raise `Execution` on a non-array/non-object input. A scalar UDF is evaluated per row over a
whole scan, so one oddly-shaped row would abort an entire exploratory query. It also matches
existing precedent: `jsonb_parse` on malformed JSON returns NULL, asserted in
`python/micromegas/tests/test_jsonb.py:14-21`. NULL and empty-list both produce zero rows under
`unnest`, so the distinction is only visible to a caller inspecting the list directly.

### Dictionary fast path

`properties` columns are dictionary-encoded precisely because the same JSONB blob repeats across
many rows. Expanding per row would re-parse the same bytes repeatedly, so when the input is a
`DictionaryArray<Int32, Binary>`:

1. Build the list array once over the dictionary's **unique values** (length = number of distinct
   blobs, not number of rows).
2. `arrow::compute::take(list_array, dict.keys())` to expand to the row count.

`take` supports `ListArray`, and null dictionary keys propagate as null list entries. This is the
same shape as `properties_to_array`'s `take`-based reconstruction (`properties_udf.rs:108-113`) and
`keys.rs`'s dedup via `build_dict_list_array`. The plain-`Binary` path builds row by row with a
`ListBuilder`.

Parsed JSONPath objects are cached in a `HashMap<&str, JsonPath>` keyed on the path string, reusing
the pattern at `path_query.rs:44,53-61`.

### Data flow

```
SELECT unnest(jsonb_entries(properties)) FROM log_entries
                    |
        JsonbEntries::invoke_with_args
                    |
        create_binary_accessor(&args[0])   -- Binary | Dictionary<Int32, Binary>
                    |
        Dictionary?  --yes-->  extract per unique dictionary value
                    |               |
                    |          ListBuilder<StructBuilder> over unique values
                    |               |
                    |          take(list, dict.keys())  -->  ListArray, len = num_rows
                    |
                    no --> ListBuilder<StructBuilder> row by row
                    |
        extract::object_or_array_entries per row
                    |
        ColumnarValue::Array(ListArray)
                    |
        DataFusion's built-in unnest  -->  one struct row per entry, source columns retained
```

## Implementation Steps

### 1. Extract shared JSONB helpers — `rust/datafusion-extensions/src/jsonb/extract.rs` (new)

Move `extract_entries_from_jsonb` (`each.rs:88-113`) and `extract_elements_from_jsonb`
(`array_elements.rs:83-92`) here as `object_or_array_entries` / `array_elements`, changing the
"wrong shape" case from `Err(Execution)` to `Ok(None)`. Add `path_select_all`, which iterates the
matches of a parsed path (`RawJsonb::select_by_path`-family, mirroring the `PathQueryMode::All`
branch at `path_query.rs:66-69`, but returning the values separately rather than wrapped in one
JSONB array).

Update `each.rs` and `array_elements.rs` to call the shared helpers and map `Ok(None)` to their
existing error text, so their behavior is unchanged.

### 2. Implement the UDFs — `rust/datafusion-extensions/src/jsonb/list_udfs.rs` (new)

Three `ScalarUDFImpl`s (`JsonbEntries`, `JsonbElements`, `JsonbPathElements`) plus
`make_jsonb_entries_udf()` / `make_jsonb_elements_udf()` / `make_jsonb_path_elements_udf()`
constructors, following the file conventions in `path_query.rs` and `keys.rs`:
`Signature::any(1 | 2, Volatility::Immutable)`, arity/type failures via `exec_err!` (not
`internal_err!` — see the error-classification entry in `CHANGELOG.md`).

Factor the two `List<Binary>` producers over one shared builder routine parameterized by the
per-row extraction closure; `jsonb_entries` needs its own `ListBuilder<StructBuilder>` variant.
Both variants share the dictionary fast path, so it should be written once and take the
"build list array from these unique blobs" closure.

### 3. Register — `rust/datafusion-extensions/src/jsonb/mod.rs`, `src/lib.rs`

`pub mod extract;` + `pub mod list_udfs;`, then three `ctx.register_udf(...)` calls next to the
existing `jsonb_path_query` registrations (`lib.rs:69-71`). Registration is shared, so the
functions become available to FlightSQL, the monolith, and the WASM query path at once.

### 4. Rust tests — `rust/datafusion-extensions/tests/jsonb_list_udfs_tests.rs` (new)

See Testing Strategy.

### 5. Documentation — `mkdocs/docs/query-guide/functions-reference.md`

Add the three functions to the JSON/JSONB scalar section (after `jsonb_path_query`, before
`jsonb_get` — i.e. around line 440), each with the syntax / parameters / returns / examples shape
used by the neighboring entries. Every example must use a spelling that actually parses: the
subquery form or `unnest(...)['key']`, never `(unnest(...)).key`.

Also, in the same pass:

- Fix `mkdocs/docs/query-guide/functions-reference.md:707`, which documents
  `jsonb_array_elements`' argument as "literal, subquery, **or expression** (e.g.
  `jsonb_path_query(...)`)". That holds only when the expression contains no column reference.
  State the restriction and point at the new UDFs. Same for the `jsonb_each` parameter note at
  line 658.
- Add a per-row expansion example to the "JSON Data Processing" patterns section (line 1470).

### 6. Python integration tests — `python/micromegas/tests/test_jsonb.py`

Add cases in the existing style (plain `client.query`, assertions on the returned DataFrame).

### 7. CHANGELOG entry

One bullet under `## Unreleased` → `* **Analytics:**`, describing the three functions, the
NULL-on-shape-mismatch semantics, and the fact that the UDTFs' uncorrelated-only restriction is
now documented rather than silently hit. Purely additive to the SQL surface — no breaking-change
clause needed.

## Files to Modify

| File | Change |
|---|---|
| `rust/datafusion-extensions/src/jsonb/extract.rs` | **New** — shared extraction helpers |
| `rust/datafusion-extensions/src/jsonb/list_udfs.rs` | **New** — the three `ScalarUDFImpl`s |
| `rust/datafusion-extensions/src/jsonb/each.rs` | Use `extract.rs`; map `Ok(None)` to existing error |
| `rust/datafusion-extensions/src/jsonb/array_elements.rs` | Same |
| `rust/datafusion-extensions/src/jsonb/mod.rs` | Declare the two new modules |
| `rust/datafusion-extensions/src/lib.rs` | Register the three UDFs |
| `rust/datafusion-extensions/tests/jsonb_list_udfs_tests.rs` | **New** — unit + SQL integration tests |
| `mkdocs/docs/query-guide/functions-reference.md` | Document the three functions; fix the UDTF expression-argument claims; add a usage pattern |
| `python/micromegas/tests/test_jsonb.py` | End-to-end cases against a running service |
| `CHANGELOG.md` | Unreleased entry |

## Trade-offs

**List-returning scalars vs. fixing the UDTFs.** Supporting a column reference in
`jsonb_each(t.col)` means correlated table-function evaluation — `LATERAL` — which DataFusion does
not offer at the `TableFunctionImpl` level. `unnest()` already does per-row expansion correctly, so
producing a list and letting `unnest` expand it reuses working machinery instead of rebuilding it.

**Plain `List` vs. `Dictionary<Int32, List<…>>`.** Dictionary-wrapping would compress the repeated
`properties` case, but it is exactly what makes `jsonb_object_keys` unusable with `unnest` today
and breaks arrow-row encoding for `DISTINCT`. The dictionary fast path recovers the *compute*
saving (extract once per unique blob) while the output stays a plain list, which is the part
`unnest` cares about. Memory for the expanded list array is the unavoidable cost of expansion.

**NULL vs. error on shape mismatch.** Erroring matches Postgres and the existing UDTFs, but aborts
a whole scan over heterogeneous JSON on one bad row. NULL follows `jsonb_parse`'s existing
precedent. Recorded as a documented divergence rather than a silent one.

**Three functions vs. one.** `jsonb_elements(j)` is expressible as `jsonb_path_elements(j, '$[*]')`,
and `jsonb_entries` covers arrays too (with the index as `key`). The redundancy is deliberate:
`$[*]` is obscure, and the two array spellings are what a Postgres user reaches for. All three
share one extraction and one builder, so the incremental cost is a signature and a doc entry each.

**`value` as `Binary` vs. `Utf8`.** `Utf8` would make `unnest(jsonb_entries(properties))` directly
readable without a `jsonb_as_string` wrapper, since property values are almost always strings —
but it would silently mangle nested objects and arrays. `Binary` keeps composition with the
existing accessor UDFs. A `properties`-specific `Utf8` convenience wrapper is a possible follow-up,
noted below.

## Out of Scope

Tracked separately under #1475:

- Changing `jsonb_object_keys` to return plain `List<Utf8>` instead of
  `Dictionary<Int32, List<Utf8>>` (would remove the `arrow_cast` workaround and fix `DISTINCT`).
  This is a SQL-surface type change under the interface-stability rule and needs to be a deliberate
  staged change, not a side effect of this work.
- Making the UDTFs *reject* a column-referencing expression with a clear error at plan time. This
  plan only corrects the documentation; the code-level signpost is a separate small change.
- Correlated table functions / `LATERAL`.

## Testing Strategy

`rust/datafusion-extensions/tests/jsonb_list_udfs_tests.rs`, following
`jsonb_path_query_tests.rs`'s harness (`setup_ctx`, `create_binary_table`,
`create_dictionary_table`, `jsonb_to_json_string`):

1. **Return types** — `arrow_typeof` on each function is the exact plain-`List` type above
   (guards against a Dictionary regression, which is the whole point of the design).
2. **`jsonb_entries`** — object → one row per field with correct `key`; array → index keys
   `"0"`, `"1"`, …; nested object value survives as JSONB (`jsonb_format_json` round-trip);
   scalar and NULL inputs → zero rows under `unnest`; empty object → zero rows.
3. **`jsonb_elements`** — array → one row per element; object and scalar → NULL; empty array →
   zero rows.
4. **`jsonb_path_elements`** — `$.items[*]`, a nested path, a no-match path (zero rows), an
   invalid path (error text matches `jsonb_path_query`'s).
5. **Per-row correlation** — the case the issue is about: a multi-row table where each row has a
   different array, asserting each expanded row keeps its own source column. This must fail if
   anyone reintroduces uncorrelated evaluation.
6. **Dictionary input** — same assertions against a `Dictionary<Int32, Binary>` column with
   repeated blobs, including a null dictionary key, so the fast path and the plain path are proven
   to agree row for row.
7. **Composition** — `jsonb_as_string` / `jsonb_get` / `jsonb_as_i64` applied to unnested values;
   `GROUP BY` and `SELECT DISTINCT` over an unnested key column.
8. **UDTF regression** — existing `jsonb_each_tests.rs` and `jsonb_array_elements_tests.rs` must
   pass unchanged after step 1's extraction, including the "not a JSONB array" / "not a JSONB
   object or array" error-text assertions (`jsonb_array_elements_tests.rs:102,117`,
   `jsonb_each_tests.rs:118`).

Then:

- `cargo build`, `cargo clippy --workspace -- -D warnings`, `cargo test -p micromegas-datafusion-extensions`
- Optionally add a case to `rust/datafusion-wasm/tests/wasm_integration.rs` alongside the existing
  `test_jsonb_object_keys` / `test_jsonb_array_length` cases, confirming the UDFs reach the WASM
  query path through shared registration.
- Python end-to-end against local services (`python3 local_test_env/ai_scripts/start_services.py`),
  run from the poetry venv in `python/micromegas`:

  ```sql
  SELECT kv['key'] AS key, count(*) AS n
  FROM (SELECT unnest(jsonb_entries(properties)) AS kv FROM log_entries)
  GROUP BY key ORDER BY n DESC LIMIT 20;
  ```

  This is also the query worth eyeballing for cost on a real `properties` column — it is the
  motivating use case and the one that exercises the dictionary fast path.

## Open Questions

1. **Naming.** This plan uses `jsonb_entries` / `jsonb_elements` / `jsonb_path_elements` — short
   names paired with the Postgres-named UDTFs (`jsonb_each` → `jsonb_entries`,
   `jsonb_array_elements` → `jsonb_elements`). Issue #1475 originally suggested a `_list` suffix
   (`jsonb_path_query_list`), which pairs each list UDF with its scalar counterpart instead. Either
   is defensible; the names are a permanent SQL-surface commitment, so worth confirming before
   implementation.
2. **A `properties`-shaped convenience wrapper.** Since properties values are nearly always
   strings, a `properties_to_pairs(properties)` → `List<Struct<key: Utf8, value: Utf8>>` would make
   the dominant case a one-liner and restore the exact pre-JSONB `unnest(properties)` shape.
   Deliberately left out of this plan — worth adding only if the `jsonb_as_string` wrapper proves
   annoying in practice.
