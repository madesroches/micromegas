# View State Ownership Refactor

## Problem

The current analytics web app has unclear ownership of view state (time range, selected measure, properties, etc.) in **built-in pages**:

1. **URL is treated as source of truth** - Components read state from URL params via `useSearchParams()`
2. **Multiple update paths** - Any component can update URL params, leading to race conditions
3. **No single owner** - State changes come from time pickers, chart drag-to-zoom, dropdowns, etc.
4. **React Router instability** - `useSearchParams()` returns a new object every render, causing callback instability and chart flickering

### Current (broken) data flow for built-in pages:
```
URL params
    ↓
useSearchParams() / useTimeRange()
    ↓
Components read from hooks
    ↓
Multiple callbacks update URL (race conditions)
    ↓
Chart flickers on re-render
```

### User-defined screens (already correct):
```
Screen Config (source of truth, persisted to DB)
    ↓
Renderer reads config
    ↓
User changes → onConfigChange() → updates config
    ↓
URL is just a projection for sharing
```

## Proposed Architecture

**User-defined screens already have the correct pattern.** Built-in pages should adopt the same config-driven architecture.

### Unified data flow:
```
Screen Config (source of truth)
    │
    ├── User-defined screens: config persisted to DB
    │
    └── Built-in pages: config is ephemeral (session state)

    ↓
Components read from config, dispatch changes via callbacks
    ↓
URL is a side-effect projection (for sharing/bookmarking)
```

## Design Principles

1. **Config is source of truth**: Every screen (built-in or user-defined) has a config object that owns view state
2. **One update path**: Components call `onConfigChange()` or typed setters, never directly update URL
3. **URL is a projection**: URL reflects state for sharing but doesn't drive it (after initial load)
4. **Stable callbacks**: Config setters are stable, no re-render cascades
5. **Pattern convergence**: Built-in pages use the same pattern as user-defined screens
6. **Future: saveable views**: Built-in pages could eventually allow "save this view as a custom screen"

### Time Range Handling

Config stores raw time range strings, which can be **relative** or **absolute**:

```typescript
// Relative (user intent preserved)
{ timeRangeFrom: "now-1h", timeRangeTo: "now" }

// Absolute (specific window)
{ timeRangeFrom: "2024-01-15T10:00:00Z", timeRangeTo: "2024-01-15T10:30:00Z" }
```

**Behavior by interaction:**

| Interaction | Result | Rationale |
|-------------|--------|-----------|
| User picks preset (e.g., "Last 1 hour") | Store relative: `now-1h` → `now` | Preserves user intent; refreshing shows latest data |
| User drag-zooms on chart | Store absolute ISOs | User selected a specific time window |
| User enters custom absolute range | Store absolute ISOs | Explicit user choice |

**Implementation in setTimeRange:**

```typescript
const setTimeRange = useCallback((from: string, to: string) => {
  // from/to are either relative ("now-1h") or absolute ISO strings
  // Just store them as-is; parsing happens when passing to API
  updateConfig({ timeRangeFrom: from, timeRangeTo: to } as Partial<T>);
}, [updateConfig]);
```

The existing `parseTimeRange()` and `getTimeRangeForApi()` utilities handle conversion to absolute dates when needed for API calls.

## Implementation Plan

### Phase 1: Extract Reusable Screen Config Pattern

Extract the config management pattern from `ScreenPage.tsx` into reusable utilities:

```typescript
// src/hooks/useScreenConfig.ts

interface UseScreenConfigOptions<T> {
  initialConfig: T;
  onPersist?: (config: T) => void;  // For user-defined screens
  syncToUrl?: boolean;              // Default true
  urlSyncDelay?: number;            // Debounce delay in ms (default 150)
}

interface UseScreenConfigResult<T> {
  config: T;
  updateConfig: (partial: Partial<T>) => void;
  // Typed helpers for common fields
  setTimeRange: (from: string, to: string) => void;
  hasUnsavedChanges: boolean;
}

export function useScreenConfig<T extends BaseScreenConfig>(
  options: UseScreenConfigOptions<T>
): UseScreenConfigResult<T> {
  const { initialConfig, onPersist, syncToUrl = true, urlSyncDelay = 150 } = options;

  // State lives here
  const [config, setConfig] = useState(initialConfig);

  // Stable update function
  const updateConfig = useCallback((partial: Partial<T>) => {
    setConfig(prev => ({ ...prev, ...partial }));
  }, []);

  // Typed helper for time range
  const setTimeRange = useCallback((from: string, to: string) => {
    updateConfig({ timeRangeFrom: from, timeRangeTo: to } as Partial<T>);
  }, [updateConfig]);

  // Debounced URL sync - critical for drag-to-zoom which fires rapid updates
  const debouncedConfig = useDebounce(config, urlSyncDelay);

  useEffect(() => {
    if (syncToUrl) {
      syncConfigToUrl(debouncedConfig);
    }
  }, [debouncedConfig, syncToUrl]);

  // Track unsaved changes (for user-defined screens)
  const hasUnsavedChanges = useMemo(() => {
    if (!onPersist) return false;
    return JSON.stringify(config) !== JSON.stringify(initialConfig);
  }, [config, initialConfig, onPersist]);

  return { config, updateConfig, setTimeRange, hasUnsavedChanges };
}
```

