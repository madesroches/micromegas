/**
 * Shared mock factory for cell-registry module.
 * Centralizes mock definitions to avoid duplication across test files.
 *
 * Usage in test files:
 *   jest.mock('../cell-registry', () => require('./cell-registry-mock').createCellRegistryMock())
 */

// eslint-disable-next-line @typescript-eslint/no-require-imports, @typescript-eslint/no-var-requires
const React = require('react')

// Import substituteMacros for execute implementations that need SQL substitution
// eslint-disable-next-line @typescript-eslint/no-require-imports, @typescript-eslint/no-var-requires
const { substituteMacros, DEFAULT_SQL } = require('../notebook-utils')

/** Mock editor component for all cell types */
const MockEditorComponent = ({ config }: { config: { type: string } }) =>
  React.createElement('div', { 'data-testid': `editor-${config.type}` }, `Editor for ${config.type}`)

/** Mock renderer component factory */
const createMockRenderer = (type: string) => {
  const MockRenderer = ({ name, onTimeRangeSelect }: { name: string; onTimeRangeSelect?: (from: Date, to: Date) => void }) =>
    React.createElement(
      'div',
      {
        'data-testid': `cell-renderer-${type}`,
        'data-cell-name': name,
        // Store callback reference for testing
        onClick: onTimeRangeSelect
          ? () => onTimeRangeSelect(new Date('2024-01-15T00:00:00Z'), new Date('2024-01-16T00:00:00Z'))
          : undefined,
      },
      `Cell: ${name}`
    )
  return MockRenderer
}

/** Base metadata for each cell type */
const BASE_METADATA = {
  table: {
    label: 'Table',
    icon: 'T',
    description: 'SQL results as a table',
    showTypeBadge: true,
    defaultHeight: 300,
    canBlockDownstream: true,
  },
  chart: {
    label: 'Chart',
    icon: 'C',
    description: 'SQL results as a chart',
    showTypeBadge: true,
    defaultHeight: 300,
    canBlockDownstream: true,
  },
  log: {
    label: 'Log',
    icon: 'L',
    description: 'Log entries',
    showTypeBadge: true,
    defaultHeight: 300,
    canBlockDownstream: true,
  },
  markdown: {
    label: 'Markdown',
    icon: 'M',
    description: 'Documentation and notes',
    showTypeBadge: false,
    defaultHeight: 150,
    canBlockDownstream: false,
  },
  variable: {
    label: 'Variable',
    icon: 'V',
    description: 'User input (dropdown, text, expression)',
    showTypeBadge: true,
    defaultHeight: 60,
    canBlockDownstream: true,
  },
} as const

type CellType = keyof typeof BASE_METADATA

/** Simple execute stub that just returns success */
const simpleExecuteStub = () => Promise.resolve({ data: null })

/** Execute implementation that performs SQL substitution (for useCellExecution tests) */
const createSqlExecute = () => {
  return async (
    config: { sql?: string },
    {
      variables,
      timeRange,
      runQuery,
    }: {
      variables: Record<string, string>
      timeRange: { begin: string; end: string }
      runQuery: (sql: string) => Promise<unknown>
    }
  ) => {
    if (!config.sql) {
      return { data: null }
    }
    const sql = substituteMacros(config.sql, variables, timeRange)
    const data = await runQuery(sql)
    return { data }
  }
}

/** Execute implementation for variable cells (combobox only) */
const createVariableExecute = () => {
  return async (
    config: { variableType?: string; sql?: string },
    {
      variables,
      timeRange,
      runQuery,
    }: {
      variables: Record<string, string>
      timeRange: { begin: string; end: string }
      runQuery: (sql: string) => Promise<unknown>
    }
  ) => {
    if (config.variableType !== 'combobox' || !config.sql) {
      return null // Nothing to execute
    }
    const sql = substituteMacros(config.sql, variables, timeRange)
    const data = await runQuery(sql)
    return { data, variableOptions: [{ label: 'Option 1', value: 'val0' }] }
  }
}

