# Screen Folders Plan

Closes #1159.

## Overview

Let users organize notebooks/screens into a folder hierarchy and find them by name search. The central design constraint: **a screen's folder path is a property of the screen, not part of its identity.** Moving a screen between folders must be a plain property update — it must never require re-keying, re-linking, or re-validating uniqueness against the screen's name. This directly overrides the issue's own proposed migration note ("preserve uniqueness (path + name)"), which would make the folder path part of the key and reintroduce exactly the coupling we're trying to avoid.

Two interactive mockups exist at `tasks/1159_folders_mockups/` (`alt-a-sidebar-tree.html`, `alt-b-breadcrumb-explorer.html`, `alt-c-grouped-search-first.html`). They're useful for **visual/interaction guidance only** — the mockup's JS looks screens up by `name` as a stand-in identity (`SCREENS.find(x => x.name === name)`), which happens to match production's actual identity, but the mockups are throwaway prototypes with known bugs and should not be read as a data-model reference.

## Current State

- `screens` table: `name VARCHAR(255) PRIMARY KEY`, `screen_type`, `config JSONB`, audit columns, `managed_by`. No folder/parent field. (`rust/analytics-web-srv/src/app_db/schema.rs:16-34`)
- `Screen` model mirrors the table 1:1 (`rust/analytics-web-srv/src/app_db/models.rs:8-19`).
- `name` is already the sole identity today: it's the primary key, the path param for `GET/PUT/DELETE /screens/:name` (`rust/analytics-web-srv/src/screens.rs:133-263`), and the frontend route `/screen/:name` (`analytics-web-app/src/router.tsx:44`). There is no rename endpoint. Import conflict resolution in `analytics-web-app/src/lib/screens-api.ts` has three `createScreen` call sites: the non-conflict path (~249-253), the `overwrite` path which does delete+recreate (261-269), and the `rename` path which creates under a suffixed name via `generateUniqueName` (271-279) — none of these actually rename an existing screen in place.
- `GET /screens` returns the full flat list, consumed by a flat grid sorted by name (`analytics-web-app/src/routes/ScreensPage.tsx:148-192`).
- `SaveScreenDialog.tsx` only captures a name; no destination picker.
- `name` uniqueness and format are enforced by `normalize_name`/`validate_name` (`rust/analytics-web-srv/src/app_db/models.rs:116-224`), shared between backend validation and the frontend's `normalizeScreenName` (`analytics-web-app/src/components/SaveScreenDialog.tsx:23-30`, kept in sync by hand today).
- App-db migrations are sequential, version-gated, one function per step (`rust/analytics-web-srv/src/app_db/migration.rs`). Current version: 3.

## Design

### Identity vs. path

