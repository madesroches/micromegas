# View State Ownership Refactor

## Problem

The analytics web app has **inconsistent state ownership patterns** between built-in pages and user-defined screens:

1. **URL as source of truth** - Built-in pages read state directly from URL params, while user-defined screens use config objects
2. **Multiple update paths** - Built-in pages have various components updating URL independently (time pickers, chart drag-to-zoom, dropdowns)
3. **No single owner** - State is scattered across URL params rather than owned by a single config object
4. **Divergent patterns** - Two different architectures for essentially the same problem

Note: Previous issues with React Router's `useSearchParams()` callback instability have been mitigated by using `location.search` strings. This refactor is about **design cleanup and pattern convergence**, not fixing acute bugs.

### Why Not URL-as-Source-of-Truth?

The current architecture treats URL as source of truth for built-in pages. An alternative would be to fix callback instability issues while keeping URL as the single source. However:

1. **URL params don't scale to complex state** - Upcoming features like notebook screens have document state that cannot practically be serialized to URL params (size limits, encoding complexity).

2. **User-defined screens already use config-as-source-of-truth** - They persist config to the database. Having two fundamentally different patterns increases cognitive load.

3. **URL is a projection, not storage** - URLs are for sharing and bookmarking, not for storing application state. Config objects are the natural home for view state.

The unified pattern: **Config is source of truth; URL is a projection for simple, shareable params.**

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

### MVC Architecture

Each screen follows an MVC-like pattern:

| Role | Component | Responsibility |
|------|-----------|----------------|
| **Model** | Config object | Source of truth for view state |
| **View** | Child components | Render based on config, dispatch user actions via callbacks |
| **Controller** | Screen/Page | Handles user actions, decides whether to update model or navigate |

```
User Action (drag-zoom, dropdown change, etc.)
    ↓
View dispatches callback (onTimeRangeChange, onMeasureChange)
    ↓
Controller (Screen) receives action
    ↓
Controller decides: update config (edit) or navigate (new history entry)
    ↓
Model (Config) updates
    ↓
URL synced as side-effect
    ↓
View re-renders with new config
```

The screen is the controller - it owns the decision logic for how each user action affects state. Components don't know or care whether their callbacks result in URL pushState or replaceState.

### Core Principles

1. **Config is source of truth**: Every screen (built-in or user-defined) has a config object that owns view state
2. **One update path**: Components call callbacks, never directly update URL
3. **URL is a projection**: URL reflects state for sharing but doesn't drive it (after initial load)
4. **Only controllers read URL**: Pages (controllers) read URL params; components receive state as props. This eliminates race conditions between URL and React state updates.
5. **Stable callbacks**: Config setters are stable, no re-render cascades
6. **Pattern convergence**: Built-in pages use the same pattern as user-defined screens
7. **Future: saveable views**: Built-in pages could eventually allow "save this view as a custom screen"

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

**Implementation in handler:**

```typescript
// In the screen (controller)
const handleTimeRangeChange = (from: string, to: string) => {
  // from/to are either relative ("now-1h") or absolute ISO strings
  // Just store them as-is; parsing happens when passing to API
  updateConfig({ timeRangeFrom: from, timeRangeTo: to });  // pushState (default)
};
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

### Phase 2: Screen Config Hook

Create a hook that manages config state with URL initialization and popstate handling:

```typescript
// src/hooks/useScreenConfig.ts

interface UpdateOptions {
  replace?: boolean;  // Use replaceState instead of pushState (default: false)
}

interface UseScreenConfigResult<T> {
  config: T;
  updateConfig: (partial: Partial<T>, options?: UpdateOptions) => void;
}

