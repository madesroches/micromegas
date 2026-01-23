# User-Editable Cell-Based Screens

Add a new screen type to the analytics web app: a user-editable screen composed of a sequence of cells. Inspired by Jupyter notebooks and Grafana dashboards.

**Mockup**: See `mockup.html` for visual reference (cell layout, delete button, collapse toggle, etc.)

The existing screen types (`process_list`, `metrics`, `log`) remain unchanged. This adds a new `notebook` screen type where users can compose their own views using cells of different kinds.

## Design Principles

- Screens are *recipes*, not *snapshots* - query results are NOT persisted
- Sequential execution: cells run top-to-bottom, user controls ordering
- Single-threaded execution: only one cell executes at a time, no concurrent queries
- Transparent internals: SQL queries visible and editable per cell
- Variable cells provide user inputs that become macros for subsequent queries
- Graceful degradation: a failed cell stops execution of cells below it, but cells above remain visible

## Cell Types

| Type | Purpose | SQL Shape |
|------|---------|-----------|
| `table` | Generic SQL results | Any columns |
| `chart` | X/Y chart (line, etc.) | Configurable X and Y columns |
| `log` | Log viewer | `time, level, target, msg` |
| `markdown` | Documentation | N/A (no query) |
| `variable` | User input control | Options from query (combobox) or free input |

**Default SQL per cell type** (inspired by existing screen defaults):
- `table`: Same as Process List screen default
- `chart`: Same as Metrics screen default
- `log`: Same as Log screen default
- `variable` (combobox): `SELECT DISTINCT name FROM measures`

## Data Model

**Architecture note:** The backend stores screen config as opaque JSON (`serde_json::Value`). The frontend `ScreenConfig` type should be `Record<string, unknown>` - each renderer casts it to its own specific interface. This means notebook types are defined in `NotebookRenderer.tsx`, not in a shared types file.

```typescript
// Config for screens with type: 'notebook'
// Defined in NotebookRenderer.tsx, not screens-api.ts
// Note: time range is handled at the screen level, same as other screen types
interface NotebookConfig {
  cells: CellConfig[]
  refreshInterval?: number
}

type CellConfig = QueryCellConfig | MarkdownCellConfig | VariableCellConfig

interface CellConfigBase {
  name: string           // Unique within notebook; display name + anchor for deep linking
  type: CellType         // For variable cells, name is also the variable name ($name)
  layout: { height: number | 'auto'; collapsed?: boolean }
}

interface QueryCellConfig extends CellConfigBase {
  type: 'table' | 'chart' | 'log'
  sql: string
  options?: object  // e.g., chart: { xColumn, yColumn, seriesColumn? }
}

interface MarkdownCellConfig extends CellConfigBase {
  type: 'markdown'
  content: string
}

interface VariableCellConfig extends CellConfigBase {
  type: 'variable'
  variableType: 'combobox' | 'text' | 'number'
  // Cell name is the variable name - available as $name in subsequent cells
  sql?: string           // For combobox: query to populate options
  valueColumn?: string
  labelColumn?: string
  defaultValue?: string
}
```

## Execution Model

Manual execution only - no automatic re-execution on time range or variable changes.

1. **Screen load**: Execute all cells sequentially top-to-bottom
2. **Execute single cell**: User can run one cell in isolation
3. **Execute from cell**: User can run a cell and continue with all cells below it
4. **Errors**: Failed cell shows error, execution stops - cells below don't run until error is fixed

## Coexistence with Existing Screens

- Existing screen types (`process_list`, `metrics`, `log`) continue to work as before
- New `notebook` screen type stores cells in its config
- Users choose screen type when creating a new screen
- No migration needed - this is additive

## Files to Modify

**Frontend:**
- `analytics-web-app/src/lib/screens-api.ts` - Change `ScreenConfig` to opaque type, add `'notebook'` to `ScreenTypeName`
- `analytics-web-app/src/lib/screen-renderers/index.ts` - Register NotebookRenderer
- `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` - New renderer (includes `NotebookConfig`, `CellConfig` types)
- `analytics-web-app/src/lib/screen-renderers/cells/` - New folder for cell components
- `analytics-web-app/src/components/CellContainer.tsx` - New component
- `analytics-web-app/src/components/CellEditor.tsx` - New component

**Backend:**
- `rust/analytics-web-srv/src/screen_types.rs` - Add `notebook` screen type + default config

---

## Tasks

### Phase 1: Multi-Cell Foundation

- [x] **1. Clean up ScreenConfig type** (`screens-api.ts`)
  - Change `ScreenConfig` from kitchen-sink interface to opaque `Record<string, unknown>`
  - Add `'notebook'` to `ScreenTypeName` union
  - (Notebook-specific types go in `NotebookRenderer.tsx`, not here)

- [x] **2. Create cell registry** (`screen-renderers/cell-registry.ts`)
  - Define `CellRendererProps` interface
  - Create `CELL_RENDERERS` map and `registerCellRenderer()` function
  - Export `getCellRenderer()` lookup function

- [x] **3. Build CellContainer component** (`components/CellContainer.tsx`)
  - Cell header with title, collapse toggle, refresh button
  - Collapsible content area
  - Loading and error states
  - Height management (fixed px or auto)