#### Why debouncing URL sync

Debouncing URL sync is a safety net for rapid config changes (e.g., typing in a search field). However, for drag-to-zoom interactions, a better pattern is **commit-on-release** (see Phase 4) which avoids intermediate updates entirely.

The debounce ensures that even if multiple config changes happen in quick succession, the URL only updates once after changes settle.

### Phase 2: Define Base Config Shape

Create a shared config interface that both built-in and user-defined screens extend:

```typescript
// src/lib/screen-config.ts

interface BaseScreenConfig {
  // Time range (common to most screens)
  timeRangeFrom?: string;
  timeRangeTo?: string;
}

// Built-in page configs extend this

interface PerformanceAnalysisConfig extends BaseScreenConfig {
  processId: string;
  selectedMeasure?: string;
  selectedProperties?: string[];
  scaleMode?: 'p99' | 'max';
}

interface ProcessMetricsConfig extends BaseScreenConfig {
  processId: string;
  selectedMeasure?: string;
  selectedProperties?: string[];
}

interface ProcessLogConfig extends BaseScreenConfig {
  processId: string;
  logLevel?: string;      // 'all' | 'trace' | 'debug' | 'info' | 'warn' | 'error' | 'fatal'
  logLimit?: number;      // default 100, max 10000
  search?: string;        // search filter for target/message
}

interface ProcessesConfig extends BaseScreenConfig {
  search?: string;
  sortField?: 'exe' | 'start_time' | 'last_update_time' | 'runtime' | 'username' | 'computer';
  sortDirection?: 'asc' | 'desc';
}
```

Note: `scaleMode` is only in `PerformanceAnalysisConfig` since ProcessMetricsPage doesn't have that feature.

### Phase 3: Migrate Built-in Pages

Update each built-in page to use the config pattern:

#### Before (PerformanceAnalysisPage):
```typescript
function PerformanceAnalysisPage() {
  const { timeRange, setTimeRange } = useTimeRange();  // URL-driven
  const [searchParams] = useSearchParams();
  const measure = searchParams.get('measure');
  // ... components update URL directly
}
```

#### After:
```typescript
function PerformanceAnalysisPage() {
  // Initialize config from URL (once) or defaults
  const initialConfig = useInitialConfig<PerformanceAnalysisConfig>();

  const { config, updateConfig, setTimeRange } = useScreenConfig({
    initialConfig,
    syncToUrl: true,
  });

  // Pass config and callbacks to children
  return (
    <MetricsChart
      timeRange={{ from: config.timeRangeFrom, to: config.timeRangeTo }}
      onTimeRangeChange={setTimeRange}
      measure={config.selectedMeasure}
      onMeasureChange={(m) => updateConfig({ selectedMeasure: m })}
    />
  );
}
```

Pages to migrate:
1. **PerformanceAnalysisPage** - time range, selected measure, properties
2. **ProcessMetricsPage** - time range, process selection
3. **ProcessLogPage** - time range, filters
4. **ProcessesPage** - time range, filters

### Phase 4: Update Components

Components receive config values and callbacks as props (like screen renderers already do):

1. **TimeRangePicker** - receives `timeRange` and `onTimeRangeChange` props
2. **XYChart** - commit-on-release drag pattern (see below)
3. **MetricsChart** - receives all config as props
4. **PropertyTimeline** - receives selected properties as props, commit-on-release drag

#### XYChart: Commit-on-Release Drag Pattern

Instead of firing `onTimeRangeChange` on every mouse move during drag-to-zoom, the chart should:
1. Track drag state locally (no config updates during drag)
2. Show visual selection feedback
3. Commit the final range only on mouse release

