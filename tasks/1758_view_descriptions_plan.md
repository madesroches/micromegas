# View Descriptions in Lakehouse Metadata Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1758

## Overview

AI agents (and people) querying micromegas need more than a schema to use a view correctly:
what it contains, how it relates to other views, which filters are mandatory, and how to decode
encoded columns. Today that guidance lives in hand-maintained agent skill files (the shipped
`claude-plugin/skills/micromegas-query/SKILL.md`, plus any copies a deployment maintains), and
those files drift from the real view definitions. This plan stores a markdown description per
view set and exposes it by appending columns to the existing `list_view_sets()` table function,
so any SQL client can discover it. Built-in views ship their description in code. DDL-defined
views carry theirs as a `description` option of their `CREATE MATERIALIZED VIEW` statement. The
view factory holds both in memory, so `list_view_sets()` stays a pure in-memory scan.

## Current State

- **DDL-defined views** are persisted in `lakehouse_view_set_definitions`
  (`rust/analytics/src/lakehouse/migration.rs:577`, `upgrade_v9_to_v10`).
  - Non-query options are serialized as JSON into the `view_options` TEXT column from
    `ViewOptions` (`rust/analytics/src/lakehouse/view_definition.rs:39`).
  - `definition_sql` keeps the verbatim `CREATE` text. `micromegas-views` diffs and applies that
    text (`python/micromegas/micromegas/cli/views.py:181`).
- **DDL parsing**: `parse_view_ddl` (`rust/analytics/src/lakehouse/view_ddl.rs:50`) hand-parses
  `CREATE [OR REPLACE] MATERIALIZED VIEW <name> WITH (...)`.
  - `view_definition_from_options` (`view_ddl.rs:166`) maps the `WITH` options, and an unknown
    key is a hard error.
  - String options accept single-quoted and dollar-quoted literals (`value_to_string`).
- **The view factory**: `ViewFactory` (`rust/analytics/src/lakehouse/view_factory.rs:255`) holds
  `view_sets` (makers) and `global_views`.
  - `default_view_factory` (`view_factory.rs:309`) builds the built-in sets:
    - global views: `log_entries`, `measures`, `processes`, `streams`, `blocks`;
    - view makers: `images`, `log_entries`, `measures`, `async_events`, `net_spans`,
      `thread_spans`, `otel_spans`.
  - `view_registry::build_factory` (`view_registry.rs:123`) clones that base factory. For each
    DDL row, it parses the row into a `ViewDefinition` with `into_definition`, builds and
    validates it, then calls `factory.add_global_view`.
  - `log_stats` is a seeded DDL row (`log_stats_view.rs`), written by the v10 migration with
    `updated_by = 'migration'`.
- **`list_view_sets()`** (`list_view_sets_table_function.rs`) is registered for every caller
  (`query.rs:154`).
  - It reads only the factory, through `catalog::list_view_sets` (`catalog.rs:21`), which
    returns `ViewSetInfo { view_set_name, current_schema_hash, schema, has_view_maker,
    global_instance_available }`.
  - `schema` is the `Debug` format of the arrow `Schema`, which is unwieldy for an agent to read.
  - Callers in the repo select columns by name (`python/micromegas/micromegas/admin.py:78`), so
    appending columns is safe.
  - `catalog::list_view_sets` is unit-tested without a database in
    `rust/analytics/tests/catalog_tests.rs`.
- Nothing in the lakehouse metadata describes a view's meaning or usage today. The user-facing
  prose is in `mkdocs/docs/query-guide/schema-reference.md`, and the agent-facing prose is in the
  skill file.

## Design

### 1. Where descriptions come from

| View kind | Set by | Source text | Held in memory by |
|---|---|---|---|
| Built-in | release (code) | `rust/analytics/src/lakehouse/view_descriptions/<name>.md`, loaded with `include_str!` | `default_view_factory` → `ViewFactory` |
| DDL-defined (incl. `log_stats`) | `description = '...'` option of `CREATE [OR REPLACE] MATERIALIZED VIEW` | `view_options` JSON (new `description` field) | `build_factory` → `ViewFactory` |

