# Open/Closed Principle Refactoring for Notebook Cells

## Status: ✅ COMPLETED

Implementation completed. All cells now follow the OCP pattern with cell-owned metadata, editors, and execution.

## Goal
Refactor the notebook cell system so that adding a new cell type requires only creating a new file with all type-specific behavior, without modifying existing code.

## Current OCP Violations

| File | Issue |
|------|-------|
| `useCellExecution.ts:99-117` | Hardcoded checks for markdown/variable execution behavior |
| `useCellExecution.ts:127-133` | Type-specific SQL extraction |
| `useCellExecution.ts:164-187` | Variable-specific options extraction and auto-selection |
| `useCellExecution.ts:218` | Markdown excluded from blocking |
| `CellContainer.tsx:7-13` | Duplicate `CELL_TYPE_LABELS` constant |
| `CellContainer.tsx:177-186` | Type badge hidden for markdown |
| `CellContainer.tsx:194,230` | Run button hidden for markdown |
| `CellEditor.tsx:7-13` | Duplicate `CELL_TYPE_LABELS` constant |
| `CellEditor.tsx:106-108` | Boolean flags for which editor sections to show |
| `CellEditor.tsx:152-213` | Conditional editor sections per type |
| `notebook-utils.ts:146-163` | Switch statement in `createDefaultCell` |
| `NotebookRenderer.tsx:42-48` | Duplicate `CELL_TYPE_OPTIONS` array |
| `NotebookRenderer.tsx:369-406` | Type-specific prop mapping |

## Solution: Cell Type Metadata System

Create a `CellTypeMetadata` interface that encapsulates all type-specific behavior, co-located with each cell renderer.

**Design choice: Static map over runtime registration**

We use explicit imports and a static `Record<CellType, CellTypeMetadata>` rather than a `registerCellType()` function with side effects. Benefits:
- **Explicit dependencies** - Import graph is visible and predictable
- **Compile-time safety** - TypeScript ensures all cell types have metadata
- **No import ordering issues** - No risk of using metadata before registration
- **Tree-shakeable** - Bundlers can analyze the static structure

**Design choice: Combined renderer + metadata files**

Each cell type lives in a single file (e.g., `TableCell.tsx`) containing both the renderer component and its metadata. Benefits:
- **High cohesion** - All behavior for a cell type is in one place
- **Easier maintenance** - No need to keep separate files in sync
- **Better discoverability** - New contributors find everything about a cell type together
- **Single import** - Registry imports one symbol per cell type

**Design choice: Each cell type owns its editor**

Rather than parameterizing editor sections with boolean flags, each cell type provides its own editor component. This avoids coupling the metadata interface to specific editor sections - a new cell type can have completely different editing needs without changing the interface. Shared UI pieces (like `SqlEditor`) are just reusable components that cell editors import as needed.

**Design choice: Cells own their execution**

The container doesn't know how cells execute - it just provides a `runQuery` helper and lets each cell handle its own execution logic. This keeps the interface query-language agnostic. A future cell type could use GraphQL, REST, or anything else without changing the metadata interface. The container only cares about results, not how they're obtained.

### New Types

