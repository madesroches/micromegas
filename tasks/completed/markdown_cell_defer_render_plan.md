# Markdown Cell Deferred Render Plan

## Status: Implemented

## Overview
Markdown cells currently render their substituted content the moment the notebook mounts — before the sequential execution pass has reached them. This means a cell that uses `$variable`, `$cell.col`, or `$begin`/`$end` macros flashes stale or broken output (from the unrendered/placeholder values) until the cell's turn in the execution order. This plan defers the markdown render until the cell has executed in sequence so the output stays empty/placeholder until the cell runs.

Related issue: https://github.com/madesroches/micromegas/issues/1023

## Current State

Markdown cells live at `analytics-web-app/src/lib/screen-renderers/cells/MarkdownCell.tsx`. The renderer (lines 15–27) unconditionally calls `substituteMacros()` with whatever `variables`, `cellResults`, and `cellSelections` are available at render time and emits the result:

```tsx
export function MarkdownCell({ content, variables, timeRange, cellResults, cellSelections }: CellRendererProps) {
  const markdownContent = useMemo(() => {
    if (!content) return ''
    return substituteMacros(content, variables, timeRange, cellResults, cellSelections)
  }, [content, variables, timeRange, cellResults, cellSelections])

  return (
    <div className="prose …">
      <Markdown remarkPlugins={[remarkGfm]}>{markdownContent}</Markdown>
    </div>
  )
}
```

`status` is passed through `CellRendererProps` (`analytics-web-app/src/lib/screen-renderers/cell-registry.ts:21`) but the renderer ignores it.

Execution model (relevant bits):

- `useCellExecution.executeFromCell()` (`useCellExecution.ts:297–345`) resets every cell from `startIndex` onward to `{ status: 'idle' }` (line 321), then iterates cells and awaits each `executeCell(i)`.
- `executeCell` (`useCellExecution.ts:121–294`) shortcuts cells without an `execute` method directly to `{ status: 'success', data: [] }` (line 130). Markdown is this path — see `markdownMetadata` at `MarkdownCell.tsx:74–96` (`canBlockDownstream: false`, no `execute`).
- If an upstream query cell halts execution (error or blocked by unresolved selection macro), the downstream loop marks cells with `canBlockDownstream === true` as `blocked` (`useCellExecution.ts:330–339`). Markdown cells have `canBlockDownstream: false`, so they **remain `idle`** when execution halts above them.

So the `CellState.status` field already carries the information we need: `idle` ⇒ not yet reached, `success` ⇒ executed. No execution-model changes are required — the renderer just has to honor the status it's given.

Same bug extends to two surfaces that reuse the renderer: cells inside horizontal groups (`HorizontalGroupCell.tsx:403` already threads `cellStates` through), and the notebook-scoped preview path through `buildCellRendererProps` (`notebook-cell-view.ts:197–233`, which already forwards `status`).

Docs: `mkdocs/docs/web-app/notebooks/cell-types.md:9–33` describes the markdown cell. The sentence "Does not execute queries or block downstream cells" is accurate but leaves out the sequencing behavior we're fixing.

## Design

### Core change

In `MarkdownCell` renderer, gate the rendered markdown on `status`:

- `status === 'success'` — render the substituted markdown content (current behavior).
- otherwise (`idle`, `loading`, `error`, `blocked`) — render an empty placeholder.

`loading` and `error` are unreachable for markdown today (no execute method), but the gate should still be "render only on success" rather than "don't render on idle" so the behavior is well-defined if the metadata ever changes.

### Placeholder

Keep it minimal — an empty `<div>` with the same outer class list preserves the cell's prose container so layout (height, padding, collapse) doesn't jump when the content appears. No spinner, no "waiting" text: the cell body simply stays blank until the cell's turn, which matches the issue's expectation ("empty/placeholder until the cell runs in sequence") and parallels how table cells show "No data available" rather than animated chrome when there's nothing to show yet.

### Status prop wiring

`MarkdownCell` already receives `status` via `CellRendererProps` — no changes needed to `buildCellRendererProps`, `markdownMetadata.getRendererProps`, or any intermediate layer. The fix is local to the renderer component.

### Edge cases

