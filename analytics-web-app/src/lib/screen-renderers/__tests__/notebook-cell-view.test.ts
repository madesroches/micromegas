import { Table } from 'apache-arrow'
import type { CellConfig, CellState, VariableValue } from '../notebook-types'
import type { CellViewContext, CellViewCallbacks } from '../notebook-cell-view'
import {
  formatBytes,
  formatElapsedMs,
  safeTableByteLength,
  buildStatusText,
  buildHgStatusText,
  buildCellRendererProps,
} from '../notebook-cell-view'

// Mock the cell registry — buildCellRendererProps calls getCellTypeMetadata
jest.mock('../cell-registry', () => ({
  getCellTypeMetadata: jest.fn(() => ({
    getRendererProps: () => ({}),
  })),
}))

import { getCellTypeMetadata } from '../cell-registry'
const mockGetCellTypeMetadata = getCellTypeMetadata as jest.Mock

// =============================================================================
// Helpers
// =============================================================================

function makeTable(numRows: number, byteLength: number): Table {
  return {
    numRows,
    batches: [{ data: { byteLength } }],
  } as unknown as Table
}

function makeState(overrides: Partial<CellState> = {}): CellState {
  return { status: 'success', data: [], ...overrides }
}

function makeCell(overrides: Partial<CellConfig> = {}): CellConfig {
  return {
    name: 'test_cell',
    type: 'table',
    layout: { height: 300 },
    sql: 'SELECT 1',
    ...overrides,
  } as CellConfig
}

function makeContext(overrides: Partial<CellViewContext> = {}): CellViewContext {
  return {
    availableVariables: {},
    allVariableValues: {},
    timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
    isEditing: false,
    ...overrides,
  }
}

function makeCallbacks(overrides: Partial<CellViewCallbacks> = {}): CellViewCallbacks {
  return {
    onRun: jest.fn(),
    onSqlChange: jest.fn(),
    onOptionsChange: jest.fn(),
    ...overrides,
  }
}

// =============================================================================
// safeTableByteLength
// =============================================================================

describe('safeTableByteLength', () => {
  it('returns 0 for a table with 0 rows', () => {
    const table = { numRows: 0, batches: [{ data: { byteLength: 100 } }] } as unknown as Table
    expect(safeTableByteLength(table)).toBe(0)
  })

  it('returns byteLength for a table with rows', () => {
    const table = makeTable(10, 2048)
    expect(safeTableByteLength(table)).toBe(2048)
  })

  it('does not crash when batch.data.byteLength throws', () => {
    const table = {
      numRows: 5,
      batches: [{
        data: {
          get byteLength(): number {
            throw new TypeError('Cannot read properties of undefined')
          },
        },
      }],
    } as unknown as Table
    expect(safeTableByteLength(table)).toBe(0)
  })
})

// =============================================================================
// formatBytes
// =============================================================================

describe('formatBytes', () => {
  it('returns "0 B" for zero', () => {
    expect(formatBytes(0)).toBe('0 B')
  })

  it('returns bytes for values under 1 KB', () => {
    expect(formatBytes(512)).toBe('512 B')
  })

  it('returns KB for 1024', () => {
    expect(formatBytes(1024)).toBe('1.0 KB')
  })

  it('returns KB with decimal', () => {
    expect(formatBytes(1536)).toBe('1.5 KB')
  })

  it('returns MB for 1048576', () => {
    expect(formatBytes(1048576)).toBe('1.0 MB')
  })
})

// =============================================================================
// formatElapsedMs
// =============================================================================

describe('formatElapsedMs', () => {
  it('returns "0ms" for zero', () => {
    expect(formatElapsedMs(0)).toBe('0ms')
  })

  it('returns milliseconds for values under 1s', () => {
    expect(formatElapsedMs(500)).toBe('500ms')
  })

  it('returns milliseconds at boundary', () => {
    expect(formatElapsedMs(999)).toBe('999ms')
  })

  it('returns seconds at 1000ms', () => {
    expect(formatElapsedMs(1000)).toBe('1.00s')
  })

  it('returns seconds with decimals', () => {
    expect(formatElapsedMs(5500)).toBe('5.50s')
  })
})

