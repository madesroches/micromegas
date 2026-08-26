# Markdown Cell Run Control Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1522

## Overview

Markdown cells have no run/play control and no way to move out of an
unrendered (`idle`/`blocked`) status except through a full sequential
notebook execution pass — in practice, a full page reload on notebooks
whose upstream query cells are slow. This plan adds a lightweight, single-cell
"Run" control to markdown cells (mirroring Jupyter's `Shift+Enter`) that
re-renders the cell in place, cheaply and synchronously, against whatever
upstream results already exist — without re-running any query cell.

Related issue: https://github.com/madesroches/micromegas/issues/1522

## Current State

Markdown cells live at `analytics-web-app/src/lib/screen-renderers/cells/MarkdownCell.tsx`.

**Rendering is already reactive to content edits — but gated on `status`.**
`MarkdownCell`'s memo (`MarkdownCell.tsx:20-23`) recomputes on every `content`
change:

```tsx
const { text: markdownContent, warnings } = useMemo(() => {
  if (status !== 'success' || !content) return { text: '', warnings: [] as string[] }
  return evaluateTemplate(content, { variables, timeRange, cellResults, cellSelections })
}, [status, content, variables, timeRange, cellResults, cellSelections])
```

Editing goes through `MarkdownCellEditor`'s `SyntaxEditor` (`MarkdownCell.tsx:53-59`)
straight to `useCellManager.updateCell` (`useCellManager.ts:162-237`) on every
keystroke — no debounce, no confirm/save step. `updateCell` calls
`onConfigChange`, which re-renders `NotebookRenderer` with the new `content`
in `cells`, which flows live into `MarkdownCell`'s `content` prop via
`buildCellRendererProps` → `markdownMetadata.getRendererProps`
(`MarkdownCell.tsx:97-99`, just `content: (config as MarkdownCellConfig).content`).

So **when `status === 'success'`, editing already re-renders live today.**
The bug is that `status` never becomes `'success'` except via the sequential
execution pipeline, and markdown has no way to trigger that pipeline for
itself alone:

- `canRun = canRunProp ?? !!meta.execute` (`CellContainer.tsx:97`), `canRun = !!meta.execute`
  (`CellEditor.tsx:95`), `canRun = !!meta.execute` (`HgChildPane.tsx:84`), and the inline
  `!!meta.execute` check gating the Run button in `ChildEditorView`
  (`HorizontalGroupCell.tsx:400`, used by `HgEditorPanel` when a hg child is open in the
  per-child editor panel) each hide the Run button, "Run from here", and "Auto-run from
  here" whenever a cell type has no `execute` method. `markdownMetadata` deliberately has
  none (`MarkdownCell.tsx:95`, `cell-registry.ts:150-158`) — four independent call sites
  currently duplicate the same `!!meta.execute` fallback with no override hook for
  `CellEditor`/`HgChildPane`/`ChildEditorView`.
- `status` is set to `'success'` for markdown via a single unconditional
  shortcut in `useCellExecution.executeCell` (`useCellExecution.ts:134-138`):
  ```ts
  if (!meta.execute) {
    completeCellExecution(cell.name, { status: 'success', data: [] })
    return true
  }
  ```
  This shortcut runs **before** the function gathers upstream
  variables/results/selections — so calling `executeCell` for a markdown cell
  directly is synchronous, free, and does not wait on or re-run anything
  above it. But it's only reachable via `executeCell`/`executeFromCell`,
  which for markdown are only invoked by: initial mount
  (`useCellExecution.ts:399-405`), `refreshTrigger` change (`:408-414`),
  time-range change (`:417-426`), or upstream selection changes
  (`updateCellSelection`, `:429-447`) — never by editing content.
- A **newly added or duplicated** markdown cell has no `cellStates` entry and
  defaults to `{ status: 'idle', data: [] }` (`NotebookRenderer.tsx:639`).
  With no Run control, the only way to make it render at all is one of the
  triggers above — in practice, reload.
- If an earlier cell resets execution (`executeFromCell(startIndex)` — e.g.
  "Run from here" on a query cell above, or a refresh/time-range change),
  every downstream cell including markdown is reset to `idle`
  (`useCellExecution.ts:356-365`) and only flips back to `success` once the
  sequential loop's `await executeCell(i)` reaches it — i.e. after every
  slower query cell in between has finished. This is the "notebook that
  takes more than a minute to re-run" pain the issue describes.

This gating was introduced deliberately in #1023 / `markdown_cell_defer_render_plan.md`
so macros wouldn't flash unresolved values before their upstream cells had
run once. That intent is preserved by this plan — nothing changes about how
markdown behaves *before* its first run; this plan only gives the user an
explicit, cheap way to re-request the cell's turn on demand instead of
waiting for/reloading into the sequential pass.

`evaluateTemplate` (`template-evaluator.ts:247`) is fully synchronous and
pure — no async plumbing is needed for the re-render itself.

## Design

### `CellTypeMetadata.canRun` — decouple "can run" from "has an execute method"

Add an optional field to `CellTypeMetadata` (`cell-registry.ts:111-176`):

```ts
/**
 * Whether this cell type shows a Run control, independent of whether it has
 * an `execute` method. Defaults to `!!execute`. Markdown sets this to `true`
 * so users get an explicit "re-render" control even though execution is a
 * free, local, synchronous no-op (see `execute`'s absence above).
 */
readonly canRun?: boolean
```

Set `canRun: true` in `markdownMetadata` (`MarkdownCell.tsx:78-100`).

Replace the four duplicated `!!meta.execute` fallbacks with `meta.canRun ?? !!meta.execute`:

- `CellContainer.tsx:97` — but `CellContainer` doesn't have `meta` in scope for callers that pass `canRunProp` directly; it already computes `meta = getCellTypeMetadata(type)` at line 96, so this is a one-line change: `const canRun = canRunProp ?? meta.canRun ?? !!meta.execute`.
- `CellEditor.tsx:95` — `const canRun = meta.canRun ?? !!meta.execute`.
- `HgChildPane.tsx:84` — `const canRun = meta.canRun ?? !!meta.execute` (covers markdown cells nested inside horizontal groups, in the compact hg child pane).
- `HorizontalGroupCell.tsx:400` — change `{onRun && !!meta.execute && (...)}` to
  `{onRun && (meta.canRun ?? !!meta.execute) && (...)}` in `ChildEditorView` (covers a
  markdown cell nested inside a horizontal group when opened for editing in the per-child
  editor panel — `meta` is already in scope there via `getCellTypeMetadata(child.type)`).

This is the single source of truth the issue's own "Notes" section points at
("`canRun` could be decoupled from `!!meta.execute`") — no other cell type's
behavior changes, since every other type already has `execute` and keeps the
same effective `canRun`.

### Suppress "Run from here" / "Auto-run from here" for markdown

`CellContainer`'s hover Play button, the dropdown's "Run from here", and
"Auto-run from here" are all gated by the *same* `canRun` boolean today
(`CellContainer.tsx:174,222,231`). Turning `canRun` on for markdown would
also surface "Run from here" and "Auto-run from here", which is undesirable
here:

- "Run from here" on a markdown cell would re-execute every cell **below**
  it — real cost, and not what a markdown "run" should mean.
  "Run" existing single-cell control) is the one the issue asks for.