1. **Editing the markdown content while in edit mode.** Editing happens through `MarkdownCellEditor`, which renders a textarea and doesn't depend on the main renderer's output — unaffected. The preview in the main cell body still respects the executed-status gate, which is the desired behavior (user sees what a viewer would see).
2. **No macros at all.** A markdown cell with plain content (no `$…` references) still won't render until executed. This is consistent and avoids a "sometimes appears immediately, sometimes doesn't" UX. Execution of markdown is synchronous (completes in the same tick as the surrounding sequential pass), so the user-visible delay is negligible in practice.
3. **Upstream cell fails / halts execution.** Markdown cells remain `idle` (they have `canBlockDownstream: false`, so they aren't marked `blocked`). Under the new rule they simply stay blank. This is the correct outcome — a markdown cell that references a cell result whose execution was aborted *should* stay blank rather than showing a stale/broken reference.
4. **Re-execution from a middle cell (`executeFromCell(n)`).** `executeFromCell` resets cells `n..end` to `idle` before re-running. Markdown cells below `n` will blank out then repopulate as the pass reaches them — matching the desired "output only appears when execution reaches you" behavior.
5. **Refresh / time-range change.** Both trigger `executeFromCell(0)` which resets all cells to `idle`. Markdown cells briefly blank and then repopulate on success. Acceptable and consistent.

## Implementation Steps

1. Update `analytics-web-app/src/lib/screen-renderers/cells/MarkdownCell.tsx`:
   - Destructure `status` from `CellRendererProps`.
   - Compute `markdownContent` only when `status === 'success'`; otherwise pass an empty string to `<Markdown>`.
   - Preserve the existing `className` on the wrapping `<div>` so layout is unchanged.

2. Update tests in `analytics-web-app/src/lib/screen-renderers/__tests__/NotebookRenderer.test.tsx`:
   - The existing test fixtures (`createMarkdownCell`, usages at lines 188, 505, 587) create markdown cells that are asserted against rendered output. With the new gate, those assertions need to wait for the initial execution pass to reach the markdown cell. The test already waits on async execution for other assertions, so verify each markdown assertion is wrapped in `waitFor` / `findBy*`. Add a new test covering the initial blank state before execution completes.

3. Run `yarn lint` and `yarn test` in `analytics-web-app/`.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/cells/MarkdownCell.tsx` — gate render on `status === 'success'`.
- `analytics-web-app/src/lib/screen-renderers/__tests__/NotebookRenderer.test.tsx` — add/adjust assertions for the deferred-render behavior.
- `mkdocs/docs/web-app/notebooks/cell-types.md` — one-line note that markdown renders on execution.

## Trade-offs

- **Gate in the renderer vs. gate in `buildCellRendererProps` / `getRendererProps`.** Gating at the prop-assembly layer (emit empty `content` while `status !== 'success'`) would also work and keeps the renderer dumb. Rejected because the renderer already owns macro substitution and already has `status` — splitting that responsibility across `notebook-cell-view.ts` just for one cell type adds indirection for no gain.
- **Tracking `hasExecuted` separately on `CellState`.** The Explore pass surfaced this as a possibility. Rejected: `status === 'success'` already means "the cell finished its execution pass" for markdown, and adding a parallel boolean duplicates that signal.
- **Change `executeCell` to not mark markdown `success` immediately.** Considered and rejected — the current fast-path for cells without `execute()` is correct, and downstream logic (e.g., `canBlockDownstream` handling) already assumes markdown reaches `success`. The fix belongs in the render layer, not the execution layer.
- **Keep the current behavior (render immediately).** Would avoid the churn but leaves the bug the issue reports — stale macros and broken links visible on page load.

## Documentation

- Update `mkdocs/docs/web-app/notebooks/cell-types.md` markdown section to note: rendered output appears only after the cell executes in sequence; blank until then. Single-line clarification next to the existing "does not execute queries" note.
- No update needed to `variables.md` or `execution.md` — the execution order description there already states cells run top-to-bottom; this change just makes markdown observably follow that ordering.

## Testing Strategy

- **Manual**: Load a notebook where a markdown cell references `$variable` and `$cell.col` from an upstream variable/query cell. Before this change, the markdown flashes unresolved macros on first load. After this change, the markdown body is blank until the execution pass reaches it, then populates with substituted output.
- **Automated**: In `NotebookRenderer.test.tsx`, assert that immediately after mount (before awaiting execution) the markdown cell's body is empty, and after awaiting initial execution the substituted content appears. Re-execution from a middle cell blanks downstream markdown cells until their turn.
- **Regression**: Existing tests at `NotebookRenderer.test.tsx:496, 504` (run-button visibility) are unaffected — they assert on cell chrome, not markdown body content.

## Open Questions

None.
