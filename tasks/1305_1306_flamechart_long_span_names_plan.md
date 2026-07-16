# Flamechart: Bound Long Span Names in Canvas Labels and Hover Tooltip Plan

**GitHub Issues**:
- https://github.com/madesroches/micromegas/issues/1305 (on-canvas label clip overlaps siblings)
- https://github.com/madesroches/micromegas/issues/1306 (DOM hover tooltip doesn't wrap/bound long names)

## Overview

The flamechart cell (`analytics-web-app`) renders span names in two independent places, and both
mishandle long or multi-line names (e.g. an LLM prompt used as a span name):

1. **On-canvas label** (`FlameGraphScene.ts`): the `ctx.clip()` rectangle drawn per label is computed
   from the span's unclamped width capped against the whole canvas, so for off-screen-left or
   very-wide spans the clip box no longer matches the span's real box and the label paints through
   sibling spans on the same row.
2. **DOM hover tooltip** (`FlameGraphCell.tsx`): the tooltip is `whiteSpace: 'nowrap'` with no
   `max-width`, so a long name renders on one ever-widening line that runs off the right edge, and
   embedded newlines are collapsed rather than shown as line breaks.

Both are self-contained rendering fixes with no data-model or query changes. This plan addresses them
together since they share a root cause (unbounded rendering of arbitrary-length names) and the same
two files.

## Current State

### On-canvas label — `FlameGraphScene.ts:216-221`

Inside the label loop (each visible span at `x1 = (begin - viewMinTime) * pxPerMs`,
`x2 = (end - viewMinTime) * pxPerMs`, `w = x2 - x1`):

```ts
ctx.save()
ctx.beginPath()
ctx.rect(Math.max(x1 + 2, 0), y, Math.min(w - 4, view.width), SPAN_HEIGHT)
ctx.clip()
ctx.fillText(name, x1 + 4, y + SPAN_HEIGHT / 2 + 1)
ctx.restore()
```

The clip rect's left edge is clamped to `0`, but its width term is `Math.min(w - 4, view.width)` —
capped against the whole canvas width instead of the span's own right edge (`x2`). Two failure modes:

- **Off-screen left edge** (`x1 < 0`, common for a long-lived parent once zoomed into a child): left
  clamps to `0` but width isn't reduced to compensate, so the clip's right edge lands `|x1|` px past
  the span's true end.
- **Very wide span** (`w - 4 > view.width`, typical for long-duration ancestors — exactly the ones
  likely to carry long descriptive names): `Math.min` collapses the width to `view.width`, so the
  clip spans the entire visible canvas regardless of where the span actually ends.

In both cases `ctx.fillText` for a long name paints through whatever sibling spans/labels sit at the
same row to the right. There is no `measureText`/ellipsis fallback anywhere in the file.

Relevant constants (`flame-model.ts:37-42`): `SPAN_HEIGHT = 20`, `LABEL_MIN_WIDTH_PX = 40` (labels
are only drawn for spans at least 40px wide — `FlameGraphScene.ts:211`).

### DOM hover tooltip — `FlameGraphCell.tsx`

- Line 269: `let info = \`<b>${escapeHtml(name)}</b>\`` — name is HTML-escaped (no XSS) but never
  truncated or wrapped. The rest of the tooltip body uses `<br>` for structure (Duration, id, depth,
  parent, filename, …).
- Lines 306-309: positioning nudges only the tooltip's *start* on-screen with hardcoded guesses
  (`Math.min(x + 12, s.width - 200)`, `Math.min(y + 12, s.height - 80)`) — nothing bounds its width or
  height.
- Lines 454-464: the tooltip div is styled `whiteSpace: 'nowrap'` with no `max-width`/`max-height`
  and `pointer-events-none`.

Result: `\n` in a name is collapsed by HTML whitespace handling and `nowrap` flattens the whole name
onto one line, which keeps extending the box off the right edge of the viewport.

### Tests

Existing flamechart tests (`src/lib/screen-renderers/cells/__tests__/`) are pure-logic tests over
`flame-model` helpers (`buildFlameIndex`, `formatBits`) and `FlameGraphLayout` — no canvas/DOM
rendering is exercised. To keep the new logic testable in the same style, the geometry and
string-shaping decisions are extracted into small pure helpers rather than left inline in the
render/event callbacks.

## Design

### 1. On-canvas label clip (issue #1305)

Compute the clip rect from the true left and right edges, each clamped independently to the canvas,
per the issue's suggested fix. Extract a pure helper so it can be unit-tested without a canvas.

New helper in `flame-model.ts` (co-located with the other shared flame constants/helpers):

