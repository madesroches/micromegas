# Open/Closed Principle Refactoring for Notebook Cells

## Goal
Refactor the notebook cell system so that adding a new cell type requires only creating a new file with all type-specific behavior, without modifying existing code.

## Current OCP Violations

| File | Issue |
|------|-------|
| `useCellExecution.ts:99-117` | Hardcoded checks for markdown/variable execution behavior |
| `useCellExecution.ts:164-187` | Variable-specific options extraction |
| `useCellExecution.ts:218` | Markdown excluded from blocking |
| `CellContainer.tsx:177-186` | Type badge hidden for markdown |
| `CellContainer.tsx:194,230` | Run button hidden for markdown |
| `CellEditor.tsx:106-108` | Boolean flags for which editor sections to show |
| `CellEditor.tsx:152-213` | Conditional editor sections per type |
| `notebook-utils.ts:146-163` | Switch statement in `createDefaultCell` |
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

### New Types

```typescript
// cell-registry.ts
export interface CellTypeMetadata {
  // Renderer component
  readonly renderer: ComponentType<CellRendererProps>

  // Display
  readonly label: string              // "Table", "Chart", etc.
  readonly icon: React.ReactNode      // "T", <TableIcon />, etc.
  readonly description: string        // For add cell modal
  readonly showTypeBadge: boolean     // false for markdown
  readonly defaultHeight: number      // 300, 150, 60, etc.

  // Execution behavior
  readonly isExecutable: boolean       // false for markdown
  readonly canBlockDownstream: boolean // false for markdown

  // Editor configuration
  readonly editorSections: {
    readonly sql: boolean | ((config: CellConfig) => boolean)
    readonly content: boolean
    readonly variableType: boolean
    readonly defaultValue: boolean
  }

  // Factory
  readonly createDefaultConfig: (baseName: string) => Omit<CellConfig, 'name' | 'layout'>

  // Execution hooks (optional)
  readonly shouldSkipExecution?: (config: CellConfig) => boolean
  readonly processResult?: (result: Table, config: CellConfig) => {
    variableOptions?: { label: string; value: string }[]
  }

  // Props extraction
  readonly getRendererProps: (config: CellConfig, state: CellState) => Partial<CellRendererProps>
}
```

## Implementation Steps

### Step 1: Create Unified Cell Type Registry
**File:** `analytics-web-app/src/lib/screen-renderers/cell-registry.ts`

- Add `CellTypeMetadata` interface (includes renderer component)
- Add explicit imports from each cell metadata file
- Build `CELL_TYPE_METADATA: Record<CellType, CellTypeMetadata>` as a static map
- Remove `registerCellRenderer()` and `CELL_RENDERERS` - derive renderer from metadata instead
- Add `getCellTypeMetadata(type)` and `getCellRenderer(type)` helper functions
- Add compile-time exhaustiveness check to catch missing registrations

```typescript
// cell-registry.ts
import { ComponentType } from 'react'
import type { CellType } from './notebook-types'
import { tableMetadata } from './cells/TableCell'
import { chartMetadata } from './cells/ChartCell'
import { logMetadata } from './cells/LogCell'
import { markdownMetadata } from './cells/MarkdownCell'
import { variableMetadata } from './cells/VariableCell'

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

// Derive cell type options for UI from metadata
export const CELL_TYPE_OPTIONS = (Object.entries(CELL_TYPE_METADATA) as [CellType, CellTypeMetadata][])
  .map(([type, meta]) => ({
    value: type,
    label: meta.label,
    icon: meta.icon,
    description: meta.description,
  }))
```

### Step 2: Add Metadata to Each Cell File
**Files:** `cells/TableCell.tsx`, `cells/ChartCell.tsx`, etc.

Each cell file becomes a self-contained module exporting a single metadata object that includes the renderer component. No separate registration call needed.