- "Auto-run from here" would be actively harmful: `useCellManager.updateCell`
  (`:225-234`) schedules a debounced re-run of downstream cells on *any*
  execution-relevant config change, and `content` is not in the
  `nonExecKeys` exclusion set. If a markdown cell had `autoRunFromHere`
  enabled, every keystroke while editing would schedule a re-run of every
  cell below it.

Rather than adding `content` to `nonExecKeys` (which would also be correct
but leaves the "Run from here" menu item showing with no real use for
markdown), suppress both affordances at the only two call sites that build
markdown's `CellContainer` props (`NotebookRenderer.tsx:707-712`, the hg
group header at `:577-578` doesn't apply — markdown isn't a group). Pass
`onRunFromHere`/`onToggleAutoRunFromHere`/`autoRunFromHere` as `undefined`
when `cell.type === 'markdown'`:

```tsx
onRunFromHere={cell.type === 'markdown' ? undefined : () => executeFromCellByName(cell.name)}
onToggleAutoRunFromHere={cell.type === 'markdown' ? undefined : () => updateCell(index, { autoRunFromHere: !cell.autoRunFromHere })}
autoRunFromHere={cell.type === 'markdown' ? undefined : cell.autoRunFromHere}
```

`CellEditor`'s footer only ever renders a single "Run" button (no
"from here" variant there), so no equivalent change is needed there.
`HgChildPane` and `ChildEditorView` likewise only render a single Run
button per child — no change needed beyond the `canRun` fallback above.

### Wiring `onRun`

No new wiring is needed: `onRun={() => executeCellByName(cell.name)}` is
already passed unconditionally to `CellContainer` for every cell
(`NotebookRenderer.tsx:707`), to the renderer props (`:663`), to the editor
panel footer (`:851`), and to `HgChildPane`. It was simply unreachable
because `canRun` hid it. Once `canRun` is `true` for markdown, clicking the
existing Play button calls `executeCellByName('cellname')` →
`executeCell(idx)`, which hits the `!meta.execute` shortcut and sets
`{ status: 'success', data: [] }` synchronously — no query re-execution,
no dependency on upstream cells having already run.

