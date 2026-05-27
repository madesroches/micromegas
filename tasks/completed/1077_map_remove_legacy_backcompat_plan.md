# Map Cell: Remove Unused Legacy Back-Compat Plan

## Issue Reference
- [#1077](https://github.com/madesroches/micromegas/issues/1077) — Map cell: remove unused back-compat for legacy `mapUrl` prefix and `markerColor`/`markerSize`

## Overview

The Map cell carries two back-compat paths for option shapes that never shipped
to production — the feature is brand new, so no saved notebook can contain them:

1. **`normalizeMapFilename`** strips a leading `/maps/` from a stored `mapUrl`,
   a remnant of a static-files-era URL scheme that was never persisted.
2. **`markerColor` / `markerSize`** scalar fallbacks in `resolveMapping` —
   the editor only ever writes the new `mapping` shape, never these keys.

Both add maintenance surface for nothing. This plan removes them and their
tests, and updates the comments that reference them. The cell-types doc already
omits these fallbacks, so the code is the only remaining gap.

## Current State

### Legacy `/maps/` prefix — `analytics-web-app/src/lib/maps-catalog.ts`

`normalizeMapFilename` (lines 45–53) strips the prefix; its only two callers are
`resolveMapBlobUrl` (lines 55–60) and `MapDropdown`:

```ts
export function normalizeMapFilename(raw: string | undefined | null): string | undefined {
  if (!raw) return undefined
  return raw.startsWith('/maps/') ? raw.slice('/maps/'.length) : raw
}

export function resolveMapBlobUrl(file: string | undefined, basePath: string): string | undefined {
  const filename = normalizeMapFilename(file)
  if (!filename) return undefined
  return `${basePath}/api/maps/blob/${filename}`
}
```

`normalizeMapFilename` does two things: the prefix strip (dead) and the
empty/null → `undefined` coercion (still wanted). Removing it means folding the
falsy check into each caller.

### Legacy marker scalars — `analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`

- Import at line 40 brings in `normalizeMapFilename`.
- `MapDropdown` at line 81: `const selectedFilename = normalizeMapFilename(selectedRaw)`.
  Used as `!!selectedFilename`, `value={selectedFilename ?? ''}`, and
  `selectedFilename && !isInCatalog`. With `selectedRaw` used directly, an empty
  string is falsy in all three sites — behavior is identical.
- `resolveMapping` at lines 128–129 reads `options.markerColor` / `options.markerSize`,
  and lines 155–161 apply them as fallbacks when the corresponding `mapping`
  channel is unset.
- The `resolvedMappingResult` memo lists `options?.markerColor` and
  `options?.markerSize` in its deps (lines 253–254) — a shadow of the removed path.
- Explanatory comments reference the legacy path at lines 113–119 (the
  `resolveMapping` header), 303–306 (the `mapUrl` / `/maps/` transition note),
  and 729–733 (editor never re-reads the marker keys).

### Builder comment — `analytics-web-app/src/components/map/overlay.ts:156-157`

`defaultMappingFor`'s doc comment says back-compat "with legacy
markerSize/markerColor is the cell's job, not the builder's." Once the cell drops
that job, the comment is stale.

### Tests

- `analytics-web-app/src/lib/__tests__/maps-catalog.test.ts`:
  - `normalizeMapFilename` describe block (lines 13–27) and the import (line 3).
  - `resolveMapBlobUrl` → "strips a legacy /maps/ prefix before composing"
    (lines 34–36).
  - The remaining `resolveMapBlobUrl` cases (bare filename, empty input, empty
    base path) stay and still pass against the inlined check.
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/MapCell.test.tsx`:
  the three tests mentioning "legacy" (lines 292, 594, 605) exercise the
  **canonical** `{ scalar: <number> }` binding shape — numeric-vs-string scalar
  rendering — **not** the `markerColor`/`markerSize` back-compat. They must be
  **kept**. There are no tests exercising the marker-scalar fallback path, so
  nothing in this file is deleted.

### Verification (already run during research)

- `git grep -n "markerColor\|markerSize"` in the web-app: only the read sites in
  `MapCell.tsx`, the comments above, and `overlay.ts`. No writer anywhere.
- `git grep -n "/maps/"` in `analytics-web-app/src/`: only the prefix-strip code,
  its tests, and `/api/maps/...` route strings (unrelated). No saved-config
  dependency on the prefix.
- Docs: `mkdocs/docs/web-app/notebooks/cell-types.md:241` describes `mapUrl` as a
  bare filename and never mentions the marker scalars — no doc edits needed.

## Design

Pure deletion plus inlining the still-needed falsy guard. No new abstractions.

**`maps-catalog.ts`** — delete `normalizeMapFilename`; inline its empty-check into
`resolveMapBlobUrl`:

```ts
export function resolveMapBlobUrl(file: string | undefined, basePath: string): string | undefined {
  if (!file) return undefined
  return `${basePath}/api/maps/blob/${file}`
}
```

**`MapCell.tsx`** —
- Drop `normalizeMapFilename` from the import (line 40).
- `MapDropdown`: use `selectedRaw` directly. Either rename the local to
  `const selectedFilename = selectedRaw` (smallest diff, keeps downstream JSX
  untouched) or replace the three usages with `selectedRaw`.
- `resolveMapping`: delete `legacyColor`/`legacySize` (128–129) and the two
  fallback `if` blocks (155–161). The function returns
  `{ shape, mapping: { ...defaults, ...filtered } }`.
- Remove `options?.markerColor` / `options?.markerSize` from the
  `resolvedMappingResult` deps (253–254).
- Update the three comments (113–119, 303–306, 729–733) to drop legacy mentions
  while preserving the still-true explanations (shape filtering, bare-filename
  storage, editor writes the `mapping` shape from first touch).

**`overlay.ts`** — reword the `defaultMappingFor` comment to drop the
marker-scalar reference.

## Implementation Steps

1. **`maps-catalog.ts`**: delete `normalizeMapFilename`; inline the falsy guard
   into `resolveMapBlobUrl`.
2. **`MapCell.tsx`**: remove the import; switch `MapDropdown` to `selectedRaw`;
   strip the legacy scalars and fallback blocks from `resolveMapping`; drop the
   two memo deps; update the three comments.
3. **`overlay.ts`**: reword the `defaultMappingFor` doc comment.
4. **`maps-catalog.test.ts`**: remove the `normalizeMapFilename` describe block,
   its import, and the `resolveMapBlobUrl` "/maps/" case. Confirm the surviving
   `resolveMapBlobUrl` cases still pass.
5. **Verify** `MapCell.test.tsx` is untouched (the "legacy numeric scalar" tests
   stay) and that `markerColor`/`markerSize`/`/maps/`-prefix appear nowhere in
   `analytics-web-app/src` afterward.
6. **Run** from `analytics-web-app/`: `yarn lint`, `yarn type-check`, `yarn test`.

## Files to Modify

- `analytics-web-app/src/lib/maps-catalog.ts`
- `analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`
- `analytics-web-app/src/components/map/overlay.ts`
- `analytics-web-app/src/lib/__tests__/maps-catalog.test.ts`

## Trade-offs

- **Inline the falsy guard vs. keep a thinner helper.** Inlining into the single
  remaining caller is simplest and removes the now-misnamed function entirely. A
  renamed helper would just wrap a one-line check used in two spots — not worth a
  named export.
- **Pure removal, no deprecation shim.** Safe because the feature is new and the
  greps prove no persisted config carries these shapes. A migration shim would
  reintroduce the surface this issue exists to remove.

## Documentation

None. `cell-types.md` already omits the legacy fallbacks; this change closes the
code-vs-doc gap rather than opening a new one.

## Testing Strategy

- `yarn test` — the trimmed `maps-catalog.test.ts` plus the unchanged
  `MapCell.test.tsx` cover the remaining behavior.
- `yarn type-check` confirms no dangling reference to the removed export.
- Optional manual smoke (`yarn dev`): open a Map cell, pick a map from the
  dropdown, confirm it loads and the marker color/size editors still drive the
  overlay via the `mapping` shape.

## Open Questions

None — the issue is self-contained and the verification greps confirm the paths
are dead.
