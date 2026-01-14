# Screen Renderer Registry

## Goal

Refactor screen type rendering to follow the Open/Closed Principle. Adding a new screen type should only require creating a new renderer and registering it, without modifying `ScreenPage.tsx`.

## Current State

`ScreenPage.tsx` has:
- Conditional rendering branches for each screen type (lines ~650-715)
- Type-specific state in parent: `metricsScaleMode`, `sortField`, `sortDirection`
- Type-specific config handling: `metrics_options`, `handleScaleModeChange`
- Type-specific effects: sync scale mode from config

This violates OCP because adding a new screen type requires modifying `ScreenPageContent`.

## Proposed Architecture

### 1. Registry Structure

```
analytics-web-app/src/lib/screen-renderers/
  index.ts              # Registry and types
  ProcessListRenderer.tsx
  MetricsRenderer.tsx
  LogRenderer.tsx
```

### 2. Common Interface

```typescript
// index.ts
export interface ScreenRendererProps {
  // Config (opaque to parent, renderer interprets it)
  config: ScreenConfig
  onConfigChange: (config: ScreenConfig) => void

  // Unsaved changes tracking
  savedConfig: ScreenConfig | null  // null if new screen
  onUnsavedChange: () => void

  // Time range - parent owns URL state, renderers can navigate
  timeRange: { begin: string; end: string }
  onTimeRangeChange: (from: string, to: string) => void  // Updates URL, triggers re-render
}

export const SCREEN_RENDERERS: Record<string, React.ComponentType<ScreenRendererProps>> = {
  process_list: ProcessListRenderer,
  metrics: MetricsRenderer,
  log: LogRenderer,
}
```

**Key insight:** Each renderer handles its own data fetching. The parent no longer calls `useStreamQuery` - renderers that need SQL queries manage that internally. This allows:
- Single-query screens (Metrics, ProcessList, Log)
- Multi-query screens (future comparison views)
- No-query screens (future dashboards composing other screens)

### 3. State Management Strategy

Each renderer manages its own state internally. The parent only provides:
- `config` / `onConfigChange` for persistence
- `savedConfig` for unsaved changes detection

**Example: MetricsRenderer**
```typescript
interface MetricsConfig {
  sql: string
  scale_mode?: 'p99' | 'max'
}

function MetricsRenderer({ config, onConfigChange, savedConfig, onUnsavedChange, timeRange, onTimeRangeChange }: ScreenRendererProps) {
  // Parse config (renderer owns the schema)
  const metricsConfig = config as MetricsConfig

  // Own query execution
  const streamQuery = useStreamQuery()
  const table = streamQuery.getTable()

  // Internal state
  const [scaleMode, setScaleMode] = useState<ScaleMode>(metricsConfig.scale_mode ?? 'p99')
  const [isPanelOpen, setIsPanelOpen] = useState(true)

  // Execute query when config or time range changes
  useEffect(() => {
    if (metricsConfig.sql) {
      streamQuery.execute({
        sql: metricsConfig.sql,
        params: { begin: timeRange.begin, end: timeRange.end },
      })
    }
  }, [metricsConfig.sql, timeRange])

  // Handle scale mode change
  const handleScaleModeChange = useCallback((mode: ScaleMode) => {
    setScaleMode(mode)
    onConfigChange({ ...metricsConfig, scale_mode: mode })
    if ((savedConfig as MetricsConfig)?.scale_mode !== mode) {
      onUnsavedChange()
    }
  }, [metricsConfig, savedConfig, onConfigChange, onUnsavedChange])

  // Handle time range selection from chart drag
  const handleTimeRangeSelect = useCallback((from: Date, to: Date) => {
    onTimeRangeChange(from.toISOString(), to.toISOString())
  }, [onTimeRangeChange])

  // Renderer owns its full layout including config panel
  return (
    <ResizablePanelGroup direction="horizontal">
      <ResizablePanel>
        <TimeSeriesChart
          data={transformData(table)}
          scaleMode={scaleMode}
          onScaleModeChange={handleScaleModeChange}
          onTimeRangeSelect={handleTimeRangeSelect}
        />
      </ResizablePanel>

      {isPanelOpen && (
        <>
          <ResizableHandle />
          <ResizablePanel defaultSize={30}>
            <SqlEditor
              sql={metricsConfig.sql}
              onChange={(sql) => {
                onConfigChange({ ...metricsConfig, sql })
                onUnsavedChange()
              }}
            />
          </ResizablePanel>
        </>
      )}
    </ResizablePanelGroup>
  )
}
```

