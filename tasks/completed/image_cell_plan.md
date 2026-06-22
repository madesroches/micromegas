# Image Cell Plan

## Overview

Add an `image` notebook cell type that queries the `images` lakehouse view and displays the results as a navigable carousel. Each row in the query result is one frame: a timestamp, a name/label, a format string, and raw binary image data. The carousel lets the notebook designer (or analyst) step through screenshots captured from an instrumented process.

## Current State

The backend already has full image support:
- **Schema**: `rust/analytics/src/images_table.rs` — 12-column Arrow schema; key columns are `time` (Timestamp ns UTC), `name` (Utf8), `format` (Dictionary Utf8, e.g. `"png"`), `data` (Binary)
- **View**: `rust/analytics/src/lakehouse/images_view.rs` — JIT materialized view, per-process, queried via `view_instance('images', process_id)`
- **Event source**: `rust/tracing/src/images/image_events.rs` — `ImageEvent { time, name, format, data }`

The web app has 13 cell types. Adding a new one requires changes to three locations:
1. `notebook-types.ts:98` — `CellType` union and config interfaces
2. `cell-registry.ts:188` — `CELL_TYPE_METADATA` map
3. New file `cells/ImageCell.tsx` — renderer, editor, and metadata export

Existing binary data handling: `arrow-utils.ts` exports `isBinaryType()`. `table-utils.tsx:781-783` renders binary fields as hex previews. Blob URL creation already exists in `cells/arrow-to-csv.ts:19-20` for CSV downloads.

## Design

### Config type

Add to `notebook-types.ts`:

```typescript
// CellType union — add 'image'
export type CellType = 'table' | 'chart' | ... | 'map' | 'image'

export interface ImageCellConfig extends CellConfigBase {
  type: 'image'
  sql: string
  dataSource?: string
}

// CellConfig union — add ImageCellConfig
export type CellConfig = QueryCellConfig | ... | ImageCellConfig
```

`ImageCellConfig` does not carry `options` — the column contract is fixed by the schema (`time`, `name`, `format`, `data`), so there is nothing to configure.

### Query contract

The cell expects the query to return rows with these columns (by name):

| Column | Arrow type | Role |
|--------|-----------|------|
| `time` | Timestamp(ns, UTC) | Sort order; display in carousel |
| `name` | Utf8 | Label below image |
| `format` | Utf8 or Dictionary(Utf8) | MIME subtype: `image/${format}` |
| `data` | Binary or LargeBinary | Raw image bytes |

Rows are displayed in the order returned. The designer is responsible for `ORDER BY time` in their SQL. The cell shows an error state if any required column is missing from the schema.

### Carousel renderer

State: `currentIndex: number` (React `useState`, starts at 0, resets to 0 on data change).

For the current index:
1. Extract `data` field → `Uint8Array`
2. Read `format` field → e.g. `"png"` → MIME type `image/png`
3. `new Blob([bytes], { type: mimeType })` → `URL.createObjectURL(blob)` → `src` of `<img>`
4. Revoke the previous blob URL before creating the new one (store in a `useRef`)
5. Revoke on unmount

Only one blob URL exists at a time. No preloading.

UI layout (top-to-bottom within the cell):
```
┌─────────────────────────────────────────┐
│  ← [prev]     3 / 12     [next] →      │
│                                         │
│         [image, object-fit:contain]     │
│         max-height: fills cell          │
│                                         │
│    name · 2025-06-22 14:32:01.123 UTC  │
└─────────────────────────────────────────┘
```

- Prev/Next buttons disabled at boundaries
- Counter: `{currentIndex + 1} / {rowCount}`
- Label: `name · formatted_time` — use the existing timestamp formatting utility already used by log and property timeline cells
- Empty state: "No images" placeholder when `data` is empty or cell hasn't run
- Missing-column error state: list which expected columns are absent

### Editor component

Follows `LogCellEditor` exactly: SQL editor (`<SyntaxEditor>`) + variables/macros panel. No cell-specific options fields — the SQL is the only knob.

Default SQL in `createDefaultConfig`:
```sql
SELECT time, name, format, data
FROM view_instance('images', '$process_id')
ORDER BY time
```

### Metadata

```typescript
export const imageMetadata: CellTypeMetadata = {
  renderer: ImageCell,
  EditorComponent: ImageCellEditor,
  label: 'Image',
  icon: <ImageIcon />,           // lucide-react `Image` icon
  description: 'Screenshot carousel from image stream',
  showTypeBadge: true,
  defaultHeight: 500,
  canBlockDownstream: false,     // carousel selection not published upstream

  createDefaultConfig: () => ({ type: 'image' as const, sql: DEFAULT_SQL }),

  execute: async (config, { variables, timeRange, runQuery }) => {
    const sql = substituteMacros((config as ImageCellConfig).sql, variables, timeRange)
    return { data: [await runQuery(sql)] }
  },

  getRendererProps: (config, state) => ({ data: state.data, status: state.status }),
}
```

## Implementation Steps

1. **`notebook-types.ts`** — add `'image'` to `CellType` (line 98); add `ImageCellConfig` interface after `HorizontalGroupCellConfig`; add `ImageCellConfig` to the `CellConfig` union (line 149)

2. **`cells/ImageCell.tsx`** (new file) — implement:
   - `ImageCell` renderer component with carousel state and blob URL lifecycle
   - `ImageCellEditor` editor component (SQL editor + macros panel, copy from LogCellEditor)
   - `imageMetadata` export

3. **`cell-registry.ts`** — import `imageMetadata` from `./cells/ImageCell`; add `image: imageMetadata` to `CELL_TYPE_METADATA` (line 188 block)

## Files to Modify

| File | Change |
|------|--------|
| `analytics-web-app/src/lib/screen-renderers/notebook-types.ts` | Add `'image'` to `CellType`; add `ImageCellConfig`; update `CellConfig` union |
| `analytics-web-app/src/lib/screen-renderers/cell-registry.ts` | Import and register `imageMetadata` |
| `analytics-web-app/src/lib/screen-renderers/cells/ImageCell.tsx` | New file |

## Trade-offs

**Fixed column names vs. configurable**: Configuring which column is binary would add flexibility but also complexity and a confusing editor UX. The images view has a well-defined schema; fixing the names keeps the cell simple and the default SQL self-documenting.

**No preloading**: Preloading adjacent images would improve navigation smoothness but multiplies memory use proportionally to the prefetch window. Deferred until there is a concrete complaint.

**`canBlockDownstream: false`**: The cell could publish the selected image's `name` or `time` as a selection, enabling downstream cells to filter by it. Excluded from this plan — adds complexity, unclear use case for now.

## Testing Strategy

- Run the app with monolith mode and a process that has image events; create a notebook cell with the default SQL and verify the carousel renders, navigates, and displays timestamps/labels correctly
- Navigate past boundaries and verify buttons are disabled
- Run a query with no rows and verify the empty state renders without errors
- Run a query missing the `data` column and verify the error state is shown
- Add and remove the cell rapidly to verify blob URLs are revoked (check DevTools Memory tab for `blob:` leaks)
- Run `yarn type-check` and `yarn lint` after implementation

## Open Questions

None — schema is known, pattern is established, scope is clear.
