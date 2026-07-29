# micromegas-screens: Round-trip folder_path Plan

**Issue**: https://github.com/madesroches/micromegas/issues/1362

## Overview

The server's screens API (`rust/analytics-web-srv/src/screens.rs`) fully supports a `folder_path` field, but the Python `micromegas-screens` CLI and its `WebClient` never read, write, or push it. As a result, any screen assigned to a folder on the server loses that assignment the moment it's imported/pulled into a local git-tracked file, and `plan`/`apply` show a spurious diff that would wipe the folder assignment on the server. Fix: thread `folder_path` through `web_client.py` and `cli/screens.py` end-to-end.

## Current State

- `rust/analytics-web-srv/src/app_db/models.rs:10-39`:
  - `Screen.folder_path: String` — always present in server responses (empty string = root).
  - `CreateScreenRequest.folder_path: String` with `#[serde(default)]` — optional in the request body, defaults to `""` (root) if omitted.
  - `UpdateScreenRequest.folder_path: Option<String>` — `None` means "don't change", `Some("")` means "move to root". Applied via `COALESCE($5, folder_path)` in `screens.rs:295`.
- `python/micromegas/micromegas/web_client.py`:
  - `create_screen(self, name, screen_type, config, managed_by=None)` (line 55) — no `folder_path` param, so new screens are always created at root.
  - `update_screen(self, name, config, managed_by=None)` (line 72) — no `folder_path` param, so folder can never be changed via the CLI.
- `python/micromegas/micromegas/cli/screens.py`:
  - `server_screen_to_file()` (line 94) — copies only `name`, `screen_type`, `config`, `managed_by`; drops `folder_path` from the server response.
  - `write_screen_file()` (line 56) — key allowlist is `("name", "screen_type", "config", "managed_by")`; `folder_path` would be dropped even if present in the dict.
  - `cmd_import` (line 219) calls `client.update_screen(name, screen["config"], managed_by=managed_by)` — no `folder_path`.
  - `cmd_apply` (lines 431-436, 446-450) calls `client.create_screen(...)` / `client.update_screen(...)` — neither passes `folder_path`.
  - `read_screen_file()` (line 46) required fields are `name`, `screen_type`, `config` — `folder_path` is optional in local files, which is correct (root screens shouldn't need to declare it).
- `python/micromegas/tests/test_screen_files.py` — covers `server_screen_to_file`, `write_screen_file`/`read_screen_file` round trip, and `compute_plan`; has no folder_path coverage today.

## Design

Treat `folder_path` as an optional field, consistent with how `managed_by` is already handled in the CLI:
- Server always returns `folder_path` (possibly `""`). The CLI only persists it locally when non-empty (root screens keep a clean file with no `folder_path` key, matching current behavior for screens without a folder).
- `web_client.create_screen`/`update_screen` gain a `folder_path=None` param. When `None`, it's omitted from the JSON payload (server defaults to root on create / leaves unchanged on update — matches Rust's `#[serde(default)]` / `Option<String>` + `COALESCE` semantics). When set (including `""`, to support moving a screen back to root), it's included.
- `cli/screens.py` passes `screen.get("folder_path")` through on create/update. `dict.get` returns `None` for local files that omit the key, which is exactly the "don't touch folder_path" signal `update_screen` needs, and `""` is fine for `create_screen` when a local file explicitly wants root.
- `compute_plan()` and `cmd_list`'s `screen_status()` must diff against `server_screen_to_file(server)`, not the raw server dict — the server always includes `"folder_path": ""` and `"managed_by": null` for root/unmanaged screens, while local files omit those keys entirely, so comparing raw dicts would flag every unmodified root screen as "modified" forever. This normalization must apply symmetrically: a local file may explicitly set `"folder_path": ""` (the documented way to move a screen to root), and `server_screen_to_file()` omits that same key for a root screen — so `local_data` needs the identical "drop falsy `folder_path`/`managed_by`" treatment before comparing, or an explicit `""` will never match the server's omitted key.

No changes needed to `read_screen_file()`'s required-field check — `folder_path` stays optional in the file format, same as `managed_by`.

## Implementation Steps

1. **`web_client.py`**
   - `create_screen`: add `folder_path=None` param; add `if folder_path is not None: payload["folder_path"] = folder_path`.
   - `update_screen`: add `folder_path=None` param; add the same conditional payload assignment.

