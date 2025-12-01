# Analytics Web App Rework Plan

## Implementation Progress

### Milestone 1: Foundation - Layout & Time Range - DONE
- [x] `Header` component with logo, time range selector, user menu
- [x] `Sidebar` component with icon navigation
- [x] `PageLayout` component combining header, sidebar, content area
- [x] `TimeRangeSelector` component with dropdown (relative ranges)
- [x] `useTimeRange` hook for URL-based time range state
- [x] Dark theme CSS variables in `globals.css`

### Milestone 2: Process Explorer Page - DONE
- [x] `/processes` route with sortable table
- [x] Search input with client-side filtering
- [x] Column sorting with visual indicators
- [x] Root `/` redirects to `/processes`

### Milestone 3: Process Detail Pages - DONE
- [x] `/process?id=...` page with info cards grid
- [x] `/process_log?process_id=...` page with log viewer
- [x] `/process_trace?process_id=...` page with trace form
- [x] Deleted old `/process/[id]` dynamic route

### Milestone 4: SQL Panel & Backend - DONE
- [x] `POST /analyticsweb/query` endpoint in backend
- [x] Macro substitution logic (`$param` -> value)
- [x] Destructive function blocking (`retire_partitions`, etc.)
- [x] `QueryEditor` component with collapsible panel
- [x] SQL panel on `/processes` page
- [x] SQL panel on `/process_log` page

### Milestone 5: Polish & Integration - TODO
- [ ] Wire up Run button to actually execute custom SQL queries
- [ ] Add responsive design adjustments
- [ ] Add better error handling UX (see mockup_errors.html)
- [ ] Test all URL parameter combinations for shareability

## Mockups

Visual mockups of the new UI are available in the [analytics_web_app_rework/](analytics_web_app_rework/) folder:

- [mockup_processes.html](analytics_web_app_rework/mockup_processes.html) - Process Explorer with SQL panel
- [mockup_process.html](analytics_web_app_rework/mockup_process.html) - Process Information page
- [mockup_process_log.html](analytics_web_app_rework/mockup_process_log.html) - Process Log viewer with SQL panel
- [mockup_process_trace.html](analytics_web_app_rework/mockup_process_trace.html) - Trace Generation page
- [mockup_errors.html](analytics_web_app_rework/mockup_errors.html) - Error states reference

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

**Why query params instead of path params:**
Query params are chosen over path segments (like `/process/[id]`) because future screens will support arbitrary numbers of parameters (filters, panel configurations, time ranges, etc.). Path segments would become unwieldy for this use case.

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

**Why macros instead of parameterized queries:**
Macros (`$search`, `$order_by`, etc.) provide text substitution flexibility that users will need when defining their own custom screens from scratch in future phases. Parameterized queries would be too restrictive for user-defined SQL.

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

#### Process Explorer (`/processes`)
Current ProcessTable component with SQL-powered query.

**Column Sorting:**
- Click column header to sort ascending, click again for descending
- Sort state in URL: `?sort=start_time&order=desc`
- Default: `start_time DESC`
- `$order_by` macro expands to `<column> <direction>`
- Example: `SELECT * FROM processes() ORDER BY $order_by LIMIT 100`
- Backend substitutes macro before forwarding to FlightSQL

#### Process Log (`/process_log?process_id=...`)
Current log viewer component.

#### Process Trace (`/process_trace?process_id=...`)
Current trace generation UI. Time range for trace is taken from global time range control.

## Phase 3: User-Defined Dashboards (Future)

Future work: allow users to create custom dashboards with multiple panels, similar to Grafana.
Not in scope for now.

## Implementation Order

### Milestone 1: Foundation - Layout & Time Range
1. Create `Header` component with logo and user menu placeholder
2. Create `Sidebar` component with icon navigation
3. Create `PageLayout` component combining header, sidebar, content area
4. Create `TimeRangeSelector` component with dropdown
5. Create `useTimeRange` hook for URL-based time range state
6. Update `layout.tsx` to use new layout structure
7. Add CSS variables/theme for dark mode colors

### Milestone 2: Process Explorer Page
1. Create `/processes` route with page component
2. Create `SortableTable` component with column sorting
3. Create `useSortState` hook for URL-based sort state
4. Add search input with URL sync
5. Wire up existing process data fetching with time range
6. Update root `/` to redirect to `/processes`