export function useScreenConfig<T extends BaseScreenConfig>(
  defaults: T,
  buildUrl: (config: T) => string
): UseScreenConfigResult<T> {
  const navigate = useNavigate();

  // Capture defaults on first render - makes hook robust against inline objects
  const defaultsRef = useRef(defaults);

  // Initialize from URL on mount
  const [config, setConfig] = useState<T>(() => {
    const fromUrl = parseUrlParams(new URLSearchParams(location.search));
    return { ...defaultsRef.current, ...fromUrl };
  });

  // Handle browser back/forward - restore config from URL
  useEffect(() => {
    const handlePopstate = () => {
      const fromUrl = parseUrlParams(new URLSearchParams(location.search));
      setConfig({ ...defaultsRef.current, ...fromUrl });
    };
    window.addEventListener('popstate', handlePopstate);
    return () => window.removeEventListener('popstate', handlePopstate);
  }, []);

  // Combined update: updates state AND syncs URL atomically
  // This prevents state/URL drift that can occur with separate calls
  const updateConfig = useCallback((partial: Partial<T>, options?: UpdateOptions) => {
    setConfig(prev => {
      const newConfig = { ...prev, ...partial };
      // Navigate inside setState callback to ensure we use the new config
      // This guarantees state and URL stay in sync
      navigate(buildUrl(newConfig), { replace: options?.replace });
      return newConfig;
    });
  }, [navigate, buildUrl]);

  return { config, updateConfig };
}
```

**Key design decisions:**

1. **Defaults captured on mount via ref** - the hook captures defaults on first render, making it robust against inline objects. Module constants are still recommended for clarity, but the hook won't break if you pass an inline object.
2. **buildUrl is a module constant** - passed to the hook, must be stable (defined outside component or memoized)
3. **Combined state + URL update** - `updateConfig` updates React state AND navigates atomically, preventing drift between state and URL
4. **Popstate restores from defaults + URL** - behaves like a fresh page load, not a merge with current state

**No debouncing needed:** Interactions that produce rapid updates (drag-to-zoom, slider dragging) should use commit-on-release, updating config only on mouse release.

#### URL Serialization Conventions

The `parseUrlParams()` and `buildUrl()` utilities must handle these cases consistently:

```typescript
// src/lib/url-params.ts

// Param name mapping (config field → URL param)
const PARAM_MAP = {
  processId: 'process_id',
  timeRangeFrom: 'from',
  timeRangeTo: 'to',
  selectedMeasure: 'measure',
  selectedProperties: 'properties',
  scaleMode: 'scale',
  logLevel: 'level',
  logLimit: 'limit',
  search: 'search',
  sortField: 'sort',
  sortDirection: 'dir',
} as const;

// Arrays: comma-separated (matches current behavior)
// URL: ?properties=cpu,memory,disk
// Config: { selectedProperties: ['cpu', 'memory', 'disk'] }

function parseUrlParams(params: URLSearchParams): Partial<BaseScreenConfig> {
  const result: Record<string, unknown> = {};

  // String params
  if (params.has('process_id')) result.processId = params.get('process_id');
  if (params.has('from')) result.timeRangeFrom = params.get('from');
  if (params.has('to')) result.timeRangeTo = params.get('to');
  if (params.has('measure')) result.selectedMeasure = params.get('measure');
  if (params.has('scale')) result.scaleMode = params.get('scale');
  if (params.has('level')) result.logLevel = params.get('level');
  if (params.has('search')) result.search = params.get('search');
  if (params.has('sort')) result.sortField = params.get('sort');
  if (params.has('dir')) result.sortDirection = params.get('dir');

  // Number params
  if (params.has('limit')) result.logLimit = parseInt(params.get('limit')!, 10);

  // Array params (comma-separated)
  if (params.has('properties')) {
    const val = params.get('properties');
    result.selectedProperties = val ? val.split(',') : [];
  }

  return result;
}

