/**
 * Tests for URL state management across ScreenPage and renderer layers.
 *
 * After the variable-ownership refactor:
 * - ScreenPage handles time range via navigate()
 * - useNotebookVariables handles variables via setSearchParams (functional updaters)
 * - Both use URLSearchParams to preserve each other's params
 * - Post-save cleanup is done by renderers, not ScreenPage
 */
import { renderHook, act } from '@testing-library/react'
import { ReactNode, useCallback, useMemo } from 'react'
import { MemoryRouter, useSearchParams, useNavigate } from 'react-router-dom'
import { RESERVED_URL_PARAMS, cleanupTimeParams } from '@/lib/url-cleanup-utils'
import { cleanupVariableParams } from '@/lib/screen-renderers/notebook-utils'
import type { ScreenConfig } from '@/lib/screens-api'

// Mock navigate
const mockNavigate = jest.fn()
jest.mock('react-router-dom', () => ({
  ...jest.requireActual('react-router-dom'),
  useNavigate: () => mockNavigate,
}))

/**
 * Hook that mirrors the URL state management pattern:
 * - ScreenPage: time range via navigate()
 * - Notebook variables: via setSearchParams (functional updaters)
 */
function useUrlStateHandlers() {
  const [searchParams, setSearchParams] = useSearchParams()
  const navigate = useNavigate()

  // Time range change handler (ScreenPage pattern - uses navigate)
  const handleTimeRangeChange = useCallback(
    (from: string, to: string) => {
      const params = new URLSearchParams(searchParams.toString())
      params.set('from', from)
      params.set('to', to)
      navigate(`?${params.toString()}`)
    },
    [searchParams, navigate]
  )

  // Variable change handler (useNotebookVariables pattern - uses setSearchParams)
  const handleUrlVariableChange = useCallback(
    (name: string, value: string) => {
      setSearchParams(prev => {
        const next = new URLSearchParams(prev)
        next.set(name, value)
        return next
      }, { replace: true })
    },
    [setSearchParams]
  )

  // Variable remove handler (useNotebookVariables pattern - uses setSearchParams)
  const handleUrlVariableRemove = useCallback(
    (name: string) => {
      setSearchParams(prev => {
        const next = new URLSearchParams(prev)
        next.delete(name)
        return next
      }, { replace: true })
    },
    [setSearchParams]
  )

  // Compute URL variables from searchParams
  const urlVariables = useMemo(() => {
    const vars: Record<string, string> = {}
    searchParams.forEach((value, key) => {
      if (!RESERVED_URL_PARAMS.has(key)) {
        vars[key] = value
      }
    })
    return vars
  }, [searchParams])

  // Get time range from URL
  const urlTimeRange = useMemo(() => ({
    from: searchParams.get('from'),
    to: searchParams.get('to'),
  }), [searchParams])

  return {
    handleTimeRangeChange,
    handleUrlVariableChange,
    handleUrlVariableRemove,
    urlVariables,
    urlTimeRange,
  }
}

// Helper to create wrapper with initial URL
function createWrapper(initialEntries: string[] = ['/']) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return <MemoryRouter initialEntries={initialEntries}>{children}</MemoryRouter>
  }
}

describe('ScreenPage URL state management', () => {
  beforeEach(() => {
    mockNavigate.mockClear()
  })

  describe('time range changes preserve variables', () => {
    it('should preserve variable params when changing time range', () => {
      const { result } = renderHook(() => useUrlStateHandlers(), {
        wrapper: createWrapper(['/screen/test?myvar=hello&othervar=world']),
      })

      // Verify initial state has variables
      expect(result.current.urlVariables).toEqual({
        myvar: 'hello',
        othervar: 'world',
      })

      // Change time range
      act(() => {
        result.current.handleTimeRangeChange('now-30m', 'now')
      })

      // Verify navigate was called with variables preserved
      const url = mockNavigate.mock.calls[0][0] as string
      expect(url).toContain('myvar=hello')
      expect(url).toContain('othervar=world')
      expect(url).toContain('from=now-30m')
      expect(url).toContain('to=now')
    })

    it('should preserve existing time range when only updating one param', () => {
      const { result } = renderHook(() => useUrlStateHandlers(), {
        wrapper: createWrapper(['/screen/test?from=now-1h&to=now']),
      })

      // Change both time params
      act(() => {
        result.current.handleTimeRangeChange('now-24h', 'now-1h')
      })

      const url = mockNavigate.mock.calls[0][0] as string
      expect(url).toContain('from=now-24h')
      expect(url).toContain('to=now-1h')
    })
  })

  describe('variable changes preserve time range', () => {
    it('should preserve time range params when changing variables', () => {
      const { result } = renderHook(() => useUrlStateHandlers(), {
        wrapper: createWrapper(['/screen/test?from=now-1h&to=now']),
      })

      // Verify initial time range
      expect(result.current.urlTimeRange).toEqual({
        from: 'now-1h',
        to: 'now',
      })

      // Change a variable (uses setSearchParams, not navigate)
      act(() => {
        result.current.handleUrlVariableChange('myvar', 'newvalue')
      })

      // After setSearchParams, the hook re-renders with updated state
      expect(result.current.urlTimeRange).toEqual({
        from: 'now-1h',
        to: 'now',
      })
      expect(result.current.urlVariables).toEqual({
        myvar: 'newvalue',
      })
    })

    it('should preserve other variables when changing one variable', () => {
      const { result } = renderHook(() => useUrlStateHandlers(), {
        wrapper: createWrapper(['/screen/test?var1=a&var2=b&var3=c']),
      })

      // Change one variable
      act(() => {
        result.current.handleUrlVariableChange('var2', 'updated')
      })

      expect(result.current.urlVariables).toEqual({
        var1: 'a',
        var2: 'updated',
        var3: 'c',
      })
    })
  })

  describe('variable removal preserves other state', () => {
    it('should preserve time range and other variables when removing a variable', () => {
      const { result } = renderHook(() => useUrlStateHandlers(), {
        wrapper: createWrapper(['/screen/test?from=now-1h&to=now&var1=a&var2=b']),
      })

      // Remove one variable
      act(() => {
        result.current.handleUrlVariableRemove('var1')
      })

      expect(result.current.urlTimeRange).toEqual({
        from: 'now-1h',
        to: 'now',
      })
      expect(result.current.urlVariables).toEqual({
        var2: 'b',
      })
    })
  })

  describe('combined operations', () => {
    it('should handle interleaved time and variable changes', () => {
      // Start with some state
      const { result } = renderHook(() => useUrlStateHandlers(), {
        wrapper: createWrapper(['/screen/test?from=now-1h&to=now&myvar=initial']),
      })

      // First: change a variable (uses setSearchParams)
      act(() => {
        result.current.handleUrlVariableChange('myvar', 'updated')
      })

      // Variable updated, time range preserved
      expect(result.current.urlVariables).toEqual({ myvar: 'updated' })
      expect(result.current.urlTimeRange).toEqual({ from: 'now-1h', to: 'now' })

      // Second: change time range (uses navigate)
      act(() => {
        result.current.handleTimeRangeChange('now-24h', 'now')
      })

      const url = mockNavigate.mock.calls[0][0] as string
      expect(url).toContain('from=now-24h')
      expect(url).toContain('to=now')
      expect(url).toContain('myvar=updated') // Variable should still be there!
    })
  })
})

