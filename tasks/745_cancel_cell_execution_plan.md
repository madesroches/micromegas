# Cancel Cell Execution Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/745

## Overview

Let a user abort a notebook run while it is in flight. A running cell's Run button becomes a Stop button; clicking it aborts the browser fetch, stops the sequential run loop, and leaves the cell in a new `cancelled` state that says so and can be re-run.

## Current State

All paths under `analytics-web-app/src/` unless noted.

- `lib/screen-renderers/useCellExecution.ts` owns execution.
  - One `abortControllerRef` is shared by the whole hook (line 114). `executeCell` aborts the previous controller and installs a new one on every call (229-230). A new run therefore already cancels the previous cell's fetch, but there is no way to cancel without starting something else.
  - The signal reaches the network through `runQuery`/`runQueryAs` -> `fetchQueryIPC` / `executeSql` -> `streamQuery` (`lib/arrow-stream.ts`), which passes it to `authenticatedFetch`. Every cell type queries through this context, so one abort covers all of them.
  - On `AbortError`, `executeCell` returns `false` and sets **no state** (357-358). The cell is left in `loading` forever unless a later run replaces it.
  - `executeFromCell` is a sequential `for` loop over `await executeCell(i)` (398-421). When `executeCell` returns `false` for a `canBlockDownstream` cell, it marks all later cells `blocked` and breaks. An abort would therefore be reported as "downstream blocked", which is wrong for a cancel. There is no run token, so an old loop also keeps going after a newer run supersedes it.
  - Queries against the local WASM engine (`notebook` data source) ignore the signal and cannot be interrupted. `executeCell` would still write their result into state after the user cancelled.
- `lib/screen-renderers/notebook-types.ts:106`: `CellStatus = 'idle' | 'loading' | 'success' | 'error' | 'blocked'`.
- `components/CellContainer.tsx`: the Run button (174-192) is `disabled` while `status === 'loading'` and is only visible on hover. `statusColor`/`statusLabel` (106-122) and the error/blocked body (~306) switch on status.
- `lib/screen-renderers/cells/HgChildPane.tsx` renders children of horizontal groups with their own status handling (100, 229).
- `lib/screen-renderers/notebook-cell-view.ts:124-135` maps cell states to an aggregate view status.
- `lib/screen-renderers/NotebookRenderer.tsx`: wires `useCellExecution` (356), derives `isExecuting` (369) which `ScreenPage.tsx` uses for the tab busy indicator and to pause the refresh interval, and passes `onRun`/`onRunFromHere` to `CellContainer` (666, 710-711, 857) and HG children (581-606, 824).
- Server: `rust/analytics-web-srv/src/stream_query.rs` has no explicit cancel code. A browser abort drops the response body, which drops the stream and the upstream FlightSQL response stream. The FlightSQL service already records dropped streams as incomplete (`CompletionTrackedStream` in `rust/public/src/servers/flight_sql_service_impl.rs`). Whether this actually halts DataFusion work promptly has **not** been verified (see Manual Verification).
- Docs: `mkdocs/docs/web-app/notebooks/execution.md` ("Cell States", "Run Cell vs. Run From Here").

## Design

### State

Add `'cancelled'` to `CellStatus`. It is distinct from `idle` ("not yet executed") and `error`, so the cell can show "Cancelled" (the feedback requirement) and so cancel is not mistaken for a failure. The compiler flags none of the status consumers: the `switch` in `notebook-cell-view.ts` (~117) has no default and is not exhaustive over a return type, so an unhandled `cancelled` silently falls through to `idle`. It, the ternary chains in `CellContainer` (106-122) and `HgChildPane` (89-100), and the direct `'idle'`/`'loading'` comparisons in `MarkdownCell.tsx:184`, `__test-utils__/cell-registry-mock.ts:42` and `hooks/useFadeOnIdle.ts:19-24`, must all be updated by hand.

A cancelled cell keeps the data it showed while loading (the loading transition already preserves previous data in `data`). Wherever `idle` with cached data is treated as showable, `cancelled` is treated the same. Cells that had not started stay `idle`, matching their meaning today.

### Execution hook (`useCellExecution`)

