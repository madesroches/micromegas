# Analytics Web App Rework Plan

## Mockups

Visual mockups of the new UI are available in the [analytics_web_app_rework/](analytics_web_app_rework/) folder:

- [mockup_processes.html](analytics_web_app_rework/mockup_processes.html) - Process Explorer with SQL panel
- [mockup_process.html](analytics_web_app_rework/mockup_process.html) - Process Information page
- [mockup_process_log.html](analytics_web_app_rework/mockup_process_log.html) - Process Log viewer with SQL panel
- [mockup_process_trace.html](analytics_web_app_rework/mockup_process_trace.html) - Trace Generation page

## Vision

Transform the analytics web app into a Grafana-inspired dashboard platform with:
- Full-screen pages instead of tab navigation
- User-configurable screens with SQL-powered panels
- Global time range selection on every page
- Reusable panel components that can be composed into custom dashboards

## Current State

The app currently uses:
- Tab-based navigation within pages
- Hardcoded backend API endpoints (no user-defined SQL)
- No global time range control
- Fixed layout with ProcessTable and process detail views

## Phase 1: Full-Screen Page Architecture

**Goal**: Replace tab navigation with full-screen routed pages.

### 1.1 Remove Tab Navigation
- Convert the home page tabs to separate routes
- Convert process detail tabs (Info, Trace, Log) to separate routes or expandable sections
- Each page occupies the full viewport

### 1.2 Create Page Layout Component
- Header with:
  - Page title/breadcrumb
  - Global time range selector
  - User menu
- Full-height content area
- Optional sidebar for navigation (collapsible)

### 1.3 Route Structure Comparison

**Current routes:**
```
/                  → Home with Process Explorer (tab-based UI)
/login             → Login page
/process/[id]      → Process detail with 3 tabs: Info, Trace, Log
```

**Proposed routes:**
```
/                     → Dashboard home (or redirect to /processes)
/login                → Login page (unchanged)
/processes            → Process Explorer (full screen)
/process?id=...       → Process Overview (info only)
/process_log?process_id=...   → Process Log (full screen)
/process_trace?process_id=... → Trace Generation (full screen)
/settings             → App settings
```

**Changes summary:**
- Move Process Explorer from `/` to `/processes`
- Replace `/process/[id]` with `/process?id=...` (query param)
- Extract tabs to separate pages with query params (`/process_log`, `/process_trace`)
- `/` becomes dashboard landing or redirect to `/processes`
- Add `/settings` for future app configuration

### 1.4 Time Range Selector Component
Create a reusable time range selector similar to Grafana:
- Relative ranges: Last 5m, 15m, 1h, 6h, 12h, 24h, 7d, 30d
- Custom absolute range with date/time pickers
- **Time range stored in URL query params** for shareability:
  - Relative: `?from=now-1h&to=now`
  - Absolute: `?from=2024-01-15T10:00:00Z&to=2024-01-15T11:00:00Z`
- Sharing a URL gives consistent results
- Component reads from URL on mount, updates URL on change
- Pass time range to all data-fetching queries

## Phase 2: SQL-Powered Configurable Pages

**Goal**: Each page is defined by an editable SQL query.

### 2.1 Backend: Generic SQL Endpoint
Add a new endpoint to `analytics-web-srv`:
```
POST /analyticsweb/query
{
  "sql": "SELECT * FROM log_entries WHERE process_id = $process_id",
  "begin": "2024-01-15T10:00:00Z",
  "end": "2024-01-15T11:00:00Z"
}
```
- Forwards query to FlightSQL service with time range headers (same as Grafana plugin)
- FlightSQL server applies implicit time range filtering
- `$begin` and `$end` available as macros in SQL if needed for explicit filtering
- Returns JSON result set

### 2.2 Page Structure
Each page has a defining SQL query that can be edited in the UI.
Results are displayed as a table.
No configuration model for now - each page is hardcoded with its default query.

### 2.3 Editable Query UI
- Each page displays its SQL query (collapsed by default, expandable)
- "Edit Query" button opens inline SQL editor
- Syntax highlighting for SQL
- "Run" button to execute and see results
- "Save" to persist changes
- "Reset" to restore default query

### 2.4 Built-in Pages
Keep current bespoke screens (process table, logs viewer, etc.) with their existing UI.
Each page gets an editable SQL query that powers it.
Time range filtering is applied implicitly by the FlightSQL server.

**Process Explorer (`/processes`)**: Current ProcessTable component
**Process Log (`/process_log?process_id=...`)**: Current log viewer component
**Process Trace (`/process_trace?process_id=...`)**: Current trace generation UI

## Phase 3: User-Defined Dashboards (Future)

Future work: allow users to create custom dashboards with multiple panels, similar to Grafana.
Not in scope for now.

## Implementation Order

### Milestone 1: Foundation
1. Create TimeRangeSelector component
2. Create new page layout with time range in header
3. Add `/processes` route (move ProcessTable there)
4. Add time range to process queries

### Milestone 2: Full-Screen Pages
1. Convert process detail to separate routes
2. Remove all tab navigation
3. Add navigation sidebar or menu
4. Ensure all pages use time range

### Milestone 3: SQL Query Support
1. Add generic query endpoint to backend
2. Create QueryEditor component
3. Add editable SQL to existing pages

## Technical Considerations

### State Management
- Time range: URL query params (source of truth), React hook to read/update
- Query results: React Query for caching

### Performance
- Debounce time range changes to avoid excessive queries
- Cache query results with React Query
- Consider pagination for large result sets

### Security
- **Block destructive functions in web app query endpoint** (admin flag not yet enforced in FlightSQL server):
  - `retire_partitions()` - retires partitions in a time range
  - `retire_partition_by_metadata()` - retires a single partition
  - `retire_partition_by_file()` - retires a partition by file path
- Reject queries containing these function names before forwarding to FlightSQL

## File Changes Summary

### New Files
- `src/components/TimeRangeSelector.tsx`
- `src/components/QueryEditor.tsx`
- `src/components/layout/PageLayout.tsx`
- `src/components/layout/Sidebar.tsx`
- `src/app/processes/page.tsx`
- `src/app/process/page.tsx`
- `src/app/process_log/page.tsx`
- `src/app/process_trace/page.tsx`
- `src/lib/time-range.ts`

### Modified Files
- `src/app/page.tsx` - Redirect to /processes or show dashboard
- `src/lib/api.ts` - Add generic query endpoint
- `src/app/layout.tsx` - Add time range provider

### Deleted Files
- `src/app/process/[id]/page.tsx` - Replaced by `/process?id=...`

### Backend Changes (rust/analytics-web-srv)
- Add `POST /analyticsweb/query` endpoint
- Add query validation/sanitization
