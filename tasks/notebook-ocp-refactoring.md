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

Create a `CellTypeMetadata` interface that encapsulates all type-specific behavior and register it alongside the renderer.

### New Types

```typescript
// cell-registry.ts
export interface CellTypeMetadata {
  // Display
  label: string              // "Table", "Chart", etc.
  icon: string               // "T", "C", etc.
  description: string        // For add cell modal
  showTypeBadge: boolean     // false for markdown
  defaultHeight: number      // 300, 150, 60, etc.

  // Execution behavior
  isExecutable: boolean      // false for markdown
  requiresSql: boolean       // false for markdown, conditional for variable
  canBlockDownstream: boolean // false for markdown

  // Editor configuration
  editorSections: {
    sql: boolean | ((config: CellConfig) => boolean)
    content: boolean
    variableType: boolean
    defaultValue: boolean
  }

  // Factory
  createDefaultConfig: (baseName: string) => Omit<CellConfig, 'name' | 'layout'>

  // Execution hooks (optional)
  shouldSkipExecution?: (config: CellConfig) => boolean
  processResult?: (result: Table, config: CellConfig) => {
    variableOptions?: { label: string; value: string }[]
  }

  // Props extraction
  getRendererProps: (config: CellConfig, state: CellState) => Partial<CellRendererProps>
}
```

## Implementation Steps

### Step 1: Create Cell Type Metadata Registry
**File:** `analytics-web-app/src/lib/screen-renderers/cell-registry.ts`

- Add `CellTypeMetadata` interface
- Add `CELL_TYPE_METADATA: Record<CellType, CellTypeMetadata>` registry
- Add `registerCellType(type, metadata)` function
- Add `getCellTypeMetadata(type)` function

### Step 2: Create Individual Cell Type Definitions
**Files:** `cells/table.meta.ts`, `cells/chart.meta.ts`, etc.

Each file exports metadata and calls `registerCellType()`:

```typescript
// cells/table.meta.ts
registerCellType('table', {
  label: 'Table',
  icon: 'T',
  description: 'Generic SQL results as a table',
  showTypeBadge: true,
  defaultHeight: 300,
  isExecutable: true,
  requiresSql: true,
  canBlockDownstream: true,
  editorSections: { sql: true, content: false, variableType: false, defaultValue: false },
  createDefaultConfig: () => ({ type: 'table', sql: DEFAULT_SQL.table }),
  getRendererProps: (config) => ({
    sql: (config as QueryCellConfig).sql,
    options: (config as QueryCellConfig).options,
  }),
})
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

### Step 8: Update Cell Index
**File:** `analytics-web-app/src/lib/screen-renderers/cells/index.ts`

Import metadata files alongside renderers:
```typescript
import './TableCell'
import './table.meta'  // or combine into single file
// ...
```

## Files to Modify

1. `cell-registry.ts` - Add metadata types and registry
2. `notebook-utils.ts` - Replace `createDefaultCell` switch
3. `useCellExecution.ts` - Replace type checks with metadata
4. `CellContainer.tsx` - Use metadata for conditional rendering
5. `CellEditor.tsx` - Use metadata for editor sections
6. `NotebookRenderer.tsx` - Derive options from registry, use metadata for props
7. `cells/index.ts` - Import metadata registrations

## Files to Create

1. `cells/table.meta.ts` (or combine with TableCell.tsx)
2. `cells/chart.meta.ts`
3. `cells/log.meta.ts`
4. `cells/markdown.meta.ts`
5. `cells/variable.meta.ts`

Alternative: Combine renderer and metadata in each cell file.

## Verification

1. `yarn type-check` - Ensure no TypeScript errors
2. `yarn lint` - Ensure no lint errors
3. `yarn test` - Run existing tests
4. Manual testing:
   - Add each cell type (table, chart, log, markdown, variable)
   - Verify execution behavior (markdown doesn't execute, variable text/number skip SQL)
   - Verify editor sections show correctly per type
   - Verify run buttons appear/hide correctly
   - Verify blocking behavior works
