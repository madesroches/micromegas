# Make `retire_partition_by_metadata` Schema-Hash Aware Plan

## Overview

`retire_partition_by_metadata` targets a partition by `(view_set_name, view_instance_id,
begin_insert_time, end_insert_time)` and never looks at `file_schema_hash`. That tuple is not
unique: the `lakehouse_partitions_no_overlap` exclusion constraint is scoped *by*
`file_schema_hash`, so an old-schema partition and a current-schema partition may legally share
one insert range. When they do, the UDF's unbounded `DELETE` removes both — silently dropping
live data — and its `fetch_optional` registers only one of the two files for cleanup, orphaning
the other in object storage. Neither shows up as an error, because `rows_affected()` is only
compared against `0`.

This plan adds an optional fifth `file_schema_hash` argument that makes the target unambiguous,
and makes the four-argument form fail closed instead of over-deleting. `retire_incompatible_partitions`
— which detects partitions *by* hash mismatch and then throws the hash away — is updated to pass it
through.

## Current State

### The UDF

`rust/analytics/src/lakehouse/retire_partition_by_metadata_udf.rs`:

- `RetirePartitionByMetadata::new` (:52) declares `Signature::exact` over four types:
  `Utf8, Utf8, Timestamp(ns, "+00:00"), Timestamp(ns, "+00:00")`.
- `retire_partition_in_transaction` (:75–150) runs two hash-blind statements against
  `lakehouse_partitions`: a `SELECT file_path, file_size ... fetch_optional` (:87) and a
  `DELETE ...` (:126). Between them it calls `add_file_for_cleanup`
  (`write_partition.rs:30`) for the single row it read.
- The only post-delete check is `delete_result.rows_affected() == 0` (:147). A two-row delete
  reports `SUCCESS:`.
- `invoke_async_with_args` (:189) hard-asserts `args.len() != 4`, downcasts the four arrays,
  and drives one transaction across the whole batch, rolling back if any row errored.

### Why two rows can share the key

`rust/analytics/src/lakehouse/migration.rs:512-517` documents cross-schema coexistence as legal:
the exclusion constraint includes `file_schema_hash WITH =`, so after a schema-hash bump an
old-schema partition keeps overlapping new-schema writes until it is retired. Queries filter
partitions by hash, so the overlap is invisible to readers. That is precisely the state this UDF
exists to clean up, and precisely the state in which its key is ambiguous.

Concurrency makes it worse than a static-state problem: the transaction runs at READ COMMITTED, so
a materialization committing between the `SELECT` and the `DELETE` can turn a one-row read into a
two-row delete even when the caller's snapshot saw a unique match.

### The Python caller

`python/micromegas/micromegas/admin.py`:

- `list_incompatible_partitions` (:12) selects `p.file_schema_hash as incompatible_schema_hash`
  alongside the four metadata columns (the column is `DataType::Binary` in
  `list_partitions_table_function.rs:110`).
- `retire_incompatible_partitions` (:175–186) then formats a `retire_partition_by_metadata(...)`
  call with only the four metadata columns. The one column that disambiguates the target is
  dropped between detection and retirement.

### Out of scope

`retire_partitions` in `write_partition.rs` is also hash-blind, correctly so: it runs in the same
transaction as the replacement insert and is meant to sweep old-schema partitions.
`retire_partition_by_file` keys on a unique file path and is unaffected.

## Design

### 1. Optional fifth argument

Widen the signature to accept either arity, so every existing four-argument caller — saved admin
queries, the documented batch example — keeps planning:

```rust
Signature::one_of(
    vec![
        TypeSignature::Exact(vec![Utf8, Utf8, TS, TS]),
        TypeSignature::Exact(vec![Utf8, Utf8, TS, TS, DataType::Binary]),
    ],
    Volatility::Volatile,
)
```

`Binary` is the type `list_partitions().file_schema_hash` already has, so the batch form needs no
cast:

```sql
SELECT retire_partition_by_metadata(
    p.view_set_name, p.view_instance_id, p.begin_insert_time, p.end_insert_time, p.file_schema_hash)
FROM list_partitions() p JOIN list_view_sets() vs USING (view_set_name)
WHERE p.file_schema_hash != vs.current_schema_hash;
```

A hand-written literal uses `decode('04', 'hex')`, which returns `DataType::Binary` in DataFusion
54 (`datafusion-functions/src/encoding/inner.rs:185`) and matches the pattern already documented in
`mkdocs/docs/query-guide/schema-reference.md:558`.

`invoke_async_with_args` accepts `4 | 5` arguments, downcasts `args[4]` as `BinaryArray` when
present, and treats a null in it exactly like a null in any other argument (per-row
`"ERROR: all arguments must be non-null"`).

### 2. Resolve to exactly one row, then delete that row by all five columns

