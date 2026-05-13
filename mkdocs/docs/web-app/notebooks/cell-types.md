# Cell Types Reference

Notebooks support 13 cell types. Each cell has a `name` (unique within the notebook), a `type`, and a `layout` controlling its display height and collapsed state.

Data cells (table, chart, log, etc.) execute SQL queries and register their results in the [local WASM query engine](execution.md#local-wasm-query-engine), making them available for downstream cells to query.

---

## ![Markdown](../../assets/images/cell-icons/file-text.svg){ .cell-icon } Markdown

Static text and documentation using GitHub Flavored Markdown.

**Configuration:**

| Field | Type | Description |
|-------|------|-------------|
| `content` | string | Markdown text to render |

**Features:**

- Full GitHub Flavored Markdown: headings, tables, lists, code blocks, strikethrough
- Supports [variable substitution](variables.md#sql-macro-substitution): `$variable`, `$variable.column`, `$begin`, `$end`
- Validates macro references during editing — warns about undefined variables
- Does not execute queries or block downstream cells
- Rendered output appears only after the cell's turn in sequential execution — the body stays blank until upstream variables and cell results are resolved, so macros never display stale or broken values on first paint

**Example content:**

```markdown
# Dashboard for $process_id

Showing data from **$begin** to **$end**.
```

---

## ![Variable](../../assets/images/cell-icons/variable.svg){ .cell-icon } Variable

User-configurable inputs that populate `$variable` references in downstream cells. Variable cells render in the cell title bar rather than taking up vertical space.

There are four variable subtypes, controlled by the `variableType` field.

**Common configuration:**

| Field | Type | Description |
|-------|------|-------------|
| `variableType` | `'text'` \| `'combobox'` \| `'expression'` \| `'datasource'` | Variable subtype |
| `defaultValue` | string or object | Default value used when no URL override is present |
| `dataSource` | string | Data source for SQL queries (combobox only) |

See [Variables](variables.md) for full documentation of the variable system including scope, expressions, and URL sync.

### Text

Free-form text input. The user types a value directly. No SQL execution.

```
variableType: 'text'
defaultValue: 'cpu_usage'
```

### Combobox

Dropdown populated by a SQL query. Single-column queries produce simple string options. Multi-column queries produce options with named fields accessible via `$variable.column` syntax.

| Field | Type | Description |
|-------|------|-------------|
| `sql` | string | SQL query returning option values |
| `dataSource` | string | Data source for the query |

```sql
-- Single column: options are strings
SELECT DISTINCT name FROM measures

-- Multi-column: options are objects with .name and .unit fields
SELECT DISTINCT name, unit FROM measures
```

After execution, the current value is validated against available options. If invalid, the default value or first option is auto-selected.

![Combobox variable with dropdown open showing metric options](../../assets/images/notebooks/variables.png){ .screenshot }

### Expression

Computed value from a JavaScript expression. Evaluated automatically — not user-editable at runtime.

| Field | Type | Description |
|-------|------|-------------|
| `expression` | string | JavaScript expression to evaluate |

Available bindings: `$begin`, `$end`, `$duration_ms`, `$innerWidth`, `$devicePixelRatio`, and upstream variables as `$variableName`.

Available functions: `snap_interval()`, `Math.*`.

```javascript
// Compute a time bin interval based on viewport width
snap_interval($duration_ms / $innerWidth)
```

See [Expression Evaluation](variables.md#expression-evaluation) for details.

### Datasource

Dropdown populated with available data sources from the API. Use with `$variableName` in a query cell's data source field to route queries to user-selected backends.

```
variableType: 'datasource'
defaultValue: 'production'
```

---

## ![Table](../../assets/images/cell-icons/table-2.svg){ .cell-icon } Table

SQL query results displayed in a sortable, paginated table.

**Configuration:**

| Field | Type | Description |
|-------|------|-------------|
| `sql` | string | SQL query with macro substitution |
| `dataSource` | string | Data source override |

**Options:**

| Field | Type | Description |
|-------|------|-------------|
| `sortColumn` | string | Currently sorted column |
| `sortDirection` | `'asc'` \| `'desc'` | Sort direction |
| `pageSize` | number | Rows per page (default: 50) |
| `hiddenColumns` | string[] | Columns to hide |
| `overrides` | array | Column format overrides |

**Features:**

- Sticky header with click-to-sort columns
- Column hiding via context menu, with a restoration bar to unhide
- Client-side pagination with configurable page size
- Sort column available as `$order_by` macro in SQL for server-side sorting
- Column format overrides with markdown and row macros (`$row.columnName`)
- Results registered in the [local WASM query engine](execution.md#local-wasm-query-engine) under the cell name for downstream queries

**Column format overrides:**

Override how a column renders by providing a markdown format string with row macros:

```markdown
[$row.exe](/process/$row.process_id?from=$begin&to=$end)
```

This renders the `exe` column as a link to the process page.

**Example SQL:**

```sql
SELECT process_id, exe, start_time, username, computer
FROM processes
ORDER BY $order_by
LIMIT 100
```

![Table cell displaying query results with 3.7M rows](../../assets/images/notebooks/table_cell.png){ .screenshot }

---

## ![Transposed Table](../../assets/images/cell-icons/table-properties.svg){ .cell-icon } Transposed Table

SQL results with rows and columns swapped. The original column names become the first column, and each result row becomes a subsequent column. Useful for displaying metadata or key-value properties.

**Configuration:**

Same as Table — `sql`, `dataSource`, and options for hidden rows, page size, and format overrides.

**Options:**

| Field | Type | Description |
|-------|------|-------------|
| `pageSize` | number | Page size for scrolling |
| `hiddenRows` | string[] | Hidden row names (original column names) |
| `overrides` | array | Row rendering overrides |

**Features:**

- Row hiding via right-click context menu, with a restoration bar to unhide
- Row format overrides with markdown and row macros (same syntax as table column overrides)
- Results registered in the [local WASM query engine](execution.md#local-wasm-query-engine) under the cell name for downstream queries

**Example:**

A query returning `process_id, exe, username, computer` for a single row displays as:

| Field | Value |
|-------|-------|
| process_id | abc-123 |
| exe | my-service |
| username | admin |
| computer | prod-01 |

---

## ![Chart](../../assets/images/cell-icons/bar-chart-3.svg){ .cell-icon } Chart

Multi-query time-series charts supporting line and bar chart types.

**Configuration:**

| Field | Type | Description |
|-------|------|-------------|
| `queries` | array | One or more query definitions |
| `options.scale_mode` | `'p99'` \| `'max'` | Y-axis scaling mode |
| `options.chart_type` | `'line'` \| `'bar'` | Chart type |

**Query definition:**

| Field | Type | Description |
|-------|------|-------------|
| `sql` | string | SQL query returning `time` + value columns |
| `name` | string | Query name (used for WASM table registration) |
| `unit` | string | Y-axis unit label |
| `label` | string | Series label override |
| `dataSource` | string | Per-query data source |

**Scale modes:**

- **p99** (default) — scales Y-axis to the 99th percentile, handling outliers gracefully
- **max** — scales Y-axis from 0 to the maximum value

**Features:**

- Multiple queries per chart, each with its own data source and unit
- Color-coded series with a rotating palette
- Drag-to-zoom: drag horizontally on the chart to select a time range and zoom in
- Chart type and scale mode toggleable via controls
- Each query's results are registered in the WASM engine as `cellName.queryName`

**Example:**

```sql
SELECT time, value
FROM measures
WHERE name = '$metric'
ORDER BY time
```

![Chart cell with frame time data and drag-to-zoom](../../assets/images/notebooks/chart_zoom.png){ .screenshot }

---

## ![Log](../../assets/images/cell-icons/scroll-text.svg){ .cell-icon } Log

SQL query results formatted as log entries with level-based coloring.

**Configuration:**

| Field | Type | Description |
|-------|------|-------------|
| `sql` | string | SQL query returning log data |
| `dataSource` | string | Data source override |

**Options:**

| Field | Type | Description |
|-------|------|-------------|
| `pageSize` | number | Entries per page (default: 50) |

**Expected columns:**

The renderer auto-classifies columns by name:

- `time` — event timestamp
- `level` — log level (`ERROR`, `WARN`, `INFO`, `DEBUG`, `TRACE`)
- `target` — logger target/module
- `msg` — log message

Additional columns are rendered as fixed-width monospace text.

- Results registered in the [local WASM query engine](execution.md#local-wasm-query-engine) under the cell name for downstream queries

**Level coloring:**

| Level | Color |
|-------|-------|
| ERROR | Red |
| WARN | Yellow |
| INFO | Gray |
| DEBUG/TRACE | Muted |

**Example SQL:**

```sql
SELECT time, level, target, msg
FROM log_entries
ORDER BY time DESC
LIMIT 500
```

![Log cell with level-based coloring and full-text search](../../assets/images/notebooks/log_cell.png){ .screenshot }

---

## ![Property Timeline](../../assets/images/cell-icons/gantt-chart.svg){ .cell-icon } Property Timeline

Visualizes how JSON properties change over time as horizontal timeline segments.

**Configuration:**

| Field | Type | Description |
|-------|------|-------------|
| `sql` | string | SQL query returning `time` and `properties` (JSON) columns |
| `dataSource` | string | Data source override |

**Options:**

| Field | Type | Description |
|-------|------|-------------|
| `selectedKeys` | string[] | Property keys to display as timeline tracks |

**Features:**

- Parses JSON properties from each row and detects value changes
- Each selected property key displays as a horizontal timeline track
- Interactive key selection — add or remove property keys from the display
- Time range zoom via drag selection
- Fills time gaps with "no value" state
- Results registered in the [local WASM query engine](execution.md#local-wasm-query-engine) under the cell name for downstream queries

**Expected columns:**

- `time` — timestamp
- `properties` — JSON object string (e.g., `{"cpu": 45, "state": "running"}`)

![Property timeline showing map property changes over time](../../assets/images/notebooks/property_timeline.png){ .screenshot }

---

## ![Swimlane](../../assets/images/cell-icons/align-center.svg){ .cell-icon } Swimlane

Horizontal lane visualization for thread or async activity over time. Each lane shows one or more time segments as horizontal bars.

**Configuration:**

| Field | Type | Description |
|-------|------|-------------|
| `sql` | string | SQL query returning lane data |
| `dataSource` | string | Data source override |

**Required columns:**

| Column | Type | Description |
|--------|------|-------------|
| `id` | string | Unique lane identifier |
| `name` | string | Lane display name |
| `begin` | timestamp | Segment start time |
| `end` | timestamp | Segment end time |

Multiple rows with the same `id` create multiple segments in one lane. Lanes are ordered by first occurrence in the query results.

**Features:**

- Fixed label column on the left with lane names
- Horizontal time bars in the center
- Drag-to-zoom time selection
- Time axis with formatted tick marks
- Results registered in the [local WASM query engine](execution.md#local-wasm-query-engine) under the cell name for downstream queries

**Example SQL:**

```sql
SELECT
  arrow_cast(stream_id, 'Utf8') as id,
  concat(
    arrow_cast(property_get("streams.properties", 'thread-name'), 'Utf8'),
    '-',
    arrow_cast(property_get("streams.properties", 'thread-id'), 'Utf8')
  ) as name,
  begin_time as begin,
  end_time as end
FROM blocks
WHERE process_id = '$process_id'
  AND array_has("streams.tags", 'cpu')
ORDER BY name, begin
```

![Swimlane showing thread activity across a task pool](../../assets/images/notebooks/swimlane.png){ .screenshot }

---

## ![Flame Graph](../../assets/images/cell-icons/flame.svg){ .cell-icon } Flame Graph

Interactive span visualization rendered with WebGL. Spans are grouped into lanes (one per thread or async scope) and stacked by call depth, with drag-to-zoom and WASD navigation.

**Configuration:**

| Field | Type | Description |
|-------|------|-------------|
| `sql` | string | SQL query returning span data |
| `dataSource` | string | Data source override |

**Options:**

| Field | Type | Description |
|-------|------|-------------|
| `initialFrom` | string | Initial zoomed-in start time (accepts `$variable`, relative like `now-1h`, or absolute) |
| `initialTo` | string | Initial zoomed-in end time |

**Required columns:**

| Column | Type | Description |
|--------|------|-------------|
| `id` | bigint | Unique span identifier |
| `parent` | bigint | Parent span id (0 or null for roots) |
| `name` | string | Span name (used for label and color hash) |
| `begin` | timestamp | Span start time |
| `end` | timestamp | Span end time |
| `depth` | int | Call depth within the lane (UInt32 in `process_spans`) |

**Optional columns:**

| Column | Type | Description |
|--------|------|-------------|
| `lane` | string | Lane name — one lane per distinct value (e.g., thread name, `async`). If omitted, all spans render in a single lane named `default`. The lane literally named `async` gets greedy-packed layout (see Features). |

**Features:**

- WebGL-rendered spans with Canvas2D label and time-axis overlay — handles millions of rows
- Color-coded by span name using a brand-derived rust/blue/gold palette
- Drag horizontally to zoom into a time range; WASD keys pan and zoom (cursor-anchored)
- Mouse wheel scrolls vertically across lanes
- Hover tooltip shows span name, duration, id, depth, and parent name
- The lane literally named `async` is laid out by greedy packing to avoid overlap; all other lanes use the raw `depth` column
- Results registered in the [local WASM query engine](execution.md#local-wasm-query-engine) under the cell name for downstream queries

**Example SQL:**

```sql
SELECT id, parent, name, begin, "end", depth, thread_name AS lane
FROM process_spans('$process_id', 'both')
ORDER BY lane, begin
```

The `process_spans(process_id, types)` table function returns thread spans, async spans, or `'both'`.

![Flame graph showing nested async spans with a hover tooltip](../../assets/images/notebooks/flame_graph.png){ .screenshot }

---

## ![Perfetto Export](../../assets/images/cell-icons/download.svg){ .cell-icon } Perfetto Export

Exports trace data to [Perfetto UI](https://ui.perfetto.dev) for visualization, or downloads it as a file.

**Configuration:**

| Field | Type | Description |
|-------|------|-------------|
| `processIdVar` | string | Variable name containing the process ID (default: `$process_id`) |
| `spanType` | `'thread'` \| `'async'` \| `'both'` | Which span types to include (default: `both`) |
| `dataSource` | string | Data source override |

**Features:**

- Split button: **Open in Perfetto** (primary) or **Download** (secondary)
- Shows a warning if the referenced variable is empty or undefined
- Caches the generated trace buffer — cleared when process ID, span type, time range, or data source changes
- Progress indicator during trace generation
- No automatic execution — triggered by user button click

---

## ![Reference Table](../../assets/images/cell-icons/book-open.svg){ .cell-icon } Reference Table

Embeds inline CSV data that is registered as a queryable table in the [local WASM query engine](execution.md#local-wasm-query-engine). Downstream cells can query it by the cell's name.

**Configuration:**

| Field | Type | Description |
|-------|------|-------------|
| `csv` | string | CSV data with headers in the first row |

**Options:**

Same as Table — `sortColumn`, `sortDirection`, `pageSize`, `hiddenColumns`.

**Features:**

- CSV is parsed into an Arrow table with automatic type inference (numeric, boolean, string)
- Registered in the WASM engine under the cell name — queryable by downstream cells
- Displays as a sortable, paginated table (same UI as the Table cell)
- Useful for lookup tables, configuration data, or reference values

**Example:**

A cell named `thresholds` with this CSV:

```csv
metric,warn_threshold,error_threshold
cpu_usage,80,95
memory_usage,70,90
disk_usage,85,95
```

Can be queried by a downstream table cell:

```sql
SELECT m.name, m.value, t.warn_threshold, t.error_threshold
FROM raw_metrics m
JOIN thresholds t ON m.name = t.metric
WHERE m.value > t.warn_threshold
```

---

## ![Map](../../assets/images/cell-icons/map.svg){ .cell-icon } Map

3D map visualization that plots spatial events on a GLB model. Events are rendered as instanced sphere markers at their native `(x, y, z)` coordinates.

**Configuration:**

| Field | Type | Description |
|-------|------|-------------|
| `sql` | string | SQL query returning spatial data |
| `dataSource` | string | Data source override |

**Required columns:**

| Column | Type | Description |
|--------|------|-------------|
| `time` | timestamp | Event timestamp |
| `x` | number | X coordinate (Unreal Engine world units) |
| `y` | number | Y coordinate (Unreal Engine world units) |
| `z` | number | Z coordinate (Unreal Engine world units) |

**Optional columns:**

| Column | Type | Description |
|--------|------|-------------|
| `process_id` | string | Process identifier (links to process page) |
| *(any other)* | string | Displayed as properties in the event detail panel |

All columns beyond the reserved names (`time`, `x`, `y`, `z`, `process_id`) are collected as key-value properties and shown when clicking a marker.

**Options:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mapUrl` | string | none | GLB filename selected from the catalog (bare filename — the renderer composes the blob URL) |
| `markerColor` | string | `#bf360c` | Marker color (hex) |
| `markerSize` | number | `10` | Marker size — scaled proportionally to the map extent |

**Map catalog:**

Maps are stored in a server-side object store and served through the analytics web tier. The catalog is derived at request time by listing every `*.glb` object directly under the configured prefix — there is no `maps.json` to keep in sync.

Configure the prefix via `MICROMEGAS_MAPS_OBJECT_STORE_URI`. The URL grammar matches the rest of the platform (passed through `object_store::parse_url_opts`):

| Environment | Example URI |
|---|---|
| Local dev | `file:///home/you/lake/maps/` (sibling of `lake/blobs/`) |
| AWS prod | `s3://my-bucket/maps/` |

Admins upload `.glb` files through the **Admin → Maps** page. The server gzips on upload and stores the object with `Content-Type: model/gltf-binary` + `Content-Encoding: gzip` so the existing read path serves it verbatim and the browser decodes transparently. Display names are derived client-side: the `.glb` extension is stripped (`Arena_North.glb` → `Arena_North`).

Ops can still drop `.glb` files directly into the configured prefix (no UI required), but the upload must set the same `Content-Type` + `Content-Encoding` attributes — the Admin → Maps page is the recommended path because it handles the compression and metadata in one shot.

If `MICROMEGAS_MAPS_OBJECT_STORE_URI` is unset, `/api/maps/catalog` returns 503 and the dropdown is empty. The local-dev start script (`start_analytics_web.py`) defaults this to `<MICROMEGAS_OBJECT_STORE_URI>/maps/` — i.e. a `maps/` sibling of the telemetry lake — so a single lake root supplies both telemetry blobs and map assets.

**Upload size:**

Uploads through the Admin → Maps page are capped per request at 256 MiB by default; override with `MICROMEGAS_MAPS_MAX_UPLOAD_BYTES` (raw byte count).

**Storage compression:**

GLBs may be stored gzipped: set `Content-Encoding: gzip` and `Content-Type: model/gltf-binary` on the object. The endpoint passes both headers through verbatim and never re-encodes — the server burns no CPU compressing on the fly, `Content-Length` reflects the wire size, and the browser auto-decodes before handing bytes to three.js. Bare uploads (no metadata) are served uncompressed.

**Coordinate frame:**

Events are placed at their raw `x`, `y`, `z` values without any runtime transform. The GLB is expected to be authored in the same frame and units the events are emitted in — see the GLB authoring contract below.

**GLB authoring contract:**

The renderer expects each GLB to satisfy these invariants — there is no fallback path:

- **Z-up, left-handed, centimeters** — matches the Unreal Engine world frame; no auto-centering. Events flow through unmodified.
- **Exactly one perspective camera** referenced from `scenes[0]`, used to seed the initial camera (position, orientation, fov, near, far). Camera roll is dropped when seeding the orbit controller.
- **`KHR_lights_punctual`** — directional lights live inside the scene tree and render automatically.
- **`MM_ambient_light`** vendor extension — `{ color: [r, g, b], intensity: number }` at the root extensions; the renderer reads it directly from `gltf.parser.json.extensions`.

GLBs missing the camera log a console error and fall back to the default seed framing (likely mis-framed); GLBs missing `MM_ambient_light` log a console error and render without ambient illumination. These are visible failure modes that signal a non-conforming GLB.

> Because the contract uses Z-up / left-handed / centimeters, the GLBs are technically out of spec for glTF 2.0 (which mandates Y-up RH meters). External viewers — Blender, online glTF validators, Windows 3D Viewer — will render them rotated or flag warnings. The micromegas web-app is the only intended consumer.

**Features:**

- Instanced rendering — handles thousands of markers efficiently
- Interactive markers — click to select, hover to highlight
- Camera controls — left-drag to pan, right-drag to orbit, scroll to zoom, WASD to fly
- Reset view with the Z key
- Event detail panel with properties and link to process logs
- Results registered in the [local WASM query engine](execution.md#local-wasm-query-engine) under the cell name for downstream queries

**Example SQL (spatial events from a JSONB-encoded payload):**

```sql
SELECT
  time,
  process_id,
  jsonb_as_f64(jsonb_path_query_first(msg_jsonb, '$.position[0]')) as x,
  jsonb_as_f64(jsonb_path_query_first(msg_jsonb, '$.position[1]')) as y,
  jsonb_as_f64(jsonb_path_query_first(msg_jsonb, '$.position[2]')) as z,
  jsonb_as_string(jsonb_path_query_first(msg_jsonb, '$.actor_id')) as actor_id,
  jsonb_as_string(jsonb_path_query_first(msg_jsonb, '$.event_type')) as event_type
FROM events
WHERE name = 'spatial_event'
ORDER BY time DESC
LIMIT 10000
```

---

## ![Horizontal Group](../../assets/images/cell-icons/group.svg){ .cell-icon } Horizontal Group (HG)

A container cell that arranges its children side by side in a horizontal layout.

**Configuration:**

| Field | Type | Description |
|-------|------|-------------|
| `children` | array | Child cell configurations (any type except HG) |

**Features:**

- Children render side by side with equal width
- Drag children horizontally to reorder within the group
- Drag a child vertically (out of the group area) to extract it to the main cell list
- Add new children via the group editor
- Each child has independent execution, state, and data source settings
- Aggregate stats (row count, byte size) displayed in the group header

**Constraints:**

- HG cells cannot be nested — no HG inside another HG
- During execution, children are flattened into the main sequence and execute left to right
- Variables defined in child cells are visible to cells below the group

**Example use case:**

Place two related charts side by side — one showing CPU usage and another showing memory usage — for a compact comparison view.

![Horizontal group with table, chart, and log cells side by side](../../assets/images/notebooks/horizontal_group.png){ .screenshot }