2. **`cli/screens.py`**
   - `server_screen_to_file()`: add `if server_screen.get("folder_path"): result["folder_path"] = server_screen["folder_path"]` (falsy/empty stays omitted, matching the `managed_by` pattern immediately above it).
   - `write_screen_file()`: add `"folder_path"` to the key-order tuple, after `"config"` and before `"managed_by"` (matches `server_screen_to_file`'s construction order).
   - `cmd_import`: no change needed — this call's `client.update_screen(...)` at line 219 exists only to set `managed_by` during ownership transfer, so `folder_path` stays at its default `None` (leave unchanged); passing the server's just-fetched `folder_path` back would add a TOCTOU window with no benefit.
   - `cmd_apply` creates loop (~line 431): pass `folder_path=screen.get("folder_path")` to `client.create_screen(...)`.
   - `cmd_apply` updates loop (~line 446): pass `folder_path=screen.get("folder_path")` to `client.update_screen(...)`.
   - `compute_plan()` (~lines 296-306): compare and diff `local_data` against `server_screen_to_file(server)` instead of the raw `server` dict — use `server_screen_to_file(server)` directly (no extra `strip_volatile_keys(...)` wrap needed; it already excludes volatile keys) in both the `screens_equal(...)` check and the `updates` tuple. Also normalize `local_data` by calling `server_screen_to_file(local_data)` — the same function, called directly on the local dict with no new helper — since `read_screen_file()` guarantees `local_data` has `name`/`screen_type`/`config`, the only keys `server_screen_to_file()` requires. Use that normalized copy consistently in **both** the `screens_equal(...)` check **and** the `local_data` placed into the `updates` tuple (not the raw `local_data`) — so a local file with an explicit `"folder_path": ""`/`"managed_by": null` compares equal to the server's omitted key for a root/unmanaged screen, and doesn't reappear as spurious noise in `format_screen_diff`'s output when an unrelated field actually changed.
   - `cmd_list`'s `screen_status()` (~line 491): same normalization on both sides — compare `server_screen_to_file(local[name])` against `server_screen_to_file(server_by_name[name])` instead of the raw dicts, so an explicit `folder_path: ""`/`managed_by: null` in the local file doesn't cause a false "modified" status.
   - `cmd_pull`: normalize `existing = read_screen_file(local_path)` the same way, via `server_screen_to_file(existing)`, before comparing it to `new_content = server_screen_to_file(screen)` — no new helper, same function used on both sides — so an explicit `"folder_path": ""`/`"managed_by": null` in the local file matches the server's omitted key instead of being reported as "updated" and rewritten on every pull.

3. **Tests**
   - `python/micromegas/tests/test_web_client.py` (new file): stub the HTTP session (`unittest.mock`, matching the mocking pattern already used elsewhere in the test suite) and assert the JSON payload built by `create_screen`/`update_screen` includes/omits `"folder_path"` correctly when the argument is `None` (omitted), `""` (included, empty), and a real path (included) — this is the mechanism the whole `folder_path` round-trip depends on and isn't covered by the CLI-level tests below.
   - Extend `python/micromegas/tests/test_screen_files.py`:
   - `TestServerScreenToFile`: add a case asserting `folder_path` is copied when present and non-empty, and stays absent when the server returns `""` or omits it.
   - `TestWriteReadRoundTrip`: add a case with `folder_path` set, asserting it round-trips through `write_screen_file`/`read_screen_file`.
   - `TestComputePlan`: add a case where local and server differ only by `folder_path` to confirm it now surfaces as an `update` (proving the round-trip bug is fixed — today this case doesn't exist because the field is dropped before comparison). Also add a case for an *unmodified root screen* — local file with no `folder_path` key vs. server returning `folder_path: ""` (and `managed_by: null`) — asserting it lands in `unchanged`, to confirm the normalization fix in `compute_plan()` covers the common root-level case, not just non-root folders. Also add a case where the local file explicitly sets `"folder_path": ""` for a root screen (the Testing Strategy step 5 workflow) and the server also returns root (`folder_path: ""`) — asserting it lands in `unchanged`, confirming the local-side normalization treats an explicit empty string the same as an omitted key.

4. **Documentation** — `mkdocs/docs/web-app/notebooks/screens-as-code.md`, "File Format" section: add an optional `"folder_path"` key to the example JSON (or a note beside it), plus a bullet documenting the convention: key omitted = no folder / don't move; `""` = explicit root.

## Files to Modify

- `python/micromegas/micromegas/web_client.py`
- `python/micromegas/micromegas/cli/screens.py`
- `python/micromegas/tests/test_screen_files.py`
- `python/micromegas/tests/test_web_client.py` (new)
- `mkdocs/docs/web-app/notebooks/screens-as-code.md`

## Trade-offs

- **Omit vs. always-send `folder_path`**: chose to omit when `None`/absent (like `managed_by`) rather than always sending `""`, so that `update_screen` can distinguish "leave folder alone" from "move to root" — the server's `Option<String>` + `COALESCE` already requires this distinction, and collapsing it to always-send would make it impossible to update `config` without also implicitly resetting `folder_path` to root whenever a local file has no `folder_path` key.
- **Drop empty `folder_path` from local files**: keeps existing root-level screen files unchanged (no noisy diff for the common case), consistent with how `managed_by` is already omitted when falsy.

## Testing Strategy

- `poetry run pytest` in `python/micromegas/` (unit tests above cover the CLI logic without needing a live server).
- Manual/integration smoke test against a running `analytics-web-srv` (`local_test_env/ai_scripts/start_services.py`):
  1. Create a screen in a folder via the web app.
  2. `micromegas-screens import <name>` → inspect the JSON file for a `folder_path` key.
  3. `micromegas-screens plan` → expect no diff.
  4. Move the screen to a different folder locally (edit `folder_path` in the file) → `micromegas-screens apply` → confirm the server reflects the new folder.
  5. Remove the `folder_path` key locally (move back to root intent) — confirm behavior matches design (no-op on update since `None` means "don't change"; if root is desired, the local file should set `"folder_path": ""` explicitly). Document this in the CLI's future help text if it proves confusing.

## Open Questions

None. (Resolved: moving a screen back to root uses an explicit `"folder_path": ""` in the local file rather than treating absence as "move to root" — absence stays a no-op, matching the `managed_by` precedent. Explicit is preferred over implicit for this action.)