`retire_partition_in_transaction` gains a `file_schema_hash: Option<&[u8]>` parameter. One `SELECT`
serves both arities:

```sql
SELECT file_path, file_size, file_schema_hash FROM lakehouse_partitions
 WHERE view_set_name = $1 AND view_instance_id = $2
   AND begin_insert_time = $3 AND end_insert_time = $4
   AND ($5::bytea IS NULL OR file_schema_hash = $5)
```

with `fetch_all` instead of `fetch_optional`, then:

- **0 rows** → bail `Partition not found: ...` (today's message, with the hash appended when one
  was given).
- **>1 rows** → bail naming the colliding hashes and pointing at the fix, e.g.
  `Ambiguous partition <view_set>/<instance> [<begin>, <end>): 2 partitions match, with
  file_schema_hash 04 and 05 — pass file_schema_hash as a fifth argument to pick one`.
  Only reachable on the four-argument path.
- **1 row** → `add_file_for_cleanup` for its `file_path` (unchanged), then `DELETE` keyed on all
  five columns using the hash **read from that row**, regardless of which arity the caller used.

Deleting by the resolved row's own hash is what closes the READ COMMITTED race: even if a
concurrent writer commits a new-schema partition on the same range between the two statements, the
`DELETE` cannot reach it. The post-delete check tightens from `rows_affected() == 0` to
`rows_affected() != 1`, so any surviving surprise aborts the batch and rolls back rather than
reporting `SUCCESS:`.

Since exactly one row is ever deleted, "register every returned `file_path` for cleanup" collapses
into the existing single-file call — the leak the issue describes is fixed by never deleting the
second row in the first place.

### 3. Success message carries the hash

`SUCCESS: Retired partition <view_set>/<instance> [<begin>, <end>) schema_hash=<hex>` — so a
four-argument caller can see after the fact which of several coexisting schema versions it hit.
`retire_incompatible_partitions` only tests `message.startswith("SUCCESS:")`, so the prefix
contract is preserved.

### 4. Python passes the hash through

In `retire_incompatible_partitions`, format the fifth argument from the
`incompatible_schema_hash` column the detection query already returns:

```python
schema_hash_hex = bytes(partition["incompatible_schema_hash"]).hex()
# ... decode('{schema_hash_hex}', 'hex')
```

`bytes(...)` normalizes whatever pandas hands back for an Arrow `Binary` column (`bytes`,
`bytearray`, or `memoryview`). The value is hex, so it cannot carry a quote — no new injection
surface beyond the f-string SQL this function already builds.

## Implementation Steps

1. **Signature and argument plumbing** (`retire_partition_by_metadata_udf.rs`)
   - Replace `Signature::exact` with `Signature::one_of` over the 4- and 5-type exact variants.
   - In `invoke_async_with_args`, allow `args.len()` of 4 or 5, downcast `args[4]` as
     `BinaryArray` into an `Option`, extend the per-row null check to it, and pass
     `Option<&[u8]>` down.
   - Update the `internal_err!` arity message.

2. **Fail-closed resolution** (same file, `retire_partition_in_transaction`)
   - Add the `file_schema_hash: Option<&[u8]>` parameter.
   - Add `file_schema_hash` to the `SELECT` projection and the `($5::bytea IS NULL OR ...)`
     predicate; switch to `fetch_all`.
   - Add the empty / ambiguous / single-row branches described above.
   - Key the `DELETE` on the resolved row's hash; assert `rows_affected() == 1`.
   - Refresh the doc comments on the struct (:22–30) and on
     `make_retire_partition_by_metadata_udf` (:305–325), including the SQL usage example and the
     documented return strings.

3. **Python** (`python/micromegas/micromegas/admin.py`)
   - Pass `decode('<hex>', 'hex')` as the fifth argument in `retire_incompatible_partitions`.
   - Update its `Note:` docstring paragraph, which currently enumerates the four identifiers.

4. **Rust test** — new `rust/analytics/tests/retire_partition_by_metadata_db_test.rs`
   (`#[ignore]`, live-DB, following `net_spans_retire_overlap_db_test.rs` for synthetic partition
   rows and `prong_b_guard_db_test.rs` for an admin `make_session_context`).

5. **Python test** (`python/micromegas/tests/test_admin.py`) — extend `MockFlightSQLClient` and
   add SQL-shape assertions.

6. **Docs** — `mkdocs/docs/admin/functions-reference.md`,
   `mkdocs/docs/query-guide/functions-reference.md`.

7. **CHANGELOG** — entry under `## Unreleased` → **Analytics**.

## Files to Modify