```typescript
// cell-registry.ts

// Context provided to cell execution
export interface CellExecutionContext {
  variables: Record<string, string>
  timeRange: { begin: string; end: string }
  runQuery: (sql: string) => Promise<Table>
}

// Props for cell-specific editor content
// Includes variables/timeRange so editors can show available variables panel if needed
export interface CellEditorProps {
  config: CellConfig
  onChange: (config: CellConfig) => void
  variables: Record<string, string>
  timeRange: { begin: string; end: string }
}

export interface CellTypeMetadata {
  // Renderer component (displays cell output)
  readonly renderer: ComponentType<CellRendererProps>

  // Editor component (type-specific config fields only)
  readonly EditorComponent: ComponentType<CellEditorProps>

  // Display
  readonly label: string              // "Table", "Chart", etc.
  readonly icon: React.ReactNode      // "T", <TableIcon />, etc.
  readonly description: string        // For add cell modal
  readonly showTypeBadge: boolean     // false for markdown
  readonly defaultHeight: number      // 300, 150, 60, etc.

  // Execution behavior
  readonly canBlockDownstream: boolean // false for markdown

  // Factory
  readonly createDefaultConfig: (baseName: string) => Omit<CellConfig, 'name' | 'layout'>

  // Execution - cell owns its execution entirely
  // Returns null if nothing to execute, CellState updates otherwise
  // Absence of this method means the cell doesn't execute
  readonly execute?: (
    config: CellConfig,
    context: CellExecutionContext
  ) => Promise<Partial<CellState> | null>

  // Post-execution hook (e.g., auto-select first option for variables)
  readonly onExecutionComplete?: (
    config: CellConfig,
    state: CellState,
    context: { setVariableValue: (name: string, value: string) => void }
  ) => void

  // Props extraction
  readonly getRendererProps: (config: CellConfig, state: CellState) => Partial<CellRendererProps>
}
```

## Implementation Steps

> All steps below have been completed. See commit history for details.

### Step 1: Create Unified Cell Type Registry ✅
**File:** `analytics-web-app/src/lib/screen-renderers/cell-registry.ts`

- Add `CellTypeMetadata` interface (includes renderer component)
- Add `CellExecutionContext` interface
- Add explicit imports from each cell metadata file
- Build `CELL_TYPE_METADATA: Record<CellType, CellTypeMetadata>` as a static map
- Remove `registerCellRenderer()` and `CELL_RENDERERS` - derive renderer from metadata instead
- Add `getCellTypeMetadata(type)` and `getCellRenderer(type)` helper functions
- Add compile-time exhaustiveness check to catch missing registrations

```typescript
// cell-registry.ts
import { ComponentType } from 'react'
import type { CellType, CellConfig, CellState } from './notebook-types'
import type { Table } from 'apache-arrow'
import { tableMetadata } from './cells/TableCell'
import { chartMetadata } from './cells/ChartCell'
import { logMetadata } from './cells/LogCell'
import { markdownMetadata } from './cells/MarkdownCell'
import { variableMetadata } from './cells/VariableCell'

// Context provided to cell execution
export interface CellExecutionContext {
  variables: Record<string, string>
  timeRange: { begin: string; end: string }
  runQuery: (sql: string) => Promise<Table>
}

// Props for cell-specific editors (each cell type implements its own)
export interface CellEditorProps {
  config: CellConfig
  onChange: (config: CellConfig) => void
  variables: Record<string, string>
  timeRange: { begin: string; end: string }
}

export const CELL_TYPE_METADATA: Record<CellType, CellTypeMetadata> = {
  table: tableMetadata,
  chart: chartMetadata,
  log: logMetadata,
  markdown: markdownMetadata,
  variable: variableMetadata,
}

// Compile-time exhaustiveness check - fails if CellType has values not in the map
const _ensureAllCellTypesHaveMetadata: Record<CellType, CellTypeMetadata> = CELL_TYPE_METADATA

export function getCellTypeMetadata(type: CellType): CellTypeMetadata {
  return CELL_TYPE_METADATA[type]
}

// Renderer lookup derived from metadata - replaces registerCellRenderer pattern
export function getCellRenderer(type: CellType): ComponentType<CellRendererProps> {
  return CELL_TYPE_METADATA[type].renderer
}

// Editor lookup derived from metadata
export function getCellEditor(type: CellType): ComponentType<CellEditorProps> {
  return CELL_TYPE_METADATA[type].EditorComponent
}

// Derive cell type options for UI from metadata
export const CELL_TYPE_OPTIONS = (Object.entries(CELL_TYPE_METADATA) as [CellType, CellTypeMetadata][])
  .map(([type, meta]) => ({
    value: type,
    label: meta.label,
    icon: meta.icon,
    description: meta.description,
  }))
```

### Step 2: Add Metadata to Each Cell File ✅
**Files:** `cells/TableCell.tsx`, `cells/ChartCell.tsx`, etc.