### New/duplicated markdown cells

`handleAddCell`/`handleInsertCell`/`handleDuplicateCell`
(`useCellManager.ts:65-160`) leave the new cell's `cellStates` entry absent,
so it starts `idle` and stays blank until Run is clicked or the notebook
next executes. With the Run control now visible, this is a single click
rather than a reload — acceptable, and consistent with how every other cell
type behaves when newly added (a query cell doesn't auto-run either). No
special-casing needed; see Open Questions for the alternative (eager
auto-success on creation) considered and deferred.

## Implementation Steps

1. `analytics-web-app/src/lib/screen-renderers/cell-registry.ts`
   - Add `canRun?: boolean` to `CellTypeMetadata`.
2. `analytics-web-app/src/lib/screen-renderers/cells/MarkdownCell.tsx`
   - Add `canRun: true` to `markdownMetadata`.
3. `analytics-web-app/src/components/CellContainer.tsx`
   - Change `canRun = canRunProp ?? !!meta.execute` to
     `canRun = canRunProp ?? meta.canRun ?? !!meta.execute`.
4. `analytics-web-app/src/components/CellEditor.tsx`
   - Change `canRun = !!meta.execute` to `canRun = meta.canRun ?? !!meta.execute`.
5. `analytics-web-app/src/lib/screen-renderers/cells/HgChildPane.tsx`
   - Change `canRun = !!meta.execute` to `canRun = meta.canRun ?? !!meta.execute`.
6. `analytics-web-app/src/lib/screen-renderers/cells/HorizontalGroupCell.tsx`
   - In `ChildEditorView` (~line 400), change `{onRun && !!meta.execute && (...)}` to
     `{onRun && (meta.canRun ?? !!meta.execute) && (...)}`.
7. `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`
   - In `renderCell`, make `onRunFromHere`, `onToggleAutoRunFromHere`, and
     `autoRunFromHere` passed to `CellContainer` (around lines 707-712)
     `undefined` for `cell.type === 'markdown'`.
8. `analytics-web-app/src/lib/screen-renderers/cells/__tests__/MarkdownCell.test.tsx`
   - Add a test asserting `markdownMetadata.canRun === true`.
9. `analytics-web-app/src/components/__tests__/CellContainer.test.tsx`
   - Add a case: a cell type with `execute` absent but `canRun: true`
     (or a mock) shows the Run button; confirm "Run from here"/"Auto-run"
     visibility is driven by the presence of the corresponding callbacks,
     not by `canRun` alone.
10. `analytics-web-app/src/lib/screen-renderers/cells/__tests__/HorizontalGroupCell.test.tsx`
    - Add a case: a markdown child selected in the per-child editor panel
      (`ChildEditorView`) shows the Run button when `onRun` is provided —
      covers the same `canRun` fallback as `HgChildPane` but for the
      full-panel editor, which the existing suite doesn't exercise.
11. `analytics-web-app/src/lib/screen-renderers/__tests__/NotebookRenderer.test.tsx`
    - Update/replace the existing `'should not show run button for markdown
      cells'` test (line 506) — markdown now *does* show a Run button, but
      not "Run from here" or "Auto-run from here".
    - Add a test: with a markdown cell already at `status: 'success'`, edit
      its content directly (no Run click) and assert the rendered output
      updates (covers the already-working live-reactivity path, which had no
      regression test before).
    - Add a test: a markdown cell at `status: 'idle'` (e.g. freshly added)
      renders blank, clicking Run flips it to rendered content, and no
      upstream query cell's `executeCell` is invoked as a result (assert via
      a spy/mock data source that no additional query fires).