// =============================================================================
// buildStatusText
// =============================================================================

describe('buildStatusText', () => {
  it('returns undefined for non-combobox variable cell', () => {
    const cell = makeCell({ type: 'variable', variableType: 'text' })
    const state = makeState({ data: [makeTable(10, 100)] })
    expect(buildStatusText(cell, state)).toBeUndefined()
  })

  it('returns row/byte string for combobox variable cell with data', () => {
    const cell = makeCell({ type: 'variable', variableType: 'combobox' })
    const state = makeState({ data: [makeTable(5, 2048)] })
    expect(buildStatusText(cell, state)).toBe('5 rows (2.0 KB)')
  })

  it('returns fetch progress during loading', () => {
    const cell = makeCell()
    const state = makeState({
      status: 'loading',
      fetchProgress: { rows: 100, bytes: 2048 },
    })
    expect(buildStatusText(cell, state)).toBe('100 rows (2.0 KB)')
  })

  it('returns undefined during loading without fetchProgress', () => {
    const cell = makeCell()
    const state = makeState({ status: 'loading' })
    expect(buildStatusText(cell, state)).toBeUndefined()
  })

  it('includes elapsed time when present', () => {
    const cell = makeCell()
    const state = makeState({
      data: [makeTable(50, 1024)],
      elapsedMs: 320,
    })
    expect(buildStatusText(cell, state)).toBe('50 rows (1.0 KB) in 320ms')
  })

  it('omits elapsed time when not present', () => {
    const cell = makeCell()
    const state = makeState({ data: [makeTable(50, 1024)] })
    expect(buildStatusText(cell, state)).toBe('50 rows (1.0 KB)')
  })

  it('returns undefined for empty data array', () => {
    const cell = makeCell()
    const state = makeState({ data: [] })
    expect(buildStatusText(cell, state)).toBeUndefined()
  })

  it('does not crash for 0-row table with malformed batch data', () => {
    const cell = makeCell()
    const badTable = {
      numRows: 0,
      batches: [{
        data: {
          get byteLength(): number {
            throw new TypeError('Cannot read properties of undefined')
          },
        },
      }],
    } as unknown as Table
    const state = makeState({ data: [badTable] })
    expect(buildStatusText(cell, state)).toBe('0 rows (0 B)')
  })
})

// =============================================================================
// buildHgStatusText
// =============================================================================

describe('buildHgStatusText', () => {
  it('returns undefined for no children', () => {
    expect(buildHgStatusText([], {})).toBeUndefined()
  })

  it('returns undefined when all children are idle', () => {
    const children = [makeCell({ name: 'a' }), makeCell({ name: 'b' })]
    const states = {
      a: makeState({ data: [] }),
      b: makeState({ data: [] }),
    }
    expect(buildHgStatusText(children, states)).toBeUndefined()
  })

  it('returns stats for single child with data and elapsed', () => {
    const children = [makeCell({ name: 'a' })]
    const states = {
      a: makeState({ data: [makeTable(100, 4096)], elapsedMs: 200 }),
    }
    expect(buildHgStatusText(children, states)).toBe('100 rows (4.0 KB) in 200ms')
  })

  it('sums rows, bytes, and elapsed across two children', () => {
    const children = [makeCell({ name: 'a' }), makeCell({ name: 'b' })]
    const states = {
      a: makeState({ data: [makeTable(100, 2048)], elapsedMs: 200 }),
      b: makeState({ data: [makeTable(50, 1024)], elapsedMs: 100 }),
    }
    expect(buildHgStatusText(children, states)).toBe('150 rows (3.0 KB) in 300ms')
  })

  it('sums only children with data when mixed with idle', () => {
    const children = [makeCell({ name: 'a' }), makeCell({ name: 'b' })]
    const states = {
      a: makeState({ data: [makeTable(100, 2048)], elapsedMs: 200 }),
      b: makeState({ data: [] }),
    }
    expect(buildHgStatusText(children, states)).toBe('100 rows (2.0 KB) in 200ms')
  })

  it('omits elapsed when some children with data lack elapsedMs', () => {
    const children = [makeCell({ name: 'a' }), makeCell({ name: 'b' })]
    const states = {
      a: makeState({ data: [makeTable(100, 2048)], elapsedMs: 200 }),
      b: makeState({ data: [makeTable(50, 1024)] }), // no elapsedMs
    }
    expect(buildHgStatusText(children, states)).toBe('150 rows (3.0 KB)')
  })
})

