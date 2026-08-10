# Advertise Originating Notebook and Cell on Query Requests Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1437

## Overview

Every FlightSQL query issued by the analytics web app reports `client=web` and nothing else, so a
notebook cell, the standalone query editor, and a log view are indistinguishable in
`flightsql_query_audit`. This plan threads the originating notebook name and cell name down to the
request, forwards them from `analytics-web-srv` to FlightSQL as gRPC metadata, and records them in
the audit record next to `client` — mirroring the `x-client-agent`/`x-client-entrypoint`/
`x-client-session` pattern added for the Python client in #1436, so both clients' audit records read
consistently. Both fields stay optional: a query with no notebook context (the standalone editor)
simply omits them and remains plain `client=web`.

## Current State

### The path is browser → `analytics-web-srv` → FlightSQL

- `rust/analytics-web-srv/src/web_server.rs:234` serves `{base_path}/api/query-stream`.
- `rust/analytics-web-srv/src/stream_query.rs:30-39` — `StreamQueryRequest` is the browser
  contract: `sql`, `params`, `begin`, `end`, `data_source`. No origin fields. `begin`/`end` are
  already `Option<T>` with no `#[serde(default)]` attribute — serde's derive treats missing JSON
  keys as `None` for `Option<T>` fields automatically, which is the pattern new optional fields
  should follow.
- `rust/analytics-web-srv/src/stream_query.rs:244-248` — inside the `stream!{}` block, the handler
  builds `BearerFlightSQLClientFactory::new_with_client_type(flightsql_url, auth_token.0,
  "web".to_string())` once per query. This is the only call site of the factory in the whole
  codebase (verified via repo-wide grep), so it's safe to extend without combinatorial-constructor
  concerns.
- `rust/public/src/client/flightsql_client_factory.rs:14-73` — `BearerFlightSQLClientFactory`
  carries only `client_type: Option<String>`, set via `set_header("x-client-type", ...)` in
  `make_client()` (`:118-123`). There's no generic way to attach arbitrary metadata.

### The browser side funnels through one shared type — but not the path the issue names

`analytics-web-app/src/lib/arrow-stream.ts:136-142` defines `StreamQueryParams` (`sql`, `params`,
`begin`, `end`, `dataSource`), consumed by both `streamQuery()` (`:157-170`, POST body at
`:162-168`) and `fetchQueryIPC()` (`:319-337`, POST body at `:329-335`). This is the actual single
choke point for every FlightSQL-bound query the web app issues — the issue's own claim that "this is
one change in one place rather than per-`fetch`" is correct, but the *specific* chain it names
(`ScreenQueryParams` → `useStreamQuery` → `StreamQueryParams`) is not where notebook cells go:

- `useScreenQuery` (`analytics-web-app/src/lib/screen-renderers/useScreenQuery.ts`) builds a
  `StreamQueryParams` from its `ScreenQueryParams` input and calls `useStreamQuery().execute()`.
  Its **only** caller is `MetricsRenderer.tsx` — a non-notebook screen type with no cell concept.
