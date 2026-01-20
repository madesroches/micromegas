# State Management Architecture

## Overview

The analytics web app uses an MVC-like pattern for state management across all screens:

- **Model**: Config object is the source of truth for view state
- **View**: Components receive state as props and dispatch user actions via callbacks
- **Controller**: Screen/Page handles actions and decides navigation strategy

This pattern applies to both **built-in pages** (ProcessesPage, ProcessLogPage, etc.) and **user-defined screens** (ScreenPage with custom renderers).

```
┌─────────────────────────────────────────────────────────────────┐
│                      MVC Data Flow                              │
│                                                                 │
│   User Action (click, drag, type)                               │
│         │                                                       │
│         ▼                                                       │
│   ┌───────────┐     callback      ┌────────────┐                │
│   │   View    │ ─────────────────→│ Controller │                │
│   │(Component)│                   │  (Page)    │                │
│   └───────────┘                   └────────────┘                │
│         ▲                               │                       │
│         │                               │ updateConfig()        │
│         │ props                         ▼                       │
│         │                         ┌───────────┐                 │
│         └─────────────────────────│   Model   │                 │
│                                   │  (Config) │                 │
│                                   └───────────┘                 │
│                                         │                       │
│                                         │ URL sync (side effect)│
│                                         ▼                       │
│                                   ┌───────────┐                 │
│                                   │    URL    │                 │
│                                   │(Shareable)│                 │
│                                   └───────────┘                 │
└─────────────────────────────────────────────────────────────────┘
```

## Core Principles

1. **Config is source of truth**: Every screen has a config object that owns view state
2. **One update path**: Components call callbacks, never update URL directly
3. **URL is a projection**: URL reflects state for sharing but doesn't drive it (after initial load)
4. **Only controllers read URL**: Pages read URL params; components receive state as props
5. **Stable callbacks**: Config setters are stable, preventing re-render cascades

## Built-in Pages

Built-in pages use the `useScreenConfig` hook for state management.

### Pattern

```typescript
// Default config (module-level constant)
const DEFAULT_CONFIG: ProcessesConfig = {
  timeRangeFrom: 'now-1h',
  timeRangeTo: 'now',
  search: '',
  sortField: 'last_update_time',
  sortDirection: 'desc',
}

// URL builder (module-level constant)
const buildUrl = (cfg: ProcessesConfig): string => {
  const params = new URLSearchParams()
  if (cfg.timeRangeFrom !== 'now-1h') params.set('from', cfg.timeRangeFrom)
  if (cfg.timeRangeTo !== 'now') params.set('to', cfg.timeRangeTo)
  if (cfg.search) params.set('search', cfg.search)
  // ... other non-default values
  return params.toString() ? `?${params}` : ''
}

function PageContent() {
  const { config, updateConfig } = useScreenConfig(DEFAULT_CONFIG, buildUrl)

  // Time range: creates history entry (navigational)
  const handleTimeRangeChange = (from: string, to: string) => {
    updateConfig({ timeRangeFrom: from, timeRangeTo: to })
  }

  // Search: replaces current entry (editing)
  const handleSearchChange = (search: string) => {
    updateConfig({ search }, { replace: true })
  }

  return (
    <PageLayout
      timeRangeControl={{
        timeRangeFrom: config.timeRangeFrom,
        timeRangeTo: config.timeRangeTo,
        onTimeRangeChange: handleTimeRangeChange,
      }}
    >
      <SearchInput value={config.search} onChange={handleSearchChange} />
    </PageLayout>
  )
}
```

### useScreenConfig Hook

The hook manages config state with URL synchronization:

- **Initializes from URL on mount**: Merges URL params over defaults
- **Handles browser back/forward**: Restores config from URL on popstate
- **Atomic state + URL update**: `updateConfig` updates React state and navigates together
- **Push vs replace**: Controller decides via `{ replace: true }` option

```typescript
const { config, updateConfig } = useScreenConfig(DEFAULT_CONFIG, buildUrl)

// Push (default) - creates history entry
updateConfig({ timeRangeFrom: 'now-24h' })

// Replace - updates current entry
updateConfig({ search: 'query' }, { replace: true })
```

### Identity Params and Remounting

Pages with identity params (e.g., `process_id`) key their content component to force remount when the identity changes:

```typescript
export default function ProcessLogPage() {
  const [searchParams] = useSearchParams()
  const processId = searchParams.get('process_id')

  return (
    <Suspense fallback={<PageLoader />}>
      <ProcessLogContent key={processId} />
    </Suspense>
  )
}
```

| Page | Identity Param |
|------|----------------|
| ProcessLogPage | `process_id` |
| ProcessMetricsPage | `process_id` |
| PerformanceAnalysisPage | `process_id` |
| ProcessesPage | (none) |

## User-Defined Screens (ScreenPage)

User-defined screens persist config to the database and track unsaved changes.

### Architecture

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
│                              ▼                                 │
│                     ┌─────────────────┐                        │
│                     │  Save/Load API  │                        │
│                     └─────────────────┘                        │
└─────────────────────────────────────────────────────────────────┘
```

### Ownership

| Role | Component | Responsibility |
|------|-----------|----------------|
| **Model** | Config object | JSON stored in database |
| **View** | Renderer | Renders based on config, tracks changes |
| **Controller** | ScreenPage | Loads/saves config, manages time sync |

### Change Tracking

Renderers track changes via `useEffect` with a ref to previous values:

```typescript
const prevConfigRef = useRef<ConfigValues | null>(null)