```typescript
// cells/TableCell.tsx
import type { CellTypeMetadata, CellRendererProps } from '../cell-registry'
import type { QueryCellConfig, CellConfig, CellState } from '../notebook-types'
import { DEFAULT_SQL } from '../notebook-utils'

// =============================================================================
// Renderer Component
// =============================================================================

function TableCell({ data, status }: CellRendererProps) {
  // ... existing implementation
}

// =============================================================================
// Cell Type Metadata (includes renderer)
// =============================================================================

export const tableMetadata: CellTypeMetadata = {
  // Renderer
  renderer: TableCell,

  // Display
  label: 'Table',
  icon: 'T',
  description: 'Generic SQL results as a table',
  showTypeBadge: true,
  defaultHeight: 300,

  // Execution behavior
  isExecutable: true,
  canBlockDownstream: true,

  // Editor configuration
  editorSections: {
    sql: true,
    content: false,
    variableType: false,
    defaultValue: false,
  },

  // Factory
  createDefaultConfig: () => ({
    type: 'table',
    sql: DEFAULT_SQL.table,
  }),

  // Props extraction
  getRendererProps: (config: CellConfig, state: CellState) => ({
    sql: (config as QueryCellConfig).sql,
    options: (config as QueryCellConfig).options,
    data: state.data,
    status: state.status,
  }),
}
```

**Variable cell example** (showing conditional behavior):

```typescript
// cells/VariableCell.tsx
function VariableCell({ value, onValueChange, variableType, variableOptions }: CellRendererProps) {
  // ... existing implementation
}

export const variableMetadata: CellTypeMetadata = {
  renderer: VariableCell,

  label: 'Variable',
  icon: 'V',
  description: 'Reusable parameter for queries',
  showTypeBadge: true,
  defaultHeight: 60,

  isExecutable: true,
  canBlockDownstream: true,

  editorSections: {
    sql: (config) => (config as VariableCellConfig).variableType === 'options',
    content: false,
    variableType: true,
    defaultValue: true,
  },

  createDefaultConfig: () => ({
    type: 'variable',
    variableType: 'text',
    defaultValue: '',
  }),

  shouldSkipExecution: (config) => {
    const varConfig = config as VariableCellConfig
    return varConfig.variableType !== 'options'
  },

  processResult: (result) => ({
    variableOptions: result.toArray().map((row) => ({
      label: String(row.label ?? row.value),
      value: String(row.value),
    })),
  }),

  getRendererProps: (config, state) => ({
    variableType: (config as VariableCellConfig).variableType,
    defaultValue: (config as VariableCellConfig).defaultValue,
    options: state.variableOptions,
  }),
}
```

**Markdown cell example** (non-executable, no SQL):

```typescript
// cells/MarkdownCell.tsx
function MarkdownCell({ content, isEditing, onContentChange }: CellRendererProps) {
  // ... existing implementation
}

export const markdownMetadata: CellTypeMetadata = {
  renderer: MarkdownCell,

  label: 'Markdown',
  icon: 'M',
  description: 'Documentation and notes',
  showTypeBadge: false,
  defaultHeight: 150,

  isExecutable: false,
  canBlockDownstream: false,

  editorSections: {
    sql: false,
    content: true,
    variableType: false,
    defaultValue: false,
  },

  createDefaultConfig: () => ({
    type: 'markdown',
    content: '# Notes\n\nAdd your documentation here.',
  }),

  getRendererProps: (config) => ({
    content: (config as MarkdownCellConfig).content,
  }),
}
```

### Step 3: Refactor `createDefaultCell`
**File:** `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`

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

### Step 4: Refactor `useCellExecution`
**File:** `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts`

Replace type checks with metadata lookups:
```typescript
const executeCell = async (cellIndex: number): Promise<boolean> => {
  const cell = cells[cellIndex]
  const meta = getCellTypeMetadata(cell.type)

  // Check if execution should be skipped
  if (!meta.isExecutable || meta.shouldSkipExecution?.(cell)) {
    setCellStates(prev => ({
      ...prev,
      [cell.name]: { status: 'success', data: null },
    }))
    return true
  }

  // ... rest of execution logic

  // Process result using metadata hook
  if (meta.processResult) {
    const processed = meta.processResult(result, cell)
    setCellStates(prev => ({
      ...prev,
      [cell.name]: { status: 'success', data: result, ...processed },
    }))
  }
}
```

