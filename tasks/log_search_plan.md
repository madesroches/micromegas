# Log Screen Search Implementation Plan

## Overview

Add multi-word search functionality to the Process Log screen in the analytics web app. Users can enter multiple space-separated words, and log entries matching ALL fragments will be returned. A fragment matches if it can be found in either the `target` or `msg` field.

## Example

Search input: `error database`

Returns entries where:
- (`target` contains "error" OR `msg` contains "error") AND
- (`target` contains "database" OR `msg` contains "database")

## Implementation Steps

### Step 1: Backend - Add Search Filter Expansion

**File**: `rust/analytics-web-srv/src/main.rs`

Add special handling for a `$search_filter` variable that expands a space-separated search string into SQL LIKE clauses.

```rust
// In the variable substitution logic, detect $search_filter
// Input: "error database"
// Output: AND (target ILIKE '%error%' OR msg ILIKE '%error%') AND (target ILIKE '%database%' OR msg ILIKE '%database%')
```

Implementation:
1. Before standard variable substitution, check if `params` contains a `search` key
2. Parse the search string into words (split on whitespace, filter empty)
3. For each word, escape SQL special characters (`%`, `_`, `'`)
4. Generate the WHERE clause fragment
5. Replace `$search_filter` in the SQL with the generated clause
6. If search is empty, replace `$search_filter` with empty string

Use `ILIKE` for case-insensitive matching (PostgreSQL/DataFusion supported).

### Step 2: Frontend - Update Default SQL

**File**: `analytics-web-app/src/app/process_log/page.tsx`

Update `DEFAULT_SQL` to include the search filter placeholder:

```typescript
const DEFAULT_SQL = `SELECT time, level, target, msg
FROM log_entries
WHERE process_id = '$process_id'
  AND level <= $max_level
  $search_filter
ORDER BY time DESC
LIMIT $limit`
```

### Step 3: Frontend - Add Search State

**File**: `analytics-web-app/src/app/process_log/page.tsx`

Add state and URL sync for search:

```typescript
// Read from URL
const searchParam = searchParams.get('search')
const initialSearch = searchParam || ''

// State
const [search, setSearch] = useState<string>(initialSearch)
const [searchInputValue, setSearchInputValue] = useState<string>(initialSearch)
```

### Step 4: Frontend - Add Search Input Component

**File**: `analytics-web-app/src/app/process_log/page.tsx`

Add search input to the filters section (before the level dropdown):

```tsx
<div className="flex gap-3 mb-4">
  {/* Search Input */}
  <div className="flex items-center gap-2">
    <input
      type="text"
      value={searchInputValue}
      onChange={(e) => setSearchInputValue(e.target.value)}
      onBlur={handleSearchBlur}
      onKeyDown={handleSearchKeyDown}
      placeholder="Search target or message..."
      className="w-64 px-3 py-2 bg-app-panel border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link placeholder:text-theme-text-muted"
    />
  </div>

  {/* Existing level dropdown and limit inputs */}
  ...
</div>
```

### Step 5: Frontend - URL Sync for Search

**File**: `analytics-web-app/src/app/process_log/page.tsx`

Add handler functions to sync search with URL:

```typescript
const updateSearch = useCallback(
  (value: string) => {
    setSearch(value)
    const params = new URLSearchParams(searchParams.toString())
    if (value.trim() === '') {
      params.delete('search')
    } else {
      params.set('search', value.trim())
    }
    router.push(`${pathname}?${params.toString()}`)
  },
  [searchParams, router, pathname]
)

const handleSearchBlur = useCallback(() => {
  updateSearch(searchInputValue)
}, [searchInputValue, updateSearch])

const handleSearchKeyDown = useCallback(
  (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      e.currentTarget.blur()
    }
  },
  []
)
```

### Step 6: Frontend - Pass Search to API

**File**: `analytics-web-app/src/app/process_log/page.tsx`

Update `loadData` to include search in params:

```typescript
const loadData = useCallback(
  (sql: string = DEFAULT_SQL) => {
    if (!processId) return
    setQueryError(null)
    const params: Record<string, string> = {
      process_id: processId,
      max_level: String(LOG_LEVELS[logLevel] || 6),
      limit: String(logLimit),
      search: search,  // Add this
    }
    sqlMutateRef.current({
      sql,
      params,
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
    })
  },
  [processId, logLevel, logLimit, search, apiTimeRange]  // Add search to deps
)
```

### Step 7: Frontend - Update Variables List

**File**: `analytics-web-app/src/app/process_log/page.tsx`

Add search to the VARIABLES array for the QueryEditor panel:

```typescript
const VARIABLES = [
  { name: 'process_id', description: 'Current process ID' },
  { name: 'max_level', description: 'Max log level filter (1-6)' },
  { name: 'limit', description: 'Row limit' },
  { name: 'search', description: 'Search terms (space-separated)' },  // Add this
]
```

Update `currentValues` to include search.

### Step 8: Frontend - Trigger Reload on Search Change

**File**: `analytics-web-app/src/app/process_log/page.tsx`

Add search to the filter change effect:

```typescript
const prevFiltersRef = useRef<{ logLevel: string; logLimit: number; search: string } | null>(null)
useEffect(() => {
  if (!hasLoaded) return

  if (prevFiltersRef.current === null) {
    prevFiltersRef.current = { logLevel, logLimit, search }
    return
  }

  if (prevFiltersRef.current.logLevel !== logLevel ||
      prevFiltersRef.current.logLimit !== logLimit ||
      prevFiltersRef.current.search !== search) {
    prevFiltersRef.current = { logLevel, logLimit, search }
    loadData()
  }
}, [logLevel, logLimit, search, hasLoaded, loadData])
```

## Backend Implementation Details

### Search Filter Expansion Logic

In `execute_sql_query()` handler (around line 573-657 in main.rs):

```rust
fn expand_search_filter(search: &str) -> String {
    let words: Vec<&str> = search.split_whitespace().collect();
    if words.is_empty() {
        return String::new();
    }

    let clauses: Vec<String> = words.iter().map(|word| {
        // Escape SQL special characters
        let escaped = word
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
            .replace('\'', "''");
        format!("(target ILIKE '%{}%' OR msg ILIKE '%{}%')", escaped, escaped)
    }).collect();

    format!("AND {}", clauses.join(" AND "))
}
```

Apply before other variable substitutions:
1. Get `search` param from request
2. Expand to SQL clause using `expand_search_filter()`
3. Replace `$search_filter` in SQL with the result
4. Continue with regular variable substitution

## Testing

1. **Empty search**: Returns all entries (within level/limit constraints)
2. **Single word**: Filters to entries containing word in target OR msg
3. **Multiple words**: Filters to entries containing ALL words (each in target OR msg)
4. **Special characters**: Test with `%`, `_`, `'` in search - should be escaped
5. **Case insensitivity**: "ERROR" and "error" should match the same entries
6. **URL persistence**: Search term should appear in URL and survive page refresh
7. **Clear search**: Emptying the input should remove filter and return all entries

## UI/UX Notes

- Search input width: ~256px (w-64)
- Placeholder text: "Search target or message..."
- Search applies on Enter or blur (same pattern as limit input)
- Results count updates to reflect filtered count
- Search term visible in SQL panel's current values section
