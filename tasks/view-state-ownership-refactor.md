# View State Ownership Refactor

## Problem

The analytics web app has **inconsistent state ownership patterns** between built-in pages and user-defined screens:

1. **URL as source of truth** - Built-in pages read state directly from URL params, while user-defined screens use config objects
2. **Multiple update paths** - Built-in pages have various components updating URL independently (time pickers, chart drag-to-zoom, dropdowns)
3. **No single owner** - State is scattered across URL params rather than owned by a single config object
4. **Divergent patterns** - Two different architectures for essentially the same problem

Note: Previous issues with React Router's `useSearchParams()` callback instability have been mitigated by using `location.search` strings. This refactor is about **design cleanup and pattern convergence**, not fixing acute bugs.

### Current data flow for built-in pages:
```
URL params (source of truth)
    ↓
useTimeRange() / location.search parsing
    ↓
Components read from hooks
    ↓
Multiple callbacks update URL independently
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

### Phase 1: Define Base Config Shape

Create a shared config interface that both built-in and user-defined screens extend:

```typescript
// src/lib/screen-config.ts

interface BaseScreenConfig {
  timeRangeFrom?: string;
  timeRangeTo?: string;
}

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

### Phase 2: URL Initialization Helper

Create a helper to initialize config from URL on first load:

```typescript
// src/hooks/useInitialConfig.ts

function useInitialConfig<T extends BaseScreenConfig>(defaults: Partial<T>): T {
  const [searchParams] = useSearchParams();

  // useState initializer runs exactly once - captures URL params on mount
  const [initialConfig] = useState(() => {
    const fromUrl = parseUrlParams(searchParams);
    return { ...defaults, ...fromUrl } as T;
  });

  return initialConfig;
}
```

Using `useState` initializer instead of `useMemo` with empty deps:
- Guaranteed to run exactly once (clearer intent)
- No ESLint exhaustive-deps warning

### Phase 3: Screen Config Hook

Extract the config management pattern from `ScreenPage.tsx`:

```typescript
// src/hooks/useScreenConfig.ts

interface UseScreenConfigOptions<T> {
  initialConfig: T;
  syncToUrl?: boolean;  // Default true
}

interface UseScreenConfigResult<T> {
  config: T;
  updateConfig: (partial: Partial<T>) => void;
  setTimeRange: (from: string, to: string) => void;
}

export function useScreenConfig<T extends BaseScreenConfig>(
  options: UseScreenConfigOptions<T>
): UseScreenConfigResult<T> {
  const { initialConfig, syncToUrl = true } = options;

  const [config, setConfig] = useState(initialConfig);

  const updateConfig = useCallback((partial: Partial<T>) => {
    setConfig(prev => ({ ...prev, ...partial }));
  }, []);

  const setTimeRange = useCallback((from: string, to: string) => {
    updateConfig({ timeRangeFrom: from, timeRangeTo: to } as Partial<T>);
  }, [updateConfig]);

  useEffect(() => {
    if (syncToUrl) {
      syncConfigToUrl(config);
    }
  }, [config, syncToUrl]);

  return { config, updateConfig, setTimeRange };
}
```

**No debouncing needed:** Interactions that produce rapid updates (drag-to-zoom, slider dragging) should use commit-on-release, updating config only on mouse release.

### Phase 4: Migrate Built-in Pages

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

### Phase 5: Update Components

Components receive config values and callbacks as props (like screen renderers already do):

1. **TimeRangePicker** - receives `timeRange` and `onTimeRangeChange` props
2. **XYChart** - already implements commit-on-release (no changes needed)
3. **MetricsChart** - receives all config as props
4. **PropertyTimeline** - receives selected properties as props

#### XYChart: Already Correct

XYChart already implements commit-on-release via uPlot's `setSelect` hook:

