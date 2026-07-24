# Dictionary Key Overflow in Span/Event/Metrics/Log Table Builders Plan

GitHub issue: https://github.com/madesroches/micromegas/issues/1341

## Overview

`span_table.rs`, `async_events_table.rs`, `net_spans_table.rs`, `metrics_table.rs`, and
`log_entries_table.rs` build several columns with `StringDictionaryBuilder<Int16Type>` and
append via the infallible `append_value()`/`append_values()`. `Int16Type` caps the
dictionary at 32,767 distinct values per `RecordBatch`; Arrow's
`GenericByteDictionaryBuilder::append_value()` panics once that cap is exceeded
(`arrow-array-58.3.0/src/builder/generic_bytes_dictionary_builder.rs:328`). A query over a
wide enough time range (many distinct call sites, blocks, or streams) can accumulate more
than 32,767 unique values for one of these columns in a single batch, panicking the
background query task. This was observed in production inside
`thread_block_processor::parse_thread_block`, degrading the flightsql server silently (the
gRPC listener and health checks kept working while query processing was broken).

This plan widens the dictionary key type to `Int32Type` (~2.1B ceiling, matching the
existing `process_properties`/`properties` binary-dictionary pattern) and replaces every
panicking `append_value`/`append_values` call on these builders with the fallible
`append`/`append_n`, propagated with `?`. This mirrors the read-side precedent already set
in this codebase for the analogous problem (commit `d1747f37a`, "Return error instead of
panic in `StringColumnAccessor::value()`") — same problem class, same fix philosophy, write
side this time.

## Current State

### The panic

```rust
// rust/analytics/src/span_table.rs
names: StringDictionaryBuilder<Int16Type>,
...
self.names.append_value(&*row.name);
```

`GenericByteDictionaryBuilder::append_value()` (arrow-array) is:

```rust
pub fn append_value(&mut self, value: impl AsRef<T::Native>) {
    self.append(value).expect("dictionary key overflow");
}
```

`append()` itself is fallible — `Result<K::Native, ArrowError>` — and is already exactly
what's needed; only the panicking wrapper is in use today.

### Where the pattern lives

Five builders match the issue's description (`StringDictionaryBuilder<Int16Type>` fields,
`append`/`finish` methods already returning `anyhow::Result`, only the inner dictionary
calls are infallible):

| File | Builder | Int16 dictionary columns |
|---|---|---|
| `analytics/src/span_table.rs` | `SpanRecordBuilder` | `names`, `targets`, `filenames` |
| `analytics/src/async_events_table.rs` | `AsyncEventRecordBuilder` | `stream_ids`, `block_ids`, `event_types`, `names`, `filenames`, `targets` |
| `analytics/src/net_spans_table.rs` | `NetSpanRecordBuilder` | `process_ids`, `stream_ids`, `kinds`, `names`, `connection_names` |
| `analytics/src/metrics_table.rs` | `MetricsRecordBuilder` | `process_ids`, `stream_ids`, `block_ids`, `exes`, `usernames`, `computers`, `targets`, `names`, `units` |
| `analytics/src/log_entries_table.rs` | `LogEntriesRecordBuilder` | `process_ids`, `stream_ids`, `block_ids`, `exes`, `usernames`, `computers`, `targets` |

The issue calls out `block_ids`/`stream_ids`/`process_ids` in the last two specifically
("scale with query time-range size"), but every `Int16Type` column in a given builder
struct hits the same 32,767 ceiling and the same panic path — there's no reason to widen
some columns in a struct and not others, so this plan widens every `Int16Type` field in
all five builders.

`properties` (`PropertySetJsonbDictionaryBuilder`, metrics/log) and `process_properties`
(`BinaryDictionaryBuilder<Int32Type>`) are already `Int32Type` and out of scope — they're
the precedent this plan follows.

### Call sites are already `Result`-clean

