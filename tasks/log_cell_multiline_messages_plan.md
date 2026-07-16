# Log Cell Multiline Messages Plan

## Overview

The notebook Log cell (`LogCell.tsx`) currently renders every column — including `msg` — as a single truncated line (`truncate` → `overflow-hidden text-overflow-ellipsis whitespace-nowrap`), with the full value only reachable via the `title` hover tooltip. Long or multi-line messages (e.g. stack traces, multi-field log lines) get cut off and the hover tooltip is a poor substitute for reading them. This adds a per-cell "Wrap text" toggle that switches all columns from single-line-truncated to wrapped/multiline rendering, so rows grow to show the full message.

## Current State

- `renderLogColumn()` (`analytics-web-app/src/lib/screen-renderers/log-utils.tsx:120-179`) is the single shared rendering function for all four column kinds (`time`, `level`, `target`, generic/`default` — which includes `msg`). The `default` case (lines 162-177) and the `target` case (lines 150-161) both use Tailwind's `truncate` class; `time` and `level` use `whitespace-nowrap`/no wrapping (their formatted values are always short, so this is moot).
- `LogCell.tsx` (`analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx:248-296`) renders each visible row as a `flex px-2 py-0.5` `<div>`, with one `<span>` per column plus a `<LogDivider>` between adjacent columns. There is **no virtualization** (no react-window/react-virtual) — every row in the current page is a real DOM node inside a scrollable `overflow-auto` container, and pagination (not virtualization) bounds the row count. This means row height is not fixed anywhere; letting a cell wrap to multiple lines works with zero virtualizer changes.
- Column widths come from `computeFlexWidths()` (`log-utils.tsx:202-251`), which measures the longest *formatted string* per column over the visible page and clamps to `[MIN_FLEX_WIDTH_PX, MAX_FLEX_WIDTH_PX]` (60–700px), or `[MIN_FLEX_WIDTH_PX, 200]` for `target`. Users can also pin an explicit width per column by dragging a `LogDivider` (`1130_log_cell_resizable_columns_plan.md`); pinned widths are stored in `options.columnWidths`.
- `options` is the existing per-cell persistence bag (`options.pageSize`, `options.columnWidths`) — round-tripped through `onOptionsChange` and saved with the notebook.
- `whitespace-pre-wrap` is already used as a Tailwind utility class elsewhere in this codebase for similar "show the whole thing" text blocks (`MapCell.tsx:401`); `FlameGraphCell.tsx:464` uses the same behavior via the inline style `whiteSpace: 'pre-wrap'`. So preserving embedded newlines isn't a new pattern for the app.
- `formatRowForCopy()` (`log-utils.tsx:181-200`) already strips embedded `\t\r\n` when building the clipboard string for the row-copy button, precisely because `msg` commonly contains embedded newlines (e.g. stack traces) — confirming multiline `msg` values are a real, expected case today, just not visible.
- The standalone `LogRenderer.tsx` screen is `@deprecated` in favor of `LogCell` and is out of scope for this change (per explicit direction) — it keeps its current truncate-only behavior unchanged.

## Design

### Toggle, not always-on

Add a **"Wrap text" toggle** rather than making wrapping unconditional:
- Default **on**, so the actual `msg` value is visible immediately — no extra click needed to solve the problem this plan exists for. A click switches the whole cell back to today's compact single-line-truncated look, for anyone who prefers it.
- Applies to the whole cell (all columns, all rows on the current page) — no per-row or per-column granularity, keeping the UI simple and consistent with the "no special case for `msg`" precedent from `log_msg_content_width_plan.md`.
- Persisted in `options.wrapText: boolean`, the same mechanism as `pageSize` and `columnWidths`, so it survives notebook reloads. Undefined (e.g. notebooks saved before this change) is treated as `true` — new and existing cells alike open wrapped; a user who explicitly turns it off gets `wrapText: false` persisted, which is respected on reload.

### `renderLogColumn` changes

Add `wrap?: boolean` to `RenderLogColumnOptions`. Introduce a small helper to avoid duplicating the class logic across the four `switch` cases:

```ts
function textCellClasses(wrap: boolean | undefined): string {
  return wrap ? 'whitespace-pre-wrap break-words' : 'truncate'
}
```

- `target` and `default` cases: replace the hardcoded `'truncate'` class fragment with `textCellClasses(opts?.wrap)`.
- `time`/`level` cases: their formatted values never wrap in practice (fixed 29-char timestamp; short level names), but apply the same helper for consistency rather than special-casing them out — keeps the four cases uniform, matching the existing "no special case" precedent.
- Keep the `title={formatted}` attribute regardless of `wrap` — harmless when the text is already fully visible, and still useful if a value is capped by `MAX_FLEX_WIDTH_PX`/`MAX_COL_WIDTH_PX` and wraps mid-word.
- No change to `computeFlexWidths()`: column width is still measured from the longest formatted string on the page (capped at 700px / 200px for `target`). When wrapping is on, a value longer than its column's width simply wraps into multiple lines instead of being clipped — the existing cap just becomes a "reading width" rather than a hard clip point. This is a deliberate simplification: measuring by longest *individual line* (splitting on `\n`) instead of total string length would size columns more precisely for multi-line stack traces, but adds complexity for a secondary refinement; the plain approach is a reasonable starting point and can be revisited if the capped-width column reads too narrow in practice.

### Row layout changes