/** onExecutionComplete for variable cells - validates/auto-selects value */
const variableOnExecutionComplete = (
  config: { name: string; defaultValue?: string },
  state: { variableOptions?: { value: string }[] },
  {
    setVariableValue,
    currentValue,
  }: { setVariableValue: (name: string, value: string) => void; currentValue?: string }
) => {
  const options = state.variableOptions
  if (!options || options.length === 0) return

  // If current value exists and is valid, keep it
  if (currentValue && options.some((o) => o.value === currentValue)) {
    return
  }

  // Current value is missing or invalid - use default or first option
  const fallbackValue = config.defaultValue || options[0]?.value
  if (fallbackValue) {
    setVariableValue(config.name, fallbackValue)
  }
}

export interface MockOptions {
  /** Use full SQL execution (with substituteMacros) instead of simple stubs */
  withSqlExecution?: boolean
  /** Include renderer components */
  withRenderers?: boolean
  /** Include editor components */
  withEditors?: boolean
}

/**
 * Creates a mock for the cell-registry module.
 * @param options - Configure which features to include
 */
export function createCellRegistryMock(options: MockOptions = {}) {
  const { withSqlExecution = false, withRenderers = false, withEditors = false } = options

  // Build metadata for each cell type
  const buildMetadata = (type: CellType) => {
    const base = BASE_METADATA[type]
    const meta: Record<string, unknown> = { ...base }

    // Add editor component if requested
    if (withEditors) {
      meta.EditorComponent = MockEditorComponent
    }

    // Add getRendererProps (always a simple stub)
    meta.getRendererProps = () => ({})

    // Add createDefaultConfig
    switch (type) {
      case 'table':
        meta.createDefaultConfig = () => ({ type: 'table', sql: DEFAULT_SQL.table })
        break
      case 'chart':
        meta.createDefaultConfig = () => ({ type: 'chart', sql: DEFAULT_SQL.chart })
        break
      case 'log':
        meta.createDefaultConfig = () => ({ type: 'log', sql: DEFAULT_SQL.log })
        break
      case 'markdown':
        meta.createDefaultConfig = () => ({ type: 'markdown', content: '# Notes\n\nAdd your documentation here.' })
        break
      case 'variable':
        meta.createDefaultConfig = () => ({ type: 'variable', variableType: 'combobox', sql: DEFAULT_SQL.variable })
        break
    }

    // Add execute method (except markdown which doesn't execute)
    if (type !== 'markdown') {
      if (withSqlExecution) {
        if (type === 'variable') {
          meta.execute = createVariableExecute()
          meta.onExecutionComplete = variableOnExecutionComplete
        } else {
          meta.execute = createSqlExecute()
        }
      } else {
        meta.execute = simpleExecuteStub
      }
    }

    return meta
  }

  const metadata: Record<string, ReturnType<typeof buildMetadata>> = {
    table: buildMetadata('table'),
    chart: buildMetadata('chart'),
    log: buildMetadata('log'),
    markdown: buildMetadata('markdown'),
    variable: buildMetadata('variable'),
  }

  const mock: Record<string, unknown> = {
    getCellTypeMetadata: (type: string) => metadata[type] || metadata['table'],

    CELL_TYPE_OPTIONS: Object.entries(BASE_METADATA).map(([type, meta]) => ({
      type,
      name: meta.label,
      description: meta.description,
      icon: meta.icon,
    })),

    createDefaultCell: (type: string, existingNames: Set<string>) => {
      const meta = metadata[type] || metadata['table']
      let name = meta.label as string
      let counter = 1
      while (existingNames.has(name)) {
        counter++
        name = `${meta.label}_${counter}`
      }
      const createDefaultConfig = meta.createDefaultConfig as () => object
      return {
        name,
        layout: { height: meta.defaultHeight },
        ...createDefaultConfig(),
      }
    },

    // Re-export for type compatibility
    CELL_TYPE_METADATA: metadata,
  }

  // Add renderer getter if requested
  if (withRenderers) {
    mock.getCellRenderer = createMockRenderer
  }

  return mock
}