Every external caller of these builders' `append`/`append_entry_only`/`fill_constant_columns`
methods already propagates `Result` with `?` (e.g. `lakehouse/metrics_block_processor.rs:40`,
`lakehouse/log_block_processor.rs:40`, `span_table.rs`'s own `append_call_tree`). No caller
reaches into the dictionary builders directly — the panicking calls are entirely internal to
the five files above, so **no call-site changes are needed**; the fix is contained to the
builder methods themselves.

### Mandatory companions: OTLP block processors build the same schema locally

`lakehouse/otel/logs_block_processor.rs` and `lakehouse/otel/metrics_block_processor.rs` do
**not** reuse `LogEntriesRecordBuilder`/`MetricsRecordBuilder`. They build their own local
`StringDictionaryBuilder::<Int16Type>` fields, then construct the final `RecordBatch` against
`crate::log_entries_table::log_table_schema()` /
`crate::metrics_table::metrics_table_schema()` directly
(`logs_block_processor.rs:214-215`, `metrics_block_processor.rs:250-251`). Once those schema
functions are widened to `Int32`, these two processors' local builders **must** widen in
lockstep — otherwise `RecordBatch::try_new` fails with a schema/array type mismatch on every
OTLP log/metric ingest. This is not optional cleanup; it's required for the change to be
correct.

`lakehouse/process_spans_table_function.rs` also builds local
`StringDictionaryBuilder::<Int16Type>` fields (`stream_id`, `thread_name`), but each is a
single value repeated `n` times via `append_values` — one unique value per batch, never at
risk of the overflow — and it's a JIT table function with no materialized file schema. Out
of scope.

`analytics/src/images_table.rs` (`ImagesRecordBuilder`) has the identical
`StringDictionaryBuilder<Int16Type>` / panicking-`append_value` pattern
(`process_ids`/`stream_ids`/`block_ids`/`exes`/`usernames`/`computers`/`formats`) but isn't
named in the issue. Same bug class, same fix, lower likelihood of hitting it in practice
(image events are much lower volume). See Open Questions.

### Materialized-view schema versioning already handles the migration

Each affected table's schema is gated behind a view-level `SCHEMA_VERSION` constant used for
both `get_schema_hash()` (live/JIT schema) and `get_file_schema_hash()` (materialized parquet
schema):

| Table | View file | Current `SCHEMA_VERSION` |
|---|---|---|
| spans | `lakehouse/thread_spans_view.rs:33` | `0` |
| async events | `lakehouse/async_events_view.rs:34` | `2` |
| net spans | `lakehouse/net_spans_view.rs:33` | `0` |
| metrics | `lakehouse/metrics_view.rs:39` | `5` |
| log entries | `lakehouse/log_view.rs:35` | `5` |
| images (optional) | `lakehouse/images_view.rs:35` | `1` |

`PartitionCache::filter` (`lakehouse/partition_cache.rs:223-245`) matches materialized
partitions to a view by **exact** `file_schema_hash` equality, and
`MaterializedView::scan` only fetches partitions matching the view's *current* hash — so
bumping `SCHEMA_VERSION` is the existing, established way to signal "this table's on-disk
format changed": old-schema partitions are simply excluded from future queries (not read
with a mismatched schema — no risk of a read-side type-mismatch crash), and the daemon/JIT
path transparently recomputes new partitions from source going forward
(`batch_update.rs`'s `verify_overlapping_partitions`, `jit_partitions.rs:571-576`
`is_jit_partition_up_to_date`).

The trade-off: this is **not** an automatic backfill. Historical time ranges whose
partitions were materialized under the old hash won't be visible under the new hash until
something (re-)materializes that range. For the daemon-driven tables here, ongoing
materialization naturally recomputes recent/future ranges; older ranges stay queryable only
via JIT recomputation (already happens per-query for on-demand views) or by running the
existing `regenerate_partitions(view_set_name, begin, end, delta)` table function for a
range, followed by `retire_partitions`/`retire_partition_by_metadata` to drop the stale rows
once satisfied. This plan bumps every affected `SCHEMA_VERSION` by 1 as a normal part of the
fix (see Implementation Steps) — it's required regardless, since the on-disk Arrow schema is
changing.

### Tests that assume `Int16Type`

- `analytics/tests/net_spans_test.rs` — `collect_rows` hard-downcasts five columns to
  `DictionaryArray<Int16Type>` (`process_id`, `stream_id`, `kind`, `name`,
  `connection_name`).