12. Run `yarn lint`, `yarn type-check`, and `yarn test` in `analytics-web-app/`.
13. Manual check with `yarn dev` (or the monolith): open a notebook with a
    slow upstream query cell and a markdown cell below it referencing
    `$variable`/`$cell.col`. Edit the markdown content after the initial
    load — confirm it updates live. Then trigger "Run from here" on the
    upstream query cell, and while it's still loading, click the markdown
    cell's own Run button — confirm it renders immediately against the
    still-in-flight/previous upstream results rather than waiting.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/cell-registry.ts`
- `analytics-web-app/src/lib/screen-renderers/cells/MarkdownCell.tsx`
- `analytics-web-app/src/components/CellContainer.tsx`
- `analytics-web-app/src/components/CellEditor.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/HgChildPane.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/HorizontalGroupCell.tsx`
- `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/MarkdownCell.test.tsx`
- `analytics-web-app/src/components/__tests__/CellContainer.test.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/HorizontalGroupCell.test.tsx`
- `analytics-web-app/src/lib/screen-renderers/__tests__/NotebookRenderer.test.tsx`
- `mkdocs/docs/web-app/notebooks/cell-types.md`

## Trade-offs

- **`CellTypeMetadata.canRun` flag vs. per-call-site prop overrides.** The
  codebase already has a `canRunProp` override mechanism on `CellContainer`,
  but `CellEditor` and `HgChildPane` don't, and threading an override through
  every call site (three components, several JSX call sites) duplicates the
  `!!meta.execute` fallback logic three times with three different answers
  if a future cell type wants the same treatment. A metadata field is a
  single source of truth and matches how every other per-type behavior flag
  already works (`canBlockDownstream`, `defaultSelectionMode`, etc.).
- **Suppressing Run-from-here/auto-run via `undefined` callbacks at the call
  site vs. adding a second metadata flag (e.g. `canRunFromHere`).** A second
  flag would be more general but there's currently exactly one cell type
  (markdown) that wants Run without Run-from-here, and the two callbacks are
  only ever assembled in one place (`NotebookRenderer.tsx`'s `renderCell`).
  Introducing a second boolean for a single caller is unwarranted
  abstraction; a one-line type check at the existing call site is simpler
  and just as clear. Revisit if a second cell type needs the same split.
- **Not touching `useCellManager`'s `nonExecKeys` auto-run exclusion.**
  Adding `content` there would prevent the auto-run footgun too, but only
  matters if `autoRunFromHere` can ever be set on a markdown cell — which
  this plan prevents by hiding the toggle. Changing `nonExecKeys` without
  also hiding the toggle would leave a confusing UI (a toggle with no
  observable effect); hiding the toggle is the more honest fix.
- **Eagerly marking new markdown cells `success` on creation** (see Open
  Questions) was considered as a way to skip even the one Run click for
  brand-new cells, but adds a new callback into `useCellManager` for a minor
  polish; deferred rather than bundled into this fix.
- **Keeping the deferred-render gate (`status === 'success'`) as-is.** An
  alternative would be to drop the gate entirely and always render markdown
  eagerly from raw `content`/`variables` regardless of status, sidestepping
  the whole `canRun` question. Rejected: that's exactly the flash-of-stale-
  macros bug #1023 fixed, and the issue explicitly asks to preserve
  "re-rendering against the *existing* upstream results" rather than
  reintroducing unresolved-macro flashes on first paint.

## Documentation

- `mkdocs/docs/web-app/notebooks/cell-types.md:452-476` (Markdown section) —
  update the "Does not execute queries or block downstream cells" bullet to
  clarify markdown now has its own Run control that re-renders in place
  without running any query, and note that the deferred-until-first-run
  behavior on initial load is unchanged.

## Testing Strategy

- **Automated**: see Implementation Steps 8-11 — `canRun` metadata assertion,
  `CellContainer` Run-button visibility for a no-`execute` type with
  `canRun: true`, the same Run-button visibility for a markdown child in the
  hg per-child editor panel (`ChildEditorView`), `NotebookRenderer` markdown
  Run button now visible while "Run from here"/"Auto-run from here" stay
  hidden, live content-edit reactivity while `status: 'success'`, and
  Run-click recovery from `idle` without touching upstream cells.
- **Manual**: see Implementation Step 13 — the scenario the issue describes
  (slow upstream query, markdown edit, no reload needed; Run works
  independently of an in-flight upstream re-run).
- **Regression**: existing `NotebookRenderer.test.tsx` assertions for
  non-markdown Run-button visibility (line 498) and the deferred-render
  `MarkdownCell.test.tsx` suite (blank on `idle`/`loading`/`blocked`) are
  unaffected — this plan doesn't change when markdown renders blank, only
  how it can be nudged out of that state.

## Open Questions

- **Should newly added/duplicated markdown cells auto-execute (mark
  `success`) immediately on creation**, so the very first character typed
  into a brand-new cell renders without even one Run click? It's a small,
  safe addition (the shortcut is synchronous/free) but touches
  `useCellManager`'s creation paths, which today don't call into execution
  at all. Leaning toward yes as a fast follow rather than bundling it here —
  flagging for a decision before implementation.
- **Keyboard shortcut parity with Jupyter's `Shift+Enter`.** The issue
  frames the problem via that comparison but doesn't explicitly ask for the
  same binding. `useNotebookKeyboardNav.ts` currently has no run-cell
  shortcut for any cell type, so adding one would be new scope beyond
  markdown. Recommend leaving to a separate issue unless the reporter
  confirms it's wanted now.
