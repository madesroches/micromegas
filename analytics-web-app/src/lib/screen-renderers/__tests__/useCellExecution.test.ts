/**
 * Tests for useCellExecution hook
 */
import { renderHook, act, waitFor } from '@testing-library/react'
import React from 'react'

// Mock streamQuery function
const mockStreamQuery = jest.fn()

jest.mock('@/lib/arrow-stream', () => ({
  streamQuery: (...args: unknown[]) => mockStreamQuery(...args),
}))

// Mock Apache Arrow
jest.mock('apache-arrow', () => {
  class MockTable {
    numRows: number
    numCols: number
    schema: { fields: { name: string }[] }

    constructor(public batches: unknown[] = []) {
      this.numRows = batches.length > 0 ? 5 : 0
      this.numCols = 2
      this.schema = { fields: [{ name: 'value' }, { name: 'label' }] }
    }

    get(index: number) {
      if (index < this.numRows) {
        return { value: `val${index}`, label: `Label ${index}` }
      }
      return null
    }
  }

  return {
    Table: MockTable,
    Schema: class MockSchema {
      constructor(public fields: unknown[] = []) {}
    },
    RecordBatch: class MockRecordBatch {
      numRows = 5
    },
  }
})

import { useCellExecution } from '../useCellExecution'
import { CellConfig } from '../notebook-utils'

// Helper to create mock async generator
function createMockGenerator<T>(results: T[]): AsyncGenerator<T> {
  let index = 0
  return {
    async next() {
      if (index < results.length) {
        return { done: false, value: results[index++] }
      }
      return { done: true, value: undefined }
    },
    async return(value?: unknown) {
      return { done: true, value: value as T }
    },
    async throw(e?: unknown) {
      throw e
    },
    [Symbol.asyncIterator]() {
      return this
    },
  } as AsyncGenerator<T>
}

// Helper to create mock batch results
function createSuccessResults() {
  return createMockGenerator([
    { type: 'batch', batch: { numRows: 5 } },
    { type: 'done' },
  ])
}

function createErrorResults(message: string) {
  // The streamQuery yields an error which gets thrown in executeSql
  return createMockGenerator([{ type: 'error', error: { message } }])
}

// Create a generator that throws an error
function createThrowingGenerator(message: string): AsyncGenerator<unknown> {
  return {
    async next() {
      throw new Error(message)
    },
    async return(value?: unknown) {
      return { done: true, value }
    },
    async throw(e?: unknown) {
      throw e
    },
    [Symbol.asyncIterator]() {
      return this
    },
  } as AsyncGenerator<unknown>
}