describe('cleanupTimeParams', () => {
  it('should remove from/to params that match saved config', () => {
    const params = new URLSearchParams('from=now-1h&to=now&myvar=hello')
    const savedConfig: ScreenConfig = { timeRangeFrom: 'now-1h', timeRangeTo: 'now' }

    cleanupTimeParams(params, savedConfig)

    expect(params.has('from')).toBe(false)
    expect(params.has('to')).toBe(false)
    expect(params.get('myvar')).toBe('hello')
  })

  it('should keep from/to params that differ from saved config', () => {
    const params = new URLSearchParams('from=now-24h&to=now')
    const savedConfig: ScreenConfig = { timeRangeFrom: 'now-1h', timeRangeTo: 'now' }

    cleanupTimeParams(params, savedConfig)

    expect(params.get('from')).toBe('now-24h')
    expect(params.has('to')).toBe(false) // to matches
  })

  it('should not remove params when saved config has no time range', () => {
    const params = new URLSearchParams('from=now-1h&to=now')
    const savedConfig: ScreenConfig = {}

    cleanupTimeParams(params, savedConfig)

    expect(params.get('from')).toBe('now-1h')
    expect(params.get('to')).toBe('now')
  })
})

describe('cleanupVariableParams', () => {
  it('should remove variable params that match saved cell defaults', () => {
    const params = new URLSearchParams('from=now-1h&to=now&region=us-east-1&env=prod')
    const savedConfig: ScreenConfig = {
      cells: [
        { type: 'variable', name: 'region', defaultValue: 'us-east-1' },
        { type: 'variable', name: 'env', defaultValue: 'staging' },
      ],
    }

    cleanupVariableParams(params, savedConfig)

    expect(params.has('region')).toBe(false) // matches default
    expect(params.get('env')).toBe('prod') // differs from default
    expect(params.get('from')).toBe('now-1h') // reserved, untouched
    expect(params.get('to')).toBe('now') // reserved, untouched
  })

  it('should not touch params when no cells in config', () => {
    const params = new URLSearchParams('region=us-east-1')
    const savedConfig: ScreenConfig = {}

    cleanupVariableParams(params, savedConfig)

    expect(params.get('region')).toBe('us-east-1')
  })

  it('should not touch params for non-variable cells', () => {
    const params = new URLSearchParams('query1=test')
    const savedConfig: ScreenConfig = {
      cells: [
        { type: 'query', name: 'query1', sql: 'SELECT 1' },
      ],
    }

    cleanupVariableParams(params, savedConfig)

    expect(params.get('query1')).toBe('test')
  })

  it('should compose with cleanupTimeParams in a single pass', () => {
    const params = new URLSearchParams('from=now-1h&to=now&region=us-east-1&env=prod')
    const savedConfig: ScreenConfig = {
      timeRangeFrom: 'now-1h',
      timeRangeTo: 'now',
      cells: [
        { type: 'variable', name: 'region', defaultValue: 'us-east-1' },
        { type: 'variable', name: 'env', defaultValue: 'staging' },
      ],
    }

    // Both cleanup functions compose on the same URLSearchParams
    cleanupTimeParams(params, savedConfig)
    cleanupVariableParams(params, savedConfig)

    expect(params.has('from')).toBe(false)
    expect(params.has('to')).toBe(false)
    expect(params.has('region')).toBe(false)
    expect(params.get('env')).toBe('prod')
    expect(params.toString()).toBe('env=prod')
  })
})
