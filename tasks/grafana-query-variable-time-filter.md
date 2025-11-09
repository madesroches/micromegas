# Grafana Query Variable Time Filter

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/579

## Overview
Add a time filter checkbox to query variables in the Grafana plugin, allowing dashboard variables to be constrained by the current time range. This feature already exists for regular queries but is missing from the variable query editor.

## Current State
- Regular queries in `QueryEditor.tsx` have **Time Filter** and **Auto Limit** checkboxes
- Variable queries in `VariableQueryEditor.tsx` only have a simple text input field
- The `SQLQuery` type already supports the `timeFilter` boolean field
- The backend `metricFindQuery` function executes variable queries but doesn't expose time filter options
- Time filter infrastructure is fully implemented in the backend (`query_data.go`) and passes time range via gRPC metadata

## Requirements

### Functional Requirements
- Add a **Time Filter** checkbox to the variable query editor UI
- When enabled, pass dashboard time range to backend as metadata (`query_range_begin` and `query_range_end`)
- Backend analytics service applies time filtering based on the metadata
- Maintain backwards compatibility - existing variables without time filter should continue working
- Persist time filter setting in dashboard JSON

### Optional Enhancements
- Add **Auto Limit** checkbox to variable queries for consistency with regular queries
- Help prevent overwhelming variable dropdowns with too many values

## Implementation Plan

### Phase 1: Update Variable Query UI
**File**: `grafana/src/components/VariableQueryEditor.tsx`

- Add state management for `timeFilter` boolean (similar to `QueryEditor.tsx:91-101`)
- Add checkbox UI component (similar to `QueryEditor.tsx:214`)
- Update query object when timeFilter changes
- Initialize timeFilter from existing query values on component mount
- Add proper TypeScript types

### Phase 2: Update Type Definitions
**File**: `grafana/src/types.ts`

- Verify `VariableQuery` interface includes or extends `timeFilter` field
- Ensure proper defaults are set (currently `DEFAULT_QUERY.timeFilter = true`)
- Consider if `VariableQuery` should extend `SQLQuery` for type safety

### Phase 3: Update Datasource Logic
**File**: `grafana/src/datasource.ts`

- Modify `metricFindQuery()` method to:
  - Accept time range from options parameter
  - Pass `timeFilter` flag through to query execution
  - Ensure time range metadata is properly sent to backend when executing variable queries

### Phase 4: Testing
- Create test dashboard with query variables
- Verify checkbox appears and functions in variable editor
- Test that time filter properly constrains variable query results
- Test variables both with and without time filter enabled
- Test backwards compatibility with existing dashboards
- Verify dashboard JSON serialization/deserialization
- Confirm time range metadata is correctly passed to backend when checkbox is enabled

## Technical Details

### Time Filter Implementation
From `grafana/pkg/flightsql/query_data.go:108-157`:
```go
query_metadata := make(map[string]string)
if q.TimeFilter {
  query_metadata["query_range_begin"] = dataQuery.TimeRange.From.Format(time.RFC3339Nano)
  query_metadata["query_range_end"] = dataQuery.TimeRange.To.Format(time.RFC3339Nano)
}
```

When the **Time Filter** checkbox is enabled:
1. Frontend passes `timeFilter: true` in the query request
2. Backend extracts the dashboard time range from Grafana's `TimeRange` object
3. Time range boundaries are added to gRPC metadata headers
4. Analytics service receives metadata and applies time filtering during query execution

This same mechanism needs to be applied to variable queries via `metricFindQuery()`.

## Files to Modify
1. `grafana/src/components/VariableQueryEditor.tsx` - Add UI checkbox and state management
2. `grafana/src/types.ts` - Verify/update type definitions
3. `grafana/src/datasource.ts` - Update `metricFindQuery` to respect timeFilter

## Usage Example

### Before
Variable query editor only shows:
```
Query: SELECT DISTINCT process_id FROM processes
```
No way to constrain results to the dashboard time range.

### After
Variable query editor shows:
```
Query: SELECT DISTINCT process_id FROM processes
☑ Time Filter
```

When the **Time Filter** checkbox is enabled and dashboard time range is "Last 24 hours", the backend automatically receives the time range as metadata (`query_range_begin` and `query_range_end`) and the analytics service applies time filtering to constrain the query results.

**How it works**:
- Checkbox enabled → Backend receives time range metadata
- Analytics service applies time filter during query execution
- No SQL modification needed - filtering happens at the data layer
- Same behavior as regular queries, but now available for dashboard variables

## Benefits
- Improves variable performance by reducing result set size
- Enables time-aware variables that adapt to dashboard time range
- Provides consistency with regular query editor UI
- Allows users to build more dynamic, time-aware dashboards

## Risks and Considerations
- **Backwards compatibility**: Existing variable queries without time filter must continue working
- **Dashboard migration**: Old dashboards should work without modification (treat missing timeFilter as false)
- **Default behavior**: Regular queries default to `timeFilter: true`, but variables might need different defaults:
  - `true` = Consistency with regular queries, better performance
  - `false` = Variables often need to show all available options across all time, not just current range
  - Decision needed during implementation
- **UI clarity**: Users must understand when time filter affects their variables (tooltip/documentation)