Each cell file becomes a self-contained module exporting a single metadata object that includes the renderer component. No separate registration call needed.

```typescript
// cells/TableCell.tsx
import type { CellTypeMetadata, CellRendererProps, CellEditorProps, CellExecutionContext } from '../cell-registry'
import type { QueryCellConfig, CellConfig, CellState } from '../notebook-types'
import { SqlEditor } from '@/components/SqlEditor'
import { AvailableVariablesPanel } from '@/components/AvailableVariablesPanel'
import { substituteVariables } from '../notebook-utils'
import { DEFAULT_SQL } from '../notebook-utils'

// =============================================================================
// Renderer Component
// =============================================================================

function TableCell({ data, status }: CellRendererProps) {
  // ... existing implementation
}

// =============================================================================
// Editor Component
// =============================================================================

function TableCellEditor({ config, onChange, variables, timeRange }: CellEditorProps) {
  const tableConfig = config as QueryCellConfig
  return (
    <>
      <SqlEditor
        value={tableConfig.sql}
        onChange={(sql) => onChange({ ...tableConfig, sql })}
      />
      <AvailableVariablesPanel variables={variables} timeRange={timeRange} />
    </>
  )
}

// =============================================================================
// Cell Type Metadata
// =============================================================================

export const tableMetadata: CellTypeMetadata = {
  renderer: TableCell,
  EditorComponent: TableCellEditor,

  label: 'Table',
  icon: 'T',
  description: 'Generic SQL results as a table',
  showTypeBadge: true,
  defaultHeight: 300,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'table',
    sql: DEFAULT_SQL.table,
  }),

  execute: async (config, { variables, timeRange, runQuery }) => {
    const sql = substituteVariables((config as QueryCellConfig).sql, variables, timeRange)
    const data = await runQuery(sql)
    return { data }
  },

  getRendererProps: (config: CellConfig, state: CellState) => ({
    data: state.data,
    status: state.status,
  }),
}
```

**Variable cell example** (showing conditional SQL editor and custom fields):

```typescript
// cells/VariableCell.tsx
import type { CellTypeMetadata, CellRendererProps, CellEditorProps, CellExecutionContext } from '../cell-registry'
import type { VariableCellConfig, CellConfig, CellState } from '../notebook-types'
import { SqlEditor } from '@/components/SqlEditor'
import { AvailableVariablesPanel } from '@/components/AvailableVariablesPanel'
import { VariableTypeSelector } from '@/components/VariableTypeSelector'
import { DefaultValueInput } from '@/components/DefaultValueInput'
import { substituteVariables } from '../notebook-utils'

function VariableCell({ value, onValueChange, variableType, variableOptions }: CellRendererProps) {
  // ... existing implementation
}

function VariableCellEditor({ config, onChange, variables, timeRange }: CellEditorProps) {
  const varConfig = config as VariableCellConfig
  return (
    <>
      <VariableTypeSelector
        value={varConfig.variableType}
        onChange={(variableType) => onChange({ ...varConfig, variableType })}
      />
      {varConfig.variableType === 'combobox' && (
        <>
          <SqlEditor
            value={varConfig.sql ?? ''}
            onChange={(sql) => onChange({ ...varConfig, sql })}
          />
          <AvailableVariablesPanel variables={variables} timeRange={timeRange} />
        </>
      )}
      <DefaultValueInput
        value={varConfig.defaultValue ?? ''}
        onChange={(defaultValue) => onChange({ ...varConfig, defaultValue })}
      />
    </>
  )
}

export const variableMetadata: CellTypeMetadata = {
  renderer: VariableCell,
  EditorComponent: VariableCellEditor,

  label: 'Variable',
  icon: 'V',
  description: 'Reusable parameter for queries',
  showTypeBadge: true,
  defaultHeight: 60,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'variable',
    variableType: 'text',
    defaultValue: '',
  }),

  execute: async (config, { variables, timeRange, runQuery }) => {
    const varConfig = config as VariableCellConfig
    // Only combobox variables need execution
    if (varConfig.variableType !== 'combobox' || !varConfig.sql) {
      return null  // Nothing to execute
    }
    const sql = substituteVariables(varConfig.sql, variables, timeRange)
    const result = await runQuery(sql)
    return {
      variableOptions: result.toArray().map((row) => ({
        label: String(row.label ?? row.value),
        value: String(row.value),
      })),
    }
  },

  // Auto-select first option if no value is set
  onExecutionComplete: (config, state, { setVariableValue }) => {
    const options = state.variableOptions
    if (options && options.length > 0) {
      // Only set if not already set (checked by caller)
      setVariableValue(config.name, options[0].value)
    }
  },

  getRendererProps: (config, state) => ({
    variableType: (config as VariableCellConfig).variableType,
    defaultValue: (config as VariableCellConfig).defaultValue,
    variableOptions: state.variableOptions,
  }),
}
```

