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
`rust/datafusion-extensions/src/lib.rs:62-76`:

- `jsonb_parse`, `jsonb_format_json`, `jsonb_get`, `jsonb_as_string` / `_f64` / `_i64`,
  `jsonb_array_length`, `jsonb_object_keys`, `jsonb_path_query`, `jsonb_path_query_first`
- Two UDTFs: `jsonb_each` (`jsonb/each.rs`), `jsonb_array_elements` (`jsonb/array_elements.rs`)

Input dispatch over `Binary` / `Dictionary<Int32, Binary>` is already abstracted by
`create_binary_accessor` in `rust/datafusion-extensions/src/binary_column_accessor.rs`;
`path_query.rs:30` uses it, and it is the reuse point for the new UDFs' plain-`Binary` path.
`BinaryColumnAccessor` only exposes `value(i)` / `len()` / `is_null(i)`, with no way to learn
whether the input was dictionary-encoded or to reach the dictionary's keys/values — so the
dictionary fast path (below) cannot be built on it and instead follows `jsonb_object_keys`'s
`DataType::Dictionary<Int32, Binary>` downcast (`keys.rs:94-143`).

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
   falls into the `other =>` branch (`each.rs:67-72`, `array_elements.rs:63-69`), which builds
   `LogicalPlanBuilder::empty(true).project(...)` — an empty relation with no columns, so a
   column reference cannot resolve at plan time. The `tasks/completed/jsonb_array_elements_udtf_plan.md`
   design aimed explicitly at "lateral join patterns"; this branch is why it never delivered them.
2. Even the working uncorrelated subquery form concatenates entries from all rows with no
   source-row column (`each.rs` `JsonbSource::Subquery` scan path), so results cannot be joined
   back to the row they came from.

`properties_to_array` does not help: it only unwraps the dictionary via `take`
(`rust/datafusion-extensions/src/properties/properties_udf.rs:83-121`), so on a JSONB `properties`
column it returns `Binary`.
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
  extract.rs      NEW — shared JSONB extraction, no DataFusion array/UDF plumbing
  list_udfs.rs    NEW — the three ScalarUDFImpls
  each.rs         MODIFIED — extraction moved to extract.rs
  array_elements.rs MODIFIED — extraction moved to extract.rs
