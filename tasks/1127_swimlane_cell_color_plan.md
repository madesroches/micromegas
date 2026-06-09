# Swimlane Cell Color Support Plan

Issue: [#1127](https://github.com/madesroches/micromegas/issues/1127)

## Overview

Swimlane cells render every bar in the same hardcoded `bg-chart-line` color.
This plan adds an optional `color` column to the swimlane query contract so
segments can be individually colored, matching the convention already in use
by chart and map cells.

## Current State

`analytics-web-app/src/lib/screen-renderers/cells/SwimlaneCell.tsx`

- **`Segment` type** (line 33): `{ begin: number; end: number; label?: string }` — no color field.
- **`extractLanesFromTable`** (line 57): reads `id`, `name`, `begin`, `end`, `label` columns; `label` is optional. Color column is not read.
- **Segment rendering** (line 326): every segment gets `className="... bg-chart-line ..."` with no per-segment style override.
- **Default SQL** (`notebook-utils.ts:154`): does not include a `color` column.

Existing color infrastructure the implementation will reuse:

- `src/lib/color-utils.ts` — `cellColorToCss(value, kind)` decodes integer
  (packed RGBA u32), string (`#rrggbb`/`#rrggbbaa`), or binary color cells to a
  CSS color string. Returns `null` for invalid strings; callers fall back to the
  default.
- `src/lib/arrow-utils.ts` — `isIntegerType`, `isStringType`, `isBinaryType`,
  `unwrapDictionary` for detecting the Arrow column kind.
- The chart cell (`XYChart.tsx`) and map cell (`overlay.ts`) both follow the
  same pattern: detect the column kind once at data-extraction time, then call
  `cellColorToCss` per row.

## Design

### Data model changes

Add an optional `color` field to `Segment`:

```ts
interface Segment {
  begin: number
  end: number
  label?: string
  color?: string  // CSS color string, e.g. '#bf360cff'
}
```

No change to `Lane`.

### Column extraction in `extractLanesFromTable`

1. After resolving `labelCol`, detect the optional `color` column:
   - Look up the column: `const colorCol = table.getChild('color') ?? null`
   - If present, detect its kind using `unwrapDictionary` + `isIntegerType` /
     `isStringType` / `isBinaryType` (same pattern as `arrow-utils.ts:231-238`).
   - Reject unsupported types with a clear error message (matching the chart
     cell's wording).
2. Per row, if `colorCol` is present call `cellColorToCss(colorCol.get(i), colorColumnKind)`.
   If the result is non-null, store it on the segment; otherwise omit (fall back
   to default at render time).

### Rendering change

In the segment `<div>` (line 325–328 of `SwimlaneCell.tsx`), replace the
hard-coded `bg-chart-line` class with an inline `backgroundColor` style when a
per-segment color is available:

```tsx
<div
  className="absolute top-1 bottom-1 rounded-sm flex items-center overflow-hidden transition-opacity hover:opacity-85 hover:ring-1 hover:ring-brand-gold"
  style={{
    left: `${startPercent}%`,
    width: `${Math.max(widthPercent, 0.5)}%`,
    backgroundColor: segment.color ?? 'var(--chart-line)',
  }}
  ...
>
```

Using a CSS variable as the fallback keeps the hardcoded default behavior
unchanged when no `color` column is present.

### Editor placeholder update

Update `SwimlaneCellEditor`'s SQL placeholder (line 479) from
`"SELECT id, name, begin, end [, label] FROM ..."` to
`"SELECT id, name, begin, end [, label] [, color] FROM ..."`.

### Default SQL

No change to `DEFAULT_SQL.swimlane` — it omits the optional columns and is
already correct as a minimal example.

## Implementation Steps

1. **Export `extractLanesFromTable`** by adding the `export` keyword to its function declaration in `SwimlaneCell.tsx`.
2. **Add `color?: string` to `Segment`** in `SwimlaneCell.tsx`.
3. **Detect the color column kind** in `extractLanesFromTable`:
   - Import `isIntegerType`, `isStringType`, `isBinaryType`, `unwrapDictionary`
     from `@/lib/arrow-utils`.
   - Import `cellColorToCss` from `@/lib/color-utils`.
   - After `labelCol` lookup, add `colorCol` lookup + kind detection + unsupported-type
     error return (matching the pattern from `arrow-utils.ts:229-244`).
4. **Extract per-row color** in the row loop, alongside `label`, calling
   `cellColorToCss` and storing the result on the `Segment`.
5. **Update the segment renderer** to use `backgroundColor: segment.color ?? 'var(--chart-line)'`
   instead of the `bg-chart-line` Tailwind class.
6. **Update the SQL placeholder** in `SwimlaneCellEditor`.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/cells/SwimlaneCell.tsx` — all changes.

## Trade-offs

- **CSS variable fallback vs. hardcoded hex.** `var(--chart-line)` keeps the
  default color theme-aware (dark/light mode), matching the current Tailwind
  class behavior. A hardcoded hex would break theming.
- **Inline style vs. Tailwind class.** Per-segment colors must be inline
  (Tailwind can't generate arbitrary dynamic classes at runtime). The static
  structural classes (`rounded-sm`, hover ring, etc.) stay as Tailwind classes.
- **Reject unsupported column types.** Consistent with the map and chart cells:
  an integer/string/binary column that can't be decoded as color should produce
  a schema error rather than silently using the default, so authors notice the
  issue. Invalid string values per-row produce `null` from `cellColorToCss` and
  fall back silently — same policy as chart/map.
- **No per-lane color.** The issue specifies per-segment color via a query
  column. A per-lane color would require schema changes to the `Lane` type and
  a second column convention; out of scope here.

## Testing Strategy

- Add a test in `SwimlaneCell` (or a co-located `__tests__/SwimlaneCell.test.tsx`
  if one is created) that calls `extractLanesFromTable` with a mock Arrow table
  containing an integer `color` column, and asserts the resulting segment has
  the expected CSS color string.
- Add a test for the `null` fallback (absent `color` column → `segment.color`
  is `undefined`).
- Add a test for the unsupported-type error path.
- Manual smoke test: write a swimlane query that returns `rgba(1,0,0,1) as color`
  and verify bars render red.

## Open Questions

None — the design follows established patterns from chart and map cells.
