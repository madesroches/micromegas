# Screen Config Architecture

## Overview

Screen config is the single source of truth for all screen state that needs to be persisted. Each renderer owns its complete config and is responsible for tracking changes.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     ScreenPage (Controller)                     │
│                                                                 │
│  ┌─────────────┐         ┌─────────────┐         ┌───────────┐ │
│  │ Time Picker │ ──────→ │  Renderer   │ ──────→ │  Config   │ │
│  │  (Header)   │         │   (Owner)   │         │  (Model)  │ │
│  └─────────────┘         └─────────────┘         └───────────┘ │
│        │                       │                       │       │
│        │ rawTimeRange          │ onConfigChange        │       │
│        └───────────────────────┘                       │       │
│                                                        │       │
│                              ┌─────────────────────────┘       │
│                              ↓                                 │
│                     ┌─────────────────┐                        │
│                     │  Save/Load API  │                        │
│                     └─────────────────┘                        │
└─────────────────────────────────────────────────────────────────┘
```

## Ownership

### Renderer (Owner)
- **Owns the complete config** including all type-specific fields and time range
- Tracks changes via `useEffect` with a ref to previous values
- Calls `onConfigChange(config)` when any value changes
- Calls `onUnsavedChange()` when current differs from saved
- Each renderer defines its own config interface (e.g., `LogConfig`, `MetricsConfig`)

### ScreenPage (Controller)
- Loads config from API on mount
- Initializes time picker from saved `config.timeRangeFrom/To`
- Passes `rawTimeRange` to renderer (read-only input from header UI)
- Merges config updates from renderer
- Saves config to API when user clicks Save

### Config (Model)
- JSON object stored in database
- Contains all persisted state for the screen
- Type-specific fields defined by each renderer

## Config Fields by Screen Type

### LogRenderer
```typescript
interface LogConfig {
  sql: string
  logLevel?: string      // 'all' | 'debug' | 'info' | 'warn' | 'error' | 'fatal'
  limit?: number         // Row limit (1-10000)
  search?: string        // Search filter text
  timeRangeFrom?: string // e.g., 'now-1h', 'now-5m'
  timeRangeTo?: string   // e.g., 'now'
}
```

### MetricsRenderer
```typescript
interface MetricsConfig {
  sql: string
  metrics_options?: {
    scale_mode?: 'p99' | 'max'
  }
  timeRangeFrom?: string
  timeRangeTo?: string
}
```

### ProcessListRenderer
```typescript
interface ProcessListConfig {
  sql: string
  timeRangeFrom?: string
  timeRangeTo?: string
}
```

## URL Sync (LogRenderer)

LogRenderer syncs filter state to URL params, enabling shareable links with filter state.

### Filter Initialization Priority
When LogRenderer mounts, filters are initialized in this order:
1. **URL params** (`?level=error&limit=500&search=foo`)
2. **Config defaults** (from saved screen config)
3. **Hardcoded defaults** (`level='all'`, `limit=100`, `search=''`)

### URL Update Behavior
- Filters sync to URL after initial mount (not on first render)
- Only non-default values appear in URL (keeps URLs clean)
- Uses `setSearchParams` with functional update to preserve other params (e.g., time range)

```typescript
useEffect(() => {
  if (isInitialMount) return

  setSearchParams((prev) => {
    const params = new URLSearchParams(prev)
    // Set or delete based on whether value is default
    if (logLevel === 'all') params.delete('level')
    else params.set('level', logLevel)
    // ...
    return params
  }, { replace: true })
}, [logLevel, logLimit, search, setSearchParams])
```

## Change Tracking Pattern

Each renderer follows the same pattern for tracking changes:

```typescript
const prevConfigRef = useRef<ConfigValues | null>(null)
// Track fields not in local state (e.g., SQL) via ref to avoid effect re-runs
const sqlRef = useRef(config.sql)
sqlRef.current = config.sql

useEffect(() => {
  const current = { /* current values from local state */ }

  // First run: store initial values, don't trigger changes
  if (prevConfigRef.current === null) {
    prevConfigRef.current = current
    return
  }

  // Detect changes
  const prev = prevConfigRef.current
  const hasChanges = /* compare prev vs current */

  if (!hasChanges) return

  prevConfigRef.current = current

  // Check if differs from saved config
  if (/* current !== saved */) {
    onUnsavedChange()
  }

  // Update config - explicitly list all fields (no spreading)
  onConfigChange({
    sql: sqlRef.current,
    field1: current.field1,
    field2: current.field2,
    // ...all tracked fields
  })
}, [/* all tracked values, callbacks - but NOT config */])
```

## SQL Change Handling

SQL changes are handled separately from filter tracking because:
- SQL is edited via a dedicated editor component
- SQL changes should update config immediately (not wait for effect)
- The filter effect needs to preserve the current SQL value

```typescript
const handleSqlChange = useCallback((sql: string) => {
  // Explicitly set all fields - SQL from param, filters from local state
  onConfigChange({
    sql,
    logLevel,
    limit: logLimit,
    search,
    timeRangeFrom: rawTimeRange.from,
    timeRangeTo: rawTimeRange.to,
  })

  if (savedConfig && sql !== savedConfig.sql) {
    onUnsavedChange()
  }
}, [savedConfig, onUnsavedChange, onConfigChange, logLevel, logLimit, search, rawTimeRange])
```

## Renderer Remount on Navigation

ScreenPage uses a `key` prop to force renderer remount when navigating between screens:

```tsx
<Renderer
  key={screen?.name ?? 'new'}
  config={config}
  // ...
/>
```

This ensures:
- Fresh local state when switching screens
- No stale filter values from previous screen
- Clean ref initialization for change tracking

## Time Range Sync on Load

When loading a saved screen with a custom time range:

1. ScreenPage loads config with `timeRangeFrom: "now-1h"`
2. ScreenPage sets `expectedTimeRange` and calls `setTimeRange("now-1h", "now")`
3. ScreenPage shows loading state while `expectedTimeRange !== null`
4. Once `rawTimeRange` matches expected values, `expectedTimeRange` is cleared
5. Renderer mounts with `rawTimeRange` already synced to saved values
6. Renderer's first effect run stores the correct initial values

This prevents the race condition where the renderer would see stale time range values before the picker syncs.

## Data Flow

### User changes filter (e.g., log level):
1. User selects new value in dropdown
2. Renderer's local state updates
3. Effect detects change, calls `onConfigChange` and `onUnsavedChange`
4. ScreenPage merges into config state
5. "(unsaved changes)" indicator appears

### User changes time range (header picker):
1. User selects new range in time picker
2. `rawTimeRange` updates (via `useTimeRange` hook)
3. Renderer's effect detects change, calls `onConfigChange` and `onUnsavedChange`
4. ScreenPage merges into config state
5. "(unsaved changes)" indicator appears

### User saves screen:
1. User clicks Save button
2. ScreenPage calls `updateScreen(name, { config })`
3. API persists config to database
4. `hasUnsavedChanges` is cleared

### User loads saved screen:
1. ScreenPage fetches screen from API
2. Config is set, time picker is synced (with loading state)
3. Renderer mounts with synced values
4. Renderer stores initial values in ref (no change detected)