1. **Run generation.** Add `runGenerationRef`. `executeFromCell` increments it on entry and captures the value; the loop breaks when the captured value no longer matches, checked at the top of each iteration as well as after each `executeCell` returns (a cancel can land between cells, when no cell is loading and no abort is raised). This also fixes the existing problem of a superseded loop continuing.
2. **`cancelExecution()`.** New function returned from the hook:
   - increments `runGenerationRef` (stops the loop),
   - aborts `abortControllerRef.current`,
   - calls `markLoadingCancelled()`.

   `markLoadingCancelled(exceptName?)` is a helper that sets every cell currently `loading` (other than `exceptName`) to `{ ...prev[name], status: 'cancelled', fetchProgress: undefined }`, keeping the existing `data`.
3. **Late results are discarded.** `executeCell` captures its own `controller` in a local, builds the `runQuery`/`runQueryAs` signals from that local (not a lazy read of `abortControllerRef.current`, which would hand a superseded cell's later queries the new run's signal), and, after `await meta.execute(...)`, returns `false` without touching state if `controller.signal.aborted`. This also covers WASM-local queries that cannot be interrupted: they run to completion but their result is dropped. The engine side effects get the same guard inside `runQuery`/`runQueryAs`: after `fetchQueryIPC` resolves, skip `engine.register_table` when the local `controller.signal.aborted`; after `engine.execute_and_register` returns, if aborted, call `engine.deregister_table(name)` (already exposed by the WASM engine) so a superseded query cannot leave a stale table in the engine reset by `executeFromCell(0)`. `onExecutionComplete` is skipped in this path. The same guard applies to the other late writers: the `catch` returns `false` with no state change when the local `controller.signal.aborted` (so a late non-abort error cannot overwrite `cancelled`), and both `onProgress` callbacks return early when the local `controller.signal.aborted` (so a late progress update cannot undo the `fetchProgress` reset or create a partial entry).
4. **Abort is not a failure.** In `executeFromCell`, a `false` from `executeCell` only triggers the "mark downstream blocked" logic when the run generation is unchanged; otherwise the loop just breaks. The `AbortError` catch keeps returning `false` with no state change, since the state of an aborted cell is set by `markLoadingCancelled` (items 2 and 5), not by the aborted `executeCell`.
5. **Direct runs supersede a loop.** A single-cell run (`onRun` -> `executeCellByName`) or a sort-check run (`useCellSortCheck`) calls `executeCell`, which aborts the previous controller; without a bump the in-flight loop cell returns `false` with an unchanged generation and the loop marks downstream cells `blocked`. Split the body into an internal `runCell(index)` (no generation change) that the loop calls, and make the exported `executeCell` a thin wrapper that increments `runGenerationRef` and then calls `runCell`. The loop captures its generation on entry, so its own `runCell` calls never invalidate it, while any direct `executeCell` does. `executeFromCell` therefore calls `runCell`, not the exported wrapper (its `useCallback` dependency changes from `executeCell` to `runCell`), since the wrapper would bump the generation and cancel its own loop. A sort-check run now cancels a running loop and leaves later cells `idle` (previously the aborted cell stayed `loading` and blocked downstream). `getAvailableCellResults` (`NotebookRenderer.tsx` ~514) only includes `success` cells, so a cancelled cell's preserved data is excluded from cell results exactly as for `idle`.

   The aborted in-flight cell would otherwise stay `loading` forever (the abort path writes no state), keeping `isExecuting` true and its Run button disabled. So the wrapper calls `markLoadingCancelled(cellName)` after the generation bump and before `runCell`, marking any other `loading` cell `cancelled`; `executeFromCell` calls `markLoadingCancelled()` on entry for the same reason.

No new React state is needed: `isExecuting` is already derived from `cellStates`, so the tab busy indicator clears on cancel and the refresh-interval pause ends.

### UI