- `analytics/tests/sql_view_test.rs` — `LogSummaryMerger::execute_merge_query` downcasts a
  `process_id` column (sourced from `log_entries` via SQL) to `DictionaryArray<Int16Type>`
  (line 135), and `test_log_summary` asserts an exact `ref_schema` `Field` with
  `DataType::Dictionary(DataType::Int16.into(), DataType::Utf8.into())` (line 385) plus a
  hardcoded `ref_schema_hash` byte vector (line 396) for the derived `SqlBatchView`
  `"log_entries_per_process_per_minute"`. That view's `get_file_schema_hash()`
  (`sql_batch_view.rs:190-194`) hashes the *probed* Arrow schema with `DefaultHasher` — it
  isn't a manual `SCHEMA_VERSION`, so once `log_entries_table`'s `process_id` column widens,
  this derived schema and its hash change automatically and the literal must be updated to
  match.
- `analytics/tests/async_events_tests.rs` and `analytics/tests/thread_spans_ordering_tests.rs`
  use higher-level APIs only — no `Int16Type` references, no changes needed.

## Design

### 1. Widen dictionary key type: `Int16Type` → `Int32Type`

In each of the five (six, if images is included) table files:
- Swap every `StringDictionaryBuilder<Int16Type>` field to `StringDictionaryBuilder<Int32Type>`.
- Swap every `Field::new(..., DataType::Dictionary(Box::new(DataType::Int16), ...), ...)` in
  the corresponding `*_table_schema()`/`get_*_schema()` function to
  `DataType::Dictionary(Box::new(DataType::Int32), ...)`.