- Notebook cells execute through `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts`,
  which calls `streamQuery()` (its own `executeSql()` helper, `:53-82`) and `fetchQueryIPC()`
  (inline in `context.runQuery`/`runQueryAs`, `:209-275`) **directly**, never going through
  `useScreenQuery`/`ScreenQueryParams` at all. This is the code the issue is actually motivated by
  ("notebook and cell identity is not available at the request site... the renderers know which
  notebook and cell they are; the information is dropped on the way down").

So the correct extension point is `StreamQueryParams` (the shared type both paths already funnel
through) plus `useCellExecution.ts` (the path that actually knows the cell). `useScreenQuery`/
`ScreenQueryParams` need no changes — `MetricsRenderer` is not a notebook cell and has nothing to
attribute.

### Where "notebook" identity comes from

- `NotebookRenderer.tsx` (`analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx:335-343`)
  calls `useCellExecution({ cells, rawTimeRange, variableValuesRef, setVariableValue,
  refreshTrigger, dataSource, engine })` — no notebook identity passed in today.
- `ScreenRendererProps` (`analytics-web-app/src/lib/screen-renderers/index.ts:16-45`) — the prop
  contract every renderer receives from `ScreenPage.tsx` — has no screen/notebook name field.
- `ScreenPage.tsx:468-483` renders `<Renderer key={screen?.name ?? 'new'} config={...} ... />` —
  `screen?.name` (the notebook's saved name, `undefined` for an unsaved new screen) is already in
  scope there but isn't forwarded as a prop.
- Cells are keyed by **name** in the notebook model (`CellConfig.name`); `useCellExecution.ts`
  already threads `cell.name` through `migrateCellState`/`removeCellState`/`updateCellSelection`,
  confirming name is the established identifier, per the issue's own reasoning.

### The audit record already has the analogous fields from #1436

- `rust/public/src/servers/flight_sql_service_impl.rs:558-574` reads `x-client-type`,
  `x-client-agent`, `x-client-entrypoint`, `x-client-session` from gRPC metadata (last one
  `Option<String>`, others default `"unknown"`), logs `agent`/`entrypoint` in both start-of-query
  `info!` lines (`:580-593`), and populates `QueryAuditState` (`:262-295`) accordingly.
- `rust/public/src/servers/query_audit.rs:79-123` — `QueryAuditRecord`, the struct serialized to
  the `flightsql_query_audit` log target, has `client`, `agent`, `entrypoint`,
  `session: Option<String>` (`#[serde(skip_serializing_if = "Option::is_none")]`) as its first four
  fields.
- `rust/public/src/servers/http_gateway.rs:44-56` — `HeaderForwardingConfig::default()`'s
  `allowed_headers` already forwards `X-Client-Type`/`X-Client-Agent`/`X-Client-Entrypoint`/
  `X-Client-Session` through the gateway. The web app's own path (browser →
  `analytics-web-srv` → FlightSQL) does not go through this gateway, but #1436 forwards its three
  headers here anyway for any deployment that does route through it — the new headers follow the
  same precedent for consistency.
- `tasks/completed/1436_client_attribution_headers_plan.md` is the direct precedent for this
  plan's shape (header naming, `Option<String>` vs defaulted `String`, gateway allowlist,
  `QueryAuditRecord` placement, doc/changelog structure).

### Sanitization boundary

- `rust/analytics-web-srv/src/stream_query.rs:92-99` — `contains_blocked_function` is already the
  validation boundary for this endpoint, confirming `analytics-web-srv` (not the browser, not
  FlightSQL) is the right place to sanitize per the issue's "Trust and sanitization" section.
- gRPC metadata values must be ASCII (`FlightSqlServiceClient::set_header`, used via
  `client.inner_mut().set_header(...)` in `flightsql_client_factory.rs:119-123`); an unsanitized
  non-ASCII or control-character value risks a panic deep in the tonic/arrow-flight metadata
  encoding, not a catchable error. The Python client's `attribution.py::_sanitize_override`
  (`python/micromegas/micromegas/flightsql/attribution.py:44-53`) already solves the identical
  problem for `MICROMEGAS_CLIENT_AGENT`/`MICROMEGAS_CLIENT_ENTRYPOINT`: reject (not
  truncate-and-keep) the whole value if it isn't bounded, printable ASCII. This plan reuses that
  same reject-whole-value strategy in Rust for `notebook`/`cell`, rather than inventing a
  stripping/truncation scheme.

## Design

### 1. Browser: thread `notebook`/`cell` through the shared `StreamQueryParams`

`analytics-web-app/src/lib/arrow-stream.ts`:

```ts
export interface StreamQueryParams {
  sql: string
  params?: Record<string, string>
  begin?: string
  end?: string
  dataSource?: string
  /** Originating notebook name, for query attribution. Omitted outside a notebook. */
  notebook?: string
  /** Originating cell name within the notebook, for query attribution. */
  cell?: string
}
```

