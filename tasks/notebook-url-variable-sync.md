# Notebook URL Variable Synchronization

## Overview

Synchronize notebook variable values to the URL, enabling:
- Shareable notebook URLs with pre-filled variable values
- Browser back/forward navigation through variable state changes
- Bookmark/restore of specific notebook configurations
- Team collaboration with consistent starting states

## Current State

### Variable Storage
Variables are currently stored only in React state (`useNotebookVariables.ts`):
```typescript
const [variableValues, setVariableValues] = useState<Record<string, string>>({})
```

**Limitations:**
- Values lost on page reload
- Cannot share URLs with specific variable values
- No browser history integration
- Cannot bookmark specific configurations

### URL State Pattern (Existing)
Time range is already synced to URL via `useScreenConfig`:
```
/screen/my-notebook?from=now-1h&to=now
```

The `useScreenConfig` hook provides atomic state + URL updates with browser history support.

## Design Principles

1. **Config is the single source of truth** - Variables live in screen config, not in separate React state
2. **URL is read-only on specific events** - Only parsed on mount, popstate, or manual URL edit
3. **Unidirectional data flow** - Config flows down as props, changes flow up via `updateConfig()`
4. **No bidirectional sync** - Components don't sync state back to config; they call `updateConfig()` directly
5. **Consistent with existing patterns** - Follows the same architecture as ProcessLogPage, PerformanceAnalysisPage

## Proposed Solution

### URL Format
Add variable values directly as query parameters:
```
/screen/my-notebook?from=now-1h&to=now&process_filter=pid-123&log_level=ERROR
```

**Parameter naming:**
- Key: sanitized cell name (already enforced by `sanitizeVariableName`)
- Value: URL-encoded variable value
- Reserved names cannot be used as variable names

**Reserved Parameter Names:**
```typescript
const RESERVED_PARAMS = ['from', 'to', 'type'] as const
```

**Example URLs:**
```
# Notebook with process filter and log level
/screen/analysis?process=abc-123&level=WARN

# Notebook with time range and variables
/screen/dashboard?from=now-24h&to=now&env=production&service=api

# Notebook with no variables set (uses defaults)
/screen/dashboard?from=now-1h&to=now
```

### State Flow

**Source of Truth:** The screen config (managed by `useScreenConfig`) is the single source of truth.
The URL is only read on specific events, not continuously synced.

**When URL is read:**
1. Initial page load (mount)
2. Browser back/forward navigation (popstate event)
3. User manually edits URL and presses Enter

**Data flow is unidirectional:**

```
┌─────────────────────────────────────────────────────────────────┐
│                           URL                                    │
│  (read ONLY on: mount, popstate, manual URL edit)               │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼ parseUrlParams (one-way, on events only)
┌─────────────────────────────────────────────────────────────────┐
│                    useScreenConfig                               │
│                    config = SOURCE OF TRUTH                      │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │ config: {                                                 │  │
│  │   timeRangeFrom, timeRangeTo,                             │  │
│  │   variables: { process_filter: 'x', level: 'y' }          │  │
│  │ }                                                         │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼ props (one-way down)
┌─────────────────────────────────────────────────────────────────┐
│                    NotebookRenderer                              │
│  - Receives config.variables as props                            │
│  - Calls updateConfig() when user changes a variable             │
│  - Does NOT maintain separate variable state                     │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼ updateConfig() (one-way up)
┌─────────────────────────────────────────────────────────────────┐
│                    useScreenConfig                               │
│  - Updates config state                                          │
│  - Atomically updates URL (via queueMicrotask)                   │
│  - Uses { replace: true } for variable changes                   │
└─────────────────────────────────────────────────────────────────┘
```

This matches how other screens (ProcessLogPage, PerformanceAnalysisPage) handle URL sync.

### Initialization Priority
When loading a notebook:
1. URL parameters (highest priority - enables sharing)
2. Cell `defaultValue` (fallback when no URL param)
3. First option from SQL query (for combobox with no default)

## Implementation Plan

### Phase 1: Extend Screen Config Types

**File:** `src/lib/screen-config.ts`