function buildUrlParams(config: Partial<BaseScreenConfig>): URLSearchParams {
  const params = new URLSearchParams();

  // Only include non-default values to keep URLs clean
  // Each page's buildUrl can decide what to omit

  if (config.processId) params.set('process_id', config.processId);
  if (config.timeRangeFrom) params.set('from', config.timeRangeFrom);
  if (config.timeRangeTo) params.set('to', config.timeRangeTo);
  if (config.selectedMeasure) params.set('measure', config.selectedMeasure);
  if (config.selectedProperties?.length) {
    params.set('properties', config.selectedProperties.join(','));
  }
  // ... etc

  return params;
}
```

**Edge cases:**
- Empty array: omit param entirely (not `?properties=`)
- Single item array: no comma (`?properties=cpu`)
- Missing param: field not set in result (uses default from config)

### Phase 3: Refactor TimeRangePicker to Props-Driven

**Current state:** TimeRangePicker uses `useTimeRange()` hook internally - it reads URL state directly and updates URL on user interaction. This violates the MVC pattern where components should be views that receive props and dispatch callbacks.

**Target state:** TimeRangePicker becomes a controlled component that receives time range values and an onChange callback from its parent (the controller).

#### Current Implementation (hook-driven):
```typescript
function TimeRangePicker() {
  const { timeRange, parsed, setTimeRange } = useTimeRange();
  // Reads URL internally, updates URL directly

  const handlePresetClick = (preset: Preset) => {
    setTimeRange(preset.from, preset.to);  // Updates URL
  };

  return (/* UI using parsed.label, etc. */);
}
```

#### New Implementation (props-driven):
```typescript
interface TimeRangePickerProps {
  from: string;   // Raw value: "now-1h" or ISO string
  to: string;     // Raw value: "now" or ISO string
  onChange: (from: string, to: string) => void;
}

function TimeRangePicker({ from, to, onChange }: TimeRangePickerProps) {
  // Derive display values from props
  const parsed = parseTimeRange(from, to);
  const label = parsed.label;  // "Last 1 hour", "Jan 15 10:00 - 11:00", etc.

  // Check if a preset matches current values
  const isPresetSelected = (preset: Preset) =>
    preset.from === from && preset.to === to;

  // User picks preset → call onChange with preset's relative strings
  const handlePresetClick = (preset: Preset) => {
    onChange(preset.from, preset.to);  // e.g., "now-1h", "now"
  };

  // User picks custom absolute range → call onChange with ISO strings
  const handleCustomRange = (fromDate: Date, toDate: Date) => {
    onChange(fromDate.toISOString(), toDate.toISOString());
  };

  return (/* UI */);
}
```

#### Key Changes:

| Aspect | Before (hook-driven) | After (props-driven) |
|--------|---------------------|----------------------|
| State source | `useTimeRange()` reads URL | Props from parent |
| Label display | Hook provides `parsed.label` | Derive via `parseTimeRange(from, to)` |
| Preset detection | Compare against URL values | Compare props against preset values |
| User interaction | `setTimeRange()` updates URL | `onChange()` notifies parent |
| Testability | Hard (needs URL/router mocking) | Easy (just pass props) |

#### Usage in Controller (Page):

```typescript
function ProcessMetricsContent() {
  const { config, updateConfig } = useScreenConfig(DEFAULT_CONFIG, buildUrl);

  const handleTimeRangeChange = (from: string, to: string) => {
    updateConfig({ timeRangeFrom: from, timeRangeTo: to });  // pushState (default)
  };

  return (
    <PageLayout>
      <TimeRangePicker
        from={config.timeRangeFrom ?? 'now-1h'}
        to={config.timeRangeTo ?? 'now'}
        onChange={handleTimeRangeChange}
      />
      {/* ... rest of page */}
    </PageLayout>
  );
}
```

#### Implementation Notes:

1. **Presets array:** Keep using existing `TIME_RANGE_PRESETS` constant - just compare against props instead of hook values
2. **Custom date picker:** If the component has a date picker for custom ranges, it calls `onChange` with ISO strings when user confirms selection
3. **Refresh button:** If there's a refresh/reload button, it should call `onChange(from, to)` with the same values (or parent provides a separate `onRefresh` callback)

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
// Default config for this page (module-level constant for stability)
const DEFAULT_CONFIG: PerformanceAnalysisConfig = {
  processId: '',
  timeRangeFrom: 'now-1h',
  timeRangeTo: 'now',
  scaleMode: 'p99',
};

// URL builder (module-level constant for stability)
const buildUrl = (cfg: PerformanceAnalysisConfig) => {
  const params = new URLSearchParams();
  if (cfg.processId) params.set('process_id', cfg.processId);
  if (cfg.timeRangeFrom) params.set('from', cfg.timeRangeFrom);
  if (cfg.timeRangeTo) params.set('to', cfg.timeRangeTo);
  if (cfg.scaleMode) params.set('scale', cfg.scaleMode);
  return `?${params.toString()}`;
};

// Content component - remounts when processId changes (via key)
function PerformanceAnalysisContent() {
  const { config, updateConfig } = useScreenConfig(DEFAULT_CONFIG, buildUrl);

  // Time range changes create history entries (user can go back)
  const handleTimeRangeChange = (from: string, to: string) => {
    updateConfig({ timeRangeFrom: from, timeRangeTo: to });  // pushState (default)
  };

  // Other config changes replace current entry (no back navigation)
  const handleScaleModeChange = (mode: 'p99' | 'max') => {
    updateConfig({ scaleMode: mode }, { replace: true });  // replaceState
  };

  // Pass config and callbacks to children
  return (
    <MetricsChart
      timeRange={{ from: config.timeRangeFrom, to: config.timeRangeTo }}
      onTimeRangeChange={handleTimeRangeChange}
      measure={config.selectedMeasure}
      onMeasureChange={(m) => updateConfig({ selectedMeasure: m }, { replace: true })}
      scaleMode={config.scaleMode}
      onScaleModeChange={handleScaleModeChange}
    />
  );
}

// Wrapper component - handles keying on identity param
export default function PerformanceAnalysisPage() {
  const [searchParams] = useSearchParams()
  const processId = searchParams.get('process_id')

  return (
    <AuthGuard>
      <Suspense fallback={<PageLoader />}>
        <PerformanceAnalysisContent key={processId} />
      </Suspense>
    </AuthGuard>
  )
}
```

