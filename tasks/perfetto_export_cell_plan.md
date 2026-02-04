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
  processIdVar?: string    // Variable name holding process_id (default: "$process_id")
  spanType?: 'thread' | 'async' | 'both'  // Default: 'both'
}

// Update CellConfig union (line 126)
export type CellConfig = QueryCellConfig | MarkdownCellConfig | VariableCellConfig | PerfettoExportCellConfig
```

## spanType to API Mapping

The `spanType` config option maps to `GenerateTraceRequest` fields:

| spanType | include_thread_spans | include_async_spans |
|----------|---------------------|---------------------|
| `'thread'` | `true` | `false` |
| `'async'` | `false` | `true` |
| `'both'` | `true` | `true` |

Helper function in the cell:

```typescript
function getTraceRequest(
  spanType: 'thread' | 'async' | 'both',
  timeRange: { begin: string; end: string }
): GenerateTraceRequest {
  return {
    include_thread_spans: spanType !== 'async',
    include_async_spans: spanType !== 'thread',
    time_range: timeRange,
  }
}
```

## Component Design

The cell renders:
- SplitButton with "Open in Perfetto" (primary) and "Download" (secondary)
- Progress indicator during generation
- Error state with retry

Gets `process_id` from notebook variables (using `getVariableString()`) and `timeRange` from props.

## Local State Management

Based on `PerformanceAnalysisPage.tsx` pattern (lines 166-172):

```typescript
// Generation state
const [isGenerating, setIsGenerating] = useState(false)
const [traceMode, setTraceMode] = useState<'perfetto' | 'download' | null>(null)
const [progress, setProgress] = useState<ProgressUpdate | null>(null)
const [traceError, setTraceError] = useState<string | null>(null)

// Cache to avoid regenerating on repeated clicks
const [cachedTraceBuffer, setCachedTraceBuffer] = useState<ArrayBuffer | null>(null)
const [cachedTraceTimeRange, setCachedTraceTimeRange] = useState<{ begin: string; end: string } | null>(null)
```

### Cache Validation

```typescript
const canUseCachedBuffer = useCallback(() => {
  if (!cachedTraceBuffer || !cachedTraceTimeRange) return false
  return cachedTraceTimeRange.begin === timeRange.begin &&
         cachedTraceTimeRange.end === timeRange.end
}, [cachedTraceBuffer, cachedTraceTimeRange, timeRange])
```

### Action Handlers

**Open in Perfetto:**
1. If cache valid → call `openInPerfetto()` with cached buffer
2. Else → call `generateTrace()` with `returnBuffer: true`, cache result, then `openInPerfetto()`

**Download:**
1. If cache valid → create Blob from cached buffer, trigger download
2. Else → call `generateTrace()` without `returnBuffer` (triggers download automatically)

### State Transitions

```
Idle → [click] → Generating (isGenerating=true, traceMode='perfetto'|'download')
  → [progress] → Update progress message
  → [success] → Idle (cache buffer if perfetto mode)
  → [error] → Error state (traceError set, buttons enabled for retry)
```

## Validation

The cell must validate that the referenced variable exists and show clear errors.

### Renderer Validation

```typescript
// In PerfettoExportCell renderer
const processIdVar = options?.processIdVar ?? '$process_id'

// Strip $ prefix to get variable name for lookup
const varName = processIdVar.startsWith('$') ? processIdVar.slice(1) : processIdVar
const processId = variables[varName]
const hasProcessId = processId !== undefined && getVariableString(processId) !== ''

// Show warning if variable not found
{!hasProcessId && (
  <div className="flex items-center gap-2 px-3 py-2 bg-amber-500/10 border border-amber-500/30 rounded-md mb-3">
    <AlertTriangle className="w-4 h-4 text-amber-500" />
    <span className="text-sm text-amber-500">
      Variable "{processIdVar}" not found. Add a Variable cell above.
    </span>
  </div>
)}

// Disable buttons when no process ID
<SplitButton
  disabled={isGenerating || !hasProcessId}
  ...
/>
```

### Editor Validation

```typescript
// In PerfettoExportCellEditor
const varName = (perfConfig.processIdVar || '$process_id').replace(/^\$/, '')
const varExists = varName in variables

// Show error below the input
{!varExists && perfConfig.processIdVar && (
  <div className="text-red-400 text-sm mt-1">
    ⚠ Variable "{perfConfig.processIdVar}" not found
  </div>
)}
```

## Editor Component

The editor allows configuring which variable holds the process ID and which span types to include.

```typescript
function PerfettoExportCellEditor({ config, onChange, variables }: CellEditorProps) {
  const perfConfig = config as PerfettoExportCellConfig

  // Get list of available variable names for the dropdown
  const availableVars = Object.keys(variables)

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Process ID Variable
        </label>
        <input
          type="text"
          value={perfConfig.processIdVar || '$process_id'}
          onChange={(e) => onChange({ ...perfConfig, processIdVar: e.target.value })}
          className="w-full px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
          placeholder="$process_id"
        />
        <p className="text-xs text-theme-text-muted mt-1">
          Name of the variable containing the process ID
        </p>
        {availableVars.length > 0 && (
          <p className="text-xs text-theme-text-muted mt-1">
            Available: {availableVars.join(', ')}
          </p>
        )}
      </div>

      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Span Type
        </label>
        <select
          value={perfConfig.spanType || 'both'}
          onChange={(e) =>
            onChange({ ...perfConfig, spanType: e.target.value as 'thread' | 'async' | 'both' })
          }
          className="w-full px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
        >
          <option value="both">Both (Thread + Async)</option>
          <option value="thread">Thread Spans Only</option>
          <option value="async">Async Spans Only</option>
        </select>
        <p className="text-xs text-theme-text-muted mt-1">
          Which span types to include in the trace
        </p>
      </div>
    </>
  )
}
```

### Editor Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| Process ID Variable | text input | `"$process_id"` | Variable name to read process ID from |
| Span Type | dropdown | `"both"` | Thread, Async, or Both |

## Cell Metadata

```typescript
export const perfettoExportMetadata: CellTypeMetadata = {
  renderer: PerfettoExportCell,
  EditorComponent: PerfettoExportCellEditor,
  label: 'Perfetto Export',
  icon: 'E',
  description: 'Export spans to Perfetto trace viewer',
  showTypeBadge: true,
  defaultHeight: 80,
  canBlockDownstream: false,  // No data output for other cells
  createDefaultConfig: () => ({
    type: 'perfettoexport' as const,
    processIdVar: '$process_id',
    spanType: 'both',
  }),
  // No execute method - action is user-triggered via button
  getRendererProps: (config: CellConfig, state: CellState) => {
    const perfConfig = config as PerfettoExportCellConfig
    return {
      status: state.status,
      options: {
        processIdVar: perfConfig.processIdVar ?? '$process_id',
        spanType: perfConfig.spanType ?? 'both',
      },
    }
  },
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