```ts
/**
 * Clip rectangle (x-axis only) for a span's on-canvas label, in CSS pixels.
 * Derives left/right from the span's true edges, each clamped independently to
 * the canvas — so the clip never extends past the span's real box even when the
 * span starts off-screen-left (x1 < 0) or is wider than the viewport (x2 > width).
 * Returns width 0 when the span's visible portion is empty.
 */
export function labelClipRect(x1: number, x2: number, viewWidth: number): { left: number; width: number } {
  const left = Math.max(x1 + 2, 0)
  const right = Math.min(x2 - 2, viewWidth)
  return { left, width: Math.max(right - left, 0) }
}
```

**Ellipsis truncation** (second line of defense from the issue): a name that is too long for its
(correctly clipped) span box is truncated with a trailing `…` so it reads as intentionally-cut rather
than clipped mid-glyph. The label font is `11px monospace` (`FlameGraphScene.ts:184`), so every glyph
has the same advance width — the fit can be computed arithmetically from a single character
measurement taken **once per frame**, avoiding a per-label `measureText`. The pure part (given an
available width and a per-character advance, produce the fitted string) is extracted for testing:

```ts
/**
 * Truncate `name` to fit `availWidth` px given a fixed per-character advance
 * (`charWidth`, valid for the monospace label font), appending '…' when cut.
 * Returns '' when not even one glyph fits.
 */
export function fitLabelText(name: string, availWidth: number, charWidth: number): string {
  if (charWidth <= 0) return name
  const maxChars = Math.floor(availWidth / charWidth)
  if (name.length <= maxChars) return name
  if (maxChars < 1) return ''
  return name.slice(0, maxChars - 1) + '…' // '…' occupies the last cell
}
```

Measure the character advance once, right after the font is set (near `FlameGraphScene.ts:184-185`):

```ts
ctx.font = '11px monospace'
ctx.textBaseline = 'middle'
const charWidth = ctx.measureText('0').width // monospace ⇒ constant per-glyph advance
```

Call site in `FlameGraphScene.ts` (replacing lines 216-221):

```ts
const { left, width: clipW } = labelClipRect(x1, x2, view.width)
if (clipW <= 0) continue // span's visible box is too narrow to hold any label

// Pin the label to the visible left edge so it stays readable when the span
// starts off-screen-left; clip still constrains it to the span's box.
const textX = Math.max(x1 + 4, left + 2)
const availWidth = left + clipW - textX
const label = fitLabelText(name, availWidth, charWidth)
if (!label) continue

ctx.save()
ctx.beginPath()
ctx.rect(left, y, clipW, SPAN_HEIGHT)
ctx.clip()
ctx.fillText(label, textX, y + SPAN_HEIGHT / 2 + 1)
ctx.restore()
```

Three changes beyond the raw clip fix:

- **`if (clipW <= 0) continue`** — a span whose only visible pixels are its 2px inset margins has no
  room for a label; skip it rather than push an empty clip path.
- **Pin the label start to the visible left edge** (`Math.max(x1 + 4, left + 2)` instead of `x1 + 4`).
  Today, when a parent span starts off-screen-left (`x1 < 0`) the label is drawn at a negative x and
  disappears even though the span's bar fills the row. Pinning it to the visible left edge keeps the
  name readable — the behavior every mainstream flamechart (Perfetto, speedscope) uses. The clip still
  bounds it to the span's true right edge, so it can't overlap the right-hand sibling.
- **Ellipsis via `fitLabelText`** — truncate to the pinned text's available width. The clip rectangle
  is retained as cheap defense-in-depth (sub-pixel rounding, or a proportional fallback font if
  `monospace` ever resolves unexpectedly), but with correct truncation it should rarely cut anything.

### 2. DOM hover tooltip (issue #1306)

Three coordinated changes in `FlameGraphCell.tsx`:

**(a) Style the tooltip as a bounded, wrapping box.** Replace `whiteSpace: 'nowrap'` (lines 454-464):

```ts
style={{
  display: 'none',
  backgroundColor: 'rgba(15, 15, 30, 0.95)',
  color: '#e5e7eb',
  border: '1px solid rgba(75, 85, 99, 0.5)',
  whiteSpace: 'pre-wrap',   // render embedded \n as line breaks; wrap long lines
  overflowWrap: 'anywhere', // break unbreakable tokens (URLs, base64, long ids)
  maxWidth: '360px',
  maxHeight: '40vh',
  overflow: 'hidden',       // final safety; see note below on pointer-events
}}
```

- `whiteSpace: 'pre-wrap'` makes the escaped name's `\n` characters render as line breaks *and* wraps
  long lines within the box. It does not interfere with the `<br>`-structured metadata below the name.
- `overflowWrap: 'anywhere'` breaks a single very long unbroken token (a URL or base64 blob with no
  spaces — realistic for an LLM prompt) instead of forcing horizontal overflow.
