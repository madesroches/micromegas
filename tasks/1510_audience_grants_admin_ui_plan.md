# Audience Grants Admin UI Plan (#1510)

## Overview

Add an **Admin → Audience Grants** page to `analytics-web-app` for the `audience_grants` store
behind `/api/audience-grants` (#1489): see every grant grouped by audience, create a grant, delete
one by its `(audience, axis, selector)` natural key, with `created_at`/`created_by` visible for
auditability. This closes for grants the gap #1411 closed for API keys — today the only way to
answer "why can't this user see their data" is `micromegas-grants` over SSH or hand-written
`curl`.

**Scope change from the issue.** #1510 framed this as UI-only ("No new server-side routes"). Two
decisions taken during design override that, on explicit direction:

1. The UI groups grants **by audience** (mockup Option B). Grouping is only honest if the page
   holds the *complete* grant set; the current JSON list route pages by row under `created_at
   DESC`, so a page boundary would split an audience's grants and the card would claim "1 grant"
   for an audience that has three.
2. Rather than work around that client-side (loop every page, cap the total, warn when the cap is
   hit), **`GET /api/audience-grants` is converted from a paginated JSON array to a streamed Arrow
   IPC result set** over the JSON-framed protocol `/api/query-stream` already uses. Row count
   stops being a design constraint, the client-side page-loop and its cap disappear, and the
   route's existing silent-truncation bug goes with them.

`POST` and `DELETE` stay JSON — a single row and an empty body gain nothing from Arrow.
`GET .../my-audiences` stays JSON: it is caller-scoped, tiny, and CLI-facing.

## Current State

### Server — `rust/analytics-web-srv/src/audience_grants.rs`

All handlers `AdminUser`-gated except `my-audiences`.

| Route | Notes |
|---|---|
| `GET /api/audience-grants?audience=&axis=&limit=&offset=` | JSON array, `ORDER BY created_at DESC`. **`limit` defaults to 100 and is clamped to `MAX_LIMIT = 500`** — omitting it silently truncates with no indication anything is missing. `limit <= 0` / `offset < 0` are 400. `axis` is validated; `audience` is **not**, and is matched `WHERE audience = $1` — exact equality, not a prefix. |
| `POST /api/audience-grants` | Body `{audience, axis, selector}`, `deny_unknown_fields`. **`201` when this call created the row, `200` when it already existed** (`insert_or_get`'s CTE upsert); full `GrantResponse` either way. |
| `DELETE /api/audience-grants?audience=&axis=&selector=` | Natural key in **query params**, not a path segment — a `group:<id>` may contain `/`, `?`, and other URL-significant characters. `204` on success, `404` if absent. |
| `GET /api/audience-grants/my-audiences` | Caller-scoped, `AuthenticatedUser`-gated, behind `MICROMEGAS_SELF_SERVICE_MINT`. Not used by this page. |

Validation on create: `validate_audience` → `is_valid_audience` (`[A-Za-z0-9_-]{1,255}`);
`validate_axis` (`read`/`mint`); `validate_selector` → `valid_selector` (`*`, `user:<id>`,
`group:<id>`) **plus** `selector.len() > MAX_SELECTOR_BYTES (255)` → 400 — a **byte** bound
(`str::len`), because the column is `VARCHAR(255)`.

Errors are uniformly `{code, message}`: `BAD_REQUEST`, `NOT_FOUND`, `NOT_CONFIGURED` (503,
`MICROMEGAS_SQL_CONNECTION_STRING` unset), `DATABASE_ERROR`, `INTERNAL_ERROR`. Separately, under
`--disable-auth`, `web_server.rs` merges `key_management_disabled_router` in place of the real
router, so every `/api/audience-grants` request is answered by a fixed **503**.

### Server — the Arrow streaming precedent

`rust/analytics-web-srv/src/stream_query.rs` already implements the whole transport, for
`POST /api/query-stream`:

- A JSON-framed protocol over one `Body::from_stream`, content type
  `application/x-micromegas-arrow-stream`: `{"type":"schema","size":N}\n` + N bytes,
  `{"type":"batch","size":N,"rows":M}\n` + N bytes, `{"type":"done"}\n`, or
  `{"type":"error","code":"..","message":".."}\n`.
- `pub fn encode_schema(...)` / `pub fn encode_batch(...)` (lines 142-176) over
  `arrow_ipc::writer::{IpcDataGenerator, DictionaryTracker, IpcWriteOptions, write_message}`,
  with `LZ4_FRAME` compression.
- Private `json_line`, `DataHeader`, `DoneFrame`, `ErrorFrame`, and a query-specific `ErrorCode`
  enum serialized `SCREAMING_SNAKE_CASE`.

`arrow-ipc` is already a dependency of the crate (`Cargo.toml:16`).

### Frontend — `analytics-web-app`

- `src/lib/arrow-stream.ts` — the client half of that protocol: `BufferedReader`, the frame loop,
  and a `RecordBatchReader` fed from a byte queue. Currently hard-wired to
  `POST ${getApiBase()}/query-stream` inside `streamQuery` (line 157) and `fetchQueryIPC`
  (line 325); `ErrorCode` is a closed union of the four query codes.
- `src/lib/arrow-compression.ts` — registers the LZ4 decoder; imported for side effects.
- `src/lib/arrow-utils.ts` — `timestampToDate(value, type)`, already the way
  `query-deny-list-api.ts` decodes `created_at` out of an Arrow column.
- `src/components/ApiKeysAdminPage.tsx` — page skeleton to match: `AuthGuard requireAdmin` →
  `PageLayout onRefresh` → breadcrumb → header + primary action → `ErrorBanner` → modal create
  dialog → `ConfirmDialog` → loading / empty / content.
- `src/routes/QueryDenyListPage.tsx` — the file-organization precedent (single route file, local
  dialog component, `Suspense` wrapper on the default export).
- `src/lib/data-sources-api.ts` — the REST-client shape for the JSON half (typed interfaces, one
  `Error` subclass with `code`/`status`, private `handleResponse`, `authenticatedFetch`).
- `src/routes/AdminPage.tsx` (seven cards) and `src/router.tsx` (lazy import + one `<Route>` each).

### Other clients of the list route

- `python/micromegas/micromegas/web_client.py:256` — `list_audience_grants(...)`, returns
  `resp.json()`.
- `python/micromegas/micromegas/cli/grants.py` — `cmd_list`, `--format table|json`,
  `--limit`/`--offset`.
- `python/micromegas/tests/cli/test_grants.py`.
- `pyarrow ^23.0.0` is already a dependency (`python/micromegas/pyproject.toml:19`).

No `curl` example for this route exists in the docs (only for the two key-mint routes,
`api-keys.md:308,324`).

### Documentation

- `mkdocs/docs/admin/web-app.md:56-57` says outright that grants are *"not yet a web UI page"*.
- `mkdocs/docs/admin/authentication.md` §*Audiences and Grants* / §*Self-service ingestion key mint*
  show `micromegas-grants` invocations.
- `mkdocs/docs/admin/api-keys.md:247-254, 405-410` reference the grants API and CLI.

## Design

### 1. Server: extract the Arrow-stream transport — `rust/analytics-web-srv/src/arrow_stream.rs` (new)

The framing lives in `stream_query.rs` today with a query-specific error enum. A second producer
makes that the wrong home. Move the transport out, unchanged in behavior:

```rust
pub const ARROW_STREAM_CONTENT_TYPE: &str = "application/x-micromegas-arrow-stream";

pub fn json_line<T: Serialize>(value: &T) -> Bytes;

#[derive(Serialize)] pub struct DataHeader { .. }   // type/size/rows
#[derive(Serialize)] pub struct DoneFrame  { .. }
/// `code` is a plain `&'static str`, not an enum: it is wire-identical to what
/// `stream_query::ErrorCode`'s SCREAMING_SNAKE_CASE serialization already emits, and each
/// producer has its own code vocabulary (`INVALID_SQL` here, `NOT_CONFIGURED` there).
#[derive(Serialize)] pub struct ErrorFrame { pub code: &'static str, pub message: String }

/// Owns the `DictionaryTracker` + `IpcWriteOptions` pair that must stay consistent between the
/// schema message and every batch — the invariant `stream_query.rs`'s comment calls out and
/// currently maintains by hand at each call site.
pub struct ArrowStreamEncoder { .. }
impl ArrowStreamEncoder {
    /// LZ4_FRAME compression, same as today.
    pub fn new() -> Self;
    pub fn encode_schema(&mut self, schema: &Schema) -> Result<Vec<u8>>;
    pub fn encode_batch(&mut self, batch: &RecordBatch) -> Result<Vec<u8>>;
}
```

`stream_query.rs` drops its copies and uses these; its `ErrorCode` enum becomes a set of
`&'static str` constants, or keeps the enum and passes `.as_str()`. Its wire output does not
change. (Rust API churn is fine here per `CLAUDE.md`; the wire format is what must stay put — and
does.)

### 2. Server: `list_grants` returns an Arrow stream

Same route, same query params, new representation.

**Schema** (column order is the stable part — it is what the UI and the CLI read positionally):

| Column | Arrow type | Null |
|---|---|---|
| `audience` | `Utf8` | no |
| `axis` | `Utf8` | no |
| `selector` | `Utf8` | no |
| `created_at` | `Timestamp(Nanosecond, Some("UTC"))` | no |
| `created_by` | `Utf8` | no |

`axis` stays plain `Utf8` rather than `Dictionary(Int32, Utf8)`: two distinct values across a
LZ4-compressed batch cost almost nothing, and a dictionary column adds cross-batch tracker
subtleties for no measurable gain.

**Paging changes.** `limit`/`offset` remain accepted (the CLI and bounded probes still use them,
and dropping accepted params is a gratuitous break), but:

- **`limit` absent now means "all rows", not 100.** This is the bug fix: the current
  `DEFAULT_LIMIT` silently truncates any deployment with more than 100 grants. A streaming
  response has no reason to truncate.
- When `limit` *is* supplied it is still validated (`> 0`) and still clamped to `MAX_LIMIT`.
  `offset` is unchanged.
- `DEFAULT_LIMIT` is deleted.

**Streaming, not buffering.** The handler uses `sqlx::query_as(...).fetch(&pool)` (a `Stream`),
accumulating rows into `RecordBatch`es of `GRANT_BATCH_ROWS = 4096` and yielding each as it fills,
so peak memory is one batch rather than the whole table. This holds a pool connection for the
duration of the response — acceptable on an admin route, and the reason the batch size is small
enough to keep the stream moving.

**Errors.** A failure *before* the first byte (pool absent → `NOT_CONFIGURED`, bad `limit`/`axis`
→ `BAD_REQUEST`) is returned as today: an HTTP status plus the `{code, message}` JSON body, so
`AudienceGrantError::into_response` keeps working unchanged and existing 4xx/5xx handling is
untouched. A failure *mid-stream* (a `sqlx` error on row N, an encode error) can no longer change
the status code, so it is emitted as a terminal `{"type":"error","code":"DATABASE_ERROR",...}`
frame — the same shape `stream_query.rs` uses for the identical situation. The client must treat a
stream that ends without a `done` frame as a failure, never as an empty result.

The four `(audience, axis)` filter branches keep their current shape; only the `limit` clause
becomes conditional.

### 3. Frontend: generalize `arrow-stream.ts`

Extract the transport half of `streamQuery` so a second endpoint can use it:

```ts
/** Widened from the closed four-code union: `code` is server-authored and there are now two
 *  producers with different vocabularies. `isRetryable` still only knows the query codes. */
export type ErrorCode = string

/** Parses the JSON-framed Arrow protocol out of an already-issued Response.
 *  This is `streamQuery`'s existing body, verbatim, minus the fetch. */
export async function* readArrowStream(
  response: Response, signal?: AbortSignal
): AsyncGenerator<StreamResult>

/** GET a JSON-framed Arrow endpoint and accumulate it into one Table.
 *  Rejects if the stream ends without a `done` frame. `onProgress` reports rows so far. */
export async function fetchArrowTable(
  url: string, opts?: { signal?: AbortSignal; onProgress?: (rows: number) => void }
): Promise<Table>
```

`streamQuery` becomes `authenticatedFetch(POST /query-stream)` + `readArrowStream`, with its
existing pre-stream 401/403 handling intact. No behavior change for any current caller.

### 4. Frontend: `src/lib/audience-grants-api.ts` (new)

```ts
export type GrantAxis = 'read' | 'mint'

export interface AudienceGrant {
  audience: string
  axis: GrantAxis
  selector: string
  createdAt: Date | null
  createdBy: string
}

export class AudienceGrantError extends Error {
  constructor(public code: string, message: string, public status: number) { ... }
}

/** GET /api/audience-grants as Arrow. No limit/offset — the stream carries the whole
 *  (optionally filtered) set, which is what makes grouping by audience honest. */
export function listAudienceGrants(
  params: { audience?: string; axis?: GrantAxis },
  opts?: { signal?: AbortSignal; onProgress?: (rows: number) => void }
): Promise<AudienceGrant[]>

/** `created` is false when the row already existed (server answered 200, not 201). */
export function createAudienceGrant(
  audience: string, axis: GrantAxis, selector: string
): Promise<{ grant: AudienceGrantResponse; created: boolean }>

/** Resolves on 204. Every key component goes through `encodeURIComponent`. */
export function deleteAudienceGrant(
  audience: string, axis: GrantAxis, selector: string
): Promise<void>
```

Details:

- `listAudienceGrants` builds its query with `URLSearchParams`, appending `audience`/`axis` only
  when non-empty (an empty `audience=` is an exact match against `""`, which matches nothing —
  not "no filter"), calls `fetchArrowTable`, and decodes with `decodeAudienceGrants(table)` using
  `timestampToDate` from `arrow-utils.ts`, exactly as `decodeQueryDenyRules` does.
- Create/delete stay plain JSON on `authenticatedFetch`, in `data-sources-api.ts`'s shape.
  `createAudienceGrant` reads `response.status === 201` **before** awaiting the body.
  `deleteAudienceGrant` must not call `.json()` on the `204` — a sibling `handleEmptyResponse`
  parses an error body on `!ok` and otherwise returns nothing.
- Client-side validation mirrors (**not** the authority — the server re-runs its own and its
  message wins on disagreement), used only to gate the submit button and show an inline hint:

```ts
export const AUDIENCE_PATTERN = /^[A-Za-z0-9_-]{1,255}$/   // mirrors is_valid_audience
export const MAX_SELECTOR_BYTES = 255                       // a BYTE bound — use TextEncoder
export function validateSelector(selector: string): string | null
```

### 5. Frontend: `src/routes/AudienceGrantsPage.tsx` (new)

One route file with a local `AddGrantDialog`, wrapped in `Suspense` on the default export the way
`QueryDenyListPage` is.

```
grants: AudienceGrant[]          // the complete (optionally server-filtered) set
isLoading, loadedRows, error     // loadedRows drives the progressive "N rows" counter
focusAudience: string | null     // server-side ?audience= exact filter
axisFilter: GrantAxis | null     // server-side ?axis=
findText: string                 // client-side substring, over the whole set
showAddDialog, addPrefill, addError, isAdding, alreadyExistedNote
deleteTarget, isDeleting, deleteError
```

`loadGrants` is a `useCallback` over `[focusAudience, axisFilter]`, invoked by the
load-on-dep-change effect (with the IIFE-inside-effect form the repo's
`react-hooks/set-state-in-effect` lint requires) and passed to `PageLayout`'s `onRefresh`. It
holds an `AbortController` in a ref so a filter change mid-stream cancels the in-flight request
rather than racing it.

**Grouping** (`useMemo` over `grants` + `findText`): bucket by `audience`, then by `axis`.
Audiences sorted by `localeCompare`; within an axis, `*` first, then selectors alphabetically —
`*` is the most consequential value on the page. `findText` matches case-insensitively against
the audience name **or** any selector, and an audience survives if either its name matches or at
least one of its selectors does (a name match keeps all of its selectors visible, so a card is
never a partial view of an audience it claims to show).

Layout (see the Option B mockup):

1. Breadcrumb `Admin / Audience Grants`; header with `<h1>` + subtitle *"Who can read from, and
   mint into, each audience."* and the primary **Add Grant** button.
2. Filter bar: **Find** (client-side substring across the whole set) and **Axis** (Both / read only
   / mint only → server-side `?axis=`). A summary line: *"N grants across M audiences."*
3. When `focusAudience` is set, a dismissible pill — *Showing only `team-alpha`* — above the cards.
   It is set by the per-card **Focus** button, and sends the exact-match `?audience=` param the
   API already offers (a cheap way to shrink the stream on a large store; the free-text Find box
   is the substring search operators actually reach for, which the API has no param for).
4. Two standing notes:
   - **Propagation**: read grants take effect within the grant-cache TTL
     (`MICROMEGAS_AUDIENCE_GRANT_CACHE_TTL_SECONDS`, default 60 s) because
     `DbAudienceGrantsSource` serves a whole-table snapshot; mint grants are a per-request point
     query and take effect immediately. Without this, an operator adds a read grant, reloads the
     user's dashboard, sees nothing, and concludes the page is broken.
   - **Scope**: `public` is always readable by every authenticated principal — it needs no grant
     row here, and adding one would change nothing.
5. `ErrorBanner` (`onRetry={loadGrants}`) for load and delete failures.
6. One card per audience: header = audience name (monospace) + grant count + **Focus**; then a
   `read` row and a `mint` row, each an axis badge plus selector chips and a *"+ Add read grant"* /
   *"+ Add mint grant"* button that opens the dialog pre-filled with that audience and axis.
   - Each chip is two lines: the selector in monospace, then `created_by · created_at` beneath —
     auditability on the face of the card rather than behind a tooltip.
   - A `*` selector gets a red-tinted border and the words *any authenticated principal*.
   - Delete is an `×` on the chip, `aria-label={`Delete ${axis} grant on ${audience} for
     ${selector}`}`.
   - An axis with no grants shows *"No mint grants — nobody can issue ingestion keys stamped with
     this audience."* This line is **only** rendered when no axis filter is active. Under
     `axis=read` the mint rows were never fetched, and claiming "no mint grants" would be a lie;
     the other axis row is hidden entirely instead.
   - React key is the natural key, `` `${audience} ${axis} ${selector}` `` — there is no
     surrogate id, and no component can contain a NUL.
7. Loading: a spinner with *"Streaming grants… N rows"* driven by `onProgress`. Cards are not
   rendered until the stream completes — a partially-arrived set would group into cards that
   under-count, which is precisely the failure this design exists to avoid.
8. Empty states: no grants at all (*"No audience grants yet. Every authenticated principal can
   already read `public`; add a grant to open up a named audience."* + the Add button); and
   *"No grants match this filter."* when `findText`/filters exclude everything.

### 6. Add Grant dialog

`ApiKeysAdminPage`'s modal chrome (fixed overlay, `max-w-md` panel, header/body/footer). Fields:

- **Audience** — text input, pre-filled from `addPrefill` when opened from a card. Hint:
  `[A-Za-z0-9_-]`, up to 255 characters.
- **Axis** — two-button segmented control (Read / Mint), pre-selected from `addPrefill`. Hint:
  *"Read: may query data stamped with this audience. Mint: may issue ingestion keys stamped with
  it. A read grant never confers mint."*
- **Selector** — three-way segmented control (Everyone / User / Group) plus an id input, composed
  into `*`, `user:<id>`, or `group:<id>`, with a monospace preview of the exact string that will
  be sent. "Everyone" disables the id input. Hint: *"Matched against the caller's OIDC `email` /
  `groups` claim. There is no user directory here — enter the claim value verbatim."*

The composed control is the one place the page improves on the CLI: `user:`/`group:` is a prefix
grammar the CLI makes you type, and a typo produces a silently non-matching grant rather than an
error.

Submit is disabled while `isAdding`, while the audience fails `AUDIENCE_PATTERN`, or while
`validateSelector` returns non-null. Server errors render inline at the top of the dialog body
(the `BAD_REQUEST` message verbatim), never as a page banner — the dialog stays open with the
input intact. On success the dialog closes and the list reloads; if the server answered `200`
(`created === false`), a neutral dismissible note reports *"That grant already existed (created
2026-08-14 by ops@example.com)."* rather than implying a write happened.

### 7. Delete flow

`ConfirmDialog` (`variant="danger"`, `error={deleteError}`, `isLoading={isDeleting}`), naming the
full triple:

> Delete the **read** grant on `team-alpha` for `group:eng`? Principals matching this selector
> lose access to that audience once the grant cache expires (up to 60 s).

A `404` (the CLI or another admin got there first) is reported in the dialog and the list reloads,
so the chip disappears instead of lingering.

### 8. Python client + CLI

`web_client.list_audience_grants` now decodes an Arrow stream instead of `resp.json()`. The
framing is the same JSON-framed protocol, so this needs a small reader:
`stream=True`, read a line, read `size` bytes, feed the concatenated IPC messages to
`pyarrow.ipc.open_stream`, and raise on an `error` frame or a stream with no `done` frame. Returns
the same list of dicts as before, so `grants.py`'s `cmd_list` (both `--format` branches) is
unchanged apart from `created_at` now arriving as a `datetime` rather than a string.

If a reusable framed-Arrow reader already exists on the Python side it should be used; otherwise
the new helper belongs next to `WebClient`, not inside `grants.py`, since the key-list routes are
plausible future converts.

### 9. Admin card + route wiring

- `AdminPage.tsx`: an eighth card, `/admin/audience-grants`, `Users` icon, `bg-blue-500/15
  text-blue-500` (green, accent-link, yellow, rust, orange, purple and red are taken). Copy:
  *"Grant users and groups read or mint access to an audience."*
- `router.tsx`: `const AudienceGrantsPage = lazy(() => import('@/routes/AudienceGrantsPage'))` and
  `<Route path="/admin/audience-grants" element={<AudienceGrantsPage />} />` after
  `query-deny-list`.

## Mockups

Self-contained HTML, opened directly in a browser, using the app's real dark-theme tokens from
`src/styles/globals.css` (`--app-bg #0a0a0f`, `--panel-bg #12121a`, `--border-color #2a2a3e`,
`--accent-link #1565c0`, `--accent-warning #ffb300`) and Inter.

- **`tasks/1510_audience_grants_admin_ui_mockups/option-b-grouped-by-audience.html` — chosen.**
  One card per audience, `read`/`mint` chip rows, two-line chips carrying `created_by · created_at`,
  per-axis "+ Add", per-card Focus, the two standing notes, the axis-filtered card (other axis
  hidden, not shown as empty), the streaming loading state, the empty state, and the Add Grant
  dialog in both its clean and server-error states.
- `tasks/1510_audience_grants_admin_ui_mockups/option-a-flat-table.html` — the alternative that was
  considered: a flat one-row-per-grant table with axis badges and an offset pager.

**Why B over A.** B answers the operator's actual question ("who can see `team-alpha`?" / "can
anyone mint here?") directly, and an empty mint axis becomes a visible statement rather than an
absence the flat table cannot express at all. A was the safer choice only because of the paginated
JSON list; converting the route to Arrow removes that constraint, which is what makes B correct
rather than merely nicer. A remains better for date-ordered auditing ("what changed this week"),
which is a plausible follow-up as a view toggle — not built here.

## Implementation Steps

**Phase 1 — server transport**

1. `rust/analytics-web-srv/src/arrow_stream.rs` (new) — `ARROW_STREAM_CONTENT_TYPE`, `json_line`,
   `DataHeader`/`DoneFrame`/`ErrorFrame`, `ArrowStreamEncoder`. Register in `lib.rs`.
2. `rust/analytics-web-srv/src/stream_query.rs` — delete the moved copies, use `arrow_stream`.
   Wire output must be byte-identical; existing tests must pass untouched.

**Phase 2 — server list route**

3. `rust/analytics-web-srv/src/audience_grants.rs` — `list_grants` returns
   `Body::from_stream` over `sqlx …fetch(&pool)`, batching at `GRANT_BATCH_ROWS = 4096`; drop
   `DEFAULT_LIMIT` so an absent `limit` means all rows; keep the `> 0` check and `MAX_LIMIT` clamp
   for an explicit `limit`; pre-stream failures keep their current status+JSON responses,
   mid-stream failures become a terminal `error` frame.
4. Rust tests: schema/column order, an empty result still emitting schema + `done`, batching across
   the 4096 boundary, `limit` absent → all rows, explicit `limit` still clamped, and a mid-stream
   error producing an `error` frame after a partial batch.

**Phase 3 — frontend transport + client**

5. `analytics-web-app/src/lib/arrow-stream.ts` — extract `readArrowStream`, add `fetchArrowTable`,
   widen `ErrorCode`; `streamQuery`/`fetchQueryIPC` unchanged in behavior.
6. `analytics-web-app/src/lib/audience-grants-api.ts` (new) — per Design §4.
7. `analytics-web-app/src/lib/__tests__/audience-grants-api.test.ts`.

**Phase 4 — page**

8. `analytics-web-app/src/routes/AudienceGrantsPage.tsx` (new) — per Design §5/§6/§7.
9. `analytics-web-app/src/router.tsx`, `analytics-web-app/src/routes/AdminPage.tsx`.
10. `analytics-web-app/src/routes/__tests__/AudienceGrantsPage.test.tsx`.

**Phase 5 — Python + docs**

11. `python/micromegas/micromegas/web_client.py` — framed-Arrow reader for
    `list_audience_grants`; `python/micromegas/tests/cli/test_grants.py` updated.
12. Docs and `CHANGELOG.md` per the Documentation section.

**Phase 6 — verification**

13. `cargo build && cargo test && cargo clippy` in `rust/`; `npm run lint`, typecheck and `npm test`
    in `analytics-web-app/`; `pytest` for the CLI tests.

## Files to Modify

Created:

- `rust/analytics-web-srv/src/arrow_stream.rs`
- `analytics-web-app/src/lib/audience-grants-api.ts`
- `analytics-web-app/src/lib/__tests__/audience-grants-api.test.ts`
- `analytics-web-app/src/routes/AudienceGrantsPage.tsx`
- `analytics-web-app/src/routes/__tests__/AudienceGrantsPage.test.tsx`

Modified:

- `rust/analytics-web-srv/src/audience_grants.rs`
- `rust/analytics-web-srv/src/stream_query.rs`
- `rust/analytics-web-srv/src/lib.rs`
- `analytics-web-app/src/lib/arrow-stream.ts`
- `analytics-web-app/src/router.tsx`
- `analytics-web-app/src/routes/AdminPage.tsx`
- `python/micromegas/micromegas/web_client.py`
- `python/micromegas/tests/cli/test_grants.py`
- `mkdocs/docs/admin/web-app.md`, `authentication.md`, `api-keys.md`
- `CHANGELOG.md`

## Trade-offs

- **Arrow for `GET`, JSON for `POST`/`DELETE`.** A mixed-representation resource is slightly odd,
  but Arrow buys nothing for a single row or an empty body, and converting them would break the
  Python client and any `curl` user for no gain.
- **Converting the existing route rather than adding `GET .../stream`.** A parallel Arrow route
  would avoid the break, at the cost of two list paths to keep in sync forever and a JSON path
  that keeps its silent-truncation default. One representation is the cleaner end state; the blast
  radius is small and fully enumerated (the Python client, its CLI, and its test — no documented
  `curl` example exists for this route).
- **`limit` absent now means "all rows".** A behavior change for `micromegas-grants list` with no
  `--limit`, which previously returned at most 100 rows. Strictly more correct — the old default
  truncated with no signal — but worth an explicit CHANGELOG line, since a script that relied on
  the implicit cap will now see everything.
- **Grouped cards over the flat table.** See Mockups. The cost is that date-ordered auditing gets
  harder; the underlying rows are still `created_at DESC` on the wire, so a future view toggle is
  cheap.
- **No pagination in the UI at all.** With the whole set streamed, a pathological store (millions
  of grants) would be slow to render — the client-side `Find` box and the server-side Focus/axis
  filters are the escape hatch, not a row cap. A row cap is what this design deliberately removed.
- **Cards render only after the stream completes.** Costs progressive paint on a large store; buys
  a grouping that is never transiently wrong. The row counter keeps the wait legible.
- **Composed selector control instead of a raw text field.** Diverges from the CLI's raw-string
  input; the preview line keeps the wire value visible, and the failure it prevents (a typo'd
  prefix that validates and then matches nobody) is exactly what this page exists to catch.
- **The DB store is treated as the whole picture.** The `MICROMEGAS_AUDIENCE_GRANTS` startup env
  map is on its way out, so the page deliberately says nothing about it rather than teaching an
  operator about a source that is being removed. The one rule the store doesn't contain is the
  built-in "`public` is always readable", which the page states directly.
- **No bulk create.** Provisioning many grants at once (one per user, one per audience) is a loop,
  and `micromegas-grants` already scripts a loop better than a browser form can. The page is for
  inspecting and fixing individual grants, which is the job that currently has no tool at all.

## Documentation

- `mkdocs/docs/admin/web-app.md` — replace the stale *"not yet a web UI page"* comment at lines
  56-57; add a `## Audience Grants` section next to `## Query Deny List` covering what the page
  fronts, the axis semantics, Focus vs. Find, and the propagation note (read = cache TTL, mint =
  immediate).
- `mkdocs/docs/admin/authentication.md` — in §*Audiences and Grants* and §*Self-service ingestion
  key mint*, note that each `micromegas-grants create` has an equivalent in **Admin → Audience
  Grants**.
- `mkdocs/docs/admin/api-keys.md` — at ≈247-254 and ≈405-410, add the UI as a third client of the
  grants API and document that `GET /api/audience-grants` responds with a streamed Arrow IPC
  result set (`application/x-micromegas-arrow-stream`), while `POST`/`DELETE` stay JSON.
- `CHANGELOG.md` under `## Unreleased` — a `**Web App:**` bullet for the page, and an entry for
  the list-route representation change carrying the **Minor breaking change** clause: response is
  now Arrow, and an absent `limit` returns all rows instead of 100. No SQL surface is touched.

## Testing Strategy

**Rust** (`rust/analytics-web-srv`) — per Implementation Step 4, plus a `stream_query.rs`
regression check that the refactor left its frames byte-identical.

**Frontend** (vitest + Testing Library), following `IngestionApiKeysPage.test.tsx`'s style: mock
`@/lib/auth` to an admin user, `@/lib/config` to a pinned `basePath`, `@/hooks/usePageTitle` and
`@/components/layout`; drive `global.fetch`. Arrow responses are built in-test with
`apache-arrow`'s `RecordBatchStreamWriter` and wrapped in the JSON framing, so the tests exercise
the real decoder rather than a stub.

`audience-grants-api.test.ts`:

- List URL omits `audience`/`axis` when unset and includes them when set; never sends
  `limit`/`offset`.
- A framed Arrow stream decodes to `AudienceGrant[]` with `createdAt` as a `Date`.
- A stream ending without a `done` frame rejects; an `error` frame rejects with its `code` and
  `message`.
- A pre-stream `503 NOT_CONFIGURED` JSON body becomes an `AudienceGrantError` carrying code and
  status.
- Delete URL percent-encodes every key component; a `group:` selector containing `/` and `?`
  round-trips. `204` resolves without a JSON parse.
- `201` → `created: true`; `200` → `created: false`. A non-JSON error body falls back to
  `UNKNOWN_ERROR` / `HTTP <status>`.
- `validateSelector`: accepts `*`, `user:a@b`, `group:x/y`; rejects `""`, `user:`, `group:`,
  `alice@example.com`; rejects a selector whose UTF-8 encoding exceeds 255 bytes while its
  `String.length` does not.

`AudienceGrantsPage.test.tsx`:

- Grants from one stream group into per-audience cards with the right counts, sorted by audience,
  `*` first within an axis.
- An audience with read grants but no mint grants shows the "No mint grants" line — and does
  **not** show it when an axis filter is active.
- Chips render selector, `created_by` and a formatted `created_at`.
- `Find` narrows across the whole set without issuing a fetch; a name match keeps all of that
  audience's selectors visible.
- Axis filter and Focus each re-fetch with the right query param; clearing Focus re-fetches.
- Add flow from a card's "+ Add mint grant" pre-fills audience and axis; picking Group and typing
  an id makes the preview read `group:<id>`; submit posts the right body and reloads.
- A `400 BAD_REQUEST` on create keeps the dialog open with the server message; a `200` shows the
  "already existed" note rather than a created message.
- Delete: the confirm names the triple; confirming issues the DELETE with the right query params
  and reloads; a `404` surfaces in the dialog.
- A `503` list response renders the server message in the error banner (this also covers the
  `--disable-auth` fixed 503).

**Python**: `test_grants.py` updated to serve a framed Arrow body and assert `cmd_list` renders
both formats.

**Manual**: run the monolith with `--disable-auth` to confirm the 503 banner; then against a real
OIDC config and a v7 telemetry DB, exercise create/list/delete end to end and cross-check with
`micromegas-grants list`. Seed a few thousand grants to confirm the stream, the row counter, and
the grouping hold up.

## Open Questions

1. **Does the Python side already have a framed-Arrow reader?** If not, Design §8 adds one next to
   `WebClient`. Worth confirming before writing a second implementation of the frame loop.
2. **Should the key-list routes (`/api/ingestion-api-keys`, `/api/analytics-api-keys`) move to
   Arrow too?** They have the same 500-row cap and the same silent-truncation default. Out of scope
   here, but `arrow_stream.rs` makes it cheap, and leaving them JSON keeps two conventions in one
   admin API.
