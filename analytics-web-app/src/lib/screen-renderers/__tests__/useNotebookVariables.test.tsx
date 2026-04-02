/**
 * Tests for useNotebookVariables hook.
 *
 * The hook reads URL params once at mount (to restore bookmarked state),
 * then maintains variable values as local React state. The URL is only
 * written to (for bookmarkability), never read back.
 *
 * Key scenario: rapid setVariableValue calls in one tick (user changes
 * datasource → auto-run synchronously evaluates expression variable)
 * must all persist since they update local state directly.
 *
 * useSearchParams is mocked with React state since MemoryRouter's
 * setSearchParams doesn't flush in renderHook.
 */
import { renderHook, act } from '@testing-library/react'
import { ReactNode, useState, useCallback, useMemo, useRef } from 'react'
import type { CellConfig, VariableCellConfig } from '../notebook-types'

// ---------------------------------------------------------------------------
// Mock useSearchParams with real React state
// ---------------------------------------------------------------------------
type SetSearchParamsFn = (
  nextInit: URLSearchParams | ((prev: URLSearchParams) => URLSearchParams),
  opts?: { replace?: boolean },
) => void

let mockInitialSearch = ''

jest.mock('react-router-dom', () => {
  const actual = jest.requireActual('react-router-dom')
  return {
    ...actual,
    useSearchParams: (): [URLSearchParams, SetSearchParamsFn] => {
      // eslint-disable-next-line react-hooks/rules-of-hooks
      const [raw, setRaw] = useState(mockInitialSearch)
      // eslint-disable-next-line react-hooks/rules-of-hooks
      const params = useMemo(() => new URLSearchParams(raw), [raw])

      // Replicate stale-closure behavior of real React Router:
      // functional updater receives the *current render's* params (not latest).
      // eslint-disable-next-line react-hooks/rules-of-hooks
      const staleRef = useRef(params)
      staleRef.current = params

      // eslint-disable-next-line react-hooks/rules-of-hooks
      const setSearchParams: SetSearchParamsFn = useCallback(
        (nextInit) => {
          if (typeof nextInit === 'function') {
            const next = nextInit(staleRef.current)
            setRaw(next.toString())
          } else {
            setRaw(nextInit.toString())
          }
        },
        [],
      )

      return [params, setSearchParams]
    },
  }
})

// Import after mock so the hook gets the mocked useSearchParams
import { useNotebookVariables } from '../useNotebookVariables'

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function Wrapper({ children }: { children: ReactNode }) {
  return <>{children}</>
}

