# Perfetto Export Cell - Implementation Plan

Issue: https://github.com/madesroches/micromegas/issues/764

## Overview

Add a notebook cell type that generates Perfetto traces with "Open in Perfetto" and "Download" actions, leveraging the existing implementation from the Performance Analysis screen.

## Files to Create

| File | Purpose |
|------|---------|
| `analytics-web-app/src/lib/screen-renderers/cells/PerfettoExportCell.tsx` | Cell renderer and metadata |

## Files to Modify

| File | Change |
|------|--------|
| `notebook-types.ts` | Add `'perfettoexport'` to `CellType` union, add `PerfettoExportCellConfig` interface, update `CellConfig` union |
| `cell-registry.ts` | Import and register `perfettoExportMetadata` |

## Cell Schema

Add to `notebook-types.ts`:

```typescript
// Add to CellType union (line 98)
export type CellType = 'table' | 'chart' | 'log' | 'markdown' | 'variable' | 'propertytimeline' | 'swimlane' | 'perfettoexport'

// Add new interface (after VariableCellConfig)
export interface PerfettoExportCellConfig extends CellConfigBase {
  type: 'perfettoexport'
  processIdVar?: string    // Variable name holding process_id (default: "process_id")
  spanType?: 'thread' | 'async' | 'both'  // Default: 'both'
}

// Update CellConfig union (line 126)
export type CellConfig = QueryCellConfig | MarkdownCellConfig | VariableCellConfig | PerfettoExportCellConfig
```

## Component Design

The cell renders:
- SplitButton with "Open in Perfetto" (primary) and "Download" (secondary)
- Progress indicator during generation
- Error state with retry

Gets `process_id` from notebook variables and `timeRange` from context.

## Cell Metadata

```typescript
export const perfettoExportMetadata: CellTypeMetadata = {
  renderer: PerfettoExportCell,
  EditorComponent: PerfettoExportCellEditor,
  label: 'Perfetto Export',
  icon: 'P',
  description: 'Export spans to Perfetto trace viewer',
  showTypeBadge: true,
  defaultHeight: 80,
  canBlockDownstream: false,  // No data output for other cells
  createDefaultConfig: () => ({
    type: 'perfettoexport' as const,
    processIdVar: 'process_id',
    spanType: 'both',
  }),
  // No execute method - action is user-triggered via button
  getRendererProps: (config, state) => ({
    status: state.status,
  }),
}
```

## Implementation Steps

1. **Update notebook-types.ts**:
   - Add `'perfettoexport'` to `CellType` union
   - Add `PerfettoExportCellConfig` interface
   - Add `PerfettoExportCellConfig` to `CellConfig` union
2. **Create cell component** - `PerfettoExportCell.tsx` with:
   - Renderer: SplitButton + progress UI
   - Editor: Process variable selector + span type dropdown
   - Metadata object (no `execute` method - action is user-triggered via button)
   - Set `canBlockDownstream: false` (doesn't produce data for other cells)
3. **Register cell** - Import and add to `CELL_TYPE_METADATA` in `cell-registry.ts`
4. **Test** - Verify in a notebook with a process variable

## Key Reuse

- `SplitButton` component from `components/ui/`
- `generateTrace()` from `lib/api.ts`
- `openInPerfetto()` from `lib/perfetto.ts`
- Progress/error handling pattern from `PerformanceAnalysisPage.tsx`
