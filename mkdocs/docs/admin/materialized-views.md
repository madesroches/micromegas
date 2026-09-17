# Materialized Views

An admin can define a new, eagerly materialized view set at runtime with a SQL DDL statement,
instead of editing server code and redeploying. The definition is stored in Postgres; both
`flight-sql-srv` and `telemetry-maintenance-srv` pick it up on a short interval, so the
maintenance daemon starts materializing it and FlightSQL starts answering queries against it,
both without a restart.

The unit being defined is exactly the same kind of view the built-in `log_stats` table is: a
query that extracts rows into a partition (`extract_query`), a query that checks whether the
source data has changed since the last materialization (`count_src_query`), and a query that
merges multiple partitions into the table a user actually queries (`merge_partitions_query`). In
fact, `log_stats` itself is defined this way — it ships as a row in the same table a
`CREATE MATERIALIZED VIEW` statement writes to, and an admin may inspect, replace, or drop it like
any other definition.

## Creating and dropping a view

```sql
CREATE [OR REPLACE] MATERIALIZED VIEW <name> WITH (
  extract_query = $$
    SELECT date_bin('1 minute', time) as time_bin, target, count(*) as count,
           arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience
      FROM log_entries
     WHERE insert_time >= '{begin}' AND insert_time < '{end}'
     GROUP BY time_bin, target, audience
  $$,
  count_src_query = $$
    SELECT sum(nb_objects) as count FROM blocks
     WHERE insert_time >= '{begin}' AND insert_time < '{end}'
  $$,
  merge_partitions_query = $$
    SELECT time_bin, target, sum(count) as count, audience
      FROM {source} GROUP BY time_bin, target, audience
  $$,
  update_group           = 4000,
  time_column            = 'time_bin',
  source_partition_delta = '1 day',
  merge_partition_delta  = '1 day',
  merge_sort_order       = 'time_bin, target'
);

DROP MATERIALIZED VIEW [IF EXISTS] <name>;
```

Both statements require an authenticated admin — the same gate as `retire_partitions()` and the
other admin-gated functions in [Admin SQL Functions](functions-reference.md). A non-admin caller
gets a permission-denied error.

Each query is a `WITH` option rather than a statement body, so all three are dollar-quoted
(`$$...$$`) string literals — the recommended form, since these queries almost always contain
single quotes (`'{begin}'`, a string literal in a `WHERE` clause, ...) that a plain
`'...'`-quoted literal would otherwise force you to double. A single-quoted literal works too,
if you prefer it and have no embedded quotes to escape.

### Options

| Option | Required | Meaning |
|---|---|---|
| `extract_query` | Yes | Extracts one partition's worth of rows for the time range being materialized |
| `count_src_query` | Yes | Counts the underlying source rows, to detect a stale partition. Must count a raw, `insert_time`-bearing source table (typically `blocks`) — never the view's own extract source, and never another materialized view |
| `merge_partitions_query` | Yes | Combines multiple partitions (and answers a query spanning more than one) — `{source}` is the placeholder for the partition set being merged |
| `update_group` | Yes | Where this view sits in the daemon's materialization order. Must be a number strictly greater than the `update_group` of every view this definition reads — the daemon materializes in ascending group order, and this is the only dependency mechanism there is. There is no default and no automatic inference |
| `time_column` | Yes, unless both of the two below are given | The event-time column bounding each partition |
| `min_time_column` / `max_time_column` | No | Override `time_column` individually, if the minimum and maximum event-time bounds come from different columns |
| `source_partition_delta` | No (default `'1 day'`) | How wide a fresh partition may be, as `'<n> <unit>'` (`second(s)`, `minute(s)`, `hour(s)`, `day(s)`) |
| `merge_partition_delta` | No (defaults to `source_partition_delta`) | How wide a merged partition may be |
| `merge_sort_order` | No | Comma-separated column list the merged output is sorted by, enabling a streaming k-way merge instead of a buffering sort |

An unrecognized option name is rejected outright rather than silently ignored.

## What is checked at `CREATE` time

The three queries are planned and inspected before the definition is written anywhere, so a
mistake that would otherwise only surface on the daemon's first materialization attempt — or,
worse, never surface as an error at all — is caught immediately instead:

- The extract query must plan (this also yields the view's schema).
- The schema must carry an `audience` column or a `process_id` column, of a string type, so
  non-admin queries can be filtered by audience. Without one, the table would silently serve
  every row to every caller.
- The merge query must plan against the extract query's schema, and its output columns must
  agree with it (names, types, and order — not nullability).
- The count query must produce a single `count` column of integer type.
- The count query must reference both `{begin}` and `{end}`; the merge query must reference
  `{source}`.
- Neither the extract, merge, nor count query may call a volatile function (`now()`,
  `random()`) — its result would otherwise be frozen into a partition, or corrupt freshness
  detection, instead of being recomputed each time.
- None of the three may reference a mutating admin function (`retire_partitions`,
  `materialize_partitions`, `regenerate_partitions`, `deny_queries`) or a per-instance table
  function (`view_instance`, `process_spans`, `perfetto_trace_chunks`) — storing a call to one of
  these would re-run it on every daemon tick.
- `update_group` must be strictly greater than the group of every other view set the definition
  reads.
- If `merge_sort_order` is given, it must actually apply to the extract query.
- The resolved time column(s) must exist in the extract query's schema and be nanosecond
  timestamps.
- No embedded DDL or DML (e.g. `CREATE EXTERNAL TABLE`, `COPY ... TO`) is allowed inside any of
  the three queries.

A `CREATE OR REPLACE` or a `DROP` is additionally validated against every other definition in the
deployment: if dropping or replacing a view would break a *different* view that reads it, the
statement is refused and the error names the definition it would have broken. There is no
`CASCADE` — drop the dependent first.

## What a redefinition means

A view's stored schema hash is derived purely from its inferred Arrow schema, not from the text
of its queries. So a `CREATE OR REPLACE` that changes the *output schema* self-invalidates: the
new schema hashes differently, and existing partitions (with the old hash) simply stop being
read. Query the view again — after the daemon or an explicit `materialize_partitions()` call
fills in fresh partitions — and only new-schema data comes back.

A `CREATE OR REPLACE` that changes *content* without changing the output schema (a widened
filter, a different `date_bin` interval, a different source view) does **not** self-invalidate:
the existing partitions are still schema-compatible, so the view keeps serving a mix of
old-definition and new-definition data until an admin explicitly reclaims the old partitions with
`retire_partitions(...)` (or, for the schema-changed case,
`micromegas.admin.retire_incompatible_partitions()`) and re-materializes the range with
`materialize_partitions(...)`.

Two timing races follow from this, both accepted trade-offs of a design that reloads on an
interval rather than synchronously everywhere:

- After a `DROP`, a replica of the maintenance daemon that has not yet reloaded the new
  definition set can still write a partition for the dropped view for a short window. Recreating
  the same name shortly after a drop should be followed by an explicit `retire_partitions()` call
  to clean up any such orphan.
- After a content-only `CREATE OR REPLACE`, a lagging daemon replica can still write, into a
  range an admin just retired and rebuilt under the new definition, a partition under the *old*
  definition — and since the schema hash is unchanged, it is indistinguishable from legitimate
  new-definition data. Wait for the reload interval to elapse everywhere before retiring and
  rebuilding a range after a content-only replace.

## Author obligations (not enforced, but easy to get wrong)

A few properties can't be checked from a query's structure alone, so they are left as documented
conventions instead:

- The extract query need not filter on `{begin}`/`{end}` at all (a materialized partition is
  already scoped by the range being extracted), but because that scoping matches by *overlap*, a
  wider partition may be scanned in full — so an extract query that does not filter on the range
  explicitly must be idempotent under seeing extra rows (e.g. a `GROUP BY` using `max`/
  `first_value` rather than `count(*)`, which would double-count).
- Any range filter belongs on `insert_time` (when to materialize), not on the event-time column
  (what the row is about) — except when reading another materialized view, where the source has
  no reliable `insert_time` of its own and the event-time column is filtered instead. In that
  case, a row that lands late in the upstream view's own partitions, after the downstream view
  has already covered that time range, is not picked up until the downstream range is
  re-materialized.
- The merge query's aggregates must be composable over already-aggregated rows — `sum(count)`,
  never `count(*)`; carry `sum` and `count` separately and divide at read time rather than
  averaging an average.
- Every row's audience/process_id column must be non-`NULL` — a `NULL` is filtered out for every
  non-admin caller silently, with no error.

## Introspection

- `list_view_sets()` includes every DDL-defined view set alongside the built-in ones, since it
  simply walks whatever is currently loaded.
- [`list_view_set_definitions()`](functions-reference.md#list_view_set_definitions) lists every row in
  Postgres directly, including one that failed to load — so it is the way to see a definition
  `list_view_sets()` doesn't know about.

## Related settings

See the `MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS` and `MICROMEGAS_STATIC_TABLES_URL` entries
in [Maintenance Daemon](maintenance.md) and [FlightSQL](flight-sql.md) for the reload interval and
the static-tables resolution both roles share.