A DDL view's description is part of its `CREATE` statement, and therefore part of
`definition_sql`, so `micromegas-views plan/apply/pull` keep it under version control in git
with no client change:

```sql
CREATE OR REPLACE MATERIALIZED VIEW error_counts WITH (
  description = $md$
Per-minute error counts by `exe`, pre-aggregated from `log_entries` (level <= 2).
Cheap to scan over days; prefer it to `log_entries` for trends.
$md$,
  extract_query = '...',
  ...
);
```

### 2. Types

- **`ViewOptions`** gains
  `#[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>`.
  - It lives in the JSON, so the schema needs no column, and `SELECT_COLUMNS` does not change.
  - Older servers ignore the unknown field when parsing, but the v11 bump stops pre-v11
    lakehouse services from starting against a migrated database. Lakehouse services upgrade
    together, and a rollback requires a database restore.
  - `validate_view_definition` ignores the field.
  - A `CREATE OR REPLACE` that only changes the description bumps `updated_at`, which triggers a
    reload that picks up the new text. It does not change the file schema hash, so no partition
    is rebuilt.
- **`view_definition_from_options`** accepts `description` through `value_to_string`. It rejects
  an empty or whitespace-only value.
- **`ViewFactory`** gains `descriptions: HashMap<String, Arc<str>>`, with
  `set_description(&mut self, view_set_name: &str, text: Arc<str>)` and
  `get_description(&self, view_set_name: &str) -> Option<Arc<str>>`.
  - It is cloned with the rest of the factory, so `build_factory`'s clone of the base carries the
    built-in descriptions forward.
- **`default_view_factory`** calls `set_description` for each built-in set, using
  `include_str!("view_descriptions/<name>.md")`.
  - Markdown files keep long prose out of Rust string literals and can be reviewed as documents.
  - `log_stats.md` lives in the same folder, and `log_stats_view.rs` reads it (see Migration).
- **`build_factory`** calls `factory.set_description(&name, ...)` right after
  `factory.add_global_view(...)`, when `def.options.description` is `Some`. A row that fails to
  parse, build or validate is skipped before that point, so its description is never registered.
  Only queryable views are described. DDL validation already rejects a name that collides with a
  built-in set, so a DDL description can never shadow a built-in one.
- **`catalog::ViewSetInfo`** gains three fields:
  - `description: Option<Arc<str>>`, from `factory.get_description`.
  - `columns: String`, with one `name: DataType` line per field, using arrow's `Display`. It is
    taken from the same schema `schema` is formatted from.
  - `sort_order: Option<Vec<String>>`, the column names of the global view's
    `get_scan_output_ordering()` (`Concatenated` or `PerFile`). Names only, because ordered
    scans are ascending-only (a descending column is never certified). It is `None` for
    `Unordered` and for maker-only sets, which have no instance to ask without building one.

### 3. `list_view_sets()` output

Three columns are appended after the existing five, which are unchanged in name, type and order:

| Column | Type | Notes |
|---|---|---|
| `view_set_name` | Utf8 | existing |
| `current_schema_hash` | Binary | existing |
| `schema` | Utf8 | existing (`Debug` format) |
| `has_view_maker` | Boolean | existing; queryable per process via `view_instance()` |
| `global_instance_available` | Boolean | existing; queryable as a plain table |
| `description` | Utf8, nullable | **new**: markdown usage guide |
| `columns` | Utf8 | **new**: one `name: DataType` line per field, readable in the CLI |
| `sort_order` | Utf8, nullable | **new**: comma-joined sort column names (ascending only) |

`ListViewSetsTableProvider::scan` keeps its shape: a call to `catalog::list_view_sets`, a limit,
array construction, then a `MemorySourceConfig`. It never touches Postgres.

### 4. Shipped descriptions