function makeVariableCell(
  name: string,
  variableType: VariableCellConfig['variableType'],
  defaultValue?: string,
): VariableCellConfig {
  return {
    name,
    type: 'variable',
    variableType,
    layout: { height: 0 },
    ...(defaultValue !== undefined ? { defaultValue } : {}),
  }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('useNotebookVariables', () => {
  beforeEach(() => {
    mockInitialSearch = ''
  })

  describe('basic variable values', () => {
    it('returns default values when no URL overrides', () => {
      const cells: CellConfig[] = [
        makeVariableCell('source', 'datasource', 'prod'),
        makeVariableCell('interval', 'expression', '1 seconds'),
      ]

      const { result } = renderHook(
        () => useNotebookVariables(cells, cells),
        { wrapper: Wrapper },
      )

      expect(result.current.variableValues).toEqual({
        source: 'prod',
        interval: '1 seconds',
      })
    })

    it('applies URL overrides over defaults', () => {
      mockInitialSearch = 'source=staging&interval=500ms'

      const cells: CellConfig[] = [
        makeVariableCell('source', 'datasource', 'prod'),
        makeVariableCell('interval', 'expression', '1 seconds'),
      ]

      const { result } = renderHook(
        () => useNotebookVariables(cells, cells),
        { wrapper: Wrapper },
      )

      expect(result.current.variableValues).toEqual({
        source: 'staging',
        interval: '500ms',
      })
    })
  })

  describe('setVariableValue', () => {
    it('updates a variable and reflects in variableValues', () => {
      const cells: CellConfig[] = [
        makeVariableCell('source', 'datasource', 'prod'),
      ]

      const { result } = renderHook(
        () => useNotebookVariables(cells, cells),
        { wrapper: Wrapper },
      )

      act(() => {
        result.current.setVariableValue('source', 'staging')
      })

      expect(result.current.variableValues.source).toBe('staging')
    })

    it('removes URL param when value matches saved default (delta logic)', () => {
      mockInitialSearch = 'source=staging'

      const cells: CellConfig[] = [
        makeVariableCell('source', 'datasource', 'prod'),
      ]

      const { result } = renderHook(
        () => useNotebookVariables(cells, cells),
        { wrapper: Wrapper },
      )

      // Initially overridden
      expect(result.current.variableValues.source).toBe('staging')

      // Set back to default
      act(() => {
        result.current.setVariableValue('source', 'prod')
      })

      // Should be back to default (param removed from URL)
      expect(result.current.variableValues.source).toBe('prod')
    })
  })

  describe('rapid successive setVariableValue calls (the auto-run race condition)', () => {
    it('second call must not overwrite the first', () => {
      const cells: CellConfig[] = [
        makeVariableCell('source', 'datasource', 'prod'),
        makeVariableCell('interval', 'expression', '1 seconds'),
      ]

      const { result } = renderHook(
        () => useNotebookVariables(cells, cells),
        { wrapper: Wrapper },
      )

      // Simulate: user changes source dropdown, then auto-run immediately
      // evaluates expression variable — both in the same tick
      act(() => {
        result.current.setVariableValue('source', 'staging')
        result.current.setVariableValue('interval', '500ms')
      })

      // Both values must be present — before the fix, the second call
      // would read stale URL params and clobber source back to default.
      expect(result.current.variableValues.source).toBe('staging')
      expect(result.current.variableValues.interval).toBe('500ms')
    })

    it('three rapid calls all persist', () => {
      const cells: CellConfig[] = [
        makeVariableCell('a', 'datasource', 'default_a'),
        makeVariableCell('b', 'text', 'default_b'),
        makeVariableCell('c', 'expression', 'default_c'),
      ]

      const { result } = renderHook(
        () => useNotebookVariables(cells, cells),
        { wrapper: Wrapper },
      )

      act(() => {
        result.current.setVariableValue('a', 'new_a')
        result.current.setVariableValue('b', 'new_b')
        result.current.setVariableValue('c', 'new_c')
      })

      expect(result.current.variableValues).toEqual({
        a: 'new_a',
        b: 'new_b',
        c: 'new_c',
      })
    })

    it('rapid set + reset-to-default does not clobber the set', () => {
      mockInitialSearch = 'interval=500ms'

      const cells: CellConfig[] = [
        makeVariableCell('source', 'datasource', 'prod'),
        makeVariableCell('interval', 'expression', '1 seconds'),
      ]

      const { result } = renderHook(
        () => useNotebookVariables(cells, cells),
        { wrapper: Wrapper },
      )

      // User changes source; expression re-evaluates to its default value
      act(() => {
        result.current.setVariableValue('source', 'staging')
        result.current.setVariableValue('interval', '1 seconds') // matches default → removed from URL
      })

      expect(result.current.variableValues.source).toBe('staging')
      expect(result.current.variableValues.interval).toBe('1 seconds')
    })
  })

  describe('ref synchronous access', () => {
    it('variableValuesRef is updated immediately after setVariableValue', () => {
      const cells: CellConfig[] = [
        makeVariableCell('source', 'datasource', 'prod'),
      ]

      const { result } = renderHook(
        () => useNotebookVariables(cells, cells),
        { wrapper: Wrapper },
      )

      act(() => {
        result.current.setVariableValue('source', 'staging')
        expect(result.current.variableValuesRef.current.source).toBe('staging')
      })
    })

    it('ref reflects all rapid calls synchronously', () => {
      const cells: CellConfig[] = [
        makeVariableCell('source', 'datasource', 'prod'),
        makeVariableCell('interval', 'expression', '1 seconds'),
      ]

      const { result } = renderHook(
        () => useNotebookVariables(cells, cells),
        { wrapper: Wrapper },
      )

      act(() => {
        result.current.setVariableValue('source', 'staging')
        result.current.setVariableValue('interval', '500ms')

        // Both must be visible in the ref immediately
        expect(result.current.variableValuesRef.current.source).toBe('staging')
        expect(result.current.variableValuesRef.current.interval).toBe('500ms')
      })
    })
  })
})
