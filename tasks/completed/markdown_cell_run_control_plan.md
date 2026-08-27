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

Suppress both affordances at the only two call sites that build markdown's
`CellContainer` props (`NotebookRenderer.tsx:707-712`, the hg group header
at `:577-578` doesn't apply — markdown isn't a group). Pass
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

Hiding the toggle only closes the normal UI path, though. `autoRunFromHere`
is declared on the generic `CellConfigBase` (`notebook-types.ts:107`), not
gated by cell type, and `NotebookSourceView`'s raw-JSON "Apply" editor
(`onConfigChange(JSON.parse(sourceText))`) only checks that `cells` is an
array — nothing stops a user from pasting/editing JSON that sets a markdown
cell's `autoRunFromHere: true` directly, bypassing the toggle entirely. So
this plan also adds `'content'` to the `nonExecKeys` set in
`useCellManager.updateCell` (`:229`), closing the gap regardless of how
`autoRunFromHere` got set. `content` is unique to `MarkdownCellConfig`
(`notebook-types.ts:125`) — no query-backed cell type has a `content` field
(they use `sql`) — so this is a no-op for every other cell type.

### Wiring `onRun`

Mostly no new wiring is needed: `onRun={() => executeCellByName(cell.name)}`
is already passed unconditionally to `CellContainer` for every cell
(`NotebookRenderer.tsx:707`), to the renderer props (`:663`), to the editor
panel footer (`:851`), and to `HgChildPane`. It was simply unreachable
because `canRun` hid it. Once `canRun` is `true` for markdown, clicking the
existing Play button calls `executeCellByName('cellname')` →
`executeCell(idx)`, which hits the `!meta.execute` shortcut and sets
`{ status: 'success', data: [] }` synchronously — no query re-execution,
no dependency on upstream cells having already run.

One gap: `CellEditor.tsx:167` already passes `onRun={onRun}` into
`meta.EditorComponent` unconditionally for every cell type, and every other
cell-type editor forwards it to `SyntaxEditor`'s `onRunShortcut` (Ctrl/Cmd+Enter)
— e.g. `ChartCell.tsx:469`, `LogCell.tsx:357`, `TableCell.tsx:254`, and eight
others. `MarkdownCellEditor` doesn't destructure `onRun` and its `SyntaxEditor`
call omits `onRunShortcut`, so the keyboard shortcut silently wouldn't work
even with `canRun: true`. Fix: destructure `onRun` from `CellEditorProps` in
`MarkdownCellEditor` and pass `onRunShortcut={onRun}` to its `SyntaxEditor`,
matching every other cell type.

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
   - In `MarkdownCellEditor`, destructure `onRun` from `CellEditorProps` and
     pass `onRunShortcut={onRun}` to its `SyntaxEditor`, matching every other
     cell-type editor.
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
8. `analytics-web-app/src/lib/screen-renderers/useCellManager.ts`
   - Add `'content'` to the `nonExecKeys` set in `updateCell` (`:229`), so
     editing a markdown cell's content never schedules a downstream auto-run
     even if `autoRunFromHere` reached it some other way than the (now
     hidden) toggle — e.g. `NotebookSourceView`'s raw-JSON editor. See
     Design's "Suppress ... for markdown" section.
9. `analytics-web-app/src/lib/screen-renderers/cells/__tests__/MarkdownCell.test.tsx`
   - Add a test asserting `markdownMetadata.canRun === true`.
10. `analytics-web-app/src/lib/screen-renderers/__test-utils__/cell-registry-mock.ts`
    - Add `canRun: true` to `BASE_METADATA.markdown`, mirroring how
      `canBlockDownstream: false` already mirrors production `markdownMetadata`
      there. Without this, `getCellTypeMetadata('markdown').canRun` stays
      `undefined` under the shared mock used by `CellContainer.test.tsx`,
      `HorizontalGroupCell.test.tsx`, and `NotebookRenderer.test.tsx`, and the
      new tests in steps 11-13 can't exercise the Run button for markdown.