The screen (controller) explicitly calls `navigate()` with push or replace based on the semantic meaning of the change. This is more explicit than having the hook decide automatically.

Pages to migrate:
1. **PerformanceAnalysisPage** - time range, selected measure, properties, scale mode
2. **ProcessMetricsPage** - time range, process selection
3. **ProcessLogPage** - time range, filters
4. **ProcessesPage** - time range, filters

**Note:** The current code already has wrapper/content separation (e.g., `ProcessLogPage` wraps `ProcessLogContent`). Migration just requires:
1. Adding `key={processId}` to the content component in the wrapper
2. Replacing URL-reading code with `useScreenConfig` in the content component
3. Adding explicit `navigate()` calls for URL sync

### Phase 5: Update Remaining Components

#### Components Currently Violating MVC (read URL directly)

These components must be refactored to receive state as props instead of reading URL:

| Component | Current Violation | Fix |
|-----------|-------------------|-----|
| `PivotButton.tsx` | Reads `process_id`, `from`, `to` via `useSearchParams` | Receive as props from parent page |
| `LogRenderer.tsx` | Reads `level`, `limit`, `search` via `useSearchParams` | Receive filter state as props from ScreenPage |

**PivotButton refactor:**
```typescript
// Before (violation)
function PivotButton() {
  const [searchParams] = useSearchParams();
  const processId = searchParams.get('process_id');
  const from = searchParams.get('from');
  const to = searchParams.get('to');
  // builds links using these values
}

// After (props-driven)
interface PivotButtonProps {
  processId: string;
  timeRangeFrom?: string;
  timeRangeTo?: string;
}

function PivotButton({ processId, timeRangeFrom, timeRangeTo }: PivotButtonProps) {
  // builds links using props
}
```

**LogRenderer refactor:**
```typescript
// Before (violation) - mixes props and URL reading
function LogRenderer({ config, rawTimeRange, ... }: ScreenRendererProps) {
  const [searchParams] = useSearchParams();
  const level = searchParams.get('level');  // WRONG: reading URL
  // ...
}

// After (props-driven) - ScreenPage passes filter state
// Option 1: Extend ScreenRendererProps with filter state
// Option 2: LogRenderer reads from config object (already passed as prop)
```

#### Components Already Correct (no changes needed)

1. **XYChart** - props-driven, commit-on-release via uPlot
2. **MetricsChart** - receives config as props, uses callbacks
3. **PropertyTimeline** - receives selected properties as props
4. **MetricsRenderer** - uses `useTimeRangeSync` correctly (receives rawTimeRange as prop)
5. **ProcessListRenderer** - uses `useTimeRangeSync` correctly

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

**What changes:** The parent components (PerformanceAnalysisPage, etc.) pass a callback that updates config:

```typescript
// Before: callback updates URL directly
onTimeRangeSelect={(from, to) => {
  navigate(`?from=${from.toISOString()}&to=${to.toISOString()}`)
}}

// After: callback updates config (which handles URL sync atomically)
onTimeRangeSelect={(from, to) => {
  updateConfig({ timeRangeFrom: from.toISOString(), timeRangeTo: to.toISOString() });
}}
```

### Phase 6: Deprecate Old Hooks

Remove URL-driven hooks:
- `useTimeRange()` - replace with config pattern
- Direct `useSearchParams()` for view state - replace with config

**Note on useTimeRangeSync:** This hook remains for user-defined screens only. It solves a different problem than `useScreenConfig`.

User-defined screens have **two sources of truth**:
- **DB config** - the persisted, saved state
- **URL time range** - a session overlay that can differ from saved state

`useTimeRangeSync` bridges these by:
1. Detecting when URL time range changes (user picked a preset, dragged to zoom, etc.)
2. Updating the working config with new time range values
3. Marking "unsaved changes" if the time range differs from the saved DB config

This enables the "unsaved changes" indicator and save functionality - users can adjust time range via URL, then save it back to the DB.

**Built-in pages don't need this** because they have no persistence layer. Their config is purely ephemeral (session state), and URL is just a projection for sharing. There's no "saved vs. unsaved" distinction to track.

## Files to Modify

| File | Change |
|------|--------|
| `src/hooks/useScreenConfig.ts` | NEW - Config state + popstate handling |
| `src/lib/screen-config.ts` | NEW - Shared config type definitions |
| `src/lib/url-params.ts` | NEW - URL parsing and building utilities |
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
| `src/components/layout/TimeRangePicker/index.tsx` | REFACTOR - Convert from hook-driven to props-driven (Phase 3) |
| `src/components/layout/PivotButton.tsx` | REFACTOR - Remove useSearchParams, receive processId/timeRange as props (Phase 5) |
| `src/lib/screen-renderers/LogRenderer.tsx` | REFACTOR - Remove useSearchParams for filters, read from config prop (Phase 5) |
| `docs/screen-config-architecture.md` | RENAME & EXPAND - Become `docs/state-management-architecture.md`, cover unified pattern (Phase 7) |

## Migration Strategy

1. Create `useScreenConfig` hook and URL utilities alongside existing code
2. **Refactor TimeRangePicker to props-driven** (required before page migrations)
3. Migrate one built-in page at a time, starting with simplest (ProcessesPage)
4. Keep `useTimeRange()` working during migration for any pages not yet migrated
5. Once all pages migrated, remove old hooks
6. Update ScreenPage to use shared utilities if beneficial

**Recommended migration order:**
1. **TimeRangePicker** - prerequisite for all pages (Phase 3)
2. **ProcessesPage** - simplest, no identity param, validates TimeRangePicker integration
3. **ProcessLogPage** - has identity param, exercises the key pattern
4. **ProcessMetricsPage** - similar to ProcessLogPage
5. **PerformanceAnalysisPage** - most complex (scaleMode, properties array)
6. **Documentation** - update architecture doc after patterns are proven (Phase 7)

## Decisions

1. **URL sync: Combined updateConfig handles both state and navigation**

   The `updateConfig` function atomically updates React state AND navigates, preventing drift between state and URL. The controller decides push vs replace via options.

   | Change Type | Method | Examples |
   |-------------|--------|----------|
   | Navigational | `updateConfig(partial)` | Time range (zoom, presets) |
   | Editing | `updateConfig(partial, { replace: true })` | Scale mode, log level, sort order, search filter |

   Implementation in each handler:

   ```typescript
   // Time range: creates history entry (user can go back)
   const handleTimeRangeChange = (from: string, to: string) => {
     updateConfig({ timeRangeFrom: from, timeRangeTo: to });  // pushState (default)
   };

   // Scale mode: replaces current entry (no back navigation)
   const handleScaleModeChange = (mode: 'p99' | 'max') => {
     updateConfig({ scaleMode: mode }, { replace: true });  // replaceState
   };
   ```

   **Why combined?** Separate `updateConfig()` + `navigate()` calls risk drift if a developer forgets the navigate call or passes wrong params. The combined approach guarantees state and URL stay in sync.

   Using React Router's `navigate()` internally keeps React Router in sync with the URL, avoiding stale `useSearchParams()` values elsewhere in the app.