- `CellContainer` gets `onCancel?: () => void`. While `status === 'loading'` and `onCancel` is set, the Run button slot renders a Stop button (`Square` icon, title "Cancel"), enabled, and the controls are forced visible (not hover-only) so a running cell always shows how to stop it. Otherwise behavior is unchanged.
- `statusLabel`/`statusColor` handle `cancelled` ("Cancelled", muted). The body treats it like `idle` with preserved data.
- `HgChildPane` gets the same Stop affordance and `cancelled` label. It is rendered by `cells/HorizontalGroupCell.tsx` (~217-223, next to `onRun={() => onChildRun(child.name)}`), which takes a new `onCancel` prop passed through to `HgChildPane` and wired from `NotebookRenderer`.
- `NotebookRenderer` passes `onCancel={cancelExecution}` only to the cell containers (the HG `CellContainer` ~581 and the regular `CellContainer` ~710) and to `HorizontalGroupCell` (via the child-run path, ~606). It does not pass it to `HgEditorPanel` (~824) or the side editor panel (~857): their run buttons (`HorizontalGroupCell.tsx` ~396-401) ignore status and need no `onCancel`. `useCellSortCheck.ts` needs no code change. Cancel is whole-run, not per-cell: the shared controller and sequential loop mean the only coherent semantics is "stop the current run", and stopping a middle cell of a run while continuing the rest has no defined meaning here.
- `notebook-cell-view.ts` (`computeHgStatus`) tracks a `hasCancelled` flag and keeps the existing precedence error > loading > blocked, then returns `cancelled` if any child is `cancelled` (before the `success` check), else `success`, else `idle`. A cancelled+success mix therefore shows `cancelled`, and an all-cancelled group no longer falls through to `idle`.
- `useFadeOnIdle` treats `cancelled` as idle.

### Server

No change planned. The abort already tears down the connection chain. If Manual Verification shows the query keeps running server-side, that becomes a separate follow-up in `stream_query.rs` / the FlightSQL service.

## Mockups

- `tasks/745_cancel_cell_execution_mockups/stop-button-and-cancelled-state.html`: self-contained page with an interactive demo (Stop during a run -> Cancelled -> Run again) plus static before/after states. Single direction, since the change is a button swap and a status label; no layout alternatives were worth comparing.

## Implementation Steps

