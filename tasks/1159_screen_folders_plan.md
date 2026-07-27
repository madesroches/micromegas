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
- Path format: `/`-delimited, no leading/trailing slash, `""` = root (matches the mockups' convention). Each segment is validated/normalized with the *existing* `normalize_name`/`validate_name` rules (reused, not reinvented) so folder names follow the same character rules as screen names.
- No hardcoded max folder depth — segments are just validated individually; nesting is unbounded like a filesystem path.

### Folders need to exist independently of screens

The issue explicitly calls for *creating* folders (not just implying them from screen locations), and the mockups back this with a "New folder" action that works on an empty folder. If folders were purely derived from `DISTINCT folder_path` on `screens`, an empty folder would vanish the moment its last screen moved out — that's a real feature gap, not a hypothetical. So:

- New `folders` table: `path VARCHAR(1024) PRIMARY KEY, created_by VARCHAR(255), created_at TIMESTAMPTZ DEFAULT NOW()`.
- A folder "exists" if it has a row in `folders` **or** appears as a prefix of some `screens.folder_path` (covers folders that were never explicitly created but contain screens — e.g. from a future bulk import). `GET /folders` computes the union.
- Renaming/moving a folder is a prefix rewrite in one transaction (materialized-path pattern, same idea the issue itself proposed for screens — just applied where it belongs, to folders):
  1. `UPDATE folders SET path = $new WHERE path = $old`
  2. `UPDATE folders SET path = $new || substring(path from length($old)+2) WHERE path LIKE $old || '/%'`
  3. `UPDATE screens SET folder_path = $new WHERE folder_path = $old`
  4. `UPDATE screens SET folder_path = $new || substring(folder_path from length($old)+2) WHERE folder_path LIKE $old || '/%'`
- Deleting a folder requires it to be empty (no screens, no subfolders) — return `409 CONFLICT` otherwise. No recursive/cascading delete: the user must move or delete the contents first. Decided, not just a default — a folder delete should never be able to take screens down with it.

### API changes

`GET /screens` keeps returning the full flat list, unchanged — no `?folder=...` filtering added now. Don't design for a scale problem that doesn't exist yet; if per-org screen counts ever make the flat fetch a real bottleneck, that's a delivery optimization to make then (pagination, folder-scoped queries, etc.), not something to build speculatively today.

`rust/analytics-web-srv/src/screens.rs`:
- `Screen`/`CreateScreenRequest` gain `folder_path: String` (default `""` via `#[serde(default)]` on create).
- `UpdateScreenRequest.config` becomes `Option<serde_json::Value>` and gains `folder_path: Option<String>`, both applied with `COALESCE` like `managed_by` already is. This lets a drag-and-drop move send just `{"folder_path": "team/x"}` without re-sending the whole JSONB config — the current endpoint requires `config` unconditionally, which would make every move payload carry the full screen config for no reason.
- `create_screen`/`update_screen` validate `folder_path` segments the same way `name` is validated.

New `folders.rs` module + routes (path passed as a query param on `DELETE`; JSON body on `POST`/`PUT`; `GET` takes no parameter and returns the full list — nested slashes in a URL path segment are exactly the kind of thing that breaks silently with naive routing, so the folder path never appears as an Axum path-extractor segment):
- `GET /folders` → `Vec<FolderInfo>` (`path`, screen count, subfolder count) — union of explicit `folders` rows and implicit prefixes from `screens.folder_path`.
- `POST /folders` `{path}` → create (idempotent — creating an already-existing path is a no-op, not an error, since two users concurrently opening "new folder" on the same path shouldn't be treated as a conflict).
- `PUT /folders` `{path, new_path}` → rename/move (the transaction above), matching the crate-wide convention of `.put(...)` for updates (`update_screen`, `update_data_source` in `web_server.rs` — there is no `.patch(...)` anywhere in `build_protected_routes`).
- `DELETE /folders?path=...` → delete if empty, else `409`.

### Frontend changes

- `screens-api.ts`: add `folder_path` to `Screen`/`CreateScreenRequest`; add `folder_path` to `UpdateScreenRequest`; add a small `folders-api.ts` (or extend this file) for the four folder endpoints.
- `ScreensPage.tsx`: replace the flat grid with a folder-aware view — sidebar tree + breadcrumb + grid of subfolders/screens for the current folder, plus the existing flat "all screens" view for search results. Visual layout follows `alt-a-sidebar-tree.html`'s structure (sidebar tree, breadcrumbs, drag-to-move onto a folder row/card, kebab-menu "Move to folder" modal) — but every operation is keyed by `name` exactly as it is today; a "move" is `updateScreen(name, { folder_path })`, never a lookup-by-path-then-rename.
- `SaveScreenDialog.tsx`: add a destination-folder field (defaults to the current screen's folder for "Save As", or root for new screens), matching the mockup's "Save Screen" modal (location chip + "Change" → folder picker). `createScreen` request includes `folder_path`.
- New shared components: `FolderTree` (sidebar), `FolderBreadcrumb`, `FolderPickerModal` — the picker backs both the kebab "Move" action and the Save dialog's "Change" location button, so there's one implementation of "pick a destination folder."
- Search: unchanged approach (client-side filter over the flat list from `GET /screens`), extended to match `folder_path` too, with matched folders auto-expanded in the tree — same idea as the mockup's `matchesQuery`/`computeMatchedFolders`, reimplemented against the real `Screen` type.

## Migration

App-db schema v3 → v4, following the existing pattern in `rust/analytics-web-srv/src/app_db/migration.rs`:
1. `CREATE TABLE folders (...)`.
2. `ALTER TABLE screens ADD COLUMN folder_path VARCHAR(1024) NOT NULL DEFAULT '';`
3. `CREATE INDEX screens_folder_path ON screens(folder_path varchar_pattern_ops);` (a plain B-tree index isn't usable for `LIKE 'x%'` prefix scans under a non-C locale, and `local_test_env/db/Dockerfile` is a bare `FROM postgres:16.1` with no locale override — `varchar_pattern_ops` makes the prefix scan usable regardless of locale).
4. Bump `LATEST_APP_SCHEMA_VERSION` to 4.

No backfill logic needed beyond the column default — existing screens land in the root folder (`''`), matching the issue's migration note (the "root default" half of it, not the "(path+name) uniqueness" half).

## Implementation Steps

1. **Schema/migration**: `folders` table, `screens.folder_path` column + index, v3→v4 migration function. (`schema.rs`, `migration.rs`)
2. **Screens API**: extend `Screen`/`CreateScreenRequest`/`UpdateScreenRequest` models and handlers for `folder_path`, reusing name-validation helpers for path segments. (`models.rs`, `screens.rs`)
3. **Folders API**: new `folders.rs` with list/create/rename/delete handlers + the prefix-rewrite transaction; wire routes in `web_server.rs`.
4. **Frontend types/API client**: `screens-api.ts` additions, new `folders-api.ts`.
5. **Folder UI components**: `FolderTree`, `FolderBreadcrumb`, `FolderPickerModal`.
6. **ScreensPage rewrite**: folder-aware browsing, drag-and-drop move, "New folder", search-with-matched-folders.
7. **SaveScreenDialog**: destination-folder field wired to the shared picker.
8. **Export/Import**: `ExportedScreen` type gains `folder_path`, and all three `createScreen` call sites in `screens-api.ts` need it threaded through: the non-conflict path (~249-253), the `overwrite` path (delete+recreate, 261-269), and the `rename` path (create-with-suffix via `generateUniqueName`, 271-279). All three stay keyed by `name`; no identity change.

## Files to Modify

- `rust/analytics-web-srv/src/app_db/schema.rs`
- `rust/analytics-web-srv/src/app_db/migration.rs`
- `rust/analytics-web-srv/src/app_db/models.rs`
- `rust/analytics-web-srv/src/screens.rs`
- `rust/analytics-web-srv/src/folders.rs` (new)
- `rust/analytics-web-srv/src/web_server.rs`
- `analytics-web-app/src/lib/screens-api.ts`
- `analytics-web-app/src/lib/folders-api.ts` (new)
- `analytics-web-app/src/routes/ScreensPage.tsx`
- `analytics-web-app/src/components/SaveScreenDialog.tsx`
- `analytics-web-app/src/components/FolderTree.tsx` (new)
- `analytics-web-app/src/components/FolderBreadcrumb.tsx` (new)
- `analytics-web-app/src/components/FolderPickerModal.tsx` (new)
- `analytics-web-app/src/routes/ExportScreensPage.tsx`, `ImportScreensPage.tsx`

## Trade-offs

- **No surrogate `id`.** The issue's flaw was proposing `(path, name)` as a composite key, not the choice of `name` as the key itself — `name` was already decoupled from folder location before this change. Adding a surrogate id would be solving a problem that doesn't exist here (YAGNI), at the cost of a breaking change to `GET/PUT/DELETE /screens/:name` and the `/screen/:name` route. If a future need arises to actually *rename* a screen without breaking bookmarks, that's a separate, well-scoped follow-up (see Open Questions).
- **Materialized path (string column) over closure table / adjacency list for folders.** Matches the issue's own suggestion ("path-style column is simplest") and avoids a second structure to keep in sync. The cost is `LIKE`-based prefix rewrites on rename, which is fine at the expected scale (per-org screen counts, not millions of rows).
- **Explicit `folders` table instead of purely-derived folders.** Costs one more table and one more migration step, but is required to support creating/keeping an empty folder — a feature the issue and mockups both call for.
- **`config` becomes optional on update.** Small API shape change to the existing `PUT /screens/:name`, justified by avoiding "resend the whole config to move a screen" payloads. Backward compatible — existing callers that always send `config` are unaffected.

## Open Questions

- **Renaming a screen without breaking bookmarks.** Out of scope for this change — `name` remains the identity and this plan doesn't touch it. If a future need arises to let users rename a screen in place (rather than delete+recreate) without invalidating existing `/screen/:name` links, that's a separate, well-scoped follow-up (e.g. a surrogate id plus redirect-on-rename), not something to design here.

## Testing Strategy

- Backend: unit/integration tests for `folders.rs` (create idempotency, rename cascades to descendant folders and screens, delete blocked on non-empty), and for the extended `update_screen` (partial update with only `folder_path`, only `config`, or both).
- Backend: no fixture currently exists in this crate for testing migrations directly — `execute_migration` is never invoked by any existing test in `rust/analytics-web-srv/tests/*.rs`, which are all pure unit/validation tests with no real Postgres connection or pinned-schema-version fixture. A new DB-fixture/harness must be built to pin a test database at schema v3, then invoke `execute_migration` directly and assert v3→v4 is idempotent and existing screens default to root.
- Frontend: extend `ScreensPage`/`SaveScreenDialog` tests for folder selection, move, and search-with-folder-match; a test asserting a folder move never sends a name-changing request.
