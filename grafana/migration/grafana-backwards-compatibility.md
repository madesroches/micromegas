# Grafana Plugin Backwards Compatibility Plan

## Implementation Status: ✅ COMPLETED

All phases of the backwards compatibility implementation have been completed successfully:
- ✅ Phase 1: Migration infrastructure added to `types.ts`
- ✅ Phase 2: Migration hooked into datasource methods
- ✅ Phase 3: Frontend components updated to use migration
- ✅ Phase 4: Comprehensive test coverage added (33 tests passing)
- ✅ All tests passing, lint clean, build successful

## Goal
Ensure the Grafana plugin can load and execute queries from existing dashboards without breaking, even as we evolve the query schema.

## Context
- Users have existing dashboards we cannot access
- Query schema has evolved:
  - `queryText` → `query` (v2 uses `query` field, keeps `queryText` in interface for compatibility)
  - Added explicit defaults for `format`, `timeFilter`, and `autoLimit` fields
  - Added `version` field for future migrations
- Need defensive migration strategy to handle old query formats
- **Critical**: `autoLimit` behavior differs by context:
  - Panels: default `true` (limit results for performance)
  - Variables: forced `false` (Grafana doesn't provide row hint for variables, sends `LIMIT 0`)
- **format field**: Part of v1 schema, defaults to `'table'` when undefined

## Current State
### What's Working ✓
- Dual field support: `query` and `queryText` both maintained
- Default value helpers: `getTimeFilter()`, `getAutoLimit()`
- QueryEditor initializes undefined values to defaults on load

### Gaps ⚠️
- No centralized migration function
- No test coverage for legacy query formats
- Backend doesn't normalize queries before execution
- Still maintaining dual `query`/`queryText` fields (should migrate to `query` only)

## Implementation Plan

### Phase 1: Add Migration Infrastructure
**File**: `grafana/src/types.ts`

1. **Update SQLQuery interface**
   - Add `version?: number` field to SQLQuery
   - Current queries without version are v1 (legacy)
   - New queries created will be v2
   - Future schema changes can increment version for targeted migrations
   - Keep both `query?` and `queryText?` fields in TypeScript interface for backwards compatibility
   - V2 queries only populate the `query` field; `queryText` remains in interface but unused

2. **Add `migrateQuery()` function**
   - Signature: `migrateQuery(query: SQLQuery, context: 'panel' | 'variable'): SQLQuery`
   - **Error handling:** Return query unchanged if null, undefined, or empty object
   - Detect query version (undefined or missing = v1)
   - Handle invalid versions: treat unknown versions (> 2) as v2 (forward compatibility)
   - Handle v1 migrations:
     - **Query text migration:**
       - If `query` exists and is non-empty → use it (takes precedence)
       - Else if `queryText` exists → copy to `query` field
       - Else → set `query` to empty string (defensive)
       - Set `queryText` to undefined (v2 doesn't populate this field)
     - If `format` is undefined → set to `'table'` (v1 default behavior)
     - If `format` is explicitly set → preserve value (user choice)
     - If `timeFilter` is undefined → set to `true` (v1 default behavior)
     - If `timeFilter` is explicitly false → preserve false (user choice)
     - For `autoLimit`:
       - **Panel context:** If undefined → set to `true`, otherwise preserve value
       - **Variable context:** Always force to `false` (overrides any existing value)
   - Set version to 2 after migration
   - Return normalized query object (new object, don't mutate input)
   - Structure to easily add v2→v3, v3→v4 migrations in future

### Phase 2: Hook Migration into Query Execution
**File**: `grafana/src/datasource.ts`

1. **In `applyTemplateVariables()`**
   - Call `migrateQuery(query, 'panel')` before template variable interpolation
   - Ensures all queries are normalized before processing
   - Uses 'panel' context since this is for dashboard queries

2. **In `metricFindQuery()`**
   - Call `migrateQuery(queryObj, 'variable')` for variable queries
   - Uses 'variable' context to force `autoLimit: false`
   - Already uses `getTimeFilter()`, but should use full migration

3. **In `getDefaultQuery()`**
   - Update DEFAULT_QUERY to be complete for v2:
     ```typescript
     DEFAULT_QUERY = { query: '', format: 'table', timeFilter: true, autoLimit: true, version: 2 }
     ```
   - Note: Context-specific `autoLimit` adjustments handled by migration function if needed

### Phase 3: Frontend Migration
**File**: `grafana/src/components/QueryEditor.tsx`

1. **On component mount (useEffect)**
   - Call `migrateQuery(query, 'panel')` immediately on load
   - Apply migrated query via `onChange()`
   - Current code (lines 129-143) does this partially, make it use migration function
   - Uses 'panel' context for dashboard queries

**File**: `grafana/src/components/VariableQueryEditor.tsx`

2. **On component mount**
   - Call `migrateQuery(query, 'variable')` for variable queries
   - Uses 'variable' context to ensure `autoLimit: false`

### Phase 4: Add Test Coverage
**File**: `grafana/src/datasource.test.ts` or new file

1. **Create legacy query fixtures**
   ```typescript
   const legacyQueries = {
     // Panel queries (autoLimit should default to true, format should default to 'table')
     v1_panel_minimal: { queryText: 'SELECT * FROM logs' },
     v1_panel_with_format: { queryText: 'SELECT * FROM logs', format: 'logs' },
     v1_panel_explicit_false: { queryText: 'SELECT * FROM logs', timeFilter: false, autoLimit: false },
     v1_panel_mixed: { queryText: 'SELECT * FROM logs', timeFilter: true },

     // Variable queries (autoLimit forced to false)
     v1_variable_minimal: { queryText: 'SELECT DISTINCT host FROM logs' },
     v1_variable_mixed: { queryText: 'SELECT DISTINCT host FROM logs', timeFilter: true, format: 'table' },

     // Edge cases - both fields present
     v1_both_fields: { query: 'SELECT * FROM metrics', queryText: 'SELECT * FROM logs' }, // query takes precedence

     // Current version
     v2_current: { query: 'SELECT * FROM logs', format: 'table', timeFilter: true, autoLimit: false, version: 2 }
   };
   ```

2. **Test migration function**
   - Verify v1 queries get migrated to v2
   - **Panel context tests:**
     - Verify undefined `format` → defaults to `'table'`
     - Verify explicit `format` value → preserved (user choice respected)
     - Verify undefined `timeFilter` → defaults to `true`
     - Verify undefined `autoLimit` → defaults to `true`
     - Verify explicit `false` values are preserved (user choice respected)
   - **Variable context tests:**
     - Verify undefined `format` → defaults to `'table'`
     - Verify undefined `timeFilter` → defaults to `true`
     - Verify undefined `autoLimit` → forced to `false`
     - Verify explicit `autoLimit: true` is overridden to `false` in variable context
   - **Query text migration tests:**
     - Verify `queryText` only → migrated to `query` field
     - Verify `query` only → preserved as-is
     - Verify both `query` and `queryText` → `query` takes precedence
     - Verify `queryText` is set to undefined after migration (v2 doesn't populate it)
   - **Edge case tests:**
     - Verify null/undefined query → handled gracefully
     - Verify empty query object → handled gracefully
     - Verify invalid version numbers (99, -1, etc.) → treated as v2
     - Verify query with version: 1 explicitly set → migrates correctly
   - Verify version field is set to 2 after migration
   - Verify idempotency (migrating v2 queries = no changes)
   - Verify immutability (migration returns new object, doesn't mutate input)

3. **Test query execution with legacy formats**
   - Verify queries execute without errors
   - Verify backend receives properly formatted queries

### Phase 5: Documentation
**File**: `grafana/CHANGELOG.md` or similar

1. **Document backwards compatibility guarantees**
   - Which fields are deprecated but supported
   - Migration timeline (if any)
   - How to test with legacy dashboards

2. **Add developer notes**
   - How to add new fields safely (with defaults)
   - How migration works
   - When to bump query version

## Testing Strategy (Without Real Dashboards)

### Unit Tests
- Test `migrateQuery()` with various legacy formats
- Test datasource methods with legacy queries
- Test template variable interpolation with legacy queries

### Integration Tests
- Create test dashboard JSON files with legacy query formats
- Load and verify they work
- Export sample dashboard JSON for regression testing

### Manual Testing
1. Create new dashboard with current plugin version
2. Manually edit dashboard JSON to simulate old format
3. Reload and verify query still works
4. Check browser console for errors

## Success Criteria
- [x] Version field added to SQLQuery interface
- [x] Both `query` and `queryText` fields kept in TypeScript interface for compatibility
- [x] All legacy query formats (v1) load without errors
- [x] V1 queries with `queryText` are migrated to use `query` field
- [x] Migration handles precedence: `query` field takes priority over `queryText`
- [x] `queryText` field is set to undefined in v2 queries after migration
- [x] All fields have sensible defaults
- [x] Migration is idempotent (can run multiple times safely)
- [x] New queries created have version: 2 and use `query` field only
- [x] Test coverage for legacy formats and precedence rules
- [x] No breaking changes to existing dashboards
- [x] Documentation updated with version history

## Risk Mitigation
- **Unknown legacy formats**: Migration function should be defensive (don't assume fields exist)
- **Backend changes**: Ensure backend handles missing fields gracefully
- **Performance**: Migration should be fast (no async operations)
- **User confusion**: Log warnings for deprecated field usage (dev console only)

## Query Version History
- **v1** (no version field): Original queries with `queryText` field, `format`/`timeFilter`/`autoLimit` can be undefined
  - `format` undefined → defaults to `'table'`
  - `timeFilter` undefined → defaults to `true`
  - `autoLimit` undefined → defaults to `true` in panels, `false` in variables
  - Uses `queryText` field for SQL
- **v2** (current): Normalized schema with `query` field, explicit `format`/`timeFilter`/`autoLimit` values, and `version` field
  - Uses `query` field (takes precedence if both fields exist)
  - `queryText` is set to undefined in v2 queries (not populated, but remains in interface)
  - `format` defaults to `'table'` if not specified