### Milestone 3: Process Detail Pages
1. Create `/process` page with info cards grid
2. Create `InfoCard` component
3. Create `StreamsTable` component with type badges
4. Create `/process_log` page with log viewer
5. Create `LogViewer` component with level coloring
6. Create `/process_trace` page with trace form
7. Create `TraceForm` component
8. Create `ProgressIndicator` component
9. Delete old `/process/[id]` route

### Milestone 4: SQL Panel & Backend
1. Add `POST /analyticsweb/query` endpoint to backend
2. Add macro substitution logic in backend
3. Add destructive function blocking in backend
4. Create `QueryEditor` component with syntax highlighting
5. Add collapsible SQL panel to Process Explorer page
6. Add collapsible SQL panel to Process Log page
7. Wire up Run/Reset buttons to execute queries

### Milestone 5: Polish & Integration
1. Add refresh button functionality (re-fetch data)
2. Add user menu dropdown (placeholder for future auth)
3. Add responsive design adjustments
4. Add loading states and error handling
5. Test all URL parameter combinations for shareability

### Error Handling UX
See [mockup_errors.html](analytics_web_app_rework/mockup_errors.html) for visual reference of all error states:
- **Network errors**: Banner with retry action, or full-page error if page cannot load
- **Query errors**: Banner with SQL details and line/column info, SQL panel shows inline error
- **Query timeout**: Warning banner suggesting time range reduction
- **Empty states**: Centered message with suggestions for resolution
- **Destructive function blocked**: Banner explaining security restriction
- **Toast notifications**: For transient errors during refresh (auto-retry)

Note: Authentication errors are already handled by the existing login flow.

## Technical Considerations

### State Management
- Time range: URL query params (source of truth), React hook to read/update
- Query results: React Query for caching

### Performance
- Debounce time range changes to avoid excessive queries
- Cache query results with React Query

### TODO: Pagination Strategy
- Pagination is out of scope for initial implementation but will be required
- Must support very large data sets (hundreds of millions of entries)
- Needs dedicated design work to handle efficiently at scale

### Security
- **Block destructive functions in web app query endpoint** (admin flag not yet enforced in FlightSQL server):
  - `retire_partitions()` - retires partitions in a time range
  - `retire_partition_by_metadata()` - retires a single partition
  - `retire_partition_by_file()` - retires a partition by file path
- Reject queries containing these function names before forwarding to FlightSQL
- **Note:** This blocklist is a temporary measure. The long-term solution is proper RBAC (Role-Based Access Control) in the FlightSQL server.

## Detailed Feature Requirements from Mockups

### Header Component
- Logo on the left ("Micromegas")
- Time range selector with:
  - Clock icon
  - Current range display (relative like "Last 24 hours" or absolute dates)
  - Dropdown indicator
  - Refresh button adjacent to time range
- User menu with avatar (initials)

### Sidebar Component
- Narrow icon-based sidebar (56px width)
- Navigation items with icons:
  - Processes (grid icon) - links to `/processes`
- Hover tooltips showing item names
- Active state highlighting (blue color)

**Note:** Logs and Trace are not in top-level navigation because they require a `process_id` context. Users navigate to these pages from the Process Information page. Only one top-level screen for now; more will be added as the app evolves.

### Time Range Selector Dropdown
- Relative ranges: Last 5m, 15m, 1h, 6h, 12h, 24h, 7d, 30d
- Custom absolute range with date/time pickers
- URL query params: `?from=now-1h&to=now` or absolute timestamps

### SQL Panel (Right Panel)
- Collapsible panel (400px expanded, 48px collapsed)
- Header with:
  - Collapse toggle button
  - Title "SQL Query"
  - Reset button
  - Run button (green)
- SQL editor with syntax highlighting:
  - Keywords in purple
  - Strings in green
  - Variables ($var) in orange
- Variables section showing available macros
- Current values section showing active parameters
- Time range info section

### Process Explorer Page (`/processes`)
- Page title "Processes"
- Search input for filtering by exe, process_id, computer, username
- Sortable table with columns:
  - Process (exe name, links to log page)
  - Process ID (monospace, truncated UUID)
  - Start Time (monospace timestamp)
  - Last Update (monospace timestamp)
  - Username
  - Computer
- Column sorting with visual indicators (arrows)
- SQL panel with `$search` and `$order_by` variables