Both `streamQuery()` and `fetchQueryIPC()`'s POST bodies add `notebook: params.notebook, cell:
params.cell,`. `JSON.stringify` omits `undefined` values, so the standalone query editor (which
never sets these) sends exactly what it sends today.

### 2. `NotebookRenderer`/`useCellExecution`: supply the actual values

- `ScreenRendererProps` (`screen-renderers/index.ts`) gains `screenName?: string` — the notebook's
  saved name, `undefined` for an unsaved new screen (matches `screen?.name`'s own optionality).
- `ScreenPage.tsx` passes `screenName={screen?.name}` alongside the other renderer props (`:468`).
- `useCellExecution`'s `UseCellExecutionParams` gains `notebookName?: string`; `NotebookRenderer`
  passes `notebookName: screenName` at its call site (`:335-343`).
- Inside `useCellExecution.ts`, the cell issuing a query is always `cell` (the `CellConfig` being
  executed in `executeCell`), so every one of the four query call sites passes `notebook:
  notebookName, cell: cell.name`:
  - `executeSql()` helper (`:53-82`) gains `cellName`/`notebookName` parameters, forwarded into its
    `streamQuery({...})` call.
  - `context.runQuery`'s remote-with-engine branch (`fetchQueryIPC`, `:217-224`) and
    no-engine branch (`executeSql`, `:237`).
  - `context.runQueryAs`'s remote-with-engine branch (`fetchQueryIPC`, `:254-261`) and no-engine
    branch (`executeSql`, `:273`).
- `PerfettoExportCell` (`screen-renderers/cells/PerfettoExportCell.tsx`) is a notebook cell that
  bypasses `useCellExecution` entirely: `handleOpenInPerfetto`/`handleDownloadTrace` call
  `fetchPerfettoTrace()` (`lib/perfetto-trace.ts:24-82`), which calls `streamQuery()` directly
  (`:35`). It needs the same `notebook`/`cell` values threaded through this separate path:
  - `CellRendererProps` (`screen-renderers/cell-registry.ts:11-...`) gains `notebookName?:
    string` (the cell's own name is already carried as `name`, `:13`).
  - `CellViewContext` (`screen-renderers/notebook-cell-view.ts:11-23`) gains `notebookName?:
    string`; `buildCellRendererProps` (`:224-...`) copies it into the returned props alongside
    `name`/`dataSource` (`:225`, `:243`).
  - `NotebookRenderer.tsx` passes `notebookName: screenName` into the context object it builds for
    `buildCellRendererProps` (`:637-646`), the same `screenName` prop introduced above.
  - `FetchPerfettoTraceOptions` (`perfetto-trace.ts:15-22`) gains `notebook?: string, cell?:
    string`; `fetchPerfettoTrace()` forwards them into its `streamQuery({...})` call (`:35`).
  - `PerfettoExportCell.tsx` destructures `name` and `notebookName` from `CellRendererProps` and
    passes `notebook: notebookName, cell: name` in both `fetchPerfettoTrace()` call sites
    (`handleOpenInPerfetto` `:123-130`, `handleDownloadTrace` `:176-183`).
- Other renderers (`TableRenderer`, `LogRenderer`, `ProcessListRenderer`, `MetricsRenderer` via
  `useScreenQuery`, the process pages) are unchanged — they have no cell concept and keep omitting
  both fields, same as today.

### 3. Server: `StreamQueryRequest` gains the two fields, sanitized before forwarding

`rust/analytics-web-srv/src/stream_query.rs`:

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct StreamQueryRequest {
    pub sql: String,
    #[serde(default)]
    pub params: HashMap<String, String>,
    pub begin: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
    #[serde(default)]
    pub data_source: String,
    pub notebook: Option<String>,
    pub cell: Option<String>,
}
```

A new pure helper, next to `contains_blocked_function`:

```rust
/// Maximum length for a client-supplied origin label (notebook/cell name) before
/// it's rejected outright rather than forwarded into a gRPC header and a log line.
const MAX_ORIGIN_LABEL_LEN: usize = 128;

/// Returns `value` if it's a safe gRPC metadata value (printable ASCII, no control
/// characters, bounded length) and non-empty after trimming, else `None` so the
/// caller silently omits the field — same reject-whole-value strategy as the
/// Python client's `_sanitize_override` (attribution.py) for the analogous
/// MICROMEGAS_CLIENT_AGENT/MICROMEGAS_CLIENT_ENTRYPOINT overrides.
pub fn sanitize_origin_label(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_ORIGIN_LABEL_LEN {
        return None;
    }
    if !trimmed.chars().all(|c| (' '..='~').contains(&c)) {
        return None;
    }
    Some(trimmed.to_string())
}
```

In `stream_query_handler`, sanitize once before building the stream:

```rust
let notebook = request.notebook.as_deref().and_then(sanitize_origin_label);
let cell = request.cell.as_deref().and_then(sanitize_origin_label);
```

Add `notebook={notebook:?} cell={cell:?}` to the existing start-of-request `info!` line
(`:168-171`), and inside the `stream!{}` block, chain the new factory builder method:

```rust
let mut client_factory = BearerFlightSQLClientFactory::new_with_client_type(
    flightsql_url,
    auth_token.0,
    "web".to_string(),
);
if let Some(notebook) = &notebook {
    client_factory = client_factory.with_metadata("x-client-notebook", notebook.clone());
}
if let Some(cell) = &cell {
    client_factory = client_factory.with_metadata("x-client-cell", cell.clone());
}
```

### 4. `BearerFlightSQLClientFactory` gains generic per-call metadata

Rather than adding more `client_type`-style constructor combinations (which would double again for
notebook/cell), `flightsql_client_factory.rs` gains one small, reusable extension point:

```rust
pub struct BearerFlightSQLClientFactory {
    url: String,
    token: String,
    client_type: Option<String>,
    extra_metadata: Vec<(String, String)>,
}
```

- Every existing constructor (`new`, `new_with_client_type`, `from_env`,
  `from_env_with_client_type`) initializes `extra_metadata: Vec::new()`.
- A new builder method:
  ```rust
  /// Attaches an additional gRPC metadata header sent with every request made by
  /// clients from this factory (e.g., the web app's notebook/cell origin labels).
  /// `key` must already be a valid lowercase gRPC metadata key.
  pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
      self.extra_metadata.push((key.into(), value.into()));
      self
  }
  ```
- `make_client()` sets these after the existing `client_type` header (`:118-123`):
  ```rust
  for (key, value) in &self.extra_metadata {
      client.inner_mut().set_header(key, value.clone());
  }
  ```

This is generic on purpose but scoped to exactly what's needed today — no new public API beyond one
builder method — and gives the one existing call site (`stream_query.rs`) a clean way to attach
origin metadata without the factory needing to know what "notebook"/"cell" mean.

### 5. Server: read, log, and audit the two new fields

