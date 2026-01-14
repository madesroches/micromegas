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
  GenericTableRenderer.tsx  # Fallback
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

  // Time range from URL (renderers that need it can use it)
  timeRange: { begin: string; end: string }
  onTimeRangeChange: (from: string, to: string) => void
}

export const SCREEN_RENDERERS: Record<string, React.ComponentType<ScreenRendererProps>> = {
  process_list: ProcessListRenderer,
  metrics: MetricsRenderer,
  log: LogRenderer,
}

export const DEFAULT_RENDERER = GenericTableRenderer
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

  return (
    <TimeSeriesChart
      data={transformData(table)}
      scaleMode={scaleMode}
      onScaleModeChange={handleScaleModeChange}
      onTimeRangeSelect={handleTimeRangeSelect}
    />
  )
}
```

**Example: ProcessListRenderer**
```typescript
interface ProcessListConfig {
  sql: string
}

function ProcessListRenderer({ config, timeRange, ... }: ScreenRendererProps) {
  const processListConfig = config as ProcessListConfig

  // Own query execution
  const streamQuery = useStreamQuery()
  const table = streamQuery.getTable()

  // UI-only state (not persisted)
  const [sortField, setSortField] = useState<ProcessSortField>('last_update_time')
  const [sortDirection, setSortDirection] = useState<SortDirection>('desc')

  useEffect(() => {
    if (processListConfig.sql) {
      streamQuery.execute({
        sql: processListConfig.sql,
        params: { begin: timeRange.begin, end: timeRange.end },
      })
    }
  }, [processListConfig.sql, timeRange])

  return <ProcessListTable table={table} sortField={sortField} ... />
}
```

### 4. ScreenPage.tsx Changes

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
import { SCREEN_RENDERERS, DEFAULT_RENDERER } from '@/lib/screen-renderers'

// No query execution in parent - each renderer handles its own
// No type-specific state in parent

const Renderer = SCREEN_RENDERERS[screenType] ?? DEFAULT_RENDERER

return (
  <Renderer
    config={config}
    onConfigChange={setConfig}
    savedConfig={screen?.config ?? null}
    onUnsavedChange={() => setHasUnsavedChanges(true)}
    timeRange={apiTimeRange}
    onTimeRangeChange={setTimeRange}
  />
)
```

**Parent responsibilities (simplified):**
- Load/save screen metadata (name, type, config)
- Track unsaved changes flag
- Provide time range from URL
- Render SQL editor panel (only if `config.sql` exists)

**Renderer responsibilities:**
- Parse and validate its config schema
- Execute queries (if any)
- Manage UI state (scale mode, sorting, etc.)
- Report config changes and unsaved status

### 5. SQL Editor Panel

The SQL editor panel in ScreenPage.tsx should only render if the config has a `sql` field:

```typescript
const hasSql = typeof config?.sql === 'string'

const sqlPanel = hasSql ? (
  <QueryEditor
    sql={config.sql}
    onChange={(sql) => onConfigChange({ ...config, sql })}
    ...
  />
) : null
```

Future screens without SQL (e.g., dashboards) won't show the editor panel.

## Migration Steps

### Phase 1: Create Infrastructure
1. Create `src/lib/screen-renderers/index.ts` with types and empty registry
2. Create `GenericTableRenderer.tsx` (extract from current ScreenPage.tsx)

### Phase 2: Migrate ProcessListRenderer
1. Create `ProcessListRenderer.tsx`
2. Move `ProcessListTable` component and sorting state into it
3. Register in `SCREEN_RENDERERS`
4. Remove process_list branch from ScreenPage.tsx

### Phase 3: Migrate MetricsRenderer
1. Create `MetricsRenderer.tsx`
2. Move `MetricsView`, scale mode state, and config sync into it
3. Register in `SCREEN_RENDERERS`
4. Remove metrics branch and state from ScreenPage.tsx

### Phase 4: Migrate LogRenderer
1. Create `LogRenderer.tsx` (currently uses generic table)
2. Can add log-specific features later (syntax highlighting, level filtering)
3. Register in `SCREEN_RENDERERS`

### Phase 5: Cleanup
1. Remove all type-specific state from `ScreenPageContent`
2. Remove conditional rendering, use registry lookup
3. Delete unused imports

## Benefits

1. **OCP Compliance**: New screen types only require:
   - Creating `NewTypeRenderer.tsx`
   - Adding one line to `SCREEN_RENDERERS`

2. **Encapsulation**: Each renderer owns its state and config handling

3. **Testability**: Renderers can be unit tested in isolation

4. **Scalability**: Complex screen types don't bloat ScreenPage.tsx

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
| `src/lib/screen-renderers/GenericTableRenderer.tsx` | Create - fallback renderer |
| `src/lib/screen-renderers/ProcessListRenderer.tsx` | Create - move from ScreenPage |
| `src/lib/screen-renderers/MetricsRenderer.tsx` | Create - move from ScreenPage |
| `src/lib/screen-renderers/LogRenderer.tsx` | Create - basic implementation |
| `src/routes/ScreenPage.tsx` | Modify - use registry, remove type-specific code |
| `src/lib/screens-api.ts` | Modify - make ScreenConfig generic (`Record<string, unknown>`) |