// =============================================================================
// buildCellRendererProps
// =============================================================================

describe('buildCellRendererProps', () => {
  beforeEach(() => {
    mockGetCellTypeMetadata.mockReturnValue({
      getRendererProps: () => ({}),
    })
  })

  it('maps base fields for a table cell', () => {
    const cell = makeCell()
    const data = [makeTable(10, 100)]
    const state = makeState({ data, error: 'oops' })
    const context = makeContext({ dataSource: 'my-ds' })
    const callbacks = makeCallbacks({
      onContentChange: jest.fn(),
      onTimeRangeSelect: jest.fn(),
    })

    const result = buildCellRendererProps(cell, state, context, callbacks)

    expect(result.name).toBe('test_cell')
    expect(result.data).toBe(data)
    expect(result.status).toBe('success')
    expect(result.error).toBe('oops')
    expect(result.timeRange).toBe(context.timeRange)
    expect(result.variables).toBe(context.availableVariables)
    expect(result.isEditing).toBe(false)
    expect(result.dataSource).toBe('my-ds')
    expect(result.onRun).toBe(callbacks.onRun)
    expect(result.onSqlChange).toBe(callbacks.onSqlChange)
    expect(result.onOptionsChange).toBe(callbacks.onOptionsChange)
    expect(result.onContentChange).toBe(callbacks.onContentChange)
    expect(result.onTimeRangeSelect).toBe(callbacks.onTimeRangeSelect)
    expect(result.value).toBeUndefined()
    expect(result.onValueChange).toBeUndefined()
  })

  it('sets value and onValueChange for variable cells', () => {
    const cell = makeCell({ type: 'variable', name: 'my_var' })
    const onValueChange = jest.fn()
    const context = makeContext({
      allVariableValues: { my_var: 'selected_val' },
    })
    const callbacks = makeCallbacks({ onValueChange })

    const result = buildCellRendererProps(cell, makeState(), context, callbacks)

    expect(result.value).toBe('selected_val')
    expect(result.onValueChange).toBe(onValueChange)
  })

  it('sets value to undefined when variable has no value in map', () => {
    const cell = makeCell({ type: 'variable', name: 'missing_var' })
    const context = makeContext({ allVariableValues: {} })
    const callbacks = makeCallbacks({ onValueChange: jest.fn() })

    const result = buildCellRendererProps(cell, makeState(), context, callbacks)

    expect(result.value).toBeUndefined()
    expect(result.onValueChange).toBeDefined()
  })

  it('applies metadata rendererProps overrides', () => {
    mockGetCellTypeMetadata.mockReturnValue({
      getRendererProps: () => ({ options: { custom: true } }),
    })

    const cell = makeCell()
    const result = buildCellRendererProps(cell, makeState(), makeContext(), makeCallbacks())

    expect(result.options).toEqual({ custom: true })
  })

  it('allows metadata to override data', () => {
    const customData = [makeTable(999, 1)]
    mockGetCellTypeMetadata.mockReturnValue({
      getRendererProps: () => ({ data: customData }),
    })

    const cell = makeCell()
    const state = makeState({ data: [makeTable(1, 1)] })
    const result = buildCellRendererProps(cell, state, makeContext(), makeCallbacks())

    expect(result.data).toBe(customData)
  })

  it('passes context fields through correctly', () => {
    const vars: Record<string, VariableValue> = { x: 'hello' }
    const tr = { begin: 'a', end: 'b' }
    const context = makeContext({
      availableVariables: vars,
      timeRange: tr,
      isEditing: true,
      dataSource: 'ds1',
    })

    const result = buildCellRendererProps(makeCell(), makeState(), context, makeCallbacks())

    expect(result.variables).toBe(vars)
    expect(result.timeRange).toBe(tr)
    expect(result.isEditing).toBe(true)
    expect(result.dataSource).toBe('ds1')
  })
})
