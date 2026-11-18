---
name: micromegas-notebooks
description: Author and edit micromegas notebook screens (cells, variables, SQL macros) through the screen tools. Use when the user wants to find, reuse, create or modify a saved notebook.
---

# Authoring micromegas notebooks

A notebook is a screen of type `notebook`. Its `config` is a JSON object holding an ordered list of
cells that run top to bottom, plus a default time range:

```json
{
  "timeRangeFrom": "now-1h",
  "timeRangeTo": "now",
  "cells": [ ... ]
}
```

Start from `get_screen_type_default("notebook")`, and look at a curated notebook with `list_screens`
and `get_screen` before inventing a layout. Reuse its queries rather than rewriting them.

## Workflow

1. `list_screens` (optionally with `folder_path`) to find an existing notebook close to the request.
2. `get_screen` to read its `config`. Large configs are truncated: ask for a smaller folder or reuse
   only the cells you need.
3. Develop and test each cell's SQL with `prepare_query` then `execute_query`, following the
   `micromegas-query` rules (bounded time range, `LIMIT`, per-view `description`). Notebook macros
   such as `$from` are not understood by the query tools: substitute concrete values while testing.
4. `create_screen` (new) or `update_screen` (existing). Pass `expected_updated_at` from the last
   `get_screen` so a concurrent edit is not overwritten. Writes may be disabled on the server.

## Cells

Every cell has a unique `name`, a `type`, and `layout: { "height": <px>, "collapsed": false }`.

| `type` | Purpose | Main fields |
|---|---|---|
| `table` | sortable table | `sql`, `options.pageSize`, `options.selectionMode` (`none` / `single`) |
| `chart` | time series (line or bar) | `queries: [{ sql, name, unit, label, color }]`, `options.chart_type`, `options.scale_mode` |
| `log` | log viewer with level colors | `sql` returning `time, level, target, msg` |
| `markdown` | text or a headline value | `content`, optional `sql` (row 0 binds `$column`) |
| `variable` | user input feeding `$name` | `variableType`: `text`, `combobox`, `expression`, `datasource`; `defaultValue`; `sql` (combobox) |
| `flamegraph` | span visualization | `sql` returning `id, parent, name, begin, end, depth` (optional `lane`) |
| `hg` | horizontal group | `children: [cells]`; groups cannot nest |
| `piechart`, `stackedbar`, `swimlane`, `propertytimeline`, `transposed`, `map`, `image`, `referencetable`, `perfettoexport` | see the web app documentation | |

Query-backed cells also accept `dataSource` (data source name, or `$variable` of a datasource
variable) and `timeRange: { "from": "...", "to": "..." }` to override the notebook range.

Chart SQL returns the X column first, the Y column second, and optionally a `color` column.

## Variables and macros

Macros are substituted into SQL and markdown before the query is sent. Substituted values have
single quotes escaped.

| Macro | Meaning |
|---|---|
| `$from`, `$to` | current time range (ISO 8601) |
| `$name` | value of variable cell `name` |
| `$name.column` | a column of a multi-column combobox value |
| `$cell[N].column` | row N of an upstream cell's result |
| `$cell.selected.column` | selected row of an upstream table or chart with `selectionMode: "single"` |
| `$order_by` | sort column of a table cell |
| `$me.email`, `$me.name`, `$me.sub` | the viewer (not an authorization mechanism) |

Cells run in order and stop at the first error, so put variable cells above the cells that use
them. Results of each cell are available to later cells under the cell name.

## Conventions

- Keep cell names short and unique; they are used in macros.
- Prefer a variable cell for process or metric selection over hard-coded identifiers.
- Do not change `managed_by` on an existing screen: the tools leave it untouched.
- Give new screens a clear name and a folder (`create_folder` first when needed).