There is one `.md` file per built-in view set: `processes`, `streams`, `blocks`, `log_entries`,
`log_stats`, `measures`, `images`, `thread_spans`, `async_events`, `net_spans` and
`otel_spans`. Each one is short and written for agents, and covers:

- what a row is;
- the access pattern (the global view or `view_instance()`);
- mandatory or recommended filters, including safe-window guidance that holds everywhere (e.g.
  the span views are only reachable through `view_instance()`);
- how to read encoded columns (`property_get`, `jsonb_*`, histogram columns, levels);
- join keys to other views;
- one or two example queries.

The files do **not** repeat the column list, which the `columns` column carries. The source
material is the per-view sections of `SKILL.md` and `schema-reference.md`. `net_spans`,
`otel_spans` and `images` have no `SKILL.md` section, so they draw only on `schema-reference.md`.

### 5. Migration (lakehouse schema v10 → v11)

`upgrade_v10_to_v11` gives `log_stats` its shipped description:

- `log_stats_view_definition()` sets
  `options.description = Some(include_str!("view_descriptions/log_stats.md").into())`.
  `log_stats_ddl_text()` gains the matching `description = $$...$$` option, so the seeded
  definition and its `definition_sql` stay in agreement. `log_stats.md` must stay free of `$$`,
  since the untagged quote would end early.
- The step runs
  `UPDATE lakehouse_view_set_definitions SET definition_sql = $1, view_options = $2,
  updated_at = now() WHERE view_set_name = 'log_stats' AND updated_by = 'migration'`. An admin
  who has redefined `log_stats` keeps their version.
- On a fresh database, the v10 `INSERT ... ON CONFLICT DO NOTHING` already seeds the new text,
  and the v11 `UPDATE` rewrites the same values.

There is no `SCHEMA_VERSION` change, since no partition content is affected. Every
lakehouse-role service (flight-sql, maintenance, monolith) applies v11 at startup.

## Implementation Steps

1. **Types and parser**
   - `view_definition.rs`: add `ViewOptions::description`.
   - `view_ddl.rs`: accept the `description` option.
2. **Factory**
   - `view_factory.rs`: add the `descriptions` map with `set_description` and
     `get_description`, and register the built-in descriptions in `default_view_factory`.
   - `view_registry.rs`: call `set_description` in `build_factory` after a DDL view loads.
   - Create `rust/analytics/src/lakehouse/view_descriptions/*.md`.
3. **log_stats and migration**
   - `log_stats_view.rs`: add the description to the definition and the DDL text.
   - `migration.rs`: add `upgrade_v10_to_v11` and set `LATEST_LAKEHOUSE_SCHEMA_VERSION = 11`.
4. **Catalog and UDTF**
   - `catalog.rs`: add the new `ViewSetInfo` fields.
   - `list_view_sets_table_function.rs`: add the three trailing columns.
5. **Tests** (see Testing Strategy).
6. **Docs, skill and CHANGELOG** (see Documentation).

## Files to Modify

- `rust/analytics/src/lakehouse/view_definition.rs`
- `rust/analytics/src/lakehouse/view_ddl.rs`
- `rust/analytics/src/lakehouse/view_factory.rs`
- `rust/analytics/src/lakehouse/view_registry.rs`
- `rust/analytics/src/lakehouse/view_descriptions/*.md` (new, 11 files)
- `rust/analytics/src/lakehouse/log_stats_view.rs`
- `rust/analytics/src/lakehouse/migration.rs`
- `rust/analytics/src/lakehouse/catalog.rs`
- `rust/analytics/src/lakehouse/list_view_sets_table_function.rs`
- `rust/analytics/tests/view_ddl_parse_tests.rs`
- `rust/analytics/tests/catalog_tests.rs`
- `rust/analytics/tests/view_registry_tests.rs`
- `rust/analytics/tests/view_definition_validation_tests.rs` (`description: None` in
  `base_options()`)