**Markdown cell example** (non-executable, content-only editor):

```typescript
// cells/MarkdownCell.tsx
import type { CellTypeMetadata, CellRendererProps, CellEditorProps } from '../cell-registry'
import type { MarkdownCellConfig, CellConfig } from '../notebook-types'
import { MarkdownEditor } from '@/components/MarkdownEditor'

function MarkdownCell({ content, isEditing, onContentChange }: CellRendererProps) {
  // ... existing implementation
}

function MarkdownCellEditor({ config, onChange }: CellEditorProps) {
  // Note: variables/timeRange available in props but not used - markdown doesn't need them
  const mdConfig = config as MarkdownCellConfig
  return (
    <MarkdownEditor
      value={mdConfig.content}
      onChange={(content) => onChange({ ...mdConfig, content })}
    />
  )
}

export const markdownMetadata: CellTypeMetadata = {
  renderer: MarkdownCell,
  EditorComponent: MarkdownCellEditor,

  label: 'Markdown',
  icon: 'M',
  description: 'Documentation and notes',
  showTypeBadge: false,
  defaultHeight: 150,

  canBlockDownstream: false,

  createDefaultConfig: () => ({
    type: 'markdown',
    content: '# Notes\n\nAdd your documentation here.',
  }),

  // No execute method - markdown cells don't execute

  getRendererProps: (config) => ({
    content: (config as MarkdownCellConfig).content,
  }),
}
```

### Step 3: Refactor `createDefaultCell` ✅
**File:** `analytics-web-app/src/lib/screen-renderers/cell-registry.ts` (moved here)

Replace switch statement:
```typescript
export function createDefaultCell(type: CellType, existingNames: Set<string>): CellConfig {
  const meta = getCellTypeMetadata(type)
  const name = generateUniqueName(meta.label, existingNames)
  return {
    name,
    type,
    layout: { height: meta.defaultHeight },
    ...meta.createDefaultConfig(name),
  } as CellConfig
}
```

### Step 4: Refactor `useCellExecution` ✅
**File:** `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts`

Replace type checks with metadata lookups. The container no longer knows about SQL - it just provides a `runQuery` helper and lets cells handle their own execution:

```typescript
const executeCell = async (cellIndex: number): Promise<boolean> => {
  const cell = cells[cellIndex]
  const meta = getCellTypeMetadata(cell.type)

  // Cell doesn't execute (e.g., markdown)
  if (!meta.execute) {
    setCellStates(prev => ({
      ...prev,
      [cell.name]: { status: 'success', data: null },
    }))
    return true
  }

  setCellStates(prev => ({
    ...prev,
    [cell.name]: { status: 'running', data: null },
  }))

  try {
    const context: CellExecutionContext = {
      variables: variableValuesRef.current,
      timeRange,
      runQuery: (sql) => flightSqlClient.query(sql),
    }

    const result = await meta.execute(cell, context)

    const newState: CellState = result
      ? { status: 'success', ...result }
      : { status: 'success', data: null }

    setCellStates(prev => ({ ...prev, [cell.name]: newState }))

    // Post-execution side effects (e.g., auto-select first option for variables)
    if (meta.onExecutionComplete && !variableValuesRef.current[cell.name]) {
      meta.onExecutionComplete(cell, newState, { setVariableValue })
    }

    return true
  } catch (error) {
    setCellStates(prev => ({
      ...prev,
      [cell.name]: { status: 'error', error: String(error), data: null },
    }))
    return false
  }
}
```