```typescript
// In XYChart.tsx
interface DragSelection {
  startX: number;  // pixel position
  endX: number;
  startTime: number;  // converted time value
  endTime: number;
}

function XYChart({ onTimeRangeChange, ...props }) {
  const [dragSelection, setDragSelection] = useState<DragSelection | null>(null);
  const chartRef = useRef<HTMLDivElement>(null);

  const xToTime = useCallback((clientX: number): number => {
    // Convert pixel position to time value using chart scale
    // ... implementation depends on chart library
  }, [/* scale deps */]);

  const handleMouseDown = useCallback((e: React.MouseEvent) => {
    const time = xToTime(e.clientX);
    setDragSelection({
      startX: e.clientX,
      endX: e.clientX,
      startTime: time,
      endTime: time,
    });
  }, [xToTime]);

  const handleMouseMove = useCallback((e: React.MouseEvent) => {
    if (!dragSelection) return;

    // Update local state only - no onTimeRangeChange call
    const time = xToTime(e.clientX);
    setDragSelection(prev => prev ? {
      ...prev,
      endX: e.clientX,
      endTime: time,
    } : null);
  }, [dragSelection, xToTime]);

  const handleMouseUp = useCallback(() => {
    if (!dragSelection) return;

    // Commit once on release
    const { startTime, endTime } = dragSelection;
    if (startTime !== endTime) {
      const from = Math.min(startTime, endTime);
      const to = Math.max(startTime, endTime);
      onTimeRangeChange(new Date(from), new Date(to));
    }
    setDragSelection(null);
  }, [dragSelection, onTimeRangeChange]);

  // Also handle mouse leaving the chart area
  const handleMouseLeave = useCallback(() => {
    setDragSelection(null);  // Cancel drag if mouse leaves
  }, []);

  return (
    <div
      ref={chartRef}
      onMouseDown={handleMouseDown}
      onMouseMove={handleMouseMove}
      onMouseUp={handleMouseUp}
      onMouseLeave={handleMouseLeave}
    >
      {/* Chart content */}

      {/* Selection overlay - visual feedback during drag */}
      {dragSelection && (
        <div
          className="absolute top-0 bottom-0 bg-accent-link/20 border-x border-accent-link pointer-events-none"
          style={{
            left: Math.min(dragSelection.startX, dragSelection.endX),
            width: Math.abs(dragSelection.endX - dragSelection.startX),
          }}
        />
      )}
    </div>
  );
}
```

**Benefits of commit-on-release:**
- Zero intermediate config/state updates during drag
- No debounce tuning needed for drag interactions
- Chart doesn't re-query or re-render during drag
- Clean separation: local UI state vs committed config state
- Same pattern applies to PropertyTimeline and ThreadCoverageTimeline

### Phase 5: Deprecate Old Hooks

Remove URL-driven hooks:
- `useTimeRange()` - replace with config pattern
- Direct `useSearchParams()` for view state - replace with config

### Phase 6: URL Initialization Helper

Create a helper to initialize config from URL on first load:

```typescript
// src/hooks/useInitialConfig.ts

function useInitialConfig<T extends BaseScreenConfig>(): T {
  const [searchParams] = useSearchParams();

  // useState initializer runs exactly once - captures URL params on mount
  const [initialConfig] = useState(() => {
    const fromUrl = parseUrlParams(searchParams);
    return {
      ...DEFAULT_CONFIG,
      ...fromUrl,
    } as T;
  });

  return initialConfig;
}
```

Using `useState` initializer instead of `useMemo` with empty deps:
- Guaranteed to run exactly once (clearer intent)
- No ESLint exhaustive-deps warning
- Same behavior, cleaner code

## Files to Modify

| File | Change |
|------|--------|
| `src/hooks/useScreenConfig.ts` | NEW - Reusable config management hook |
| `src/lib/screen-config.ts` | NEW - Shared config type definitions |
| `src/hooks/useInitialConfig.ts` | NEW - URL → initial config helper |
| `src/hooks/useTimeRange.ts` | DEPRECATE - Replace with useScreenConfig |
| `src/routes/PerformanceAnalysisPage.tsx` | Migrate to config pattern |
| `src/routes/ProcessMetricsPage.tsx` | Migrate to config pattern |
| `src/routes/ProcessLogPage.tsx` | Migrate to config pattern |
| `src/routes/ProcessesPage.tsx` | Migrate to config pattern |
| `src/routes/ScreenPage.tsx` | Minor updates to use shared utilities |
| `src/components/XYChart.tsx` | Implement commit-on-release drag pattern |
| `src/components/MetricsChart.tsx` | Ensure props-driven |
| `src/components/PropertyTimeline.tsx` | Implement commit-on-release drag pattern |
| `src/components/ThreadCoverageTimeline.tsx` | Implement commit-on-release drag pattern |
| `src/components/layout/TimeRangePicker.tsx` | Ensure props-driven |

## Migration Strategy

1. Create `useScreenConfig` hook alongside existing code
2. Migrate one built-in page at a time, starting with simplest (ProcessesPage?)
3. Keep `useTimeRange()` working during migration (can wrap useScreenConfig internally)
4. Once all pages migrated, remove old hooks
5. Update ScreenPage to use shared utilities if beneficial

## Open Questions

1. **Browser back/forward**: How to handle popstate?
   - Option A: Re-initialize config from URL on popstate
   - Option B: Ignore popstate (URL is just for sharing, not navigation)
   - Leaning toward A for better UX

2. **Cross-screen time range linking**: Should time range persist when navigating between screens?
   - Current: No, each screen has its own URL params
   - Could add: App-level "link time ranges" toggle

## Success Criteria

- [ ] No more chart flickering on built-in pages
- [ ] No race conditions when changing multiple params quickly
- [ ] Clear ownership: config owns state, URL is just a projection
- [ ] Stable callbacks that don't cause re-renders
- [ ] Built-in pages follow same pattern as user-defined screens
- [ ] Drag-to-zoom commits only on mouse release (no intermediate updates)
- [ ] Path to "save view as custom screen" is clear
