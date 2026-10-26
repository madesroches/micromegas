# Cancel Cell Execution Plan

**Issue**: [#745](https://github.com/madesroches/micromegas/issues/745)

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

Add `'cancelled'` to `CellStatus`. It is distinct from `idle` ("not yet executed") and `error`, so the cell can show "Cancelled" (the feedback requirement) and so cancel is not mistaken for a failure. Making it a new union member lets the compiler enumerate every status switch (`CellContainer`, `HgChildPane`, `notebook-cell-view`).

A cancelled cell keeps the data it showed while loading (the same `prevData` that the loading transition already preserves). Cells that had not started stay `idle`, matching their meaning today.

### Execution hook (`useCellExecution`)

1. **Run generation.** Add `runGenerationRef`. `executeFromCell` increments it on entry and captures the value; the loop breaks as soon as the captured value no longer matches. This also fixes the existing problem of a superseded loop continuing.
2. **`cancelExecution()`.** New function returned from the hook:
   - increments `runGenerationRef` (stops the loop),
   - aborts `abortControllerRef.current`,
   - sets every cell currently `loading` to `{ status: 'cancelled', data: prevData }`.
3. **Late results are discarded.** `executeCell` captures its own `controller` in a local and, after `await meta.execute(...)`, returns `false` without touching state if `controller.signal.aborted`. This also covers WASM-local queries that cannot be interrupted: they run to completion but their result is dropped. `onExecutionComplete` is skipped in this path.
4. **Abort is not a failure.** In `executeFromCell`, a `false` from `executeCell` only triggers the "mark downstream blocked" logic when the run generation is unchanged; otherwise the loop just breaks. The `AbortError` catch keeps returning `false` with no state change, since `cancelExecution` (or the superseding run) owns the state.
No new React state is needed: `isExecuting` is already derived from `cellStates`, so the tab busy indicator and refresh-interval pause clear on cancel.

### UI

- `CellContainer` gets `onCancel?: () => void`. While `status === 'loading'` and `onCancel` is set, the Run button slot renders a Stop button (`Square` icon, title "Cancel"), enabled, and the controls are forced visible (not hover-only) so a running cell always shows how to stop it. Otherwise behavior is unchanged.
- `statusLabel`/`statusColor` handle `cancelled` ("Cancelled", muted). The body treats it like `idle` with preserved data.
- `HgChildPane` gets the same Stop affordance and `cancelled` label, wired from `NotebookRenderer`.
- `NotebookRenderer` passes `onCancel={cancelExecution}` everywhere it passes `onRun`. Cancel is whole-run, not per-cell: the shared controller and sequential loop mean the only coherent semantics is "stop the current run", and stopping a middle cell of a run while continuing the rest has no defined meaning here.
- `notebook-cell-view.ts` maps `cancelled` to a non-error aggregate status.

### Server

No change planned. The abort already tears down the connection chain. If Manual Verification shows the query keeps running server-side, that becomes a separate follow-up in `stream_query.rs` / the FlightSQL service.

## Mockups

- `tasks/745_cancel_cell_execution_mockups/stop-button-and-cancelled-state.html`: self-contained page with an interactive demo (Stop during a run -> Cancelled -> Run again) plus static before/after states. Single direction, since the change is a button swap and a status label; no layout alternatives were worth comparing.

## Implementation Steps

1. `notebook-types.ts`: add `'cancelled'` to `CellStatus`; fix every compiler error that follows (`notebook-cell-view.ts`, `HgChildPane.tsx`, `CellContainer.tsx`).
2. `useCellExecution.ts`: add `runGenerationRef`, local controller capture + post-await abort check, loop break on generation change, and `cancelExecution`; return it from the hook.
3. `NotebookRenderer.tsx`: pass `cancelExecution` as `onCancel` to `CellContainer` and `HgChildPane` call sites.
4. `CellContainer.tsx` / `HgChildPane.tsx`: Stop button, always-visible while loading, `Cancelled` label.
5. Docs and `CHANGELOG.md` entry under Unreleased.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/notebook-types.ts`
- `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts`
- `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`
- `analytics-web-app/src/lib/screen-renderers/notebook-cell-view.ts`
- `analytics-web-app/src/lib/screen-renderers/cells/HgChildPane.tsx`
- `analytics-web-app/src/components/CellContainer.tsx`
- `mkdocs/docs/web-app/notebooks/execution.md`
- `CHANGELOG.md`
- Tests: `lib/screen-renderers/__tests__/useCellExecution.test.ts`, `components/__tests__/CellContainer.test.tsx`

## Trade-offs

- **New `cancelled` status vs. returning to `idle`.** `idle` would satisfy "ready state" but gives no feedback and is indistinguishable from never-run. A new status costs a few switch arms, all compiler-enumerated.
- **Whole-run cancel vs. per-cell cancel.** Per-cell would need per-cell controllers and a rule for what the loop does next; the shared controller already encodes "one run at a time".
- **Toast vs. in-cell label for feedback.** The in-cell label persists and sits where the user is looking; no new toast machinery.
- **Making WASM-local queries interruptible.** Out of scope; discarding the late result gives correct observable behavior.

## Decisions

(none yet)

## Documentation

- `mkdocs/docs/web-app/notebooks/execution.md`: add `cancelled` to the Cell States table and a short "Cancelling" section (Stop button; stops the whole run; unstarted cells stay idle; local-engine queries finish but their results are discarded).
- `CHANGELOG.md`: user-facing entry. Rust API unaffected.

## Testing Strategy

Automated, all no-DB, in `useCellExecution.test.ts` (using the existing cell-registry mock) and `CellContainer.test.tsx`:

- `cancelExecution` during a pending `execute` aborts the signal passed to `runQuery`, and the cell ends `cancelled` with its previous data preserved.
- Cancelling mid-`executeFromCell(0)`: later cells stay `idle` (not `blocked`) and are never executed; the loop stops.
- A cell whose `execute` resolves *after* cancel (simulating a WASM-local query) does not overwrite `cancelled`, and `onExecutionComplete` is not called.
- A genuine failure still marks downstream cells `blocked` (regression guard for the generation check).
- Starting a new `executeFromCell` while one is running stops the old loop.
- `CellContainer`: loading + `onCancel` renders an enabled Stop button that calls `onCancel`; not loading renders Run; `cancelled` shows the "Cancelled" label.

## Manual Verification

Server-side halting depends on tonic/hyper/DataFusion drop behavior that a frontend unit test cannot reach, and failure is silent (the UI looks cancelled either way).

1. Start services: `python3 local_test_env/ai_scripts/start_services.py --monolith`; open the web app at http://127.0.0.1:3000 and create a notebook with a table cell running a deliberately slow query over a wide time range.
2. Run it, click Stop. Expect: the cell shows "Cancelled" immediately, and the Run button is available again.
3. Inspect `/tmp/monolith.log` (`tail -f`) and confirm the cancelled query is logged as incomplete and CPU use of the monolith process returns to idle. If the query keeps running, file a follow-up for server-side cancellation.
4. In Firefox, repeat step 2 and confirm no unhandled DOMException appears in the console (see the note at `useCellExecution.ts:426`).

## Open Questions

- Should the notebook toolbar (via `ScreenPage`, which already receives `isExecuting`) also get a global Stop control, or is the per-cell Stop button sufficient for this issue? The plan assumes the latter.