```

`extract.rs` holds the pure extraction functions, lifted verbatim from the two UDTFs so the UDTF
and UDF families share one extraction instead of drifting apart. This is a different duplication
than the one recorded as an open trade-off in `tasks/completed/jsonb_array_elements_udtf_plan.md`
(`JsonbSource`, `extract_all_jsonb_bytes_from_column`, and the scalar-to-bytes conversion, still
duplicated across `each.rs` and `array_elements.rs`) — that duplication is out of scope here:

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
(`each.rs:110-112`, `array_elements.rs:88-91`). Pushing that decision to the caller lets the
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

The `jsonb` argument accepts both `Binary` and `Dictionary<Int32, Binary>` encodings via
`create_binary_accessor`; `path` is `Utf8`, matching `jsonb_path_query`.

### Semantics

| Input per row | `jsonb_entries` | `jsonb_elements` | `jsonb_path_elements` |
|---|---|---|---|
| NULL | NULL list → 0 rows under `unnest` | same | same |
| object | one entry per field | NULL (not an array) | matches of the path |
| array | one entry per element, `key` = index as string | one element per item | matches of the path |
| JSON scalar (number, string, bool) | NULL | NULL | 0 matches for a member/index/wildcard path → empty list; `'$'` returns the scalar itself |
| empty object / array | empty list → 0 rows | empty list → 0 rows | empty list → 0 rows |
| invalid/undecodable JSONB bytes | `DataFusionError::External`, same as `jsonb_each`'s decode failure (`each.rs:102`) | same, as `jsonb_array_elements` (`array_elements.rs:91`) | same |
| NULL `path` | n/a | n/a | NULL list → 0 rows (matches `jsonb_path_query`, `path_query.rs:47-48`) |
| invalid JSONPath | — | — | `DataFusionError::Execution`, same message format as `jsonb_path_query`, with this function's name in the prefix |

**NULL rather than an error for a shape mismatch** is the deliberate divergence from the UDTFs,
which raise `Execution` on a non-array/non-object input. A scalar UDF is evaluated per row over a
whole scan, so one oddly-shaped row would abort an entire exploratory query. It also matches
existing precedent: `jsonb_parse` on malformed JSON returns NULL, asserted in
`python/micromegas/tests/test_jsonb.py:14-21`. NULL and empty-list both produce zero rows under
`unnest`, so the distinction is only visible to a caller inspecting the list directly.

**Invalid/undecodable JSONB bytes still error**, unlike the shape-mismatch case above — this is
inherited from `extract.rs`'s `object_or_array_entries` / `array_elements`, which wrap a
jsonb-crate decode failure as `DataFusionError::External` (`each.rs:102`, `array_elements.rs:91`)
before the caller ever gets to decide NULL vs. error on shape. A `Binary` column is expected to
hold valid JSONB (produced by `jsonb_parse`, not arbitrary user bytes), so this is treated as a
genuine data-corruption error rather than a shape mismatch worth swallowing.

### Dictionary fast path

`properties` columns are dictionary-encoded precisely because the same JSONB blob repeats across
many rows. Expanding per row would re-parse the same bytes repeatedly, so when the input is a
`DictionaryArray<Int32, Binary>` (matched directly on `args[0].data_type()`, the `keys.rs` shape —
see "What exists" above):

1. Build the list array once, **positionally aligned with `dict.values()`** (length
   `values.len()`) — not compacted to only the referenced slots. `dict.keys()` holds indices into
   the *original* `dict.values()`, and `take` in step 2 indexes positionally, so a compacted array
   would misalign every row (or panic out-of-bounds); `properties_to_array`'s `take` over the full
   `dict.values()` (`rust/datafusion-extensions/src/properties/properties_udf.rs:108-113`) is the
   correctly-aligned precedent this follows.
   Within that full-length pass, still avoid parsing blobs no surviving row needs: compute the set
   of distinct non-null keys present in `dict.keys()` for this batch, and parse a `values` slot only
   when its index is in that set; every other (unreferenced) slot becomes a null list entry without
   being parsed. This is what keeps an undecodable blob in an unreferenced slot — which a
   sliced/filtered dictionary retains — from erroring the fast path where the row-by-row path would
   not have touched it.

   Gating is on key nullity and referenced-index membership only, with no separate
   `values.is_null(idx)` check — matching `create_binary_accessor`'s dictionary accessor
   (`binary_column_accessor.rs:64-67`) and the dictionary readers in `keys.rs:134-139` /
   `each.rs:176-182`, which all read a referenced slot's bytes regardless of
   `values.is_null(key_index)`. A referenced slot that is itself null therefore parses as an empty
   byte string and fails to decode the same way `create_binary_accessor`-based code fails on that
   row (`Error::InvalidEOF`), so the fast path and the row-by-row path agree row for row — see
   Testing Strategy item 6.
2. `arrow::compute::take(list_array, dict.keys())` to expand to the row count — valid because
   `list_array` has exactly one entry per `dict.values()` slot, the same index space `dict.keys()`
   points into.

`take` supports `ListArray`, and null dictionary keys propagate as null list entries. This is the
same shape as `properties_to_array`'s `take`-based reconstruction
(`rust/datafusion-extensions/src/properties/properties_udf.rs:108-113`) and `keys.rs`'s dedup via
`build_dict_list_array`. The plain-`Binary` path builds row by row with a
`ListBuilder`, using `create_binary_accessor`.

`jsonb_path_elements` takes a second `path` argument, so the per-unique-blob shortcut is only
sound when the result depends on the blob alone — i.e. when `path` is **non-null in every row of
the batch and constant across those rows** (a single distinct non-null value, e.g. a folded
literal; any row with a NULL `path` disqualifies the batch, since the Semantics table requires a
NULL `path` to produce a NULL list, and a per-unique-blob `take` result carries no per-row path
nullity to apply that mask against). In that case the fast path parses the path lazily — only when
it reaches the first referenced slot (i.e., the first non-null key) that actually needs it,
regardless of whether that slot's `values` entry is itself null — and reuses the
parsed `JsonPath` for every subsequent unique blob. This matters when every key in the batch is
null: no slot is ever referenced, so no slot ever gets parsed and an invalid path over such
a column succeeds, exactly as `eval_jsonb_path_query` never parses a path when no row has both a
non-null JSONB and a non-null path (`path_query.rs:46-58`). Parsing up front, before any slot is
known to need it, would error in that all-null case where the row-by-row path would not — the
divergence this laziness avoids. When `path` varies by row or contains any NULL (a column
reference — `Signature::any(2, ..)` allows this, same as `JsonbPathQuery`), the dictionary
shortcut does not apply and `jsonb_path_elements` falls back to the row-by-row builder, reading
`path` per row exactly as `eval_jsonb_path_query` does (`path_query.rs:36-59`), which naturally
handles per-row NULL `path` via `path_query.rs:47-48`. `jsonb_entries` and `jsonb_elements` take
only the `jsonb` argument, so their fast path always applies to dictionary input.

Parsed JSONPath objects are cached in a `HashMap<&str, JsonPath>` keyed on the path string, reusing
the pattern at `path_query.rs:44,53-61`.

### Data flow

```
SELECT unnest(jsonb_entries(properties)) FROM log_entries
                    |
        JsonbEntries::invoke_with_args
                    |
        match args[0].data_type()   -- Binary | Dictionary<Int32, Binary>
                    |
        Dictionary?  --yes-->  downcast to DictionaryArray<Int32Type> (keys.rs shape)
                    |               |
                    |          ListBuilder<StructBuilder>, one entry per dict.values() slot:
                    |          parse only referenced, non-null slots; else append null entry
                    |               |
                    |          take(list, dict.keys())  -->  ListArray, len = num_rows
                    |
                    no --> create_binary_accessor(&args[0])
                    |      ListBuilder<StructBuilder> row by row
                    |
        extract::object_or_array_entries per row / unique value
                    |
        ColumnarValue::Array(ListArray)
                    |
        DataFusion's built-in unnest  -->  one struct row per entry, source columns retained