`flight_sql_service_impl.rs::execute_query` (`:558-574`) reads two more headers, following the
`client_session` precedent (`Option<String>`, no default — there's no meaningful default for "not a
notebook query"):

```rust
let client_notebook = metadata
    .get("x-client-notebook")
    .and_then(|v| v.to_str().ok())
    .filter(|s| !s.is_empty())
    .map(|s| s.to_string());
let client_cell = metadata
    .get("x-client-cell")
    .and_then(|v| v.to_str().ok())
    .filter(|s| !s.is_empty())
    .map(|s| s.to_string());
```

- Both start-of-query `info!` lines (`:580-593`) gain `notebook={client_notebook:?}
  cell={client_cell:?}` — worth skimming directly, per the issue's "finding broken cells after a
  schema change" motivation (grepping logs for a cell name), same reasoning that put
  `agent`/`entrypoint` (not `session`) in these lines.
- `QueryAuditState` (`:262-295`) gains `notebook: Option<String>, cell: Option<String>`, populated
  alongside `session` at construction (`:608-630`).
- `QueryAuditState::emit` (`:307-353`) copies both into the `QueryAuditRecord` literal, alongside
  `session: self.session.clone()`.
- `QueryAuditRecord` (`query_audit.rs:79-123`) gains, placed immediately after `session`:
  ```rust
  #[serde(skip_serializing_if = "Option::is_none")]
  pub notebook: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub cell: Option<String>,
  ```

### 6. Gateway: forward, don't augment (matches #1436)

`http_gateway.rs::HeaderForwardingConfig::default()`'s `allowed_headers` (`:44-56`) gains
`"X-Client-Notebook".to_string()` and `"X-Client-Cell".to_string()`, next to the #1436 headers —
these describe who authored the SQL, the same category as `x-client-agent`/etc., not a hop-chained
header like `x-client-type`.

## Implementation Steps

**Phase 1 — Browser**

1. `analytics-web-app/src/lib/arrow-stream.ts`: add `notebook?: string`, `cell?: string` to
   `StreamQueryParams` (`:136-142`); add both to the POST bodies in `streamQuery()` (`:162-168`)
   and `fetchQueryIPC()` (`:329-335`).
2. `analytics-web-app/src/lib/screen-renderers/index.ts`: add `screenName?: string` to
   `ScreenRendererProps` (`:16-45`).
3. `analytics-web-app/src/routes/ScreenPage.tsx`: pass `screenName={screen?.name}` to `<Renderer
   .../>` (`:468-483`).
4. `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts`:
   - Add `notebookName?: string` to `UseCellExecutionParams` (`:18-33`).
   - `executeSql()` (`:53-82`) gains `notebookName?: string, cellName?: string` parameters,
     forwarded into its `streamQuery({...})` call.
   - Update all four `executeSql`/`fetchQueryIPC` call sites inside `executeCell`'s `runQuery`/
     `runQueryAs` (`:209-275`) to pass `notebook: notebookName, cell: cell.name`.
   - Add `notebookName` to `executeCell`'s `useCallback` dependency array (`:307`, currently
     `[cells, rawTimeRange, variableValuesRef, setVariableValue, dataSource, engine,
     completeCellExecution]`) — it's now read from the closure at all four call sites, so
     `eslint-plugin-react-hooks`'s exhaustive-deps rule requires it there.
5. `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`: accept `screenName` from
   `ScreenRendererProps`; pass `notebookName: screenName` into `useCellExecution({...})`
   (`:335-343`); pass `notebookName: screenName` into the context object built for
   `buildCellRendererProps` (`:637-646`), alongside `dataSource: cellDataSource`.
6. `analytics-web-app/src/lib/screen-renderers/cell-registry.ts`: add `notebookName?: string` to
   `CellRendererProps` (`:11-...`), next to the existing `name: string` (`:13`).
7. `analytics-web-app/src/lib/screen-renderers/notebook-cell-view.ts`: add `notebookName?: string`
   to `CellViewContext` (`:11-23`); `buildCellRendererProps` (`:224-...`) copies
   `notebookName: context.notebookName` into the returned props, alongside `name`/`dataSource`
   (`:225`, `:243`).
8. `analytics-web-app/src/lib/perfetto-trace.ts`: add `notebook?: string, cell?: string` to
   `FetchPerfettoTraceOptions` (`:15-22`); forward them into the `streamQuery({...})` call inside
   `fetchPerfettoTrace()` (`:35`).
9. `analytics-web-app/src/lib/screen-renderers/cells/PerfettoExportCell.tsx`: destructure `name`
   and `notebookName` from `CellRendererProps` (`:15-20`); pass `notebook: notebookName, cell:
   name` in both `fetchPerfettoTrace()` call sites (`handleOpenInPerfetto` `:123-130`,
   `handleDownloadTrace` `:176-183`) — this cell type calls `fetchPerfettoTrace()` directly and
   never goes through `useCellExecution`, so it needs this separate wiring.

**Phase 2 — Browser tests**

10. `analytics-web-app/src/lib/__tests__/arrow-stream.test.ts`: add cases asserting `notebook`/
    `cell` appear in the POST body when passed, and are absent (`undefined`, dropped by
    `JSON.stringify`) when omitted, for both `streamQuery()` and `fetchQueryIPC()`.