- **Identity stays `name`** (unchanged PK, unchanged route, unchanged validation). Nothing about the issue requires touching this, and `name` was never coupled to a folder before — the design goal is to *keep it that way* as folders are introduced, not to invent a new surrogate key. Introducing a surrogate `id` isn't needed to satisfy "path is not identity"; it's only needed if identity itself becomes unstable, which it doesn't here.
- **`folder_path` is a plain, non-unique, mutable column** on `screens` — a label describing where the screen currently lives, exactly like `screen_type` or `managed_by`. No composite uniqueness on `(folder_path, name)`. A screen named `foo` is still simply "the screen named foo," regardless of which folder it's filed under. Moving it is `UPDATE screens SET folder_path = $1 WHERE name = $2` — one property, no identity change, no re-validation of name uniqueness.
- Path format: `/`-delimited, no leading/trailing slash, `""` = root (matches the mockups' convention). The folder-path validator special-cases `""` first: an empty path is always valid root and is returned as-is without splitting on `/` or running per-segment validation (Rust's `"".split('/')` yields one empty-string segment, which would otherwise fail a min-length-1 check and reject the default root path on every ordinary screen create and on `POST/PUT /folders {path: ""}`). Only a non-empty path is split on `/` and each segment validated/normalized with a new folder-segment validator that shares `normalize_name`/`validate_name`'s character/hyphen rules but with a minimum length of 1 (not 3) and without the screen-specific `RESERVED_NAMES` check (`"new"` is reserved because of the `/screen/new` route, which doesn't apply to folders) — reusing `validate_name` verbatim would reject reasonable folder names like `qa`/`ui`/`ai` and forbid a folder literally named `new`. `validate_name`'s only caller today is `screens.rs`, so implement this as a parameterized core function (min length, whether to check `RESERVED_NAMES`) called by both `validate_name` (3, checked) and the new folder-segment validator (1, unchecked) — not a second copy-pasted implementation of the same character/hyphen logic.
- No hardcoded max folder depth — segments are just validated individually; nesting is unbounded like a filesystem path. However, the composed path is capped by the `VARCHAR(1024)` column (see Migration): the validator checks the total length of the joined path (after per-segment validation) and rejects with `400` if it exceeds 1024 characters, so an over-long `folder_path`/`new_path` is caught cleanly before it ever reaches Postgres instead of surfacing as a raw "value too long for type character varying(1024)" error.

### Folders need to exist independently of screens

The issue explicitly calls for *creating* folders (not just implying them from screen locations), and the mockups back this with a "New folder" action that works on an empty folder. If folders were purely derived from `DISTINCT folder_path` on `screens`, an empty folder would vanish the moment its last screen moved out — that's a real feature gap, not a hypothetical. So:

- New `folders` table: `path VARCHAR(1024) PRIMARY KEY, created_by VARCHAR(255), created_at TIMESTAMPTZ DEFAULT NOW()`.
- A folder "exists" if it has a row in `folders` **or** appears as a prefix of some `screens.folder_path` (covers folders that were never explicitly created but contain screens — e.g. from a future bulk import). `GET /folders` computes the union.
- Validate the request shape and reject rather than let the SQL corrupt data or surface a raw DB error:
  - Both `path` and `new_path` are validated/normalized with the folder-segment validator (same as `create_screen`/`update_screen` do for `folder_path`) before anything else runs — an unvalidated path would break the format invariant (no leading/trailing slash, valid segments) the prefix-rewrite SQL and the frontend both assume.
  - If `new_path == old_path` or `new_path` starts with `old_path || '/'` (renaming/moving a folder into its own subtree), reject with `400`/`409`. Otherwise statement 2 below (`WHERE path LIKE $old || '/%'`) re-matches the row statement 1 just rewrote, double-processing it and garbling the path — e.g. moving `team` to `team/archive/team` when `team/archive` is itself a child of `team`.
  - The existence/conflict/self-nesting checks above are cheap, stateless string checks on the request itself (no DB read), so there's no TOCTOU window to close for them. The two checks that *do* depend on current DB state — "does `old_path` exist" and "does `new_path` already exist" — are **not** done as a separate pre-check followed by a second, disconnected mutating step (that would leave a race between two concurrent renames/deletes of the same folder). Instead they're folded into the same transaction as the rewrite, with row-level locking closing the window, matching `update_data_source`'s (`data_sources.rs`) `SELECT ... FOR UPDATE` pattern: the transaction begins by running `SELECT path FROM folders WHERE path = $old FOR UPDATE` (plus the equivalent existence check against `screens.folder_path` prefixes) to lock/confirm `old_path` exists — 404 if not — and a second `SELECT` to confirm `new_path` doesn't already exist — 409 if it does — before issuing the 4 rewrite statements below in the same transaction. This mirrors `update_screen`/`delete_screen`'s convention of treating a real DB read/write as the authoritative signal, rather than trusting an earlier, disconnected pre-check.