useEffect(() => {
  const current = { logLevel, logLimit, search, ... }

  if (prevConfigRef.current === null) {
    prevConfigRef.current = current
    return
  }

  const hasChanges = /* compare prev vs current */
  if (!hasChanges) return

  prevConfigRef.current = current

  if (/* current !== savedConfig */) {
    onUnsavedChange()
  }

  onConfigChange({ ...current, sql: sqlRef.current })
}, [logLevel, logLimit, search, ...])
```

### Time Range Sync

User-defined screens use `useTimeRangeSync` to bridge URL time params with saved config:

1. URL time params act as session overlay
2. Changes from saved config trigger "unsaved changes" indicator
3. User can save time range back to database

## URL Conventions

### Parameter Mapping

| Config Field | URL Param | Type | Example |
|--------------|-----------|------|---------|
| processId | `process_id` | string | `abc-123` |
| timeRangeFrom | `from` | string | `now-1h` or ISO |
| timeRangeTo | `to` | string | `now` or ISO |
| selectedMeasure | `measure` | string | `DeltaTime` |
| selectedProperties | `properties` | comma-separated | `cpu,memory` |
| scaleMode | `scale` | string | `p99`, `max` |
| logLevel | `level` | string | `error` |
| logLimit | `limit` | number | `500` |
| search | `search` | string | `query` |
| sortField | `sort` | string | `exe` |
| sortDirection | `dir` | string | `asc`, `desc` |

### Conventions

- **Default values omitted**: Keep URLs clean (`/processes` not `/processes?from=now-1h&to=now`)
- **Arrays comma-separated**: `?properties=cpu,memory,disk`
- **Empty arrays omitted**: No `?properties=`

### Navigation Semantics

| Change Type | Method | When to Use | Examples |
|-------------|--------|-------------|----------|
| **Navigational** | `pushState` | User should be able to go back | Time range zoom, entity selection |
| **Editing** | `replaceState` | Fine-tuning current view | Search filter, sort order, scale mode |

## Time Range Handling

### Relative vs Absolute

Config stores raw time range strings which can be relative or absolute:

```typescript
// Relative (user intent preserved)
{ timeRangeFrom: "now-1h", timeRangeTo: "now" }

// Absolute (specific window from drag-zoom)
{ timeRangeFrom: "2024-01-15T10:00:00Z", timeRangeTo: "2024-01-15T10:30:00Z" }
```

| Interaction | Result | Rationale |
|-------------|--------|-----------|
| User picks preset | Store relative (`now-1h`) | Refreshing shows latest data |
| User drag-zooms | Store absolute ISOs | User selected specific window |
| User enters custom | Store absolute ISOs | Explicit user choice |

### Cross-Screen Navigation

| Navigation | Time Range Behavior |
|------------|---------------------|
| Process list → Process detail | Reset to process lifetime |
| Between detail pages (logs ↔ metrics) | Preserve (temporal correlation) |
| Any → User-defined screen | Use saved config (URL can override) |

## Config Types

### Built-in Page Configs

```typescript
interface BaseScreenConfig {
  timeRangeFrom?: string
  timeRangeTo?: string
}

interface ProcessesConfig extends BaseScreenConfig {
  search?: string
  sortField?: 'exe' | 'start_time' | 'last_update_time' | 'runtime' | 'username' | 'computer'
  sortDirection?: 'asc' | 'desc'
}

interface ProcessLogConfig extends BaseScreenConfig {
  processId: string
  logLevel?: string
  logLimit?: number
  search?: string
}

interface ProcessMetricsConfig extends BaseScreenConfig {
  processId: string
  selectedMeasure?: string
  selectedProperties?: string[]
}

interface PerformanceAnalysisConfig extends BaseScreenConfig {
  processId: string
  selectedMeasure?: string
  selectedProperties?: string[]
  scaleMode?: 'p99' | 'max'
}
```

### User-Defined Screen Configs

Each renderer defines its own config interface:

```typescript
interface LogConfig {
  sql: string
  logLevel?: string
  limit?: number
  search?: string
  timeRangeFrom?: string
  timeRangeTo?: string
}

interface MetricsConfig {
  sql: string
  metrics_options?: { scale_mode?: 'p99' | 'max' }
  timeRangeFrom?: string
  timeRangeTo?: string
}
```

## Deprecated Patterns

### useTimeRange Hook

The `useTimeRange` hook is deprecated for built-in pages. Use `useScreenConfig` instead.

```typescript
// DEPRECATED
const { timeRange, setTimeRange } = useTimeRange()

// NEW
const { config, updateConfig } = useScreenConfig(DEFAULT_CONFIG, buildUrl)
```

`useTimeRange` remains available for backwards compatibility but new code should use the config pattern.

### Direct URL Reading in Components

Components should not read URL params directly. Receive state as props from the controller (page).

```typescript
// WRONG - component reads URL
function PivotButton() {
  const [searchParams] = useSearchParams()
  const processId = searchParams.get('process_id')
  // ...
}

// CORRECT - component receives props
function PivotButton({ processId, timeRangeFrom, timeRangeTo }: PivotButtonProps) {
  // ...
}
```