- `LogCell.tsx`'s row `<div>` (currently `flex px-2 py-0.5 ...`) needs `items-start` added so that when `msg` wraps to multiple lines, the fixed-width `time`/`level`/`target` columns and the `LogDivider`s stay top-aligned with the first line of the wrapped text, instead of being vertically centered/stretched across the now-taller row (flex's default `align-items: stretch` combined with single-line inline content already renders identically to `items-start` today, so this is a no-op visually until wrap is enabled).
- The row-copy button (`absolute left-0 top-1/2 -translate-y-1/2`) stays vertically centered on the full (now possibly multi-line) row height — acceptable as-is; not worth special-casing to top-align just for the copy affordance.
- `LogDivider` already stretches to the row's height via `self-stretch`, so multi-line rows need no divider changes.

### Toggle UI

Add a "Wrap text" button to the bottom bar in `LogCell.tsx` (`analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx:298-308`), alongside the existing conditional "Reset widths" button — same visual language (small text button), using the `WrapText` icon from `lucide-react` (already a dependency) to make the toggle recognizable at a glance. Unlike "Reset widths", this button is always visible (not conditional), and reflects its on/off state visually (e.g. highlighted/accent color when active). A visual mockup of this toggle (wrapped vs. compact states) is at `tasks/log_cell_wrap_mockups/log-cell-wrap-toggle.html`.

```tsx
<button
  onClick={() => onOptionsChange({ ...options, wrapText: !wrapText })}
  className={`text-[10px] px-2 py-0.5 transition-colors flex items-center gap-1 ${
    wrapText ? 'text-accent-link' : 'text-theme-text-muted hover:text-theme-text-secondary'
  }`}
  aria-pressed={wrapText}
>
  <WrapText size={11} />
  Wrap text
</button>
```

## Implementation Steps

1. **`log-utils.tsx`**
   - Add `wrap?: boolean` to `RenderLogColumnOptions`.
   - Add the `textCellClasses(wrap)` helper.
   - Update all four cases in `renderLogColumn` to use `textCellClasses(opts?.wrap)` in place of their current hardcoded `truncate`/no-wrap class fragments.

2. **`LogCell.tsx`**
   - Add `WrapText` to the existing `lucide-react` import (currently `import { ScrollText, Copy, Check } from 'lucide-react'` at `LogCell.tsx:16`).
   - Read `wrapText` from `options` (`const wrapText = (options?.wrapText as boolean | undefined) ?? true`), same pattern as `pageSize`.
   - Pass `wrap: wrapText` into every `renderLogColumn(col, row, { width, isLast, wrap: wrapText })` call.
   - Add `items-start` to the row container's class string.
   - Add the "Wrap text" toggle button to the bottom bar, wired to `onOptionsChange({ ...options, wrapText: !wrapText })`.

3. Leave `LogRenderer.tsx` untouched — it doesn't pass `wrap`, so `renderLogColumn` defaults to today's truncate-only behavior there.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/log-utils.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx`

## Trade-offs

**Defaulting on vs. always-on with no toggle**: always-on with no escape hatch would directly satisfy "we need to see the whole messages," but removes the option to fall back to a compact, at-a-glance view for tables full of long stack-trace-style messages. Defaulting the toggle to **on** gets the same zero-click fix while still letting anyone flip it off per cell — and because it's persisted, that choice sticks per cell rather than needing to be redone every session. The trade-off accepted here: existing saved notebooks will render wrapped (taller rows) the next time they're opened, since unset `options.wrapText` now defaults to `true` — a one-time visual change, not a behavior users need to opt into.

**Whole-cell toggle vs. per-column or per-row toggle**: per-column (e.g. via the `LogDivider` context menu, which already has "Reset to auto"/"Reset all") or per-row (click-to-expand) would be more granular, but add meaningfully more state and UI surface for a problem that's really just "let me read `msg`." A single cell-wide toggle is the simplest fix consistent with the existing "no special case for `msg`" design.

**Width measurement unchanged**: not re-deriving `computeFlexWidths` to measure per-line-length instead of total string length is a known simplification (see Design section) — a very long single-line-equivalent (e.g. one giant JSON blob with no newlines) will still size the column to the 700px cap and then wrap within that cap, which is the same as today's behavior minus the clipping.

## Testing Strategy

- `yarn type-check` and `yarn lint` — no new errors.
- `yarn test` — existing `log-utils.test.ts` (pure string/classification helpers, no rendering) still passes. `renderLogColumn` returns JSX, so add a new `.tsx` test file (e.g. `log-utils.render.test.tsx`) that renders it with React Testing Library and asserts it emits `whitespace-pre-wrap break-words` when `wrap: true` and `truncate` when `wrap` is absent/`false`. There is no existing `LogCell` test file today.
- Manual, in the running app (`./start_analytics_web.py`):
  - Add a Log cell against data containing a long or multi-line `msg` (e.g. a stack trace) — confirm it renders wrapped by default, with the full message visible and other columns top-aligned to the first line.
  - Click "Wrap text" to turn it off — confirm rows collapse to single-line truncated text.
  - Reload the page — confirm the wrap toggle state persists (saved in `options.wrapText`).
  - Click "Wrap text" again — confirm it reverts to wrapped rows.
  - Paginate while wrap is on — confirm column auto-widths still recompute per page as before.
  - Open a notebook saved before this change (no `options.wrapText` key) — confirm its Log cells default to wrapped, not truncated.

## Open Questions

- Is a single cell-wide toggle sufficient, or is per-column wrapping (via the `LogDivider` context menu) actually wanted for cases where only `msg` should wrap but `target` should stay compact? Starting with the simpler whole-cell toggle; can extend later if needed.