11. `analytics-web-app/src/components/__tests__/CellContainer.test.tsx`
    - Add a case: a cell type with `execute` absent but `canRun: true`
      (or a mock) shows the Run button; confirm "Run from here"/"Auto-run"
      visibility is driven by the presence of the corresponding callbacks,
      not by `canRun` alone.
    - Update the existing `'should not show run button for markdown cells'`
      test (~line 167): with the mock registry's `markdown.canRun` now
      `true` (step 10) and no `canRunProp` override, the Run button *does*
      render for `type="markdown"` — this assertion no longer holds. Replace
      it with the positive case above (or repurpose this test into it), and
      keep a separate case confirming an explicit `canRunProp={false}` still
      hides the button regardless of type.
    - Update the existing `'should not show "Run from here" for markdown
      cells'` test (~line 218): `CellContainer` has no cell-type-specific
      logic of its own — the plan's suppression is applied only by the
      caller (`NotebookRenderer`, step 7), which now omits `onRunFromHere`
      entirely for markdown. This test passes `onRunFromHere` directly, so
      once `canRun` is metadata-driven and `true` for markdown, "Run from
      here" renders here too, regardless of `type`. Remove this test (its
      intent is now covered by the callback-presence case added above) and
      rely on the `NotebookRenderer.test.tsx` update in step 13 — where
      `onRunFromHere` is genuinely never passed for markdown — to verify the
      end-to-end suppression.
12. `analytics-web-app/src/lib/screen-renderers/cells/__tests__/HorizontalGroupCell.test.tsx`
    - Add a case: a markdown child selected in the per-child editor panel
      (`ChildEditorView`) shows the Run button when `onRun` is provided —
      covers the same `canRun` fallback as `HgChildPane` but for the
      full-panel editor, which the existing suite doesn't exercise.
13. `analytics-web-app/src/lib/screen-renderers/__tests__/NotebookRenderer.test.tsx`
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
14. Run `yarn lint`, `yarn type-check`, and `yarn test` in `analytics-web-app/`.
15. Manual check with `yarn dev` (or the monolith): open a notebook with a
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
- `analytics-web-app/src/lib/screen-renderers/useCellManager.ts`
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/MarkdownCell.test.tsx`
- `analytics-web-app/src/lib/screen-renderers/__test-utils__/cell-registry-mock.ts`
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
- **Hiding the toggle alone is not sufficient — also add `content` to
  `useCellManager`'s `nonExecKeys`.** Hiding "Auto-run from here" closes the
  normal UI path, but `autoRunFromHere` lives on the generic `CellConfigBase`
  and isn't schema-gated by cell type: `NotebookSourceView`'s raw-JSON
  editor can set `autoRunFromHere: true` on a markdown cell directly,
  bypassing the toggle entirely. So this plan does both — hides the toggle
  (the honest UI fix, since a visible toggle with no effect would be
  confusing) *and* adds `'content'` to `nonExecKeys` (the actual footgun
  fix, since it's the only thing that makes editing content schedule a
  downstream auto-run in the first place). `content` is unique to
  `MarkdownCellConfig`, so this is a no-op for every other cell type.
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

- **Automated**: see Implementation Steps 9-13 — `canRun` metadata assertion,
  the shared `cell-registry-mock.ts` update so `canRun: true` is visible to
  tests, `CellContainer` Run-button visibility for a no-`execute` type with
  `canRun: true` (with the two now-stale markdown-suppression tests in
  `CellContainer.test.tsx` updated/removed per step 11), the same Run-button
  visibility for a markdown child in the hg per-child editor panel
  (`ChildEditorView`), `NotebookRenderer` markdown Run button now visible
  while "Run from here"/"Auto-run from here" stay hidden, live content-edit
  reactivity while `status: 'success'`, and Run-click recovery from `idle`
  without touching upstream cells.
- **Manual**: see Implementation Step 15 — the scenario the issue describes
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
- **Keyboard shortcut parity with Jupyter's exact `Shift+Enter` binding.**
  The issue frames the problem via that comparison but doesn't explicitly ask
  for the same key. Run-shortcut infrastructure already exists at the editor
  level — `SyntaxEditor`'s `onRunShortcut` (Ctrl/Cmd+Enter) is wired into
  every SQL-backed cell editor, and markdown now gets it too via the
  "Wiring `onRun`" fix above — but there's no `Shift+Enter`-style binding at
  the global-nav layer (`useNotebookKeyboardNav.ts`), and Ctrl/Cmd+Enter only
  fires while the cell's editor has focus, not from the collapsed/rendered
  view. Adding a `Shift+Enter` global binding would be new scope beyond
  markdown. Recommend leaving to a separate issue unless the reporter
  confirms it's wanted now.