Add variables to the notebook screen config:
```typescript
export interface NotebookScreenConfig extends BaseScreenConfig {
  type: 'notebook'
  timeRangeFrom: string
  timeRangeTo: string
  variables: Record<string, string>  // NEW: variable name -> value
}
```

### Phase 2: Update URL Parsing/Building

**File:** `src/routes/ScreenPage.tsx`

Define reserved parameters:
```typescript
const RESERVED_PARAMS = ['from', 'to', 'type'] as const
type ReservedParam = typeof RESERVED_PARAMS[number]

function isReservedParam(key: string): key is ReservedParam {
  return RESERVED_PARAMS.includes(key as ReservedParam)
}
```

Update `parseUrlParams` to extract variable params:
```typescript
function parseUrlParams(searchParams: URLSearchParams): ScreenPageConfig {
  const variables: Record<string, string> = {}
  searchParams.forEach((value, key) => {
    if (!isReservedParam(key)) {
      variables[key] = value
    }
  })
  return {
    timeRangeFrom: searchParams.get('from') || DEFAULT_CONFIG.timeRangeFrom,
    timeRangeTo: searchParams.get('to') || DEFAULT_CONFIG.timeRangeTo,
    type: searchParams.get('type') as ScreenType | undefined,
    variables,  // NEW
  }
}
```

Update `buildUrl` to serialize variables:
```typescript
// Safe URL length threshold (conservative for older browsers/proxies)
const MAX_SAFE_URL_LENGTH = 2000

function buildUrl(cfg: ScreenPageConfig): string {
  const params = new URLSearchParams()
  if (cfg.timeRangeFrom !== DEFAULT_CONFIG.timeRangeFrom) {
    params.set('from', cfg.timeRangeFrom)
  }
  if (cfg.timeRangeTo !== DEFAULT_CONFIG.timeRangeTo) {
    params.set('to', cfg.timeRangeTo)
  }
  // NEW: Add variable params (skip reserved names as safety check)
  // Note: empty strings ARE serialized (as ?name=) to preserve explicit "cleared" state
  for (const [name, value] of Object.entries(cfg.variables || {})) {
    if (value !== undefined && !isReservedParam(name)) {
      params.set(name, value)
    }
  }
  const search = params.toString()
  const url = search ? `?${search}` : ''

  // Warn if URL exceeds safe length (variables may be lost on some browsers/proxies)
  if (url.length > MAX_SAFE_URL_LENGTH) {
    console.warn(
      `URL length (${url.length}) exceeds safe threshold (${MAX_SAFE_URL_LENGTH}). ` +
      `Some variable values may be lost when sharing or bookmarking.`
    )
  }

  return url
}
```

### Phase 3: Wire Variables to NotebookRenderer

**File:** `src/lib/screen-renderers/NotebookRenderer.tsx`

NotebookRenderer receives config (including variables) and calls `updateConfig` directly.
No bidirectional sync needed - config is the source of truth.

```typescript
interface NotebookRendererProps {
  // ... existing props
  config: ScreenPageConfig           // includes variables
  updateConfig: (partial: Partial<ScreenPageConfig>, options?: { replace?: boolean }) => void
}

function NotebookRenderer({ config, updateConfig, ... }) {
  // Handler for variable changes - updates config directly
  const handleVariableChange = useCallback((name: string, value: string) => {
    updateConfig({
      variables: { ...config.variables, [name]: value }
    }, { replace: true })  // Use replace to avoid cluttering history
  }, [config.variables, updateConfig])

  // Pass config.variables and handleVariableChange to variable cells
  // No separate state management needed
}
```

### Phase 4: Simplify useNotebookVariables Hook

**File:** `src/lib/screen-renderers/useNotebookVariables.ts`

The hook no longer owns variable state. Instead, it computes effective values
from config and cell defaults. This avoids state duplication and sync issues.