- [x] **4. Build NotebookRenderer** (`screen-renderers/NotebookRenderer.tsx`)
  - Define `NotebookConfig`, `CellConfig`, `CellType` types (renderer owns its config shape)
  - Validate cell name uniqueness within notebook
  - Vertical stack of CellContainers
  - "Add Cell" button at bottom (empty notebook shows just this button)
  - Cell type selection modal
  - Delete cell action (with confirmation for cells with content)
  - Manage cell execution state array (per-cell: idle, loading, success, error)
  - Collect variable values from variable cells (keyed by cell name)
  - "Run from here" action on each cell (executes cell and all below)

- [x] **5. Create ChartCell** (`screen-renderers/cells/ChartCell.tsx`)
  - Reuse chart logic from MetricsRenderer
  - Configurable X/Y columns (not just time)
  - Implement CellRendererProps interface
  - Register with cell registry

- [x] **6. Create LogCell** (`screen-renderers/cells/LogCell.tsx`)
  - Reuse log viewer logic from LogRenderer
  - Implement CellRendererProps interface
  - Register with cell registry

- [x] **7. Create TableCell** (`screen-renderers/cells/TableCell.tsx`)
  - Generic table for SQL results
  - Implement CellRendererProps interface
  - Register with cell registry

- [x] **8. Register notebook screen type** (backend + frontend)
  - Add `notebook` to backend ScreenType enum
  - Add default config for new notebook screens
  - Register NotebookRenderer in frontend

### Phase 2: Cell Types & Editors

- [x] **9. Enhance TableCell** (`screen-renderers/cells/TableCell.tsx`)
  - Generic SQL result table with Arrow data
  - ~~Sortable columns~~ (not implemented)
  - ~~Pagination option~~ (not implemented)

- [x] **10. Create MarkdownCell** (`screen-renderers/cells/MarkdownCell.tsx`)
  - Render markdown content (no SQL query)
  - Edit mode for content editing

- [x] **11. Build CellEditor component** (`components/CellEditor.tsx`)
  - Collapsible SQL editor per cell
  - Variable preview showing available `$vars`
  - Run button to execute cell query
  - ~~Reset button to revert to saved SQL~~ (not implemented)

- [x] **12. Add cell-level error handling**
  - Error banner within CellContainer
  - Retry button that re-executes cell and continues sequence if successful
  - Cells below a failed cell show "blocked" state until error is resolved

### Phase 3: Variables & Execution

- [x] **13. Create VariableCell component** (`screen-renderers/cells/VariableCell.tsx`)
  - Combobox: fetch options via SQL, render dropdown
  - Text: simple text input
  - Number: number input with optional min/max
  - Emit value changes to screen state

- [x] **14. Build variable value collection**
  - Screen maintains `Record<string, string>` of variable values (keyed by cell name)
  - Variable cells update their value on user interaction
  - Values passed to all query cells for macro substitution

- [x] **15. Implement macro substitution**
  - Replace `$variableName` in SQL with collected values
  - Handle missing variables gracefully (show error or use default)

- [x] ~~**16. Add auto-refresh**~~ (skipped - not needed)

- [x] **17. Polish notebook creation flow**
  - New notebook starts empty with just the "Add Cell" button
  - ~~Smooth "new screen" flow with type selection~~ (uses existing screen creation flow)

### Phase 4: Cell Reordering

- [x] **18. Add drag-and-drop reordering**
  - Added drag handle (GripVertical icon) to CellContainer header
  - Using @dnd-kit/core + @dnd-kit/sortable for drag functionality
  - Visual feedback during drag (opacity change on dragged item)
  - Reorders cells array on drop and saves config

- [x] **19. Preserve state after reorder**
  - Cell execution results keyed by name, preserved across reorder
  - Selected cell index updated when reordering

---

## Future Improvements

- **Cell duplication** - Copy an existing cell as a starting point

---

## Current Status (2026-01-22)

**Phases 1-2 Complete.** All cell types implemented and working:
- TableCell, ChartCell, LogCell, MarkdownCell, VariableCell
- CellContainer with collapse, edit, delete, and run controls
- CellEditor with SQL editing and variable preview
- Cell-level error handling with retry

**Phase 3 Complete.**
- Variable cells working (combobox, text, number types)
- Macro substitution working (`$variableName` in SQL)
- Variable values populated on initial load (fixed in adb9bbb8b)
- Cell name rename working (fixed in a9a5a3fa1)
- Auto-refresh skipped (not needed)

**Phase 4 Complete:** Drag-and-drop cell reordering implemented using @dnd-kit

**Known Issues:** None currently.

---

## Verification

- [x] Create new notebook screen, add multiple cells (table, chart, log)
- [x] Add variable cell (combobox), verify it populates options from SQL
- [x] Add query cell below that uses `$variable`, verify substitution works
- [x] Use "Run from here" to execute a cell and all cells below it
- [x] Verify a failed cell stops execution of cells below it
- [x] Verify fixing a failed cell resumes execution of cells below
- [x] Verify existing screen types (process_list, metrics, log) still work
- [x] Run `yarn lint` and `yarn test` (passes with only pre-existing warnings)
- [x] Verify cell drag-and-drop reordering works