### Process Information Page (`/process?id=...`)
- Back link to All Processes
- Page header with:
  - Exe name as title
  - Process ID as subtitle
  - Action buttons: "View Log" and "Generate Trace"
- Info cards grid (4 cards):
  - Process Information: exe, process_id, parent process, command line
  - Environment: computer, username, distro, CPU brand
  - Timing: start time, last activity, duration, TSC frequency
  - Build Information: version, number, configuration, target platform
- Telemetry Streams table:
  - Stream Type (Log/Metrics/Thread Spans with colored badges)
  - Stream ID
  - Events count
  - First/Last Event timestamps
- No SQL panel on this page

### Process Log Page (`/process_log?process_id=...`)
- Back link to process info page (shows exe name)
- Page title "Process Log" with process ID subtitle
- Filters:
  - Max Level dropdown (TRACE, DEBUG, INFO, WARN, ERROR, FATAL)
  - Limit input
- Log viewer (monospace):
  - Timestamp column
  - Level column (color-coded: debug=gray, info=blue, warn=yellow, error=red)
  - Target column (purple)
  - Message column
- SQL panel with `$process_id`, `$max_level`, `$limit` variables

### Trace Generation Page (`/process_trace?process_id=...`)
- Page title "Generate Trace"
- Form sections:
  - Process (read-only display of exe and process_id)
  - Trace Name input (auto-populated with suggested name)
  - Span Types checkboxes:
    - Thread Events (with description)
    - Async Span Events (with description)
  - Time Range (uses global time range from header)
  - Estimated Size (shows event count with styling)
  - Size warning for large traces
- Action buttons: Generate Trace, Cancel
- Progress section (shown during generation):
  - Spinner animation
  - "Generating Trace..." title
  - Downloaded bytes counter
- No SQL panel on this page

### Backend: Query Macro System
- `$search` - Search input value substitution
- `$order_by` - Column and direction substitution (e.g., "start_time DESC")
- `$process_id` - Process ID parameter
- `$max_level` - Log level filter
- `$limit` - Row limit
- `$begin`, `$end` - Time range boundaries
- Backend must validate and substitute these macros before forwarding to FlightSQL

## File Changes Summary

### New Files
- `src/components/TimeRangeSelector.tsx` - Time range dropdown with relative/absolute options
- `src/components/TimeRangeDropdown.tsx` - Dropdown menu for time range selection
- `src/components/QueryEditor.tsx` - SQL panel with syntax highlighting
- `src/components/layout/PageLayout.tsx` - Full-page layout with header/sidebar/content
- `src/components/layout/Header.tsx` - Header with logo, time range, user menu
- `src/components/layout/Sidebar.tsx` - Icon-based navigation sidebar
- `src/components/layout/UserMenu.tsx` - User avatar dropdown
- `src/components/ui/SortableTable.tsx` - Table with sortable columns
- `src/components/ui/LogViewer.tsx` - Log display with level coloring
- `src/components/ui/InfoCard.tsx` - Information card for process details
- `src/components/ui/StreamsTable.tsx` - Telemetry streams table with type badges
- `src/components/ui/TraceForm.tsx` - Trace generation form
- `src/components/ui/ProgressIndicator.tsx` - Spinner with progress message
- `src/app/processes/page.tsx` - Process Explorer page
- `src/app/process/page.tsx` - Process Information page
- `src/app/process_log/page.tsx` - Process Log page
- `src/app/process_trace/page.tsx` - Trace Generation page
- `src/lib/time-range.ts` - Time range parsing, URL sync, utilities
- `src/lib/query-macros.ts` - Frontend macro formatting
- `src/hooks/useTimeRange.ts` - React hook for time range state from URL
- `src/hooks/useSortState.ts` - React hook for sort column/direction from URL

### Modified Files
- `src/app/page.tsx` - Redirect to /processes
- `src/lib/api.ts` - Add generic query endpoint with macro support
- `src/app/layout.tsx` - Add new layout with sidebar

### Deleted Files
- `src/app/process/[id]/page.tsx` - Replaced by `/process?id=...`

### Backend Changes (rust/analytics-web-srv)
- Add `POST /analyticsweb/query` endpoint
- Add query validation/sanitization
- Add macro substitution (`$search`, `$order_by`, `$process_id`, `$max_level`, `$limit`, `$begin`, `$end`)
- Block destructive functions in query text