```typescript
export function useNotebookVariables(
  cells: CellConfig[],
  configVariables: Record<string, string>,  // From config (source of truth)
  onVariableChange: (name: string, value: string) => void
): UseNotebookVariablesResult {

  // Compute effective values: config value → defaultValue → empty
  const variableValues = useMemo(() => {
    const values: Record<string, string> = { ...configVariables }

    // Apply defaults for variables not in config
    for (const cell of cells) {
      if (cell.type === 'variable' && !(cell.name in values)) {
        if (cell.defaultValue) {
          values[cell.name] = cell.defaultValue
        }
      }
    }
    return values
  }, [cells, configVariables])

  // Wrapper that calls the config update
  const setVariableValue = useCallback((name: string, value: string) => {
    onVariableChange(name, value)
  }, [onVariableChange])

  return { variableValues, setVariableValue, ... }
}
```

**Key changes:**
- No `useState` for variables - config is source of truth
- `configVariables` comes from `config.variables` (passed down as props)
- `onVariableChange` calls `updateConfig` (passed down from ScreenPage)
- Defaults are computed, not stored

### Phase 5: Handle Variable Cell Execution

**File:** `src/lib/screen-renderers/cell-types/VariableCellRenderer.tsx`

Ensure combobox cells respect config values and handle invalid values:

```typescript
// When loading combobox options, validate the current value
useEffect(() => {
  if (cell.variableType === 'combobox' && options.length > 0) {
    const currentValue = variableValues[cell.name]

    // If current value is valid, keep it
    if (currentValue && options.some(o => o.value === currentValue)) {
      return
    }

    // Current value is missing or invalid - use default or first option
    const fallbackValue = cell.defaultValue || options[0]?.value
    if (fallbackValue && currentValue !== fallbackValue) {
      setVariableValue(cell.name, fallbackValue)
    }
  }
}, [options, cell.name, cell.defaultValue, variableValues, setVariableValue])
```

**Note:** Since `setVariableValue` calls `updateConfig({ replace: true })`, this will
update the URL to reflect the actual value when an invalid URL value is corrected.

### Phase 6: Validate Variable Names

**File:** `src/lib/screen-renderers/notebook-utils.ts`

Add validation to prevent reserved names:
```typescript
export const RESERVED_PARAMS = ['from', 'to', 'type'] as const

export function isReservedVariableName(name: string): boolean {
  return RESERVED_PARAMS.includes(name as typeof RESERVED_PARAMS[number])
}

export function validateVariableName(name: string): string | null {
  const sanitized = sanitizeVariableName(name)
  if (isReservedVariableName(sanitized)) {
    return `"${sanitized}" is a reserved name and cannot be used for variables`
  }
  return null  // Valid
}
```

**File:** `src/lib/screen-renderers/cell-types/VariableCellRenderer.tsx`

Show validation error when user tries to use reserved name:
```typescript
const validationError = validateVariableName(cell.name)
// Display error in cell header if invalid
```

### Phase 7: Debounce Text Input Changes

**File:** `src/lib/screen-renderers/cell-types/VariableCellRenderer.tsx`

For text/number inputs, debounce at the component level to prevent excessive URL updates
while typing. Uses the existing `useDebounce` hook (value debouncing pattern) from
`src/hooks/useDebounce.ts`, matching the pattern in ProcessLogPage for search input.

**Why value debouncing, not callback debouncing:**
Callback debouncing with `useMemo(() => debounce(fn, 300), [deps])` breaks when
dependencies change frequently. Since `setVariableValue` depends on `config.variables`,
it recreates on every variable change, breaking debounce timing. Value debouncing
avoids this by debouncing the value itself, not the update function.

```typescript
import { useDebounce } from '@/hooks/useDebounce'

// For text/number variable cells
const [localValue, setLocalValue] = useState(variableValues[cell.name] ?? '')
const debouncedValue = useDebounce(localValue, 300)

// Sync debounced value to config
const isInitialRef = useRef(true)
useEffect(() => {
  // Skip initial render to avoid unnecessary URL update on mount
  if (isInitialRef.current) {
    isInitialRef.current = false
    return
  }
  setVariableValue(cell.name, debouncedValue)
}, [debouncedValue, cell.name, setVariableValue])

// Handle input change - immediate local update, debounced config update
const handleChange = (e: React.ChangeEvent<HTMLInputElement>) => {
  setLocalValue(e.target.value)
}

// Sync local value when config changes externally (e.g., browser back/forward)
useEffect(() => {
  setLocalValue(variableValues[cell.name] ?? '')
}, [variableValues[cell.name]])
```