```typescript
// Current implementation in XYChart.tsx - already correct
setSelect: [
  (u: uPlot) => {
    if (xAxisMode !== 'time') return
    const { left, width } = u.select
    if (width > 0 && onTimeRangeSelectRef.current) {
      // Convert pixel positions to time values
      const fromTime = u.posToVal(left, 'x')
      const toTime = u.posToVal(left + width, 'x')
      const fromDate = new Date(fromTime * 1000)
      const toDate = new Date(toTime * 1000)
      u.setSelect({ left: 0, width: 0, top: 0, height: 0 }, false)
      onTimeRangeSelectRef.current(fromDate, toDate)
    }
  },
],
```

uPlot handles drag selection internally and only fires the `setSelect` hook on mouse release. No changes needed to XYChart itself.

**What changes:** The parent components (PerformanceAnalysisPage, etc.) pass a callback that updates config instead of URL:

```typescript
// Before: callback updates URL
onTimeRangeSelect={(from, to) => {
  navigate(`?from=${from.toISOString()}&to=${to.toISOString()}`)
}}

// After: callback updates config (drag-zoom always produces absolute time range)
onTimeRangeSelect={(from, to) => {
  setTimeRange(from.toISOString(), to.toISOString())
}}
```

### Phase 6: Deprecate Old Hooks

Remove URL-driven hooks:
- `useTimeRange()` - replace with config pattern
- Direct `useSearchParams()` for view state - replace with config

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
| `src/components/XYChart.tsx` | No changes needed (already commit-on-release via uPlot) |
| `src/components/MetricsChart.tsx` | Ensure props-driven |
| `src/components/PropertyTimeline.tsx` | No changes needed (already commit-on-release via uPlot) |
| `src/components/ThreadCoverageTimeline.tsx` | No changes needed (already commit-on-release via uPlot) |
| `src/components/layout/TimeRangePicker.tsx` | Ensure props-driven |

## Migration Strategy

1. Create `useScreenConfig` hook alongside existing code
2. Migrate one built-in page at a time, starting with simplest (ProcessesPage?)
3. Keep `useTimeRange()` working during migration (can wrap useScreenConfig internally)
4. Once all pages migrated, remove old hooks
5. Update ScreenPage to use shared utilities if beneficial

## Decisions

1. **Browser back/forward**: Sync config from URL on popstate (Option A)

   When user clicks back/forward, they're expressing intent to restore a previous known state. The URL represents checkpoints the user expects to return to.

   ```typescript
   // In useScreenConfig or at page level
   useEffect(() => {
     const handlePopstate = () => {
       const restored = parseUrlParams(new URLSearchParams(location.search));
       setConfig(prev => ({ ...prev, ...restored }));
     };
     window.addEventListener('popstate', handlePopstate);
     return () => window.removeEventListener('popstate', handlePopstate);
   }, []);
   ```

   This doesn't make URL the source of truth - config still owns state during normal operation. Popstate is treated as a user action that updates config, similar to clicking a preset in the time picker.

2. **Cross-screen time range linking**: Context-dependent, not a global toggle.

   | Navigation | Time Range Behavior | Rationale |
   |------------|---------------------|-----------|
   | Process list → Process info | Reset to process lifetime | Show full context for this process |
   | Process info → Process logs/metrics/performance | Reset to last hour | Sensible default for detail views |
   | Between process detail pages (logs ↔ metrics ↔ performance) | **Preserve** | Leverage implicit temporal correlation |
   | Any → User-defined screen | Use screen's saved config | Respect user's configured defaults |
   | Any → User-defined screen (with URL time params) | URL overrides saved config | Treat as manual change (creates unsaved diff) |

   For user-defined screens: URL params act as if the user changed the time range manually after loading. This creates a difference that can be saved back to the screen config.

## Open Questions

(None currently)

## Success Criteria

- [ ] No more chart flickering on built-in pages
- [ ] No race conditions when changing multiple params quickly
- [ ] Clear ownership: config owns state, URL is just a projection
- [ ] Stable callbacks that don't cause re-renders
- [ ] Built-in pages follow same pattern as user-defined screens
- [ ] Drag-to-zoom updates config (not URL directly) on mouse release
- [ ] Path to "save view as custom screen" is clear
