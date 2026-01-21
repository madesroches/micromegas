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

```typescript
// Config for screens with type: 'notebook'
// Note: time range is handled at the screen level, same as other screen types
interface NotebookConfig {
  cells: CellConfig[]
  refreshInterval?: number
}

type CellConfig = QueryCellConfig | MarkdownCellConfig | VariableCellConfig

interface CellConfigBase {
  title: string          // Also used as anchor for deep linking
  type: CellType
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
  variableName: string   // Available as $variableName in subsequent cells
  label: string
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
- `analytics-web-app/src/lib/screens-api.ts` - Add NotebookConfig, CellConfig types
- `analytics-web-app/src/lib/screen-renderers/index.ts` - Register NotebookRenderer
- `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` - New renderer
- `analytics-web-app/src/lib/screen-renderers/cells/` - New folder for cell components
- `analytics-web-app/src/components/CellContainer.tsx` - New component
- `analytics-web-app/src/components/CellEditor.tsx` - New component

**Backend:**
- `rust/analytics-web-srv/src/screen_types.rs` - Add `notebook` screen type + default config

---

## Tasks

### Phase 1: Multi-Cell Foundation

- [ ] **1. Define TypeScript types** (`screens-api.ts`)
  - Remove `sql` from base `ScreenConfig` - move to type-specific configs
  - Add `NotebookConfig`, `CellConfig`, `VariableCellConfig` interfaces
  - Add `CellType` union type
  - Add `notebook` to screen types

- [ ] **2. Create cell registry** (`screen-renderers/cell-registry.ts`)
  - Define `CellRendererProps` interface
  - Create `CELL_RENDERERS` map and `registerCellRenderer()` function
  - Export `getCellRenderer()` lookup function

- [ ] **3. Build CellContainer component** (`components/CellContainer.tsx`)
  - Cell header with title, collapse toggle, refresh button
  - Collapsible content area
  - Loading and error states
  - Height management (fixed px or auto)

- [ ] **4. Build NotebookRenderer** (`screen-renderers/NotebookRenderer.tsx`)
  - Vertical stack of CellContainers
  - "Add Cell" button at bottom (empty notebook shows just this button)
  - Cell type selection modal
  - Manage cell execution state array (per-cell: idle, loading, success, error)
  - Collect variable values from variable cells
  - "Run from here" action on each cell (executes cell and all below)

- [ ] **5. Create ChartCell** (`screen-renderers/cells/ChartCell.tsx`)
  - Reuse chart logic from MetricsRenderer
  - Configurable X/Y columns (not just time)
  - Implement CellRendererProps interface
  - Register with cell registry

- [ ] **6. Create LogCell** (`screen-renderers/cells/LogCell.tsx`)
  - Reuse log viewer logic from LogRenderer
  - Implement CellRendererProps interface
  - Register with cell registry

- [ ] **7. Create TableCell** (`screen-renderers/cells/TableCell.tsx`)
  - Generic table for SQL results
  - Implement CellRendererProps interface
  - Register with cell registry

- [ ] **8. Register notebook screen type** (backend + frontend)
  - Add `notebook` to backend ScreenType enum
  - Add default config for new notebook screens
  - Register NotebookRenderer in frontend

### Phase 2: Cell Types & Editors

- [ ] **9. Enhance TableCell** (`screen-renderers/cells/TableCell.tsx`)
  - Generic SQL result table with Arrow data
  - Sortable columns
  - Pagination option

- [ ] **10. Create MarkdownCell** (`screen-renderers/cells/MarkdownCell.tsx`)
  - Render markdown content (no SQL query)
  - Edit mode for content editing

- [ ] **11. Build CellEditor component** (`components/CellEditor.tsx`)
  - Collapsible SQL editor per cell
  - Variable preview showing available `$vars`
  - Run button to execute cell query
  - Reset button to revert to saved SQL

- [ ] **12. Add cell-level error handling**
  - Error banner within CellContainer
  - Retry button that re-executes cell and continues sequence if successful
  - Cells below a failed cell show "blocked" state until error is resolved

### Phase 3: Variables & Execution

- [ ] **13. Create VariableCell component** (`screen-renderers/cells/VariableCell.tsx`)
  - Combobox: fetch options via SQL, render dropdown
  - Text: simple text input
  - Number: number input with optional min/max
  - Emit value changes to screen state

- [ ] **14. Build variable value collection**
  - Screen maintains `Record<string, string>` of variable values
  - Variable cells update their value on user interaction
  - Values passed to all query cells for macro substitution

- [ ] **15. Implement macro substitution**
  - Replace `$variableName` in SQL with collected values
  - Handle missing variables gracefully (show error or use default)

- [ ] **16. Add auto-refresh**
  - Refresh interval setting in screen config
  - Dropdown to select interval (off, 5s, 10s, 30s, 1m, 5m)
  - Re-execute all query cells on interval

- [ ] **17. Polish notebook creation flow**
  - New notebook starts empty with just the "Add Cell" button
  - Smooth "new screen" flow with type selection

---

## Future Improvements

- **Cell reordering** - Drag-and-drop or move up/down buttons to reorder cells
- **Cell duplication** - Copy an existing cell as a starting point

---

## Verification

- [ ] Create new notebook screen, add multiple cells (table, chart, log)
- [ ] Add variable cell (combobox), verify it populates options from SQL
- [ ] Add query cell below that uses `$variable`, verify substitution works
- [ ] Use "Run from here" to execute a cell and all cells below it
- [ ] Verify a failed cell stops execution of cells below it
- [ ] Verify fixing a failed cell resumes execution of cells below
- [ ] Verify existing screen types (process_list, metrics, log) still work
- [ ] Run `yarn lint` and `yarn test`