- `maxWidth`/`maxHeight` + `overflow: hidden` bound the box in both dimensions. Because the tooltip is
  `pointer-events-none`, a scrollbar would not be usable, so we do not rely on scrolling — the name is
  additionally length-capped in (b) so metadata is never pushed past `maxHeight`.

**(b) Cap the displayed name length.** A multi-thousand-character prompt would otherwise fill the
whole `maxHeight` box (even wrapped) and push the Duration/id/parent/filename lines out of view. Add a
pure helper (in `flame-model.ts`, reused by any future name-display code) and apply it before escaping:

```ts
/** Cap a name for compact display, preserving embedded newlines up to the cap. */
export function truncateSpanName(name: string, max = 300): string {
  return name.length > max ? name.slice(0, max) + '…' : name
}
```

At line 269: `let info = \`<b>${escapeHtml(truncateSpanName(name))}</b>\``. The on-canvas label is
governed by its clip box, so this cap applies only to the tooltip. (The `300` default is a starting
value — see Open Questions.)

**(c) Position from the tooltip's measured size, not hardcoded guesses.** After setting
`tooltip.innerHTML` and `display = 'block'`, read `tooltip.offsetWidth`/`offsetHeight` (now bounded by
`maxWidth`/`maxHeight`) and clamp so the whole box stays in the container, flipping to the left of /
above the cursor when near the right / bottom edge. Extract a pure helper:

```ts
/** Clamp a tooltip's top-left so the box stays within [0, container] on both axes. */
export function clampTooltipPosition(
  cursorX: number, cursorY: number,
  tipW: number, tipH: number,
  containerW: number, containerH: number,
  margin = 12,
): { left: number; top: number } {
  // Prefer below-right of the cursor; flip to the other side if it would overflow.
  let left = cursorX + margin
  if (left + tipW > containerW) left = Math.max(0, cursorX - margin - tipW)
  let top = cursorY + margin
  if (top + tipH > containerH) top = Math.max(0, cursorY - margin - tipH)
  return { left: Math.min(left, Math.max(0, containerW - tipW)), top: Math.min(top, Math.max(0, containerH - tipH)) }
}
```

This helper can live in `FlameGraphCell.tsx` (module-scope, exported for its test) or in
`flame-model.ts`; put it in `FlameGraphCell.tsx` since it is tooltip-specific and has no other caller.
Replace lines 306-309:

```ts
tooltip.innerHTML = info
tooltip.style.display = 'block'
const { left, top } = clampTooltipPosition(
  x, y, tooltip.offsetWidth, tooltip.offsetHeight, s.width, s.height,
)
tooltip.style.left = `${left}px`
tooltip.style.top = `${top}px`
```

Reading `offsetWidth`/`offsetHeight` after `display = 'block'` forces one synchronous layout per hover
move, but the tooltip is a tiny bounded subtree updated only on `mousemove` over a hit span (not per
animation frame), so the cost is immaterial.

## Implementation Steps

1. **`flame-model.ts`** — add and export the pure helpers `labelClipRect(x1, x2, viewWidth)`,
   `fitLabelText(name, availWidth, charWidth)`, and `truncateSpanName(name, max?)`.
2. **`FlameGraphScene.ts`** — import `labelClipRect` and `fitLabelText`; measure `charWidth` once
   after the label font is set; replace the inline clip-rect computation (lines 216-221) with the
   helper, the `clipW <= 0` skip, the left-edge-pinned text x, and `fitLabelText` ellipsis truncation.
3. **`FlameGraphCell.tsx`** —
   - add/export `clampTooltipPosition(...)`;
   - wrap the tooltip name with `truncateSpanName(...)` at the `info` initializer (line 269);
   - swap the tooltip div style from `whiteSpace: 'nowrap'` to the bounded/wrapping style in 2(a)
     (lines 454-464);
   - replace the hardcoded positioning (lines 306-309) with the measured-size clamp.
4. Add unit tests (see Testing Strategy) alongside the existing tests under
   `src/lib/screen-renderers/cells/__tests__/`.
5. Run `yarn lint` and `yarn type-check` (and `yarn test`) from `analytics-web-app/`.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/cells/flame-model.ts` — new `labelClipRect`,
  `fitLabelText`, and `truncateSpanName` helpers.
- `analytics-web-app/src/lib/screen-renderers/cells/FlameGraphScene.ts` — use `labelClipRect`, skip
  empty clip, pin label to visible left edge, ellipsis-truncate via `fitLabelText` (one `charWidth`
  measurement per frame).
- `analytics-web-app/src/lib/screen-renderers/cells/FlameGraphCell.tsx` — bounded/wrapping tooltip
  style, name truncation, measured-size positioning, `clampTooltipPosition` helper.
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/flame-model.test.ts` (or a new test
  file) — unit tests for the new helpers.

## Trade-offs