describe('useCellExecution', () => {
  const defaultTimeRange = { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' }

  // Create a ref that can be mutated
  function createVariableValuesRef(initialValues: Record<string, string> = {}) {
    return { current: { ...initialValues } }
  }

  beforeEach(() => {
    jest.clearAllMocks()
  })

  describe('initial state', () => {
    it('should return empty cellStates initially', () => {
      const variableValuesRef = createVariableValuesRef()
      const { result } = renderHook(() =>
        useCellExecution({
          cells: [],
          timeRange: defaultTimeRange,
          variableValuesRef,
          setVariableValue: jest.fn(),
          refreshTrigger: 0,
        })
      )

      expect(result.current.cellStates).toEqual({})
    })

    it('should provide all expected functions', () => {
      const variableValuesRef = createVariableValuesRef()
      const { result } = renderHook(() =>
        useCellExecution({
          cells: [],
          timeRange: defaultTimeRange,
          variableValuesRef,
          setVariableValue: jest.fn(),
          refreshTrigger: 0,
        })
      )

      expect(typeof result.current.executeCell).toBe('function')
      expect(typeof result.current.executeFromCell).toBe('function')
      expect(typeof result.current.migrateCellState).toBe('function')
      expect(typeof result.current.removeCellState).toBe('function')
    })
  })

  describe('executeCell', () => {
    describe('markdown cells', () => {
      it('should immediately succeed for markdown cells without SQL execution', async () => {
        const cells: CellConfig[] = [
          { type: 'markdown', name: 'Notes', content: '# Hello', layout: { height: 'auto' } },
        ]
        const variableValuesRef = createVariableValuesRef()

        const { result } = renderHook(() =>
          useCellExecution({
            cells,
            timeRange: defaultTimeRange,
            variableValuesRef,
            setVariableValue: jest.fn(),
            refreshTrigger: 0,
          })
        )

        let success: boolean = false
        await act(async () => {
          success = await result.current.executeCell(0)
        })

        expect(success).toBe(true)
        expect(result.current.cellStates['Notes']).toEqual({ status: 'success', data: null })
        expect(mockStreamQuery).not.toHaveBeenCalled()
      })
    })

    describe('text/number variable cells', () => {
      it('should immediately succeed for text variable cells without SQL execution', async () => {
        const cells: CellConfig[] = [
          {
            type: 'variable',
            name: 'TextVar',
            variableType: 'text',
            defaultValue: 'test',
            layout: { height: 'auto' },
          },
        ]
        const variableValuesRef = createVariableValuesRef()

        const { result } = renderHook(() =>
          useCellExecution({
            cells,
            timeRange: defaultTimeRange,
            variableValuesRef,
            setVariableValue: jest.fn(),
            refreshTrigger: 0,
          })
        )

        let success: boolean = false
        await act(async () => {
          success = await result.current.executeCell(0)
        })

        expect(success).toBe(true)
        expect(result.current.cellStates['TextVar']).toEqual({ status: 'success', data: null })
        expect(mockStreamQuery).not.toHaveBeenCalled()
      })

      it('should immediately succeed for number variable cells without SQL execution', async () => {
        const cells: CellConfig[] = [
          {
            type: 'variable',
            name: 'NumVar',
            variableType: 'number',
            defaultValue: '42',
            layout: { height: 'auto' },
          },
        ]
        const variableValuesRef = createVariableValuesRef()

        const { result } = renderHook(() =>
          useCellExecution({
            cells,
            timeRange: defaultTimeRange,
            variableValuesRef,
            setVariableValue: jest.fn(),
            refreshTrigger: 0,
          })
        )

        let success: boolean = false
        await act(async () => {
          success = await result.current.executeCell(0)
        })

        expect(success).toBe(true)
        expect(result.current.cellStates['NumVar']).toEqual({ status: 'success', data: null })
        expect(mockStreamQuery).not.toHaveBeenCalled()
      })
    })

    describe('combobox variable cells', () => {
      it('should execute SQL and extract options for combobox variable cells', async () => {
        mockStreamQuery.mockReturnValue(createSuccessResults())

        const cells: CellConfig[] = [
          {
            type: 'variable',
            name: 'Dropdown',
            variableType: 'combobox',
            sql: 'SELECT name FROM options',
            layout: { height: 'auto' },
          },
        ]
        const setVariableValue = jest.fn()
        const variableValuesRef = createVariableValuesRef()

        const { result } = renderHook(() =>
          useCellExecution({
            cells,
            timeRange: defaultTimeRange,
            variableValuesRef,
            setVariableValue,
            refreshTrigger: 0,
          })
        )

        await act(async () => {
          await result.current.executeCell(0)
        })

        expect(mockStreamQuery).toHaveBeenCalled()
        expect(result.current.cellStates['Dropdown'].status).toBe('success')
        expect(result.current.cellStates['Dropdown'].variableOptions).toBeDefined()
        // Should auto-select first option
        expect(setVariableValue).toHaveBeenCalledWith('Dropdown', 'val0')
      })

      it('should not auto-select if value already set', async () => {
        mockStreamQuery.mockReturnValue(createSuccessResults())

        const cells: CellConfig[] = [
          {
            type: 'variable',
            name: 'Dropdown',
            variableType: 'combobox',
            sql: 'SELECT name FROM options',
            layout: { height: 'auto' },
          },
        ]
        const setVariableValue = jest.fn()
        const variableValuesRef = createVariableValuesRef({ Dropdown: 'existing' })

        const { result } = renderHook(() =>
          useCellExecution({
            cells,
            timeRange: defaultTimeRange,
            variableValuesRef,
            setVariableValue,
            refreshTrigger: 0,
          })
        )

        await act(async () => {
          await result.current.executeCell(0)
        })

        expect(setVariableValue).not.toHaveBeenCalled()
      })
    })

    describe('query cells (table, chart, log)', () => {
      it('should execute SQL for table cells', async () => {
        mockStreamQuery.mockReturnValue(createSuccessResults())

        const cells: CellConfig[] = [
          { type: 'table', name: 'Results', sql: 'SELECT * FROM logs', layout: { height: 'auto' } },
        ]
        const variableValuesRef = createVariableValuesRef()

        const { result } = renderHook(() =>
          useCellExecution({
            cells,
            timeRange: defaultTimeRange,
            variableValuesRef,
            setVariableValue: jest.fn(),
            refreshTrigger: 0,
          })
        )

        await act(async () => {
          await result.current.executeCell(0)
        })

        expect(mockStreamQuery).toHaveBeenCalled()
        expect(result.current.cellStates['Results'].status).toBe('success')
        expect(result.current.cellStates['Results'].data).not.toBeNull()
      })

      it('should succeed with null data when SQL is empty', async () => {
        const cells: CellConfig[] = [
          { type: 'table', name: 'Empty', sql: '', layout: { height: 'auto' } },
        ]
        const variableValuesRef = createVariableValuesRef()

        const { result } = renderHook(() =>
          useCellExecution({
            cells,
            timeRange: defaultTimeRange,
            variableValuesRef,
            setVariableValue: jest.fn(),
            refreshTrigger: 0,
          })
        )

        await act(async () => {
          await result.current.executeCell(0)
        })

        expect(mockStreamQuery).not.toHaveBeenCalled()
        expect(result.current.cellStates['Empty']).toEqual({ status: 'success', data: null })
      })

      it('should substitute variables from cells above', async () => {
        mockStreamQuery.mockReturnValue(createSuccessResults())

        const cells: CellConfig[] = [
          {
            type: 'variable',
            name: 'Metric',
            variableType: 'text',
            defaultValue: 'cpu',
            layout: { height: 'auto' },
          },
          {
            type: 'table',
            name: 'Results',
            sql: "SELECT * FROM measures WHERE name = '$Metric'",
            layout: { height: 'auto' },
          },
        ]
        const variableValuesRef = createVariableValuesRef({ Metric: 'memory' })

        const { result } = renderHook(() =>
          useCellExecution({
            cells,
            timeRange: defaultTimeRange,
            variableValuesRef,
            setVariableValue: jest.fn(),
            refreshTrigger: 0,
          })
        )

        await act(async () => {
          await result.current.executeCell(1)
        })

        expect(mockStreamQuery).toHaveBeenCalled()
        const callArgs = mockStreamQuery.mock.calls[0][0]
        expect(callArgs.sql).toContain('memory')
      })
    })

    describe('error handling', () => {
      it('should set error status on query failure', async () => {
        mockStreamQuery.mockReturnValue(createThrowingGenerator('Syntax error near SELECT'))

        const cells: CellConfig[] = [
          { type: 'table', name: 'Bad', sql: 'SELECT', layout: { height: 'auto' } },
        ]
        const variableValuesRef = createVariableValuesRef()

        const { result } = renderHook(() =>
          useCellExecution({
            cells,
            timeRange: defaultTimeRange,
            variableValuesRef,
            setVariableValue: jest.fn(),
            refreshTrigger: 0,
          })
        )

        let success: boolean = true
        await act(async () => {
          success = await result.current.executeCell(0)
        })

        expect(success).toBe(false)
        expect(result.current.cellStates['Bad'].status).toBe('error')
        expect(result.current.cellStates['Bad'].error).toBe('Syntax error near SELECT')
      })

      it('should return false for invalid cell index', async () => {
        const cells: CellConfig[] = []
        const variableValuesRef = createVariableValuesRef()

        const { result } = renderHook(() =>
          useCellExecution({
            cells,
            timeRange: defaultTimeRange,
            variableValuesRef,
            setVariableValue: jest.fn(),
            refreshTrigger: 0,
          })
        )

        let success: boolean = true
        await act(async () => {
          success = await result.current.executeCell(5)
        })

        expect(success).toBe(false)
      })
    })
  })

  describe('executeFromCell', () => {
    it('should execute all cells starting from given index', async () => {
      mockStreamQuery.mockReturnValue(createSuccessResults())

      const cells: CellConfig[] = [
        { type: 'table', name: 'First', sql: 'SELECT 1', layout: { height: 'auto' } },
        { type: 'table', name: 'Second', sql: 'SELECT 2', layout: { height: 'auto' } },
        { type: 'table', name: 'Third', sql: 'SELECT 3', layout: { height: 'auto' } },
      ]
      const variableValuesRef = createVariableValuesRef()

      const { result } = renderHook(() =>
        useCellExecution({
          cells,
          timeRange: defaultTimeRange,
          variableValuesRef,
          setVariableValue: jest.fn(),
          refreshTrigger: 0,
        })
      )

      // Wait for initial auto-execution to complete
      await waitFor(() => {
        expect(result.current.cellStates['First']?.status).toBe('success')
      })

      // Clear mock to track new calls
      mockStreamQuery.mockClear()

      // Execute from second cell
      await act(async () => {
        await result.current.executeFromCell(1)
      })

      // First cell should not have been re-executed (check call count)
      // Second and Third should be executed
      expect(result.current.cellStates['First'].status).toBe('success')
      expect(result.current.cellStates['Second'].status).toBe('success')
      expect(result.current.cellStates['Third'].status).toBe('success')
      // Only 2 new calls (for Second and Third)
      expect(mockStreamQuery).toHaveBeenCalledTimes(2)
    })

    it('should mark remaining cells as blocked when one fails', async () => {
      // First two calls succeed (initial auto-run), then set up failure for manual run
      mockStreamQuery.mockReturnValue(createSuccessResults())

      const cells: CellConfig[] = [
        { type: 'table', name: 'First', sql: 'SELECT 1', layout: { height: 'auto' } },
        { type: 'table', name: 'Second', sql: 'SELECT 2', layout: { height: 'auto' } },
        { type: 'table', name: 'Third', sql: 'SELECT 3', layout: { height: 'auto' } },
      ]
      const variableValuesRef = createVariableValuesRef()

      const { result } = renderHook(() =>
        useCellExecution({
          cells,
          timeRange: defaultTimeRange,
          variableValuesRef,
          setVariableValue: jest.fn(),
          refreshTrigger: 0,
        })
      )

      // Wait for initial auto-execution
      await waitFor(() => {
        expect(result.current.cellStates['Third']?.status).toBe('success')
      })

      // Now set up failure for First, success for any others
      mockStreamQuery
        .mockReturnValueOnce(createThrowingGenerator('Query failed'))
        .mockReturnValue(createSuccessResults())

      await act(async () => {
        await result.current.executeFromCell(0)
      })

      expect(result.current.cellStates['First'].status).toBe('error')
      expect(result.current.cellStates['Second'].status).toBe('blocked')
      expect(result.current.cellStates['Third'].status).toBe('blocked')
    })

    it('should not mark markdown cells as blocked', async () => {
      mockStreamQuery.mockReturnValue(createThrowingGenerator('Query failed'))

      const cells: CellConfig[] = [
        { type: 'table', name: 'Query', sql: 'SELECT 1', layout: { height: 'auto' } },
        { type: 'markdown', name: 'Notes', content: '# Notes', layout: { height: 'auto' } },
      ]
      const variableValuesRef = createVariableValuesRef()

      const { result } = renderHook(() =>
        useCellExecution({
          cells,
          timeRange: defaultTimeRange,
          variableValuesRef,
          setVariableValue: jest.fn(),
          refreshTrigger: 0,
        })
      )

      // Wait for initial auto-execution
      await waitFor(() => {
        expect(result.current.cellStates['Query']?.status).toBe('error')
      })

      // Markdown cells executed before the error should still succeed
      // Notes is after Query, so it should not be blocked (it's markdown)
      expect(result.current.cellStates['Query'].status).toBe('error')
      // Markdown cells are never blocked - they just execute independently
      expect(result.current.cellStates['Notes']).toBeUndefined()
    })
  })

  describe('migrateCellState', () => {
    it('should move cell state to new name', async () => {
      mockStreamQuery.mockReturnValue(createSuccessResults())

      const cells: CellConfig[] = [
        { type: 'table', name: 'OldName', sql: 'SELECT 1', layout: { height: 'auto' } },
      ]
      const variableValuesRef = createVariableValuesRef()

      const { result } = renderHook(() =>
        useCellExecution({
          cells,
          timeRange: defaultTimeRange,
          variableValuesRef,
          setVariableValue: jest.fn(),
          refreshTrigger: 0,
        })
      )

      await act(async () => {
        await result.current.executeCell(0)
      })

      expect(result.current.cellStates['OldName'].status).toBe('success')

      act(() => {
        result.current.migrateCellState('OldName', 'NewName')
      })

      expect(result.current.cellStates['OldName']).toBeUndefined()
      expect(result.current.cellStates['NewName'].status).toBe('success')
    })

    it('should handle migrating non-existent state', () => {
      const cells: CellConfig[] = []
      const variableValuesRef = createVariableValuesRef()

      const { result } = renderHook(() =>
        useCellExecution({
          cells,
          timeRange: defaultTimeRange,
          variableValuesRef,
          setVariableValue: jest.fn(),
          refreshTrigger: 0,
        })
      )

      // Should not throw
      act(() => {
        result.current.migrateCellState('NonExistent', 'NewName')
      })

      expect(result.current.cellStates).toEqual({})
    })
  })

  describe('removeCellState', () => {
    it('should remove cell state', async () => {
      mockStreamQuery.mockReturnValue(createSuccessResults())

      const cells: CellConfig[] = [
        { type: 'table', name: 'ToDelete', sql: 'SELECT 1', layout: { height: 'auto' } },
      ]
      const variableValuesRef = createVariableValuesRef()

      const { result } = renderHook(() =>
        useCellExecution({
          cells,
          timeRange: defaultTimeRange,
          variableValuesRef,
          setVariableValue: jest.fn(),
          refreshTrigger: 0,
        })
      )

      await act(async () => {
        await result.current.executeCell(0)
      })

      expect(result.current.cellStates['ToDelete']).toBeDefined()

      act(() => {
        result.current.removeCellState('ToDelete')
      })

      expect(result.current.cellStates['ToDelete']).toBeUndefined()
    })
  })

  describe('refresh trigger', () => {
    it('should re-execute all cells when refreshTrigger changes', async () => {
      mockStreamQuery.mockReturnValue(createSuccessResults())

      const cells: CellConfig[] = [
        { type: 'table', name: 'Query', sql: 'SELECT 1', layout: { height: 'auto' } },
      ]
      const variableValuesRef = createVariableValuesRef()

      const { result, rerender } = renderHook(
        ({ refreshTrigger }) =>
          useCellExecution({
            cells,
            timeRange: defaultTimeRange,
            variableValuesRef,
            setVariableValue: jest.fn(),
            refreshTrigger,
          }),
        { initialProps: { refreshTrigger: 0 } }
      )

      // Wait for initial execution
      await waitFor(() => {
        expect(result.current.cellStates['Query']?.status).toBe('success')
      })

      const callCountAfterInitial = mockStreamQuery.mock.calls.length

      // Trigger refresh
      await act(async () => {
        rerender({ refreshTrigger: 1 })
        await new Promise((resolve) => setTimeout(resolve, 10))
      })

      await waitFor(() => {
        expect(mockStreamQuery.mock.calls.length).toBeGreaterThan(callCountAfterInitial)
      })
    })
  })

  describe('loading state', () => {
    it('should set loading status before executing', async () => {
      let resolveQuery: () => void
      const queryPromise = new Promise<void>((resolve) => {
        resolveQuery = resolve
      })

      mockStreamQuery.mockImplementation(async function* () {
        await queryPromise
        yield { type: 'done' }
      })

      const cells: CellConfig[] = [
        { type: 'table', name: 'Slow', sql: 'SELECT 1', layout: { height: 'auto' } },
      ]
      const variableValuesRef = createVariableValuesRef()

      const { result } = renderHook(() =>
        useCellExecution({
          cells,
          timeRange: defaultTimeRange,
          variableValuesRef,
          setVariableValue: jest.fn(),
          refreshTrigger: 0,
        })
      )

      // Start execution but don't await
      let executePromise: Promise<boolean>
      act(() => {
        executePromise = result.current.executeCell(0)
      })

      // Check loading state
      await waitFor(() => {
        expect(result.current.cellStates['Slow']?.status).toBe('loading')
      })

      // Resolve and complete
      await act(async () => {
        resolveQuery!()
        await executePromise
      })

      expect(result.current.cellStates['Slow'].status).toBe('success')
    })
  })
})