**Example: ProcessListRenderer**
```typescript
interface ProcessListConfig {
  sql: string
}

function ProcessListRenderer({ config, onConfigChange, onUnsavedChange, timeRange, ... }: ScreenRendererProps) {
  const processListConfig = config as ProcessListConfig

  // Own query execution
  const streamQuery = useStreamQuery()
  const table = streamQuery.getTable()

  // UI-only state (not persisted)
  const [sortField, setSortField] = useState<ProcessSortField>('last_update_time')
  const [sortDirection, setSortDirection] = useState<SortDirection>('desc')
  const [isPanelOpen, setIsPanelOpen] = useState(true)

  useEffect(() => {
    if (processListConfig.sql) {
      streamQuery.execute({
        sql: processListConfig.sql,
        params: { begin: timeRange.begin, end: timeRange.end },
      })
    }
  }, [processListConfig.sql, timeRange])

  // Renderer owns its full layout including config panel
  return (
    <ResizablePanelGroup direction="horizontal">
      <ResizablePanel>
        <ProcessListTable table={table} sortField={sortField} ... />
      </ResizablePanel>

      {isPanelOpen && (
        <>
          <ResizableHandle />
          <ResizablePanel defaultSize={30}>
            <SqlEditor
              sql={processListConfig.sql}
              onChange={(sql) => {
                onConfigChange({ ...processListConfig, sql })
                onUnsavedChange()
              }}
            />
          </ResizablePanel>
        </>
      )}
    </ResizablePanelGroup>
  )
}
```

**Example: DashboardRenderer (future - different panel type)**
```typescript
interface DashboardConfig {
  panels: Array<{ id: string; screen_ref: string; position: Position }>
  refresh_interval?: number
}

function DashboardRenderer({ config, onConfigChange, onUnsavedChange, ... }: ScreenRendererProps) {
  const dashboardConfig = config as DashboardConfig
  const [editingPanel, setEditingPanel] = useState<string | null>(null)

  // No SQL query - dashboards compose other screens
  // Renderer owns its full layout with a completely different config panel
  return (
    <ResizablePanelGroup direction="horizontal">
      <ResizablePanel>
        <DashboardGrid
          panels={dashboardConfig.panels}
          onEditPanel={setEditingPanel}
        />
      </ResizablePanel>

      {editingPanel && (
        <>
          <ResizableHandle />
          <ResizablePanel defaultSize={25}>
            <PanelConfigEditor
              panel={dashboardConfig.panels.find(p => p.id === editingPanel)}
              onChange={(updated) => {
                const newPanels = dashboardConfig.panels.map(p =>
                  p.id === editingPanel ? updated : p
                )
                onConfigChange({ ...dashboardConfig, panels: newPanels })
                onUnsavedChange()
              }}
            />
          </ResizablePanel>
        </>
      )}
    </ResizablePanelGroup>
  )
}
```

### 4. Time Range Navigation

Renderers can navigate through time by calling `onTimeRangeChange`. The parent owns the URL state and passes the current range down.

**Flow:**
1. Parent reads `begin`/`end` from URL search params
2. Parent passes `timeRange` and `onTimeRangeChange` to renderer
3. Renderer calls `onTimeRangeChange(from, to)` when user navigates (e.g., chart drag-to-zoom)
4. Parent updates URL → triggers re-render with new `timeRange`
5. Renderer's `useEffect` detects change → re-executes query

**Example: Chart drag-to-zoom**
```typescript
function MetricsRenderer({ timeRange, onTimeRangeChange, ... }: ScreenRendererProps) {
  // ...

  const handleTimeRangeSelect = useCallback((from: Date, to: Date) => {
    // This updates the URL, which triggers parent re-render with new timeRange
    onTimeRangeChange(from.toISOString(), to.toISOString())
  }, [onTimeRangeChange])

  return (
    <TimeSeriesChart
      onTimeRangeSelect={handleTimeRangeSelect}
      // Could also add onZoomOut, onPan, etc.
    />
  )
}
```

**Example: Process list with "jump to time" on row click**
```typescript
function ProcessListRenderer({ onTimeRangeChange, ... }: ScreenRendererProps) {
  const handleProcessClick = useCallback((process: Process) => {
    // Navigate to a time window around this process's activity
    const start = new Date(process.start_time)
    const end = new Date(process.last_update_time)
    onTimeRangeChange(start.toISOString(), end.toISOString())
  }, [onTimeRangeChange])

  return <ProcessListTable onRowClick={handleProcessClick} />
}
```

**Key points:**
- URL is the source of truth for time range (enables sharing links, browser back/forward)
- Renderers request changes, parent decides how to update URL
- Each renderer can expose different time navigation UX appropriate to its visualization

### 5. ScreenPage.tsx Changes

**Before:**
```typescript
// Query execution in parent
const streamQuery = useStreamQuery()
const table = streamQuery.getTable()

// 50+ lines of type-specific state and handlers
const [metricsScaleMode, setMetricsScaleMode] = useState(...)
const [sortField, setSortField] = useState(...)
// ...effects, handlers...

// Conditional rendering
{screenType === 'process_list' ? (
  <ProcessListTable table={table} ... />
) : screenType === 'metrics' ? (
  <MetricsView table={table} ... />
) : (
  <GenericTable table={table} ... />
)}
```

