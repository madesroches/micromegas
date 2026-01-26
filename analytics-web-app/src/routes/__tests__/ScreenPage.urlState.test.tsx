/**
 * Tests for ScreenPage URL state management
 *
 * These tests verify that time range and variable changes in the URL
 * don't interfere with each other - a regression that has occurred multiple times.
 */
import { renderHook, act } from '@testing-library/react'
import { ReactNode, useCallback, useMemo } from 'react'
import { MemoryRouter, useSearchParams, useNavigate } from 'react-router-dom'
import { isReservedParam } from '@/lib/url-params'

// Mock navigate
const mockNavigate = jest.fn()
jest.mock('react-router-dom', () => ({
  ...jest.requireActual('react-router-dom'),
  useNavigate: () => mockNavigate,
}))

/**
 * Hook that mirrors the URL state management pattern used in ScreenPage.
 * This allows us to test the URL manipulation logic in isolation.
 */
function useUrlStateHandlers() {
  const [searchParams] = useSearchParams()
  const navigate = useNavigate()

  // Time range change handler - works directly with URL params to preserve variables
  const handleTimeRangeChange = useCallback(
    (from: string, to: string) => {
      const params = new URLSearchParams(searchParams.toString())
      params.set('from', from)
      params.set('to', to)
      navigate(`?${params.toString()}`)
    },
    [searchParams, navigate]
  )

  // Variable change handler - works directly with URL params to preserve time state
  const handleUrlVariableChange = useCallback(
    (name: string, value: string) => {
      const params = new URLSearchParams(searchParams.toString())
      params.set(name, value)
      navigate(`?${params.toString()}`, { replace: true })
    },
    [searchParams, navigate]
  )

  // Variable remove handler
  const handleUrlVariableRemove = useCallback(
    (name: string) => {
      const params = new URLSearchParams(searchParams.toString())
      params.delete(name)
      const qs = params.toString()
      navigate(qs ? `?${qs}` : '.', { replace: true })
    },
    [searchParams, navigate]
  )

  // Compute URL variables from searchParams
  const urlVariables = useMemo(() => {
    const vars: Record<string, string> = {}
    searchParams.forEach((value, key) => {
      if (!isReservedParam(key)) {
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

      // Change a variable
      act(() => {
        result.current.handleUrlVariableChange('myvar', 'newvalue')
      })

      // Verify navigate was called with time range preserved
      const url = mockNavigate.mock.calls[0][0] as string
      expect(url).toContain('from=now-1h')
      expect(url).toContain('to=now')
      expect(url).toContain('myvar=newvalue')
    })

    it('should preserve other variables when changing one variable', () => {
      const { result } = renderHook(() => useUrlStateHandlers(), {
        wrapper: createWrapper(['/screen/test?var1=a&var2=b&var3=c']),
      })

      // Change one variable
      act(() => {
        result.current.handleUrlVariableChange('var2', 'updated')
      })

      const url = mockNavigate.mock.calls[0][0] as string
      expect(url).toContain('var1=a')
      expect(url).toContain('var2=updated')
      expect(url).toContain('var3=c')
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

      const url = mockNavigate.mock.calls[0][0] as string
      expect(url).toContain('from=now-1h')
      expect(url).toContain('to=now')
      expect(url).toContain('var2=b')
      expect(url).not.toContain('var1')
    })
  })

  describe('combined operations', () => {
    it('should handle interleaved time and variable changes', () => {
      // Start with some state
      const { result, rerender } = renderHook(() => useUrlStateHandlers(), {
        wrapper: createWrapper(['/screen/test?from=now-1h&to=now&myvar=initial']),
      })

      // First: change a variable
      act(() => {
        result.current.handleUrlVariableChange('myvar', 'updated')
      })

      let url = mockNavigate.mock.calls[0][0] as string
      expect(url).toContain('from=now-1h')
      expect(url).toContain('myvar=updated')

      mockNavigate.mockClear()

      // Simulate URL update by creating new wrapper with updated URL
      const { result: result2 } = renderHook(() => useUrlStateHandlers(), {
        wrapper: createWrapper(['/screen/test?from=now-1h&to=now&myvar=updated']),
      })

      // Second: change time range
      act(() => {
        result2.current.handleTimeRangeChange('now-24h', 'now')
      })

      url = mockNavigate.mock.calls[0][0] as string
      expect(url).toContain('from=now-24h')
      expect(url).toContain('to=now')
      expect(url).toContain('myvar=updated') // Variable should still be there!
    })
  })
})