**Note:** Combobox changes don't need debouncing since they're discrete selections.
The `{ replace: true }` option in `updateConfig` prevents history clutter regardless.

## Edge Cases

### Reserved Name Used
When a user names a variable `from`, `to`, or `type`:
- Validation error shown in cell UI
- Variable still works locally but won't sync to URL
- Prompt user to rename

### Variable Renamed
When a variable cell is renamed in the notebook:
- Old URL param becomes orphaned (ignored)
- New name won't have a URL value
- Behavior: Falls back to defaultValue

### Variable Deleted
When a variable cell is removed:
- URL param becomes orphaned (ignored)
- No impact on remaining variables

### Invalid URL Value
When URL contains value not in combobox options:
- Validation during option load
- Falls back to defaultValue or first option
- Updates URL to reflect actual value

### Empty/Whitespace Values
- Empty string values ARE serialized to URL (as `?name=`) to preserve explicit "cleared" state
- On parse, empty string = explicitly cleared, missing key = use default
- This distinguishes "user cleared this" from "user never set this"

### URL Length Exceeded
When URL exceeds 2000 characters (safe threshold for all browsers/proxies):
- Console warning logged with actual length
- URL still generated (modern browsers handle longer URLs)
- Risk: older browsers, proxies, or bookmarking may truncate
- Mitigation: users should keep variable values concise

## Testing Plan

### Manual Testing
1. Load notebook with variables, verify URL shows defaults
2. Change variable values, verify URL updates
3. Copy URL, open in new tab, verify values restored
4. Use browser back/forward, verify state changes
5. Test with special characters in values (encoding)
6. Test combobox with invalid URL value
7. Test text/number inputs
8. Try naming a variable `from`, `to`, or `type` - verify validation error shown
9. Test with many variables / long values, verify console warning when URL > 2000 chars
10. Clear a variable with a default value, verify URL includes `?name=` (empty), shared URL preserves empty

### Automated Testing
- Unit tests for `parseUrlParams` and `buildUrl` with variables
- Unit tests for empty string handling: `buildUrl` serializes `{x: ''}` as `?x=`, `parseUrlParams` returns `{x: ''}`
- Unit tests for `useNotebookVariables` computing effective values from config
- Unit tests for `isReservedVariableName` and `validateVariableName`
- Integration test for URL → config → component → updateConfig → URL flow

## Migration

### Backwards Compatibility
- Existing URLs without variable params continue working
- Variables initialize from cell defaults as before
- No database migration needed (variables stored in URL only)

### No Breaking Changes
- Existing notebooks work unchanged
- New URLs with variables work immediately
- Old URLs without variables use default behavior

## Files to Modify

| File | Changes |
|------|---------|
| `src/lib/screen-config.ts` | Add `variables` to config type |
| `src/routes/ScreenPage.tsx` | Parse/build URL variable params, define `RESERVED_PARAMS`, pass config to renderer |
| `src/lib/screen-renderers/NotebookRenderer.tsx` | Receive config + updateConfig props, create handleVariableChange callback |
| `src/lib/screen-renderers/useNotebookVariables.ts` | Remove internal state, compute values from config, delegate changes to callback |
| `src/lib/screen-renderers/notebook-utils.ts` | Add `isReservedVariableName`, `validateVariableName` |
| `src/lib/screen-renderers/cell-types/VariableCellRenderer.tsx` | Validate config values, debounce text inputs, show validation errors |

## Future Considerations

### Not In Scope
- Persisting variables in notebook config (backend storage)
- Variable presets/saved configurations
- Variable validation rules
- Cross-notebook variable sharing

### Potential Enhancements
- Add "Copy URL with Variables" button
- Show indicator when URL differs from defaults
- Reset button to clear URL variables