- Renaming/moving a folder is a prefix rewrite in one transaction (materialized-path pattern, same idea the issue itself proposed for screens — just applied where it belongs, to folders), preceded by the locking existence/conflict checks above in the same transaction:
  1. `UPDATE folders SET path = $new WHERE path = $old`
  2. `UPDATE folders SET path = $new || substring(path from length($old)+2) WHERE path LIKE $old || '/%'`
  3. `UPDATE screens SET folder_path = $new WHERE folder_path = $old`
  4. `UPDATE screens SET folder_path = $new || substring(folder_path from length($old)+2) WHERE folder_path LIKE $old || '/%'`
- Deleting a folder requires it to be empty (no screens, no subfolders) — return `409 CONFLICT` otherwise. No recursive/cascading delete: the user must move or delete the contents first. Decided, not just a default — a folder delete should never be able to take screens down with it. Delete runs in one transaction: `SELECT path FROM folders WHERE path = $path FOR UPDATE` (plus the prefix existence check) locks/confirms `path` exists — `404 NOT FOUND` if not, the same missing-key convention applied to rename's `old_path` — then, still holding the lock, the empty-check (no subfolder/screen rows) runs and the actual `DELETE FROM folders WHERE path = $path` executes, so a concurrent create/move into this folder can't slip in between the check and the delete.

### API changes

`GET /screens` keeps returning the full flat list, unchanged — no `?folder=...` filtering added now. Don't design for a scale problem that doesn't exist yet; if per-org screen counts ever make the flat fetch a real bottleneck, that's a delivery optimization to make then (pagination, folder-scoped queries, etc.), not something to build speculatively today.

`rust/analytics-web-srv/src/screens.rs`:
- `Screen`/`CreateScreenRequest` gain `folder_path: String` (default `""` via `#[serde(default)]` on create).
- `UpdateScreenRequest.config` becomes `Option<serde_json::Value>` and gains `folder_path: Option<String>`, both applied with `COALESCE` like `managed_by` already is. This lets a drag-and-drop move send just `{"folder_path": "team/x"}` without re-sending the whole JSONB config — the current endpoint requires `config` unconditionally, which would make every move payload carry the full screen config for no reason.
- `create_screen`/`update_screen` validate `folder_path` segments the same way `name` is validated.
- The shared `SCREEN_COLUMNS` constant (used verbatim by `list_screens`, `get_screen`, and the `RETURNING` clauses of `create_screen`/`update_screen`, all via non-compile-time-checked `sqlx::query_as::<_, Screen>`) must add `folder_path`; `create_screen`'s INSERT column/value list and `update_screen`'s UPDATE column list/bindings must be extended to match, or every screens endpoint fails at runtime with a column mismatch.

