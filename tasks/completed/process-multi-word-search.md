# Process Page Multi-Word Search

Add multi-word search functionality to the process list page (`/processes`), similar to how the process log page handles search. Search filter SQL should be built entirely in the frontend.

## Current State

**Process Page** (`analytics-web-app/src/app/processes/page.tsx`):
- Single search term only
- Searches across `exe`, `computer`, `username` fields
- Uses simple `LIKE '%$search%'` pattern
- No URL persistence of search term

**Process Log Page** (reference implementation):
- Space-separated words treated as AND conditions
- Each word matches either `target` OR `msg` using ILIKE
- SQL special characters escaped (`\`, `%`, `_`, `'`)
- Search term persisted in URL query params
- Has both frontend and backend expansion (backend should be removed)

## Implementation Plan

### 1. Process Page: Add `expandSearchFilter()` function

**File:** `analytics-web-app/src/app/processes/page.tsx`

Build search filter SQL in frontend:
- Split search by whitespace
- Escape SQL special characters (`\`, `%`, `_`, `'`)
- Each word creates: `(exe ILIKE '%word%' OR computer ILIKE '%word%' OR username ILIKE '%word%')`
- Multiple words joined with AND
- Return empty string if no search terms

Example output for "chrome dev":
```sql
AND (exe ILIKE '%chrome%' OR computer ILIKE '%chrome%' OR username ILIKE '%chrome%')
AND (exe ILIKE '%dev%' OR computer ILIKE '%dev%' OR username ILIKE '%dev%')
```

### 2. Process Page: Update SQL template

**File:** `analytics-web-app/src/app/processes/page.tsx`

Change from:
```sql
WHERE exe LIKE '%$search%'
   OR computer LIKE '%$search%'
   OR username LIKE '%$search%'
```

To:
```sql
WHERE 1=1
  ${searchFilter}
```

Where `searchFilter` is the result of `expandSearchFilter(searchTerm)` interpolated directly into the SQL string before sending to backend.

### 3. Backend: Remove `expand_search_filter()` and related code

**File:** `rust/analytics-web-srv/src/main.rs`

- Remove `expand_search_filter()` function
- Remove `$search_filter` macro substitution in `execute_sql_query()`
- Remove `search` parameter extraction

### 4. Process Log Page: Update to use frontend-only expansion

**File:** `analytics-web-app/src/app/process_log/page.tsx`

- Already has `expandSearchFilter()` function
- Update SQL template to interpolate the filter directly instead of using `$search_filter` placeholder
- Remove passing `search` param to backend

### 5. Optional: URL persistence for processes page

Persist search term in URL query params for shareability:
- Add `useSearchParams` hook
- Sync search state with URL
- Initialize from URL on page load

## Testing

1. Single word search still works
2. Multi-word search with AND logic (all words must match)
3. Special characters (`%`, `_`, `'`, `\`) are properly escaped
4. Empty search returns all results
5. Debounce still works (300ms delay)

## Files to Modify

- `analytics-web-app/src/app/processes/page.tsx` - Add frontend search expansion
- `analytics-web-app/src/app/process_log/page.tsx` - Use frontend-only expansion
- `rust/analytics-web-srv/src/main.rs` - Remove backend search expansion
