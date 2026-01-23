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

```
URL params ─────────────────────────────────────────────┐
                                                        │
    ┌───────────────────────────────────────────────────▼───────┐
    │                    useScreenConfig                        │
    │  ┌─────────────────────────────────────────────────────┐  │
    │  │ config: {                                           │  │
    │  │   timeRangeFrom, timeRangeTo,                       │  │
    │  │   variables: { process_filter: 'x', level: 'y' }    │  │
    │  │ }                                                   │  │
    │  └─────────────────────────────────────────────────────┘  │
    └───────────────────────────────────────────────────────────┘
                                │
                                ▼
    ┌───────────────────────────────────────────────────────────┐
    │                  NotebookRenderer                         │
    │  - Reads variables from config on mount                   │
    │  - Passes to useNotebookVariables as initial values       │
    │  - Updates config when variables change                   │
    └───────────────────────────────────────────────────────────┘
```

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
function buildUrl(cfg: ScreenPageConfig): string {
  const params = new URLSearchParams()
  if (cfg.timeRangeFrom !== DEFAULT_CONFIG.timeRangeFrom) {
    params.set('from', cfg.timeRangeFrom)
  }
  if (cfg.timeRangeTo !== DEFAULT_CONFIG.timeRangeTo) {
    params.set('to', cfg.timeRangeTo)
  }
  // NEW: Add variable params (skip reserved names as safety check)
  for (const [name, value] of Object.entries(cfg.variables || {})) {
    if (value !== undefined && value !== '' && !isReservedParam(name)) {
      params.set(name, value)
    }
  }
  const search = params.toString()
  return search ? `?${search}` : ''
}
```

### Phase 3: Wire Variables to NotebookRenderer

**File:** `src/lib/screen-renderers/NotebookRenderer.tsx`

Pass URL variables to the notebook and sync changes back:

```typescript
interface NotebookRendererProps {
  // ... existing props
  urlVariables: Record<string, string>      // NEW: from URL
  onVariablesChange: (vars: Record<string, string>) => void  // NEW: sync back
}

function NotebookRenderer({ urlVariables, onVariablesChange, ... }) {
  const { variableValues, setVariableValue, ... } = useNotebookVariables(
    cells,
    urlVariables  // NEW: pass initial values from URL
  )

  // Sync variable changes back to URL
  useEffect(() => {
    onVariablesChange(variableValues)
  }, [variableValues, onVariablesChange])
}
```

### Phase 4: Update useNotebookVariables Hook

**File:** `src/lib/screen-renderers/useNotebookVariables.ts`

Accept initial values from URL:

```typescript
export function useNotebookVariables(
  cells: CellConfig[],
  initialValues?: Record<string, string>  // NEW: from URL
): UseNotebookVariablesResult {
  const [variableValues, setVariableValues] = useState<Record<string, string>>(
    () => initialValues || {}  // Initialize from URL if provided
  )

  // When initialValues change (e.g., browser back/forward), update state
  useEffect(() => {
    if (initialValues) {
      setVariableValues(prev => {
        // Merge URL values, preserving values not in URL
        const merged = { ...prev }
        for (const [key, value] of Object.entries(initialValues)) {
          if (value !== undefined) {
            merged[key] = value
          }
        }
        return merged
      })
    }
  }, [initialValues])
}
```

### Phase 5: Handle Variable Cell Execution

**File:** `src/lib/screen-renderers/cell-types/VariableCellRenderer.tsx`

Ensure combobox cells respect URL values:

```typescript
// When loading combobox options, check if URL already provides a value
useEffect(() => {
  if (cell.variableType === 'combobox' && options.length > 0) {
    const urlValue = variableValues[cell.name]
    if (urlValue && options.some(o => o.value === urlValue)) {
      // URL value is valid - keep it
      return
    }
    // No URL value or invalid - use default or first option
    const defaultValue = cell.defaultValue || options[0]?.value
    if (defaultValue && variableValues[cell.name] !== defaultValue) {
      setVariableValue(cell.name, defaultValue)
    }
  }
}, [options, cell.name, cell.defaultValue, variableValues])
```

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

### Phase 7: Debounce URL Updates

**File:** `src/routes/ScreenPage.tsx`

Prevent excessive history entries during rapid variable changes:

```typescript
const debouncedUpdateConfig = useMemo(
  () => debounce((newConfig: ScreenPageConfig) => {
    updateConfig(newConfig, { replace: true })  // Replace instead of push
  }, 300),
  [updateConfig]
)

// Use replace: true for variable changes to avoid cluttering history
// Only push new history entry on significant changes (like running a query)
```

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
- Empty string values are not serialized to URL
- On parse, empty values are treated as "no value set"

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

### Automated Testing
- Unit tests for `parseUrlParams` and `buildUrl`
- Unit tests for `useNotebookVariables` with initial values
- Unit tests for `isReservedVariableName` and `validateVariableName`
- Integration test for full URL → state → URL round-trip

## Migration

### Backwards Compatibility
- Existing URLs without `var_` params continue working
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
| `src/routes/ScreenPage.tsx` | Parse/build URL variable params, define `RESERVED_PARAMS` |
| `src/lib/screen-renderers/NotebookRenderer.tsx` | Accept URL vars, sync changes |
| `src/lib/screen-renderers/useNotebookVariables.ts` | Accept initial values, handle sync |
| `src/lib/screen-renderers/notebook-utils.ts` | Add `isReservedVariableName`, `validateVariableName` |
| `src/lib/screen-renderers/cell-types/VariableCellRenderer.tsx` | Respect URL values, show validation error for reserved names |

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