For blocking logic:
```typescript
if (!meta.canBlockDownstream) continue  // instead of if (cell.type !== 'markdown')
```

### Step 5: Refactor `CellContainer` ✅
**File:** `analytics-web-app/src/components/CellContainer.tsx`

- Remove local `CELL_TYPE_LABELS` constant - use `meta.label` instead
- Accept `metadata: CellTypeMetadata` prop (or look it up internally via `getCellTypeMetadata(type)`)
- Use `meta.showTypeBadge` instead of `type === 'markdown'`
- Use `meta.label` for type badge text
- Use `meta.execute` presence instead of `type !== 'markdown'` for run button

### Step 6: Refactor `CellEditor` ✅
**File:** `analytics-web-app/src/components/CellEditor.tsx`

`CellEditor` remains as a **wrapper** that handles shared concerns. It delegates the type-specific content (including whether to show the available variables panel) entirely to `meta.EditorComponent`.

**Shared concerns handled by wrapper:**
- Cell name editing with uniqueness validation
- Type badge display (using `meta.label`)
- Run button (shown when `meta.execute` exists)
- Delete button

**Type-specific content delegated to metadata:**
- SQL editor for query cells (with variables panel)
- Markdown textarea for markdown cells (no variables panel)
- Variable type selector + conditional SQL for variable cells

```typescript
// CellEditor.tsx
import { getCellTypeMetadata } from '@/lib/screen-renderers/cell-registry'

interface CellEditorWrapperProps {
  cell: CellConfig
  variables: Record<string, string>
  timeRange: { begin: string; end: string }
  existingNames: Set<string>
  onClose: () => void
  onUpdate: (updates: Partial<CellConfig>) => void
  onRun: () => void
  onDelete: () => void
}

export function CellEditor({
  cell,
  variables,
  timeRange,
  existingNames,
  onClose,
  onUpdate,
  onRun,
  onDelete,
}: CellEditorWrapperProps) {
  const meta = getCellTypeMetadata(cell.type)

  // Cell name editing with validation (shared)
  const handleNameChange = (name: string) => {
    const error = validateCellName(name, existingNames, cell.name)
    if (!error) onUpdate({ name: sanitizeCellName(name) })
  }

  // Full config change handler for type-specific editor
  const handleConfigChange = (newConfig: CellConfig) => {
    onUpdate(newConfig)
  }

  return (
    <div className="cell-editor">
      {/* Header with type badge and close button */}
      <div className="header">
        <span className="type-badge">{meta.label}</span>
        <button onClick={onClose}>×</button>
      </div>

      {/* Cell name input (shared) */}
      <CellNameInput value={cell.name} onChange={handleNameChange} />

      {/* Type-specific content - each editor decides what to show */}
      <meta.EditorComponent
        config={cell}
        onChange={handleConfigChange}
        variables={variables}
        timeRange={timeRange}
      />

      {/* Footer with Run/Delete buttons (shared) */}
      <div className="footer">
        {meta.execute && <Button onClick={onRun}>Run</Button>}
        <Button onClick={onDelete} variant="danger">Delete</Button>
      </div>
    </div>
  )
}
```

**Shared editor components** (extract from current `CellEditor.tsx`):
- `SqlEditor` - Textarea for SQL queries (could upgrade to Monaco later)
- `MarkdownEditor` - Textarea for markdown content
- `VariableTypeSelector` - Dropdown for text/number/combobox
- `DefaultValueInput` - Input for variable default value
- `CellNameInput` - Input with validation feedback
- `AvailableVariablesPanel` - Shows $begin, $end, and user variables

### Step 7: Refactor `NotebookRenderer` ✅
**File:** `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`