```

## Implementation Steps

### 1. Extract shared JSONB helpers — `rust/datafusion-extensions/src/jsonb/extract.rs` (new)

Move `extract_entries_from_jsonb` (`each.rs:90-115`) and `extract_elements_from_jsonb`
(`array_elements.rs:83-92`) here as `object_or_array_entries` / `array_elements`, changing the
"wrong shape" case from `Err(Execution)` to `Ok(None)`. Add `path_select_all`, which iterates the
matches of a parsed path (`RawJsonb::select_by_path`-family, mirroring the `PathQueryMode::All`
branch at `path_query.rs:67-70`, but returning the values separately rather than wrapped in one
JSONB array).

Update `each.rs` and `array_elements.rs` to call the shared helpers and map `Ok(None)` to their
existing error text, so their behavior is unchanged.

### 2. Implement the UDFs — `rust/datafusion-extensions/src/jsonb/list_udfs.rs` (new)

Three `ScalarUDFImpl`s (`JsonbEntries`, `JsonbElements`, `JsonbPathElements`) plus
`make_jsonb_entries_udf()` / `make_jsonb_elements_udf()` / `make_jsonb_path_elements_udf()`
constructors, following the file conventions in `path_query.rs` and `keys.rs`:
`Signature::any(1, Volatility::Immutable)` for `jsonb_entries`/`jsonb_elements` and
`Signature::any(2, Volatility::Immutable)` for `jsonb_path_elements`, arity/type failures via
`exec_err!` (not `internal_err!` — see the error-classification entry in `CHANGELOG.md`).

Factor the two `List<Binary>` producers over one shared builder routine parameterized by the
per-row extraction closure; `jsonb_entries` needs its own `ListBuilder<StructBuilder>` variant.
The dictionary fast path (`DictionaryArray<Int32Type>` downcast + `take`, see "Dictionary fast
path") is shared across all three and written once, taking a "build list array from these unique
blobs" closure. `jsonb_path_elements` only invokes it when `path` is constant across the batch;
otherwise it falls back to the row-by-row builder, reading `path` per row like
`eval_jsonb_path_query` does.

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

- Fix `mkdocs/docs/query-guide/functions-reference.md:703`, which documents
  `jsonb_array_elements`' argument as "literal, subquery, **or expression** (e.g.
  `jsonb_path_query(...)`)". That holds only when the expression contains no column reference.
  State the restriction and point at the new UDFs. Same for the `jsonb_each` parameter note at
  line 658.
- Rewrite the `jsonb_array_elements(jsonb_path_query(msg_jsonb, ...))` example at
  `functions-reference.md:430-434` — it's a bare column reference (`msg_jsonb`) inside the UDTF
  argument, exactly the broken form the two notes above now warn against — to the new
  `jsonb_path_elements` + `unnest` form.
- Add a per-row expansion example to the "JSON Data Processing" patterns section (line 1470).

Also update `claude-plugin/skills/micromegas-query/SKILL.md`'s curated function list (the "JSONB
functions" section, which is missing `jsonb_array_length` alongside its other JSONB scalar UDFs):
add `jsonb_array_length(jsonb)` and the three new functions, and add
a note next to `jsonb_each(jsonb)` under "Table functions" that it — like `jsonb_array_elements` —
only works uncorrelated (no column reference in its argument), pointing at the new list UDFs for
per-row expansion. This file is what the "Discovering UDFs" section tells readers to rely on instead
of probing, so it goes stale the same way the mkdocs reference would if left unedited.

### 6. Python integration tests — `python/micromegas/tests/test_jsonb.py`

Every case here runs against a live PostgreSQL + ingestion + flight-sql stack (`test_utils.py`'s
module-level `client`), so scope it to what the in-process Rust tests cannot reach — shape, NULL,
and error edge cases stay in the Rust suite (Testing Strategy items 2-4, 7). Two cases, in the
existing style (plain `client.query`, assertions on the returned DataFrame):

- `unnest(jsonb_entries(properties))` over a real `properties` column (e.g. `log_entries`),
  confirming per-row expansion through the dictionary fast path against genuine ingested data,
  not a hand-built column.
- `unnest(jsonb_path_elements(jsonb_parse(msg), '$.commits[*]'))`-style query over the `msg`
  nested-array case (see the `commits` array assertion in `test_otlp_e2e.py:607`), confirming the
  path-elements UDF against real OTLP JSON bodies.

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
| `claude-plugin/skills/micromegas-query/SKILL.md` | Add the three functions to the JSONB list; note `jsonb_each`'s uncorrelated-only restriction |
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

This choice — erroring rather than returning NULL — is also what the dictionary fast path's extra
machinery is paying for. Because `DictionaryBinaryAccessor::value`
(`binary_column_accessor.rs:64-67`) and the fast path's own reads never check `values.is_null`, and
the jsonb crate errors (`Error::InvalidEOF`) on empty/undecodable bytes, parsing an unreferenced or
null `values` slot would abort the whole batch. That forces the referenced-index gating in
"Dictionary fast path" point 1 (compute the distinct referenced keys, parse only those slots) and
the lazy JSONPath parsing for `jsonb_path_elements` (parse only once a referenced non-null slot
needs it), plus the three dictionary test scenarios these mechanisms motivate in Testing Strategy
item 6. The road not taken: had shape-mismatch NULL extended to decode-failure NULL as well, the
fast path could skip both mechanisms and unconditionally pass over `dict.values()` once per unique
blob. That alternative is not being adopted here — undecodable JSONB still errors, matching the
existing extraction helpers — but the gating and laziness it would have obviated are the price of
keeping that error.

**Three functions vs. one.** `jsonb_elements(j)` is expressible as `jsonb_path_elements(j, '$[*]')`,
and `jsonb_entries` covers arrays too (with the index as `key`). The redundancy is deliberate:
`$[*]` is obscure, and the two array spellings are what a Postgres user reaches for. All three
share one extraction and one builder, so the incremental cost is a signature and a doc entry each.

**`value` as `Binary` vs. `Utf8`.** `Utf8` would make `unnest(jsonb_entries(properties))` directly
readable without a `jsonb_as_string` wrapper, since property values are almost always strings —
but it would silently mangle nested objects and arrays. `Binary` keeps composition with the
existing accessor UDFs. A `properties`-specific `Utf8` convenience wrapper is a possible
follow-up, listed under Out of Scope.

## Out of Scope

Tracked separately under #1475:

- Changing `jsonb_object_keys` to return plain `List<Utf8>` instead of
  `Dictionary<Int32, List<Utf8>>` (would remove the `arrow_cast` workaround and fix `DISTINCT`).
  This is a SQL-surface type change under the interface-stability rule and needs to be a deliberate
  staged change, not a side effect of this work.
- Making the UDTFs *reject* a column-referencing expression with a clear error at plan time. This
  plan only corrects the documentation; the code-level signpost is a separate small change.
- Correlated table functions / `LATERAL`.
- A `properties`-shaped convenience wrapper (`properties_to_pairs(properties)` →
  `List<Struct<key: Utf8, value: Utf8>>`) restoring the exact pre-JSONB `unnest(properties)`
  shape. Deliberately left out of this plan — worth adding only if the `jsonb_as_string` wrapper
  proves annoying in practice.

## Testing Strategy

`rust/datafusion-extensions/tests/jsonb_list_udfs_tests.rs`, following
`jsonb_path_query_tests.rs`'s harness (`setup_ctx`, `create_binary_table`, `create_dict_table`,
`jsonb_to_json_string`). `create_dict_table` builds through `BinaryDictionaryBuilder` with a
non-nullable field and one dictionary entry per input string, so it cannot produce the null-key,
null-values-slot, or unreferenced-slot cases item 6 needs; add a new helper that hand-builds a
`DictionaryArray::<Int32Type>` from caller-supplied keys and values arrays with a nullable field,
for those three cases:

1. **Return types** — `arrow_typeof` on each function is the exact plain-`List` type above
   (guards against a Dictionary regression, which is the whole point of the design).
2. **`jsonb_entries`** — object → one row per field with correct `key`; array → index keys
   `"0"`, `"1"`, …; nested object value survives as JSONB (`jsonb_format_json` round-trip);
   scalar and NULL inputs → zero rows under `unnest`; empty object → zero rows.
3. **`jsonb_elements`** — array → one row per element; object and scalar → NULL; empty array →
   zero rows.
4. **`jsonb_path_elements`** — `$.items[*]`, a nested path, a no-match path (zero rows), an
   invalid path (error message format matches `jsonb_path_query`'s, with this function's name in
   the prefix).
5. **Per-row correlation** — the case the issue is about: a multi-row table where each row has a
   different array, asserting each expanded row keeps its own source column. This must fail if
   anyone reintroduces uncorrelated evaluation.
6. **Dictionary input** — via `create_dict_table`: same assertions against a `Dictionary<Int32,
   Binary>` column with repeated blobs. Via the new hand-built-`DictionaryArray` helper: a null
   dictionary key, so the fast path and the row-by-row `create_binary_accessor` path agree row for
   row on key nullity. Separately, a null value in the dictionary's *values* array referenced by a
   non-null key: assert the fast path and the row-by-row `create_binary_accessor` path agree row for
   row here too — both parse that slot's empty bytes and fail the same way (`Error::InvalidEOF`),
   since neither path special-cases `values.is_null` (see "Dictionary fast path" point 1). Also a
   sliced/filtered dictionary case — a values array containing blobs no surviving row's key
   references, including an undecodable one in an unreferenced slot — asserting the fast path does
   not error on it and does not degrade into parsing every value.
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
  run from the poetry venv in `python/micromegas` — the two cases from Implementation Steps §6,
  not a re-test of the shape/NULL/error coverage already in the Rust suite:

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