1. `notebook-types.ts`: add `'cancelled'` to `CellStatus`; update the `notebook-cell-view.ts` switch by hand (the compiler flags nothing there), and grep for `'idle'`/`'loading'` comparisons and status ternaries (`CellContainer.tsx`, `HgChildPane.tsx`, `MarkdownCell.tsx`, `cell-registry-mock.ts`, `useFadeOnIdle.ts`) that the compiler does not flag.
2. `useCellExecution.ts`: implement Design items 1-5: `runGenerationRef`, the `runCell`/`executeCell` wrapper split (`executeFromCell` calls `runCell` and depends on it instead of `executeCell`), local controller capture with the post-await, late-error and `onProgress` guards, the `markLoadingCancelled` helper, loop break on generation change with "mark downstream blocked" gated on an unchanged generation, and `cancelExecution`; return it from the hook.
3. `NotebookRenderer.tsx` / `HorizontalGroupCell.tsx`: pass `cancelExecution` as `onCancel` to `CellContainer`, and to `HorizontalGroupCell`, which forwards it to `HgChildPane`.
4. `CellContainer.tsx` / `HgChildPane.tsx`: Stop button, always-visible while loading, `Cancelled` label.
5. Docs and `CHANGELOG.md` entry under Unreleased.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/notebook-types.ts`
- `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts`
- `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`
- `analytics-web-app/src/lib/screen-renderers/notebook-cell-view.ts`
- `analytics-web-app/src/lib/screen-renderers/cells/HgChildPane.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/HorizontalGroupCell.tsx` (`onCancel` prop passed to `HgChildPane`)
- `analytics-web-app/src/components/CellContainer.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/MarkdownCell.tsx` (`showCached` treats `cancelled` like `idle`)
- `analytics-web-app/src/lib/screen-renderers/__test-utils__/cell-registry-mock.ts` (same check)
- `analytics-web-app/src/hooks/useFadeOnIdle.ts` (`cancelled` handled like `idle`)
- `mkdocs/docs/web-app/notebooks/execution.md`
- `CHANGELOG.md`
- Tests: `lib/screen-renderers/__tests__/useCellExecution.test.ts`, `components/__tests__/CellContainer.test.tsx`, `lib/screen-renderers/cells/__tests__/MarkdownCell.test.tsx` and `hooks/__tests__/useFadeOnIdle.test.ts`, `lib/screen-renderers/cells/__tests__/HorizontalGroupCell.test.tsx` and `lib/screen-renderers/__tests__/notebook-cell-view.test.ts` (add `cancelled` cases)

## Decisions

- Stop button appears on the running cell but cancels the whole run (shared controller); no toolbar Stop is added. A toolbar Stop is a possible follow-up (`isExecuting` flows ScreenPage -> Header -> RefreshIntervalPicker and would need extra prop plumbing).
- Feedback is an in-cell "Cancelled" label, not a toast.
- WASM-local queries are not made interruptible; their late results are discarded.
- AFK: Applied the reviewer's fix (abort check before `register_table`, `deregister_table` after a late `execute_and_register`) because it is local to `runQuery`/`runQueryAs` and uses an existing engine method, so a superseded query cannot leave a stale cell table in the reset engine.
- Auto-refresh will run the notebook again on its next tick after a cancel; pausing the timer is not part of this change.

## Documentation

- `mkdocs/docs/web-app/notebooks/execution.md`: add `blocked` and `cancelled` to the Cell States table (which currently omits `blocked`) and a short "Cancelling" section (Stop button; stops the whole run; unstarted cells stay idle; local-engine queries finish but their results are discarded).
- `CHANGELOG.md`: user-facing entry. Rust API unaffected.

## Testing Strategy

Automated, all no-DB, in `useCellExecution.test.ts` (using the existing cell-registry mock), `CellContainer.test.tsx`, `HorizontalGroupCell.test.tsx` and `notebook-cell-view.test.ts`:

- `cancelExecution` during a pending `execute` aborts the signal passed to `runQuery`, and the cell ends `cancelled` with its previous data preserved.
- Cancelling mid-`executeFromCell(0)`: later cells stay `idle` (not `blocked`) and are never executed; the loop stops.
- A superseded `runQuery`/`runQueryAs` whose `fetchQueryIPC` resolves after abort does not call `register_table`, and one whose `execute_and_register` resolves after abort triggers `deregister_table` for that name (mock engine).
- A cell whose `execute` resolves *after* cancel (simulating a WASM-local query) does not overwrite `cancelled`, and `onExecutionComplete` is not called.
- A genuine failure still marks downstream cells `blocked` (regression guard for the generation check).
- Starting a new `executeFromCell` while one is running stops the old loop.
- A late non-abort error (execute rejects after cancel) keeps the cell `cancelled`.
- A late progress update after cancel does not change a cancelled cell.
- A direct `executeCell` superseding a running `executeFromCell` loop stops the loop without marking downstream cells `blocked`.
- A direct `executeCell(A)` during a pending `B` leaves `B` `cancelled`.
- A macro-unresolved cell (returns `false`, generation unchanged) still blocks downstream cells.
- `CellContainer`: loading + `onCancel` renders an enabled Stop button that calls `onCancel`; not loading renders Run; `cancelled` shows the "Cancelled" label.
- `HorizontalGroupCell`: a loading child with `onCancel` renders a Stop button that calls it; a `cancelled` child shows the "Cancelled" label.
- `computeHgStatus`: a group with a cancelled child and no error/loading/blocked child aggregates to `cancelled` (including a cancelled+success mix); all-cancelled is not `idle`.

## Manual Verification

Server-side halting depends on tonic/hyper/DataFusion drop behavior that a frontend unit test cannot reach, and failure is silent (the UI looks cancelled either way).

1. Start services: `python3 local_test_env/ai_scripts/start_services.py --monolith`; open the web app at http://127.0.0.1:3000 and create a notebook with a table cell running a deliberately slow query over a wide time range.
2. Run it, click Stop. Expect: the cell shows "Cancelled" immediately, and the Run button is available again.
3. Inspect `/tmp/monolith.log` (`tail -f`) and confirm the cancelled query is logged as incomplete and CPU use of the monolith process returns to idle. If the query keeps running, file a follow-up for server-side cancellation.
4. In Firefox, repeat step 2 and confirm no unhandled DOMException appears in the console (see the note at `useCellExecution.ts:426-430`).