For blocking logic:
```typescript
if (!meta.canBlockDownstream) continue  // instead of if (cell.type !== 'markdown')
```

### Step 5: Refactor `CellContainer`
**File:** `analytics-web-app/src/components/CellContainer.tsx`

- Accept `metadata: CellTypeMetadata` prop (or look it up internally)
- Use `meta.showTypeBadge` instead of `type === 'markdown'`
- Use `meta.isExecutable` instead of `type !== 'markdown'` for run button

### Step 6: Refactor `CellEditor`
**File:** `analytics-web-app/src/components/CellEditor.tsx`

- Accept `metadata: CellTypeMetadata` prop
- Use `meta.editorSections.sql` (evaluate if function) instead of hardcoded checks
- Use `meta.editorSections.content`, `meta.editorSections.variableType`, etc.

### Step 7: Refactor `NotebookRenderer`
**File:** `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`

- Remove `CELL_TYPE_OPTIONS` array - derive from registry
- Use `meta.getRendererProps(cell, state)` for prop extraction
- Use `meta.canBlockDownstream` for status text logic

### Step 8: Remove cells/index.ts
**File:** `analytics-web-app/src/lib/screen-renderers/cells/index.ts`

Delete this file - it was only used for side-effect imports to trigger `registerCellRenderer`. With the unified registry, `cell-registry.ts` imports metadata directly from each cell file.

## Files to Modify

1. `cell-registry.ts` - Replace `registerCellRenderer`/`CELL_RENDERERS` with unified metadata registry
2. `notebook-utils.ts` - Replace `createDefaultCell` switch
3. `useCellExecution.ts` - Replace type checks with metadata
4. `CellContainer.tsx` - Use metadata for conditional rendering
5. `CellEditor.tsx` - Use metadata for editor sections
6. `NotebookRenderer.tsx` - Use `getCellRenderer` from registry, use metadata for props
7. `cells/TableCell.tsx` - Remove `registerCellRenderer` call, add `tableMetadata` export with renderer
8. `cells/ChartCell.tsx` - Remove `registerCellRenderer` call, add `chartMetadata` export with renderer
9. `cells/LogCell.tsx` - Remove `registerCellRenderer` call, add `logMetadata` export with renderer
10. `cells/MarkdownCell.tsx` - Remove `registerCellRenderer` call, add `markdownMetadata` export with renderer
11. `cells/VariableCell.tsx` - Remove `registerCellRenderer` call, add `variableMetadata` export with renderer

## Files to Delete

1. `cells/index.ts` - No longer needed (side-effect imports replaced by explicit metadata imports)

## Files to Create

None - metadata is co-located with renderers in existing cell files.

## Verification

1. `yarn type-check` - Ensure no TypeScript errors (also validates exhaustive metadata coverage)
2. `yarn lint` - Ensure no lint errors
3. `yarn test` - Run existing tests
4. Manual testing:
   - Add each cell type (table, chart, log, markdown, variable)
   - Verify execution behavior (markdown doesn't execute, variable text/number skip SQL)
   - Verify editor sections show correctly per type
   - Verify run buttons appear/hide correctly
   - Verify blocking behavior works

## Adding a New Cell Type (Post-Refactor)

After this refactoring, adding a new cell type requires:

1. Add the type to `CellType` union in `notebook-types.ts`
2. Create `cells/NewCell.tsx` with:
   - Renderer component
   - `export const newtypeMetadata: CellTypeMetadata = { renderer: NewCell, ... }`
3. Add import to `cell-registry.ts`: `import { newtypeMetadata } from './cells/NewCell'`
4. Add entry to `CELL_TYPE_METADATA` map

No changes needed to `useCellExecution`, `CellContainer`, `CellEditor`, or `NotebookRenderer`.