- `mkdocs/docs/admin/materialized-views.md`
- `mkdocs/docs/admin/views-as-code.md`
- `mkdocs/docs/admin/functions-reference.md`
- `mkdocs/docs/query-guide/functions-reference.md`
- `mkdocs/docs/query-guide/schema-reference.md`
- `claude-plugin/skills/micromegas-query/SKILL.md`
- `CHANGELOG.md`

## Trade-offs

- **A map on `ViewFactory` rather than a `View`/`ViewMaker` trait method.** A trait method would
  be split across views and makers: `processes`, `streams` and `blocks` have no maker, and the
  span views have no global view. It would also need a default, or a dummy implementation on
  every view. A name-keyed map with a completeness test catches a forgotten built-in just as
  loudly.
- **`view_options` JSON rather than a new `description` column.** The issue suggests a column.
  The JSON field needs no `ALTER TABLE`, and it leaves `SELECT_COLUMNS` and the registry reload
  untouched. A column would only help ad-hoc SQL against Postgres, which no client does.
- **A `WITH (description = ...)` option rather than `COMMENT ON`.** Keeping the text inside the
  `CREATE` statement makes it part of `definition_sql`, which views-as-code already diffs,
  applies and pulls. A separate statement would need a second statement per file (views-as-code
  allows exactly one), a second diff path and a new executor branch.
- **A readable `columns` column next to the existing `schema`.** `schema`'s `Debug` text cannot
  change without breaking its SQL output, and it is noisy for agents. The one-line-per-field form
  costs little and lets agents skip `schema`.
- **No single-view variant such as `view_description('<name>')`.**
  `WHERE view_set_name = '...'` does the same thing.
- **Volume is not computed.** Partition row counts are meaningless for the views reachable only
  through `view_instance()` (`thread_spans`, `async_events`, `net_spans`, `otel_spans`), and
  computing them would cost a `lakehouse_partitions` aggregation per call. `list_partitions()`
  gives exact numbers, and DDL authors may write volume into their own description.

## Decisions

- No deployment-specific notes on built-in views, so no `COMMENT ON` and no notes table (user
  call: simplicity). Built-ins get only the shipped text. If a deployment needs its own volume or
  window guidance, the follow-up is a settings-backed `view_notes` map, which can be added
  without changing anything here.
- Keep the option name `description`, not `comment` (user call).
- Extend `list_view_sets()`, and serve descriptions from memory rather than Postgres (user call);
  a DDL change appears at the inline reload after commit, or at the next periodic refresh.
  `source`, `updated_at` and `updated_by` are not added, because admins already get them from
  `list_view_set_definitions()`.
- Descriptions are readable by every caller, with no audience rule. They describe the view, not
  rows, and view names and schemas are already public through `list_view_sets()`. The docs warn
  authors not to put secrets in them.
- `SKILL.md` drops its per-view tables entirely, with no fallback for older servers (user call).
  The skill ships alongside the release that adds the description columns.
- Column-level descriptions are out of scope. A `column_descriptions` column can be appended
  later.

## Documentation

- `mkdocs/docs/admin/materialized-views.md`: document the `description` option, with a
  tagged dollar-quoted markdown example (e.g. `$md$...$md$`), so a description containing `$$`
  is safe.
- `mkdocs/docs/admin/views-as-code.md`: one sentence saying that descriptions travel inside each
  `.sql` file.
- `mkdocs/docs/admin/functions-reference.md` and `mkdocs/docs/query-guide/functions-reference.md`:
  - document the three new `list_view_sets()` columns;
  - in the query-guide page only, fix the "Administrative Function" label: every caller can use
    it, since it is registered outside the admin block.
- `mkdocs/docs/query-guide/schema-reference.md`: extend the existing `list_view_sets()` sentence
  (line ~27) to point at `SELECT view_set_name, description, columns FROM list_view_sets()` as
  the live, per-deployment list, DDL views included.