- Update the `Int16Type` import to `Int32Type` (some files, e.g. `metrics_table.rs`, already
  import `Int32Type` for `process_properties`/`properties` — reuse it, don't double-import).

### 2. Replace panicking calls with fallible calls, propagated with `?`

Mechanical, one-to-one:
- `builder.append_value(x)` → `builder.append(x)?;`
- `builder.append_values(x, n)` → `builder.append_n(x, n)?;`

Every one of these calls already sits inside a function returning `anyhow::Result<()>`
(`append`, `append_entry_only`, `fill_constant_columns`, `append_call_tree`'s inner closure),
so `?` needs no new error-handling plumbing — `ArrowError` implements `std::error::Error`,
so anyhow's blanket conversion applies directly (matches the existing style elsewhere in the
crate: `.append_property_set(...)?`, `self.properties.finish()?`).

No `.with_context(...)` is added on these calls — `RecordBatch::try_new(...).with_context(...)`
at the end of each `finish()` already gives a build-time error location, and a bare
`ArrowError::DictionaryKeyOverflowError` is self-describing; matching precedent
(`append_property_set(&row.properties)?` has no added context either).

### 3. Companion OTLP processors

Apply the same two changes (Int32 key + fallible append) to the local builders in
`lakehouse/otel/logs_block_processor.rs` and `lakehouse/otel/metrics_block_processor.rs`.
`logs_block_processor.rs` appends inline inside `process()` (returns
`Result<Option<PartitionRowSet>>`), so the same `?`-propagation applies directly.
`metrics_block_processor.rs` is different: all its dictionary appends live inside the
private `MeasuresRowBuilder::append(&mut self, …)` helper, which currently returns `()` and
is called without `?` from the two `process()` match arms (`Data::Sum`, `Data::Gauge`).
Making its internal calls fallible requires changing `MeasuresRowBuilder::append`'s
signature to `-> Result<()>` (turning its two early `return;` statements into
`return Ok(());`, and its final line into `Ok(())`), and adding `?` to both call sites in
`process()`.

### 4. Bump `SCHEMA_VERSION`

Bump the `const SCHEMA_VERSION: u8` in each affected view file by 1:
`thread_spans_view.rs` (0→1), `async_events_view.rs` (2→3), `net_spans_view.rs` (0→1),
`metrics_view.rs` (5→6), `log_view.rs` (5→6), and `images_view.rs` (1→2) if images is
included. This is what signals to the partition cache that on-disk partitions need
recomputing (see Current State).

## Implementation Steps

1. **`span_table.rs`** — widen `names`/`targets`/`filenames` to `Int32Type`; convert their
   three `append_value` calls in `append()` to `append(...)?`.
2. **`async_events_table.rs`** — widen all six `Int16Type` fields to `Int32Type`; convert
   the six `append_value` calls in `append()` to `append(...)?`.
3. **`net_spans_table.rs`** — widen all five `Int16Type` fields to `Int32Type`; convert the
   five `append_value` calls in `append()` to `append(...)?`.
4. **`metrics_table.rs`** — widen all nine `Int16Type` fields to `Int32Type`; convert
   `append_value` calls in `append()` (9 calls) and `append_entry_only()` (3 calls) to
   `append(...)?`; convert `append_values` calls in `fill_constant_columns()` (6 calls) to
   `append_n(...)?`.
5. **`log_entries_table.rs`** — widen all seven `Int16Type` fields to `Int32Type`; convert
   `append_value` calls in `append()` (7 calls) and `append_entry_only()` (1 call) to
   `append(...)?`; convert `append_values` calls in `fill_constant_columns()` (6 calls) to
   `append_n(...)?`.
6. **`lakehouse/otel/logs_block_processor.rs`** — widen the 7 local `Int16Type` builders to
   `Int32Type`; convert their `append_value`/`append_values` calls (mirrors step 5's field
   list) to fallible equivalents.
7. **`lakehouse/otel/metrics_block_processor.rs`** — widen the 9 local `Int16Type` builders
   to `Int32Type`; convert their `append_value`/`append_values` calls (mirrors step 4's field
   list) to fallible equivalents. Unlike step 6, these calls live inside the private
   `MeasuresRowBuilder::append` helper, not `process()` itself: change that method's
   signature from `fn append(&mut self, …)` to `fn append(&mut self, …) -> Result<()>`,
   convert its two early `return;` statements to `return Ok(());`, end it with `Ok(())`, and
   add `?` to both call sites (`Data::Sum` and `Data::Gauge` arms) in `process()`.
8. **Schema versions** — bump `SCHEMA_VERSION` in `thread_spans_view.rs`,
   `async_events_view.rs`, `net_spans_view.rs`, `metrics_view.rs`, `log_view.rs` (and
   `images_view.rs`, if in scope) per Design §4.
9. **Fix `analytics/tests/net_spans_test.rs`** — change the five `DictionaryArray<Int16Type>`
   downcasts (and the `Int16Type` import) in `collect_rows` to `Int32Type`.
10. **Fix `analytics/tests/sql_view_test.rs`** — change the `Int16Type` downcast at line 135
    (and its import) to `Int32Type`; update the `ref_schema` `Field` at line 385 to
    `DataType::Dictionary(DataType::Int32.into(), DataType::Utf8.into())`; run the test,
    read the new hash out of the assertion failure (or compute it directly), and update the
    `ref_schema_hash` literal at line 396.
11. **New regression test** — add `analytics/tests/dictionary_key_overflow_tests.rs` per
    Testing Strategy, proving each production builder accepts more than 32,767 distinct
    dictionary values without panicking, plus the two OTLP companion tests
    (`OtelLogsBlockProcessor`/`OtelMetricsBlockProcessor` via `BlockProcessor::process`) that
    prove steps 6/7's "mandatory companions" fix.
12. **Verify** — `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test` from
    `rust/`.

## Files to Modify

- `rust/analytics/src/span_table.rs`
- `rust/analytics/src/async_events_table.rs`
- `rust/analytics/src/net_spans_table.rs`
- `rust/analytics/src/metrics_table.rs`
- `rust/analytics/src/log_entries_table.rs`
- `rust/analytics/src/lakehouse/otel/logs_block_processor.rs`
- `rust/analytics/src/lakehouse/otel/metrics_block_processor.rs`
- `rust/analytics/src/lakehouse/thread_spans_view.rs` (`SCHEMA_VERSION` bump)
- `rust/analytics/src/lakehouse/async_events_view.rs` (`SCHEMA_VERSION` bump)
- `rust/analytics/src/lakehouse/net_spans_view.rs` (`SCHEMA_VERSION` bump)
- `rust/analytics/src/lakehouse/metrics_view.rs` (`SCHEMA_VERSION` bump)
- `rust/analytics/src/lakehouse/log_view.rs` (`SCHEMA_VERSION` bump)
- `rust/analytics/tests/net_spans_test.rs`
- `rust/analytics/tests/sql_view_test.rs`
- `rust/analytics/tests/dictionary_key_overflow_tests.rs` (new)
- `rust/analytics/src/lakehouse/view_factory.rs` (module-level doc comments)
- `mkdocs/docs/query-guide/schema-reference.md` (field type tables + Dictionary Compression note)
- Optional (see Open Questions): `rust/analytics/src/images_table.rs`,
  `rust/analytics/src/lakehouse/images_view.rs`

## Trade-offs

- **`Int32Type` vs. a fully dynamic/checked key width.** The issue's own suggested fix is
  widening plus fallible appends, matching the existing `process_properties`/`properties`
  precedent — no need to invent a new pattern (e.g. auto-promoting key width at runtime,
  which Arrow's builders don't support anyway). `Int32` (2.1B) is effectively unreachable for
  a single batch in practice; the fallible `append`/`append_n` conversion is what actually
  eliminates the panic class (a sufficiently pathological query could still exceed even
  `Int32` and would now get a clean `Err` instead of a crash).
- **Widening every `Int16Type` field in a builder vs. only the columns the issue names.**
  The issue explicitly flags `block_ids`/`stream_ids`/`process_ids` for the metrics/log
  tables, but every other `Int16Type` field in those same structs shares the identical
  32,767-value ceiling and panic path (e.g. `metrics_table.rs`'s `names`/`targets` are
  per-row and can be just as numerous as `block_ids` over a wide time range). Leaving some
  columns at `Int16` and others at `Int32` in the same struct would be an inconsistent,
  easy-to-forget half-fix for no memory savings worth mentioning (a dictionary key column is
  a few bytes per row either way).
- **Bumping `SCHEMA_VERSION` (forcing regeneration) vs. leaving it unchanged.** The Arrow
  schema of every affected table is changing on disk (`Dictionary(Int16, ...)` →
  `Dictionary(Int32, ...)`), which parquet/Arrow readers treat as a different, incompatible
  physical schema. Not bumping the version would either break reads of newly-written
  partitions mixed with old ones under the same hash, or require some other coercion
  mechanism that doesn't exist today. Bumping the version is the established, low-risk path
  already used for schema changes in this codebase.

## Documentation

`lakehouse/view_factory.rs`'s module-level doc comments (lines ~76-112) describe the
`async_events` and `net_spans` view schemas with `Dictionary(Int16, Utf8)` for the affected
columns (`stream_id`, `block_id`, `event_type`, `name`, `filename`, `target`, `process_id`,
`kind`, `connection_name`). These doc comments must be updated to `Dictionary(Int32, Utf8)`
to stay accurate.

`mkdocs/docs/query-guide/schema-reference.md` also documents these schemas field-by-field and
must be updated in lockstep, or the public docs silently disagree with the on-disk schema.
Change `Dictionary(Int16, Utf8)` to `Dictionary(Int32, Utf8)` for exactly the fields backed by
this plan's builders:

| Table (`###` heading) | Fields to change | Line numbers (current) |
|---|---|---|
| `log_entries` | `process_id`, `stream_id`, `block_id`, `exe`, `username`, `computer`, `target` | 157-165 |
| `measures` | `process_id`, `stream_id`, `block_id`, `exe`, `username`, `computer`, `target`, `name`, `unit` | 270-280 |
| `thread_spans` | `name`, `target`, `filename` | 319-321 |
| `async_events` | `stream_id`, `block_id`, `event_type`, `name`, `filename`, `target` | 349-359 |
| `net_spans` | `process_id`, `stream_id`, `kind`, `name`, `connection_name` | 461-468 |
| `images` (only if Open Question 1 resolves to include `images_table.rs`) | `process_id`, `stream_id`, `block_id`, `exe`, `username`, `computer`, `format` | 581-590 |

Leave the `processes` (lines 31-42), `streams` (lines 68-69), and `log_stats` (lines 208-210)
tables' `Dictionary(Int16, Utf8)` entries alone — `processes`/`streams` aren't backed by this
plan's builders, and `log_stats` is a `SqlBatchView` aggregating `log_entries` (its schema
hash, and thus its dictionary width, is derived automatically like
`log_entries_per_process_per_minute` in `sql_view_test.rs`, but updating its on-disk
materialization is outside this plan's builder changes). `otel_spans` already documents
`Dictionary(Int32, Utf8)` and needs no change.

The "Dictionary Compression" note (line 649: "Most string fields use dictionary compression
(`Dictionary(Int16, Utf8)`) for storage efficiency") also goes stale once the six tables above
widen — it should be reworded to note that key width varies by table (`Int16` for the
low-cardinality `processes`/`streams`/`log_stats` metadata, `Int32` for `log_entries`,
`measures`, `thread_spans`, `async_events`, `net_spans`, and `otel_spans`, matching each
table's field reference above) rather than asserting a single width for all string columns.

`mkdocs/docs/query-guide/functions-reference.md`'s `process_spans(process_id, types)` table
(lines 165-176) is out of scope for this fix: its `stream_id`/`thread_name` columns come from
`process_spans_table_function.rs`'s own local builders, which Design/Current State already
excludes from this plan, so they stay `Dictionary(Int16, Utf8)`. Note that the same table's
`name`/`target`/`filename` rows are documented as "same schema as `thread_spans`" and will
technically drift out of date once `thread_spans` widens — tracked here as a known gap, left
for a follow-up rather than pulled into this plan's scope.

## Testing Strategy

- **New regression test** (`analytics/tests/dictionary_key_overflow_tests.rs`): for each of
  `SpanRecordBuilder`, `AsyncEventRecordBuilder`, `NetSpanRecordBuilder`,
  `MetricsRecordBuilder`, and `LogEntriesRecordBuilder`, append somewhat more than 32,767
  rows with distinct values in the previously-`Int16` dictionary columns (e.g. distinct
  `name`/`target`/`filename` per row) and assert `finish()` succeeds. This is the exact
  scenario that panicked before the fix and must not panic after it.
  - `MetricsRecordBuilder`/`LogEntriesRecordBuilder`: use
    `analytics/tests/test_helpers.rs::make_process_metadata` for the `ProcessMetadata` and
    `PropertySet::empty()` for `properties`, looping `fill_constant_columns` (or `append`)
    with a distinct `block_id`/`stream_id` per call to drive the exact column the issue
    calls out.
  - `NetSpanRecordBuilder`: use existing helpers from `net_spans_test.rs`
    (`make_builder_ctx`) as a starting point.
- **OTLP companion regression test** (same `dictionary_key_overflow_tests.rs`): the
  "mandatory companions" fix (Design §3, Implementation Steps 6/7) has zero coverage
  otherwise — `MeasuresRowBuilder` is a private struct with no test referencing it today,
  and no test exercises `OtelLogsBlockProcessor`/`OtelMetricsBlockProcessor` at all. Add two
  tests that drive each processor through its real `BlockProcessor::process` entry point:
  build a `ResourceLogs`/`ResourceMetrics` proto with more than 32,767 distinct values in a
  dictionary column (e.g. one `scope_logs`/`scope_metrics` per distinct scope name, one
  record/data point each, driving `targets`), prost-encode and CBOR-wrap it as a
  `BlockPayload`, write it to an in-memory `ObjectStore` (`object_store::memory::InMemory`)
  wrapped in `BlobStorage::new(...)` at the `blobs/{process_id}/{stream_id}/{block_id}` path
  `fetch_block_payload` expects, construct a matching `PartitionSourceBlock` (process via
  `test_helpers::make_process_metadata`), call `.process(...)`, and assert it returns
  `Ok(Some(_))` with the expected row count instead of panicking. This is the exact scenario
  the "mandatory companions" fix claims to correct, so it must not ship without a test.
- `cargo test -p micromegas-analytics` from `rust/` — existing `span_tests.rs`,
  `async_events_tests.rs`, `metrics_test.rs`, `log_tests.rs`, `net_spans_test.rs`,
  `sql_view_test.rs`, `thread_spans_ordering_tests.rs` all continue to pass, proving normal
  (non-overflow) ingestion and querying is unaffected.
- `cargo fmt` and `cargo clippy --workspace -- -D warnings` from `rust/`.

## Open Questions

1. **Include `images_table.rs`/`images_view.rs` in this PR?** Same bug class
   (`StringDictionaryBuilder<Int16Type>` + panicking `append_value`), not named in the
   issue, much lower practical risk (image events are low-volume). Recommend including it
   for consistency (cheap, mechanical, same pattern) — but flagging here since it's outside
   the issue's literal scope, in case it's preferred as a separate follow-up.
2. **Historical backfill scope.** This plan bumps `SCHEMA_VERSION` so future materialization
   uses the wider key, but doesn't call `regenerate_partitions`/`retire_partitions` for
   existing historical data as part of the PR — that's an operational step for whoever
   deploys this, not a code change. Confirm that's the right split (code change here,
   backfill as a deploy-time runbook step) rather than something this plan should automate.
