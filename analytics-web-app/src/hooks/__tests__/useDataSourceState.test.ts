/**
 * Tests for useDataSourceState.
 *
 * The "adopt the default once it resolves" sync moved from a useEffect into
 * a render-time comparison against tracked previous values (see
 * react-hooks/set-state-in-effect), so these rerender-based tests exercise
 * useDefaultDataSource resolving asynchronously across renders.
 */
import { renderHook, act } from '@testing-library/react'
import { useDataSourceState } from '../useDataSourceState'

const { mockUseDefaultDataSource } = vi.hoisted(() => ({
  mockUseDefaultDataSource: vi.fn(),
}))

vi.mock('../useDefaultDataSource', () => ({
  useDefaultDataSource: mockUseDefaultDataSource,
}))

describe('useDataSourceState', () => {
  beforeEach(() => {
    mockUseDefaultDataSource.mockReset()
  })

  it('starts empty while the default has not resolved yet', () => {
    mockUseDefaultDataSource.mockReturnValue({ name: '', error: null })
    const { result } = renderHook(() => useDataSourceState())
    expect(result.current.dataSource).toBe('')
    expect(result.current.error).toBeNull()
  })

  it('adopts the default data source once it resolves', () => {
    mockUseDefaultDataSource.mockReturnValue({ name: '', error: null })
    const { result, rerender } = renderHook(() => useDataSourceState())
    expect(result.current.dataSource).toBe('')

    mockUseDefaultDataSource.mockReturnValue({ name: 'prod', error: null })
    rerender()
    expect(result.current.dataSource).toBe('prod')
  })

  it('does not override a user-selected data source when the default later changes', () => {
    // useDefaultDataSource always starts at '' (it resolves asynchronously
    // via its own effect), so mount here matches its real contract.
    mockUseDefaultDataSource.mockReturnValue({ name: '', error: null })
    const { result, rerender } = renderHook(() => useDataSourceState())

    mockUseDefaultDataSource.mockReturnValue({ name: 'prod', error: null })
    rerender()
    expect(result.current.dataSource).toBe('prod')

    act(() => {
      result.current.setDataSource('custom')
    })
    expect(result.current.dataSource).toBe('custom')

    mockUseDefaultDataSource.mockReturnValue({ name: 'staging', error: null })
    rerender()
    expect(result.current.dataSource).toBe('custom')
  })

  it('surfaces the error from useDefaultDataSource', () => {
    mockUseDefaultDataSource.mockReturnValue({
      name: '',
      error: 'No default data source configured.',
    })
    const { result } = renderHook(() => useDataSourceState())
    expect(result.current.error).toBe('No default data source configured.')
  })
})