- `claude-plugin/skills/micromegas-query/SKILL.md`:
  - make running `SELECT view_set_name, description, columns, sort_order FROM list_view_sets()`
    the first step of "Available views";
  - keep the MANDATORY RULES and the generic sections (CLI syntax, key functions, common query
    patterns, performance tips);
  - remove the per-view tables and the per-view prose, after moving any guidance they carry that
    is still missing into the matching `view_descriptions/<name>.md`;
  - reword the "Do not run DESCRIBE queries — all schemas are documented here" line (~213) to
    point at the `columns` and `description` output of `list_view_sets()`.
- `CHANGELOG.md`, under Unreleased:
  - `list_view_sets()` gains trailing `description`, `columns` and `sort_order` columns;
  - the `description` option;
  - lakehouse schema v11 updates `log_stats`, and pre-v11 lakehouse services will not start
    against a migrated database (upgrade them together; rollback needs a database restore);
  - deployments that keep `log_stats` in a views-as-code repo should re-run `micromegas-views pull` after upgrading (see `mkdocs/docs/admin/views-as-code.md`);
  - the existing columns are unchanged;
  - **Minor breaking change:** `ViewOptions` gains `description`, and `ViewSetInfo` gains
    `description`, `columns` and `sort_order`.

## Testing Strategy

All of these are unit tests in `rust/analytics/tests/`, with no database.

- **Parser** (`view_ddl_parse_tests.rs`):
  - the `description` option is accepted, single-quoted and dollar-quoted;
  - an empty or whitespace-only description is rejected.
- **`ViewOptions` serde round-trip**:
  - `view_options` JSON without `description`, as in every existing row, parses to `None`;
  - `None` serializes without the key, so unchanged rows stay byte-identical.
- **`catalog::list_view_sets`** (`catalog_tests.rs`, extending the existing no-DB tests):
  - a factory with and without `set_description` yields `Some` and `None` respectively;
  - the formatting of `columns`;
  - `sort_order` for a `PerFile` / `Concatenated` global view, and for an `Unordered` one;
  - `sort_order` is `None` for a maker-only set.
- **Completeness**:
  - every view set in `default_view_factory` has a non-empty `get_description`;
  - it uses `make_offline_lakehouse_context()` for setup and sits next to `real_view_factory_covers_every_registered_view_set` in `rust/analytics/tests/ownership_rewrite_public_view_set_tests.rs`;
  - this test catches a new built-in shipping without a description.
- **`build_factory` registers DDL descriptions**, tested through the existing `ViewRegistry`
  test seam (the `ViewDefinitionStore` fake in
  `rust/analytics/tests/view_registry_tests.rs`):
  - a valid row with a description is described in the resulting factory;
  - a row that fails validation leaves no description behind.
- **log_stats agreement**: extend the existing test in `view_ddl_parse_tests.rs` so that
  `parse_view_ddl(log_stats_ddl_text())` yields exactly `log_stats_view_definition()`, including
  the description.

The migration needs Postgres and is not a bug witnessed in the wild, so it gets no live-DB test
(see `CONTRIBUTING.md`). It is covered by Manual Verification.

## Manual Verification

The migration and the end-to-end round trip through Flight SQL only run against a real
Postgres. A failure would be immediately visible as an error or a missing description.

1. Start the services on a database last migrated by the current release:
   `python3 local_test_env/ai_scripts/start_services.py`.
   - Expected: `/tmp/analytics.log` (or `/tmp/daemon.log`) reports the lakehouse migration to
     version 11.
2. Check the descriptions:
   `micromegas-query "SELECT view_set_name, description IS NOT NULL AS has_desc, sort_order FROM list_view_sets()"`.
   - Expected: every built-in set, plus `log_stats`, has `has_desc = true`.
3. Create a test view with `description = $$# Test$$` via `micromegas-views apply`.
   - Expected: its description appears in `list_view_sets()` right after the apply, through the
     inline reload.
   - Expected: `micromegas-views plan` is clean afterwards, so the description round-trips
     through `definition_sql`.