New `folders.rs` module + routes (path passed as a query param on `DELETE`; JSON body on `POST`/`PUT`; `GET` takes no parameter and returns the full list — nested slashes in a URL path segment are exactly the kind of thing that breaks silently with naive routing, so the folder path never appears as an Axum path-extractor segment):
- `GET /folders` → `Vec<FolderInfo>` (`path`, screen count, subfolder count) — union of explicit `folders` rows and implicit prefixes from `screens.folder_path`. No recursive CTE or string-aggregation SQL: consistent with the crate's existing flat-query style (and the Trade-offs section's own low-scale justification), fetch `folders.path` (via a plain `SELECT path FROM folders`) and every `screens.folder_path` (via `SELECT folder_path FROM screens`, one row per screen — not `DISTINCT`, since each screen must contribute to every ancestor's recursive count) with two flat queries, then in Rust split each path on `/` to expand ancestor prefixes into a `HashMap<String, FolderInfo>`. `screen_count` is **recursive**, matching the cited mockup's `countScreens()` (a folder's own screens plus all descendants'): for each screen, increment `screen_count` on the folder's own path *and* every ancestor prefix (including root), not just the leaf bucket. `subfolder_count` stays direct (immediate children only), since the mockup's tree only ever expands one level of children at a time. `HashMap` iteration order is unspecified, so before returning, collect into a `Vec<FolderInfo>` and sort it (e.g. by `path`) — matching `list_screens`'s explicit `ORDER BY name` — so the response order is deterministic instead of visibly unstable in the sidebar tree.
- `POST /folders` `{path}` → create (idempotent — creating an already-existing path is a no-op, not an error, since two users concurrently opening "new folder" on the same path shouldn't be treated as a conflict). `path` is validated/normalized with the folder-segment validator before the insert.
- `PUT /folders` `{path, new_path}` → rename/move (the transaction above, including the `path`/`new_path` format validation described there), matching the crate-wide convention of `.put(...)` for updates (`update_screen`, `update_data_source` in `web_server.rs` — there is no `.patch(...)` anywhere in `build_protected_routes`).
- `DELETE /folders?path=...` → delete if empty, `409` if not, `404` if `path` doesn't exist (see above).

### Frontend changes

- `screens-api.ts`: add `folder_path` to `Screen`/`CreateScreenRequest`; add `folder_path` to `UpdateScreenRequest` and make `UpdateScreenRequest.config` optional (`config?: ScreenConfig`), mirroring the backend's `Option<serde_json::Value>` change so a folder-only move (`updateScreen(name, { folder_path })`) type-checks; add a small `folders-api.ts` (or extend this file) for the four folder endpoints.
- `ScreensPage.tsx`: replace the flat grid with a folder-aware view — sidebar tree + breadcrumb + grid of subfolders/screens for the current folder, plus the existing flat "all screens" view for search results. Visual layout follows `alt-a-sidebar-tree.html`'s structure (sidebar tree, breadcrumbs, drag-to-move onto a folder row/card, kebab-menu "Move to folder" modal) — but every operation is keyed by `name` exactly as it is today; a "move" is `updateScreen(name, { folder_path })`, never a lookup-by-path-then-rename. The current folder is reflected in and driven by a `?folder=<path>` URL query param via `useSearchParams`, following the same convention already used by `ScreenPage.tsx`, `PerformanceAnalysisPage.tsx`, `ProcessMetricsPage.tsx`, `ProcessLogPage.tsx`, and `NotebookRenderer.tsx` — so folder views are bookmarkable/shareable and browser back/forward traverse folder navigation.
- `SaveScreenDialog.tsx`: add a destination-folder field (defaults to the current screen's folder for "Save As", or root for new screens), matching the mockup's "Save Screen" modal (location chip + "Change" → folder picker). `createScreen` request includes `folder_path`. This needs a new `sourceFolderPath` prop, since the dialog itself has no way to know the current screen's folder — its sole call site, the "Save As Dialog" in `ScreenPage.tsx:481-488`, must be updated to pass `sourceFolderPath={screen?.folder_path}` (the `screen` variable is already in scope there, used one line below for `suggestedName`).
- New shared components: `FolderTree` (sidebar), `FolderBreadcrumb`, `FolderPickerModal` — the picker backs both the kebab "Move" action and the Save dialog's "Change" location button, so there's one implementation of "pick a destination folder." The "New folder" name input reuses a `normalizeScreenName`-style client-side preview ("Will be saved as: ...") for consistency with `SaveScreenDialog.tsx`'s existing live preview.
- Search: unchanged approach (client-side filter over the flat list from `GET /screens`), extended to match `folder_path` too, with matched folders auto-expanded in the tree — same idea as the mockup's `matchesQuery`/`computeMatchedFolders`, reimplemented against the real `Screen` type.

## Migration

App-db schema v3 → v4, following the existing pattern in `rust/analytics-web-srv/src/app_db/migration.rs`:
1. `CREATE TABLE folders (...)`.
2. `ALTER TABLE screens ADD COLUMN folder_path VARCHAR(1024) NOT NULL DEFAULT '';`
3. Bump `LATEST_APP_SCHEMA_VERSION` to 4.

No index on `screens.folder_path`: `GET /screens` already returns the full unfiltered list, and the only `LIKE`-based prefix scan (the rename transaction's descendant rewrite) runs at folder-rename time, not on a hot read path — at the expected per-org screen counts, a sequential scan there is fine. Same YAGNI call as not adding `?folder=...` filtering to `GET /screens` above.

No backfill logic needed beyond the column default — existing screens land in the root folder (`''`), matching the issue's migration note (the "root default" half of it, not the "(path+name) uniqueness" half).

## Implementation Steps

1. **Schema/migration**: `folders` table, `screens.folder_path` column, v3→v4 migration function. (`schema.rs`, `migration.rs`)
2. **Migration test harness**: build the DB-fixture/harness needed to pin a test database at schema v3 and invoke `execute_migration` directly, then assert v3→v4 is idempotent and existing screens default to root. This requires loosening visibility in `app_db/mod.rs` first — `schema::create_tables`/`create_data_sources_table`/`add_screens_managed_by` are only `pub(crate)` (via `pub(crate) mod schema;`) and `migration::update_schema_version` isn't re-exported, so none are reachable from `rust/analytics-web-srv/tests/*.rs` (compiled as a separate external crate) and `execute_migration` alone can't stop at v3. Change `pub(crate) mod schema;` to `pub`, or add explicit re-exports for the v1→v3 construction functions and `update_schema_version`, so the test can build v3 state and set the migration-table version directly. (new integration test file(s) under `rust/analytics-web-srv/tests/`, plus `app_db/mod.rs`)
3. **Screens API**: extend `Screen`/`CreateScreenRequest`/`UpdateScreenRequest` models and handlers for `folder_path`, reusing name-validation helpers for path segments. Includes adding `folder_path` to the `SCREEN_COLUMNS` constant and extending `create_screen`'s INSERT and `update_screen`'s UPDATE column lists/bindings to match. (`models.rs`, `screens.rs`)
4. **Folders API**: new `folders.rs` with list/create/rename/delete handlers + the prefix-rewrite transaction; wire routes in `web_server.rs`.
5. **Frontend types/API client**: `screens-api.ts` additions, new `folders-api.ts`.
6. **Folder UI components**: `FolderTree`, `FolderBreadcrumb`, `FolderPickerModal`.
7. **ScreensPage rewrite**: folder-aware browsing, drag-and-drop move, "New folder", search-with-matched-folders.
8. **SaveScreenDialog**: destination-folder field wired to the shared picker, plus a new `sourceFolderPath` prop; update its call site in `ScreenPage.tsx` to pass `sourceFolderPath={screen?.folder_path}` so "Save As" actually defaults to the current screen's folder.
9. **Export/Import**: `ExportedScreen` type gains an optional `folder_path?: string` (optional, not required — pre-existing export files lack the field, and `undefined` already flows through `createScreen` correctly since `JSON.stringify` drops it and the backend defaults via `#[serde(default)]`). `buildScreensExport` (`screens-api.ts:177-188`) must add `folder_path: s.folder_path` to its per-screen mapping so exports actually carry the field. On the import side, all three `createScreen` call sites in `screens-api.ts` need it threaded through: the non-conflict path (~249-253), the `overwrite` path (delete+recreate, 261-269), and the `rename` path (create-with-suffix via `generateUniqueName`, 271-279). All three stay keyed by `name`; no identity change.

## Files to Modify

- `rust/analytics-web-srv/src/app_db/schema.rs`
- `rust/analytics-web-srv/src/app_db/migration.rs`
- `rust/analytics-web-srv/src/app_db/mod.rs` (loosen `schema` module visibility / re-export `update_schema_version` for the migration test harness)
- `rust/analytics-web-srv/src/app_db/models.rs`
- `rust/analytics-web-srv/src/screens.rs`
- `rust/analytics-web-srv/src/folders.rs` (new)
- `rust/analytics-web-srv/src/lib.rs` (add `pub mod folders;`)
- `rust/analytics-web-srv/src/web_server.rs`
- `rust/analytics-web-srv/tests/migration_test.rs` (new — schema v3→v4 fixture/harness)
- `rust/analytics-web-srv/tests/folders_tests.rs` (new — folders.rs CRUD/rename/delete tests)
- `analytics-web-app/src/lib/screens-api.ts`
- `analytics-web-app/src/lib/folders-api.ts` (new)
- `analytics-web-app/src/routes/ScreensPage.tsx`
- `analytics-web-app/src/routes/ScreenPage.tsx` (pass `sourceFolderPath` to `SaveScreenDialog`'s "Save As Dialog" call site)
- `analytics-web-app/src/components/SaveScreenDialog.tsx`
- `analytics-web-app/src/components/FolderTree.tsx` (new)
- `analytics-web-app/src/components/FolderBreadcrumb.tsx` (new)
- `analytics-web-app/src/components/FolderPickerModal.tsx` (new)

## Trade-offs

- **No surrogate `id`.** The issue's flaw was proposing `(path, name)` as a composite key, not the choice of `name` as the key itself — `name` was already decoupled from folder location before this change. Adding a surrogate id would be solving a problem that doesn't exist here (YAGNI), at the cost of a breaking change to `GET/PUT/DELETE /screens/:name` and the `/screen/:name` route. If a future need arises to actually *rename* a screen without breaking bookmarks, that's a separate, well-scoped follow-up (e.g. a surrogate id plus redirect-on-rename), not something to design here.
- **Materialized path (string column) over closure table / adjacency list for folders.** Matches the issue's own suggestion ("path-style column is simplest") and avoids a second structure to keep in sync. The cost is `LIKE`-based prefix rewrites on rename, which is fine at the expected scale (per-org screen counts, not millions of rows).
- **Explicit `folders` table instead of purely-derived folders.** Costs one more table and one more migration step, but is required to support creating/keeping an empty folder — a feature the issue and mockups both call for.
- **`config` becomes optional on update.** Small API shape change to the existing `PUT /screens/:name`, justified by avoiding "resend the whole config to move a screen" payloads. Backward compatible — existing callers that always send `config` are unaffected.

## Testing Strategy

- Backend: unit/integration tests for `folders.rs` (create idempotency, rename cascades to descendant folders and screens, delete blocked on non-empty), and for the extended `update_screen` (partial update with only `folder_path`, only `config`, or both). Like every other DB-dependent test in this repo (`rust/analytics/tests/{histo_view_test,thread_spans_ordering_db_test,sql_view_test}.rs`, `rust/ingestion/tests/readiness.rs`, `rust/public/tests/{firehose_tests,pg_stats_test}.rs`), these are marked `#[ignore]` and run manually against `local_test_env`'s Postgres — `build/rust_ci.py` runs plain `cargo test` with no `--ignored`, so they are not part of default CI.
- Backend: no fixture currently exists in this crate for testing migrations directly — `execute_migration` is never invoked by any existing test in `rust/analytics-web-srv/tests/*.rs`, which are all pure unit/validation tests with no real Postgres connection or pinned-schema-version fixture. A new DB-fixture/harness must be built to pin a test database at schema v3, then invoke `execute_migration` directly and assert v3→v4 is idempotent and existing screens default to root. Building this fixture requires the `app_db/mod.rs` visibility change described in Implementation Step 2 (`schema`'s construction functions and `update_schema_version` are otherwise unreachable from the external test crate) — without it, fall back to duplicating the v1→v3 raw SQL directly in the test instead of calling the schema functions. Same `#[ignore]`-and-run-manually convention as above; not part of default CI.
- Frontend: extend `ScreensPage`/`SaveScreenDialog` tests for folder selection, move, and search-with-folder-match; a test asserting a folder move never sends a name-changing request.