2. **Browser back/forward**: Popstate restores config from defaults + URL

   When user clicks back/forward, behave like a fresh page load of that URL:

   ```typescript
   const defaultsRef = useRef(defaults);

   useEffect(() => {
     const handlePopstate = () => {
       const fromUrl = parseUrlParams(new URLSearchParams(location.search));
       setConfig({ ...defaultsRef.current, ...fromUrl });  // NOT merged with prev
     };
     window.addEventListener('popstate', handlePopstate);
     return () => window.removeEventListener('popstate', handlePopstate);
   }, []);  // No dependencies - ref is stable
   ```

   Key: `{ ...defaultsRef.current, ...fromUrl }` not `{ ...prev, ...fromUrl }`. This ensures back button restores the exact state, including resetting any fields not in the URL to their defaults.

3. **Manual URL edits**: Handled naturally

   When user edits URL directly and hits enter, it's a full page reload. The component mounts fresh and `useState` initializer reads the URL. No special handling needed.

4. **Cross-screen time range linking**: Context-dependent, not a global toggle.

   | Navigation | Time Range Behavior | Rationale |
   |------------|---------------------|-----------|
   | Process list → Process info | Reset to process lifetime | Show full context for this process |
   | Process info → Process logs/metrics/performance | Reset to last hour | Sensible default for detail views |
   | Between process detail pages (logs ↔ metrics ↔ performance) | **Preserve** | Leverage implicit temporal correlation |
   | Any → User-defined screen | Use screen's saved config | Respect user's configured defaults |
   | Any → User-defined screen (with URL time params) | URL overrides saved config | Treat as manual change (creates unsaved diff) |

   For user-defined screens: URL params act as if the user changed the time range manually after loading. This creates a difference that can be saved back to the screen config.

5. **Component reuse with different URL params**: Key content components on identity params.

   **Problem:** Routes use query params (e.g., `/process_log?process_id=X`). When navigating from `?process_id=A` to `?process_id=B`, React Router reuses the component since the path is identical. The `useState` initializer in `useScreenConfig` only runs once per mount, so config retains stale values.

   **Solution:** Key the content component on identity params to force remount:

   ```typescript
   // In the page's default export (the wrapper)
   export default function ProcessLogPage() {
     const [searchParams] = useSearchParams()
     const processId = searchParams.get('process_id')

     return (
       <AuthGuard>
         <Suspense fallback={<PageLoader />}>
           <ProcessLogContent key={processId} />
         </Suspense>
       </AuthGuard>
     )
   }
   ```

   This ensures:
   - Content component remounts when processId changes
   - `useScreenConfig` initializes fresh with new URL params
   - All refs and state reset naturally
   - Suspense boundary can show loading state during transition

   **Identity params by page:**
   | Page | Identity Param |
   |------|----------------|
   | ProcessLogPage | `process_id` |
   | ProcessMetricsPage | `process_id` |
   | PerformanceAnalysisPage | `process_id` |
   | ProcessesPage | (none - no identity param) |

### Phase 7: Documentation Updates

The refactor establishes new architectural patterns that should be documented. Update the existing `docs/screen-config-architecture.md` to become the single source of truth for state management across the entire app.

#### Restructure screen-config-architecture.md

**Current state:** Documents only user-defined screens (ScreenPage/Renderer pattern).

**Target state:** Unified architecture doc covering both user-defined screens AND built-in pages.

**Proposed structure:**