- Remove `CELL_TYPE_OPTIONS` array - derive from registry
- Use `meta.getRendererProps(cell, state)` for prop extraction
- Use `meta.canBlockDownstream` for status text logic

### Step 8: Remove cells/index.ts ✅
**File:** `analytics-web-app/src/lib/screen-renderers/cells/index.ts`

Deleted - it was only used for side-effect imports to trigger `registerCellRenderer`. With the unified registry, `cell-registry.ts` imports metadata directly from each cell file.

## Files Modified ✅

1. `cell-registry.ts` - Replaced `registerCellRenderer`/`CELL_RENDERERS` with unified metadata registry, added `CellEditorProps` and `CellExecutionContext` interfaces, moved `createDefaultCell` here
2. `notebook-utils.ts` - Re-exports types from `notebook-types.ts` for backwards compatibility, removed `createDefaultCell`
3. `useCellExecution.ts` - Replaced type checks with metadata, delegates execution to `meta.execute()`
4. `CellContainer.tsx` - Uses `getCellTypeMetadata()` for conditional rendering
5. `CellEditor.tsx` - Simplified to render `meta.EditorComponent`, removed conditional section logic
6. `NotebookRenderer.tsx` - Uses `getCellRenderer` from registry, uses `meta.getRendererProps()` for props
7. `cells/TableCell.tsx` - Removed `registerCellRenderer` call, added `TableCellEditor` and `tableMetadata` export
8. `cells/ChartCell.tsx` - Removed `registerCellRenderer` call, added `ChartCellEditor` and `chartMetadata` export
9. `cells/LogCell.tsx` - Removed `registerCellRenderer` call, added `LogCellEditor` and `logMetadata` export
10. `cells/MarkdownCell.tsx` - Removed `registerCellRenderer` call, added `MarkdownCellEditor` and `markdownMetadata` export (no `execute`)
11. `cells/VariableCell.tsx` - Removed `registerCellRenderer` call, added `VariableCellEditor` and `variableMetadata` export
12. `init.ts` - Updated comment, removed cells/index.ts import

## Files Deleted ✅

1. `cells/index.ts` - Deleted (side-effect imports replaced by explicit metadata imports)

## Files Created ✅

1. `notebook-types.ts` - Central type definitions to avoid circular dependencies between cell-registry and notebook-utils
2. `components/AvailableVariablesPanel.tsx` - Shared component showing $begin, $end, and user variables

Note: Other shared editor components (SqlEditor, MarkdownEditor, etc.) were kept inline in cell editors to minimize file count. They can be extracted later if needed for reuse.

## Verification

1. `yarn type-check` - Ensure no TypeScript errors (also validates exhaustive metadata coverage)
2. `yarn lint` - Ensure no lint errors
3. `yarn test` - Run existing tests
4. Manual testing:
   - Add each cell type (table, chart, log, markdown, variable)
   - Verify execution behavior (markdown doesn't execute, variable text/number return null from execute)
   - Verify each cell type's editor renders correctly (SQL editor + variables panel for table/chart/log, content editor for markdown, variable type selector + conditional SQL for variable)
   - Verify run buttons appear/hide correctly (hidden for markdown)
   - Verify type badges appear/hide correctly (hidden for markdown)
   - Verify blocking behavior works (markdown cells don't block downstream)

## Adding a New Cell Type (Post-Refactor)

After this refactoring, adding a new cell type requires:

1. Add the type to `CellType` union in `notebook-types.ts`
2. Create `cells/NewCell.tsx` with:
   - Renderer component (`NewCell`)
   - Editor component (`NewCellEditor`) - includes `AvailableVariablesPanel` if it uses queries
   - Metadata export: `export const newtypeMetadata: CellTypeMetadata = { renderer: NewCell, EditorComponent: NewCellEditor, execute: async (config, ctx) => { ... }, ... }`
3. Add import to `cell-registry.ts`: `import { newtypeMetadata } from './cells/NewCell'`
4. Add entry to `CELL_TYPE_METADATA` map

No changes needed to `useCellExecution`, `CellContainer`, `CellEditor`, or `NotebookRenderer`.