11. `analytics-web-app/src/lib/screen-renderers/__tests__/useCellExecution.test.ts`: extend the
    existing `mockStreamQuery`/`mockFetchQueryIPC` assertions (e.g. the "should execute SQL for
    table cells" and "should use fetchQueryIPC..." cases) to assert the call arguments include
    `notebook: <notebookName>, cell: <cell.name>` when `notebookName` is passed to the hook, and
    `notebook: undefined` when it isn't.
12. `analytics-web-app/src/lib/__tests__/perfetto-trace.test.ts`: extend the existing
    `mockStreamQuery` assertions to cover `notebook`/`cell` being forwarded into the `streamQuery`
    call when passed in `FetchPerfettoTraceOptions`, and absent when omitted.
13. `analytics-web-app/src/lib/screen-renderers/cells/__tests__/PerfettoExportCell.test.tsx`:
    extend the existing `fetchPerfettoTrace` mock assertions to confirm `notebook`/`cell` are
    passed through from `notebookName`/`name` props on both the "Open in Perfetto" and "Download"
    actions.

**Phase 3 — Rust server**

14. `rust/public/src/client/flightsql_client_factory.rs`: add `extra_metadata: Vec<(String,
    String)>` field (initialized empty in all four constructors), add `with_metadata(mut self,
    key: impl Into<String>, value: impl Into<String>) -> Self` builder, and set the extra headers
    in `make_client()` after the existing `client_type` header (`:118-123`).
15. `rust/analytics-web-srv/src/stream_query.rs`:
    - Add `pub notebook: Option<String>, pub cell: Option<String>` to `StreamQueryRequest`
      (`:30-39`).
    - Add `MAX_ORIGIN_LABEL_LEN` and `sanitize_origin_label()` next to `contains_blocked_function`.
    - Sanitize `request.notebook`/`request.cell` at the top of `stream_query_handler`; add
      `notebook={notebook:?} cell={cell:?}` to the start-of-request `info!` line (`:168-171`);
      chain `.with_metadata("x-client-notebook", ...)`/`.with_metadata("x-client-cell", ...)` onto
      the `client_factory` inside the `stream!{}` block (`:244-248`) when present.
16. `rust/public/src/servers/flight_sql_service_impl.rs::execute_query`: read
    `x-client-notebook`/`x-client-cell` (`Option<String>`, alongside `client_session` at
    `:570-574`); add `notebook={client_notebook:?} cell={client_cell:?}` to both `info!` lines
    (`:580-593`); add `notebook`/`cell` fields to `QueryAuditState` (`:262-295`), populated at
    construction (`:608-630`); thread them into `QueryAuditState::emit`'s `QueryAuditRecord`
    construction (`:320-348`).
17. `rust/public/src/servers/query_audit.rs`: add `#[serde(skip_serializing_if =
    "Option::is_none")] pub notebook: Option<String>` and the same for `cell`, placed after
    `session` (`:79-123`).
18. `rust/public/src/servers/http_gateway.rs::HeaderForwardingConfig::default()`: add
    `"X-Client-Notebook"` and `"X-Client-Cell"` to `allowed_headers` (`:44-56`).

**Phase 4 — Rust tests**

19. `rust/analytics-web-srv/tests/stream_query_tests.rs`: add unit tests for
    `sanitize_origin_label` — a normal name passes through unchanged; a control character
    (`"cell\u{0}name"`) is rejected; an over-`MAX_ORIGIN_LABEL_LEN` string is rejected; a non-ASCII
    string (e.g. `"café"`) is rejected; an empty or whitespace-only string is rejected; leading/
    trailing whitespace is trimmed on an otherwise-valid label.
20. `rust/public/tests/query_audit_tests.rs`: add `notebook`/`cell` to the full-record fixture
    (`full_record`, `:187-`) asserting both are present when set, and to the omits-optionals
    fixture (`:256-`) asserting both are omitted from the JSON when `None` — matching the
    `session` precedent already there.
21. `rust/public/tests/http_gateway_tests.rs`: extend `test_default_config` to assert
    `should_forward("X-Client-Notebook")` and `should_forward("X-Client-Cell")` are both `true`.

**Phase 5 — Docs**

22. `mkdocs/docs/query-guide/query-audit-log.md`: add `notebook`/`cell` rows to the `## Fields`
    table (both present only when the query originated from a notebook cell); note in `## Notes`
    that cell names are mutable — grouping by name splits across a rename (`migrateCellState`),
    which is acceptable for analytics per the issue's own caveat.
23. `mkdocs/docs/gateway/configuration.md`: add `X-Client-Notebook`/`X-Client-Cell` to the "Default
    headers" bullet list, with the same `MICROMEGAS_GATEWAY_HEADERS`-replaces-not-merges caveat
    already documented there for the #1436 headers.
24. `CHANGELOG.md`: `## Unreleased` → **Analytics:** entry for the server/audit/gateway changes,
    flagging `QueryAuditRecord` as a **minor breaking change** again (gains `notebook`, `cell`),
    and a **Web App:** entry for the notebook-cell attribution. Note that a deployment with a
    custom `MICROMEGAS_GATEWAY_HEADERS` allowlist must add the two new header names explicitly.

## Files to Modify

- `analytics-web-app/src/lib/arrow-stream.ts` — `StreamQueryParams`, both POST bodies.
- `analytics-web-app/src/lib/screen-renderers/index.ts` — `ScreenRendererProps`.
- `analytics-web-app/src/routes/ScreenPage.tsx` — pass `screenName`.
- `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts` — `notebookName` param,
  `executeSql()`, four query call sites.
- `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` — pass `notebookName` to
  `useCellExecution` and to `buildCellRendererProps`'s context.
- `analytics-web-app/src/lib/screen-renderers/cell-registry.ts` — `notebookName` on
  `CellRendererProps`.
- `analytics-web-app/src/lib/screen-renderers/notebook-cell-view.ts` — `notebookName` on
  `CellViewContext`, copied in `buildCellRendererProps`.
- `analytics-web-app/src/lib/perfetto-trace.ts` — `notebook`/`cell` on
  `FetchPerfettoTraceOptions`, forwarded to `streamQuery()`.
- `analytics-web-app/src/lib/screen-renderers/cells/PerfettoExportCell.tsx` — pass
  `notebook`/`cell` into both `fetchPerfettoTrace()` call sites.
- `analytics-web-app/src/lib/__tests__/arrow-stream.test.ts` — new cases.
- `analytics-web-app/src/lib/screen-renderers/__tests__/useCellExecution.test.ts` — extended
  assertions.
- `analytics-web-app/src/lib/__tests__/perfetto-trace.test.ts` — extended assertions.
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/PerfettoExportCell.test.tsx` —
  extended assertions.
- `rust/public/src/client/flightsql_client_factory.rs` — `extra_metadata`, `with_metadata()`.
- `rust/analytics-web-srv/src/stream_query.rs` — `StreamQueryRequest`, `sanitize_origin_label`,
  handler wiring.
- `rust/public/src/servers/flight_sql_service_impl.rs` — header reads, log lines,
  `QueryAuditState`.
- `rust/public/src/servers/query_audit.rs` — `QueryAuditRecord` fields.
- `rust/public/src/servers/http_gateway.rs` — `HeaderForwardingConfig::default()`.
- `rust/analytics-web-srv/tests/stream_query_tests.rs` — `sanitize_origin_label` tests.
- `rust/public/tests/query_audit_tests.rs` — fixture updates.
- `rust/public/tests/http_gateway_tests.rs` — allowlist assertions.
- `mkdocs/docs/query-guide/query-audit-log.md` — `## Fields` table, `## Notes`.
- `mkdocs/docs/gateway/configuration.md` — default headers list.
- `CHANGELOG.md` — `## Unreleased` entries.

## Trade-offs

- **`StreamQueryParams`/`useCellExecution`, not `ScreenQueryParams`/`useStreamQuery`.** The issue
  names the latter chain, but tracing the actual notebook-cell code path (see Current State) shows
  it never goes through `useScreenQuery`. Extending `ScreenQueryParams` would touch a hook
  (`useScreenQuery`) that no notebook cell ever calls, achieving nothing for the issue's motivating
  case. `StreamQueryParams` is the real shared choke point both paths already use.
- **Generic `with_metadata()` builder over more `client_type`-style constructors.** The factory
  already has 4 constructors for one optional field (`client_type`); adding notebook/cell as
  further constructor parameters would require another 4 (or 8) combinations. A single chainable
  builder method scales to any number of optional origin labels without combinatorial growth, and
  the one existing call site (`stream_query.rs`) already builds its factory imperatively inside a
  `stream!{}` block, so chaining fits naturally.
- **Reject-whole-value sanitization, not truncate-or-strip.** Matches the Python client's
  `_sanitize_override` precedent (#1436) for the same underlying problem (untrusted string bound
  for a gRPC metadata value): a truncated or character-stripped label could silently misattribute a
  query to a different, truncated name; rejecting outright (falling back to "no notebook/cell
  reported," same as the standalone editor) is simpler and avoids that ambiguity.
- **`notebook`/`cell` shown in the start-of-request `info!` lines, unlike `session`.** #1436 kept
  `session` out of its free-text log line because the opaque id is only useful for correlating
  structured audit records. Notebook/cell names are short, human-readable labels precisely useful
  for skimming logs (the issue's "finding broken cells" motivation is a grep-the-logs workflow), so
  they follow the `agent`/`entrypoint` precedent instead.
- **Grouping by cell *name*, not a new stable per-cell id.** Per the issue's own analysis: names are
  the established identifier throughout `useCellExecution.ts` already; a rename
  (`migrateCellState`) splits historical grouping, which is accepted as adequate for analytics. A
  stable id is a larger, separate change to the notebook format and is out of scope here. The same
  reasoning applies to `notebook`: `Screen` (`analytics-web-app/src/lib/screens-api.ts:25-35`) has
  no id field at all — `getScreen`/`updateScreen`/`deleteScreen` are all keyed by `name` — so the
  saved screen name is the only identifier available today, not a choice among alternatives.
- **No change to `useScreenQuery`/`ScreenQueryParams`, `TableRenderer`, `LogRenderer`,
  `ProcessListRenderer`, or the process pages.** None of these have a "cell" concept; they keep
  omitting `notebook`/`cell`, unchanged from today's `client=web`-only attribution.

## Documentation

- `mkdocs/docs/query-guide/query-audit-log.md` — `## Fields` table, `## Notes` (cell-rename
  caveat).
- `mkdocs/docs/gateway/configuration.md` — default forwarded-headers list.
- `CHANGELOG.md` — `## Unreleased` entries (Analytics + Web App).

## Testing Strategy

1. `yarn test` from `analytics-web-app/` (per `analytics-web-app/CLAUDE.md`) — covers the new
   `arrow-stream.test.ts` cases and the extended `useCellExecution.test.ts` assertions.
2. `yarn lint` and `yarn type-check` from `analytics-web-app/`.
3. `cargo test -p micromegas --features server` and `cargo test -p analytics-web-srv` — covering
   the new `sanitize_origin_label` unit tests, the updated `query_audit_tests.rs`, and
   `http_gateway_tests.rs`.
4. `cargo fmt` and `cargo clippy --workspace -- -D warnings` (per `rust/CLAUDE.md`).
5. Manual smoke test: start services (`python3 local_test_env/ai_scripts/start_services.py`),
   open a notebook screen in the web app, run a cell, and `tail -f /tmp/analytics.log` for that
   query's `execute_query` line — confirm `notebook=Some("<screen name>") cell=Some("<cell
   name>")` appears. Then query `flightsql_query_audit` (per `query-audit-log.md`'s pattern) and
   confirm the JSON record has `"notebook"` and `"cell"` keys with the expected values. Run a query
   from the standalone query editor (a non-notebook screen) and confirm both keys are absent from
   its audit record, same as today.