**After:**
```typescript
import { SCREEN_RENDERERS } from '@/lib/screen-renderers'

// No query execution in parent - each renderer handles its own
// No type-specific state in parent
// No layout/panel management - renderers own their full UI

const Renderer = SCREEN_RENDERERS[screenType]

// Unknown screen type is a bug - fail explicitly
if (!Renderer) {
  return <ErrorDisplay message={`Unknown screen type: ${screenType}`} />
}

return (
  <div className="flex flex-col h-full">
    <ScreenHeader
      name={name}
      hasUnsavedChanges={hasUnsavedChanges}
      onSave={handleSave}
    />
    <div className="flex-1 overflow-hidden">
      <Renderer
        config={config}
        onConfigChange={setConfig}
        savedConfig={screen?.config ?? null}
        onUnsavedChange={() => setHasUnsavedChanges(true)}
        timeRange={apiTimeRange}
        onTimeRangeChange={setTimeRange}
      />
    </div>
  </div>
)
```

**Parent responsibilities (minimal):**
- Load/save screen metadata (name, type, config)
- Track unsaved changes flag
- Provide time range from URL
- Render header with save button

**Renderer responsibilities (full ownership):**
- Parse and validate its config schema
- Execute queries (if any)
- Manage UI state (scale mode, sorting, etc.)
- Report config changes and unsaved status
- **Own full layout including any config/editor panels**

## Migration Steps

### Phase 1: Create Infrastructure
1. Create `src/lib/screen-renderers/index.ts` with types and empty registry

### Phase 2: Migrate ProcessListRenderer
1. Create `ProcessListRenderer.tsx`
2. Move `ProcessListTable` component and sorting state into it
3. Include `ResizablePanelGroup` layout with SQL editor panel
4. Register in `SCREEN_RENDERERS`
5. Remove process_list branch from ScreenPage.tsx

### Phase 3: Migrate MetricsRenderer
1. Create `MetricsRenderer.tsx`
2. Move `MetricsView`, scale mode state, and config sync into it
3. Include `ResizablePanelGroup` layout with SQL editor panel
4. Register in `SCREEN_RENDERERS`
5. Remove metrics branch and state from ScreenPage.tsx

### Phase 4: Migrate LogRenderer
1. Create `LogRenderer.tsx` (currently uses generic table)
2. Can add log-specific features later (syntax highlighting, level filtering)
3. Register in `SCREEN_RENDERERS`

### Phase 5: Cleanup
1. Remove all type-specific state from `ScreenPageContent`
2. Remove conditional rendering, use registry lookup
3. Remove `ResizablePanelGroup` and SQL editor from parent (now in renderers)
4. Delete unused imports

## Benefits

1. **OCP Compliance**: New screen types only require:
   - Creating `NewTypeRenderer.tsx`
   - Adding one line to `SCREEN_RENDERERS`

2. **Encapsulation**: Each renderer owns its state, config handling, and layout

3. **Testability**: Renderers can be unit tested in isolation

4. **Scalability**: Complex screen types don't bloat ScreenPage.tsx

5. **Flexible Panels**: Each renderer can have a completely different config panel:
   - SQL-based screens show a query editor
   - Dashboards show a panel configuration editor
   - Future screens can have no panel at all

## Config Structure

Config is entirely renderer-specific. The generic system makes no assumptions about structure.

```typescript
// screens-api.ts - generic, no assumptions
type ScreenConfig = Record<string, unknown>
```

Each renderer defines and validates its own config type:

```typescript
// MetricsRenderer
interface MetricsConfig {
  sql: string
  scale_mode?: 'p99' | 'max'
}

// ProcessListRenderer
interface ProcessListConfig {
  sql: string
}

// Future: MultiChartRenderer
interface MultiChartConfig {
  queries: Array<{ name: string; sql: string }>
  layout: 'grid' | 'tabs'
}

// Future: DashboardRenderer (no SQL at all)
interface DashboardConfig {
  panels: Array<{ screen_ref: string; position: Position }>
  refresh_interval?: number
}
```

**Benefits:**
- No assumptions about SQL (some screens may have none, some may have many)
- Each renderer owns its config schema
- Adding new screen types doesn't touch shared interfaces
- Backend `default_config()` already returns type-appropriate JSON

## Files to Create/Modify

| File | Action |
|------|--------|
| `src/lib/screen-renderers/index.ts` | Create - registry and types |
| `src/lib/screen-renderers/ProcessListRenderer.tsx` | Create - move from ScreenPage |
| `src/lib/screen-renderers/MetricsRenderer.tsx` | Create - move from ScreenPage |
| `src/lib/screen-renderers/LogRenderer.tsx` | Create - basic implementation |
| `src/routes/ScreenPage.tsx` | Modify - use registry, remove type-specific code |
| `src/lib/screens-api.ts` | Modify - make ScreenConfig generic (`Record<string, unknown>`) |