```markdown
# State Management Architecture

## Overview
- MVC pattern overview
- Config as source of truth
- URL as projection

## Core Concepts

### MVC Roles
- Model: Config object
- View: Components (receive props, dispatch callbacks)
- Controller: Screen/Page (handles actions, decides navigation)

### State Ownership Rules
1. Config owns view state
2. Components never read URL directly
3. Only controllers (pages) read URL params
4. URL sync is explicit (controller calls navigate)

## URL Conventions

### Parameter Mapping
| Config Field | URL Param | Type |
|--------------|-----------|------|
| processId | process_id | string |
| timeRangeFrom | from | string |
| timeRangeTo | to | string |
| selectedMeasure | measure | string |
| selectedProperties | properties | comma-separated |
| scaleMode | scale | string |
| logLevel | level | string |
| logLimit | limit | number |
| search | search | string |
| sortField | sort | string |
| sortDirection | dir | string |

### Navigation Semantics
| Change Type | Method | When to Use |
|-------------|--------|-------------|
| pushState | `navigate(url)` | Navigational changes (time range, entity selection) |
| replaceState | `navigate(url, { replace: true })` | Editing changes (filters, sort, display options) |

## Built-in Pages

### Pattern
- useScreenConfig hook manages config state
- Page component is the controller
- Child components receive props and callbacks
- Page decides push vs replace for each action

### Identity Params
Pages key their content component on identity params to force remount:
- ProcessLogPage: `process_id`
- ProcessMetricsPage: `process_id`
- PerformanceAnalysisPage: `process_id`
- ProcessesPage: (none)

### Example Flow
[Include the MVC diagram from this refactor plan]

## User-Defined Screens (ScreenPage)

### Pattern
- Config persisted to database
- Renderer owns config, tracks changes
- ScreenPage manages save/load lifecycle
- URL params override saved config (creates unsaved diff)

### Change Tracking
[Keep existing change tracking documentation]

### Data Flow
[Keep existing data flow documentation]

## Shared Utilities

### useScreenConfig Hook
- Initializes from URL on mount
- Handles popstate (browser back/forward)
- Returns stable updateConfig callback

### URL Utilities
- parseUrlParams(): URL → config partial
- buildUrlParams(): config → URLSearchParams

## Time Range Handling

### Relative vs Absolute
- Presets store relative strings: `now-1h`, `now`
- Drag-zoom stores absolute ISOs
- Config preserves user intent

### Cross-Screen Behavior
[Include the navigation table from this refactor plan]
```

#### Key Changes from Current Doc

1. **Scope expansion:** Cover built-in pages, not just ScreenPage
2. **MVC framing:** Formalize the controller/view/model terminology
3. **URL conventions:** Document param mapping as contract
4. **Navigation semantics:** Clarify push vs replace decision rules
5. **Shared utilities:** Document the new hooks and utilities

#### Implementation Notes

1. Keep the doc in `docs/screen-config-architecture.md` (same location, expanded scope)
2. Rename file to `docs/state-management-architecture.md` for clarity
3. Add diagrams using ASCII art (consistent with existing style)
4. Include code examples for common patterns
5. Link from README.md architecture section

## Open Questions

(None currently)

## Implementation Status

**Completed: January 2026**

All core implementation phases have been completed:
- [x] Phase 1: Created `src/lib/screen-config.ts` with base config types
- [x] Phase 2: Created `src/lib/url-params.ts` with URL parsing utilities
- [x] Phase 2: Created `src/hooks/useScreenConfig.ts` hook
- [x] Phase 3: Refactored TimeRangePicker to props-driven component
- [x] Phase 4: Migrated ProcessesPage to config pattern
- [x] Phase 4: Migrated ProcessLogPage to config pattern
- [x] Phase 4: Migrated ProcessMetricsPage to config pattern
- [x] Phase 4: Migrated PerformanceAnalysisPage to config pattern
- [x] Phase 5: Refactored PivotButton to receive props
- [x] Phase 5: LogRenderer already props-driven (receives timeRange via ScreenRendererProps)
- [x] Phase 6: Added deprecation notice to useTimeRange hook

- [x] Phase 7: Renamed `screen-config-architecture.md` to `state-management-architecture.md` with unified MVC docs

## Success Criteria

- [x] No more chart flickering on built-in pages
- [x] No race conditions when changing multiple params quickly
- [x] Clear ownership: config owns state, URL is just a projection
- [x] Stable callbacks that don't cause re-renders
- [x] Built-in pages follow same pattern as user-defined screens
- [x] Drag-to-zoom updates config (not URL directly) on mouse release
- [x] Path to "save view as custom screen" is clear
- [x] Architecture doc updated to cover unified pattern (Phase 7)