- **Clip from true edges *and* ellipsis, at O(1) per label.** The clip fix restores the invariant that
  a label can never paint outside its own span box (the reported bug). The ellipsis is the issue's
  requested second line of defense so long names read as intentionally cut rather than clipped
  mid-glyph. The obvious ellipsis implementation — a `measureText` (or binary search) per visible
  label per frame — was the reason to hesitate, but the label font is monospace, so a single
  per-frame character measurement plus arithmetic (`fitLabelText`) gives exact truncation without any
  per-label measurement. The clip is kept as cheap defense-in-depth behind the ellipsis.
- **Pin the label to the visible left edge vs. leave it at `x1 + 4`.** Pinning is included because the
  issue's headline scenario ("long-lived parent span once zoomed into a child") is exactly when
  `x1 < 0`, and without pinning the parent's label vanishes even though its bar fills the row — a
  visible regression relative to what users expect from a flamechart. The clip still bounds the label
  to the span's right edge, so pinning cannot reintroduce sibling overlap. The only behavior change is
  that a partially-scrolled-off span's label sticks to the left edge instead of scrolling with the bar
  — standard flamechart behavior.
- **`pre-wrap` vs. converting `\n` to explicit `<br>`.** `pre-wrap` handles both wrapping and embedded
  newlines with a single CSS property and no string manipulation of the (already HTML-escaped) name,
  so it is simpler and avoids a second escaping pass. The issue lists both options; `pre-wrap` is the
  lighter one.
- **Length-cap the name vs. rely solely on `max-height` + scroll.** The tooltip is
  `pointer-events-none`, so a scrollbar inside it is not usable; an uncapped name would either overflow
  (if we dropped `overflow: hidden`) or silently hide the metadata lines below it (with
  `overflow: hidden`). Capping the displayed name keeps the always-useful metadata (duration, id,
  file:line) visible while still showing a generous prefix of the name. `max-height`/`overflow:hidden`
  remain as a defense-in-depth bound.
- **Extract pure helpers vs. inline the math.** The existing test suite only covers pure functions;
  extracting `labelClipRect` / `truncateSpanName` / `clampTooltipPosition` makes the fix testable in
  the same style (no canvas/jsdom-DOM harness needed) and keeps the render/event callbacks readable.

## Documentation

No user-facing documentation changes. The flamechart cell's behavior for normal-length names is
unchanged; these are rendering-correctness fixes for the long/multi-line-name edge case. No
`mkdocs/` page documents flamechart label/tooltip rendering internals.

## Testing Strategy

- **`labelClipRect` unit tests** (pure): normal on-screen span (`x1=100, x2=300, width=800`) →
  `{left:102, width:196}`; off-screen-left span (`x1=-500, x2=200, width=800`) → left `0`, width
  `198` (right edge tracks `x2`, not the canvas); wider-than-canvas span (`x1=-100, x2=5000,
  width=800`) → `{left:0, width:800}` clamped to canvas but never beyond `x2`; degenerate span whose
  visible width is below the inset (`x1=0, x2=3, width=800`) → width `0`. These directly encode the
  two failure modes from issue #1305.
- **`fitLabelText` unit tests** (pure): a name that fits is returned unchanged; a too-long name is cut
  to `maxChars` glyphs with the last being `…` (e.g. `fitLabelText('abcdef', 30, 10)` → `'ab…'`);
  zero/negative available width → `''`; the `charWidth <= 0` guard returns the name unchanged (never
  divides by zero).
- **`truncateSpanName` unit tests** (pure): short name returned unchanged; name longer than `max`
  returns exactly `max` chars + `…`; embedded newlines within the cap are preserved.
- **`clampTooltipPosition` unit tests** (pure): cursor mid-container → below-right of cursor; cursor
  near right edge → box flips left and its right edge stays `<= containerW`; cursor near bottom edge →
  flips above; a box larger than the container clamps to `0` rather than going negative.
- **Manual verification** in the running web app (`./start_analytics_web.py`): render a flamechart
  with (1) a long-named root span, zoom into a deep child so the root's `x1 < 0`, and confirm the
  root label stays left-pinned and does not bleed over its right sibling; (2) hover a span whose name
  is a long multi-line string (LLM-prompt-shaped) and confirm the tooltip wraps within a bounded box,
  shows line breaks, keeps the metadata lines visible, and never runs off the right/bottom edge near
  the viewport corners.
- `yarn lint`, `yarn type-check`, `yarn test` from `analytics-web-app/`.

## Resolved Decisions

- **Canvas-label ellipsis: included.** `fitLabelText` truncates over-long names with `…` on top of the
  clip fix, at O(1) per label (one per-frame monospace character measurement).
- **Defaults confirmed:** tooltip name cap `truncateSpanName(..., 300)`, tooltip `maxWidth: 360px` /
  `maxHeight: 40vh`. These may still be nudged during manual verification if they read poorly against
  the default 400px cell height, but no design change hinges on them.