- `rust/analytics/src/lakehouse/retire_partition_by_metadata_udf.rs`
- `rust/analytics/tests/retire_partition_by_metadata_db_test.rs` (new)
- `python/micromegas/micromegas/admin.py`
- `python/micromegas/tests/test_admin.py`
- `mkdocs/docs/admin/functions-reference.md`
- `mkdocs/docs/query-guide/functions-reference.md`
- `CHANGELOG.md`

## Trade-offs

**Optional fifth argument vs. required fifth argument.** The issue's primary suggestion is a
required fifth argument. That is a SQL-surface break: the documented batch example, any saved
admin query, and `lakehouse_admin_gate_test.rs:77` all call the four-argument form, and under this
repo's interface rule the SQL layer stays compatible while Rust APIs may churn. `Signature::one_of`
makes the change purely additive at no real cost — the four-argument path is still *correct*
(it just refuses to guess), and the five-argument path is available wherever the caller knows the
hash.

**Fail closed vs. delete-all-and-clean-up-every-file.** Deleting every matching row and registering
each file for cleanup would fix the storage leak but keep the data loss: the current-schema
partition would still disappear. Erroring is the only behavior that preserves the invariant the
caller actually wants.

**Delete by the resolved row's hash vs. by the caller's argument.** Using the argument would leave
the four-argument path's `DELETE` hash-blind, so the READ COMMITTED race stays open. Reading the
hash back from the `SELECT` costs one extra column and closes it for both arities.

## Decisions

- The success message gains a `schema_hash=<hex>` suffix; only the `SUCCESS:`/`ERROR:` prefix is
  treated as contract.
- A null fifth argument is an error, not a fallback to the four-argument behavior — "I have no hash"
  is spelled by omitting the argument.

## Documentation

- `mkdocs/docs/admin/functions-reference.md:157` — update the `###` heading to the five-argument
  signature, document the optional parameter, document the ambiguity error under **Safety**, and
  change the "Batch retire incompatible partitions" example (:204) to pass `p.file_schema_hash`.
  Changing the heading changes its slug, so update the cross-link in
  `mkdocs/docs/query-guide/functions-reference.md:97` in the same commit; `mkdocs/site/` is
  generated output and is not edited by hand.
- `mkdocs/docs/query-guide/functions-reference.md:93` — mirror the signature in the heading.
- `mkdocs/docs/admin/maintenance.md:234` and `mkdocs/docs/query-guide/python-api.md:640` mention
  the function only by name and by admin requirement; no change needed.

## Testing Strategy

**Rust, live DB** (`retire_partition_by_metadata_db_test.rs`, `#[ignore]`, requires
`MICROMEGAS_SQL_CONNECTION_STRING` / `MICROMEGAS_OBJECT_STORE_URI`). Insert synthetic
`lakehouse_partitions` rows under a unique `view_instance_id` per test and drive the UDF through
SQL on an admin `SessionContext`:

1. **Collision, four arguments** — two rows, same range, hashes `[4]` and `[5]`. The result string
   starts with `ERROR:` and names both hashes; both rows survive; no `temporary_files` row was
   added for either file. This is the regression test for the issue.
2. **Collision, five arguments** — same fixture, passing `decode('04','hex')`. Only the `[4]` row
   is gone, the `[5]` row survives, and exactly one `temporary_files` row exists, for the `[4]`
   file.
3. **Unique partition, four arguments** — unchanged behavior: `SUCCESS:`, row deleted, file queued.
4. **Five arguments, hash does not match** — `ERROR: Partition not found`, row untouched.
5. **Empty partition** (`file_path IS NULL`) retires with no `temporary_files` insert, confirming
   the `fetch_optional` → `fetch_all` switch did not regress the NULL-file path.

**Rust, offline** — extend `MUTATING_UDF_CALLS` in `lakehouse_admin_gate_test.rs:77` with the
five-argument spelling, so both arities stay behind the admin gate and both keep planning.

**Python** — in `test_admin.py`, assert the SQL that `retire_incompatible_partitions` emits
contains `decode('<expected hex>', 'hex')` for a fixture whose `incompatible_schema_hash` is
`b"\x04"`, and that the retirement still succeeds end-to-end through the mock. Existing tests must
keep passing after `MockFlightSQLClient`'s regex is widened for the extra argument.

**Manual** — against the local test env, bump a view's `SCHEMA_VERSION`, materialize over a range
already covered by an old-schema partition to create a real collision, then confirm
`list_incompatible_partitions` still reports the old row and `retire_incompatible_partitions`
removes only it.

## Open Questions

None blocking. One thing to confirm at implementation time: that DataFusion 54's `OneOf` coercion
accepts a `BinaryView`-typed expression for the `Binary` slot (relevant only if a caller passes a
value that plans as `BinaryView`; `list_partitions().file_schema_hash` and `decode(...)` both
produce `Binary`). If it does not, the five-argument variant can additionally accept `Utf8` and
hex-decode in Rust.
