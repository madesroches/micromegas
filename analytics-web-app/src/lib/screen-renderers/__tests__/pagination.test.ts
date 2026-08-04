/**
 * Tests for usePagination's page-clamp behavior.
 *
 * These are rerender-based: react-hooks/set-state-in-effect's fix moved the
 * "clamp page when totalRows/pageSize changes" logic from a useEffect into a
 * render-time comparison against tracked previous values, so a test that
 * never calls `rerender` can't exercise the clamp at all.
 */
import { renderHook, act } from '@testing-library/react'
import { usePagination } from '../pagination'

describe('usePagination', () => {
  it('does not clamp on mount when the current page is already in range', () => {
    const onPageSizeChange = vi.fn()
    const { result } = renderHook(() => usePagination(1000, 100, onPageSizeChange))
    expect(result.current.currentPage).toBe(0)
    expect(result.current.totalPages).toBe(10)
  })

  it('clamps the current page down when totalRows shrinks below it', () => {
    const onPageSizeChange = vi.fn()
    const { result, rerender } = renderHook(
      ({ totalRows, pageSize }) => usePagination(totalRows, pageSize, onPageSizeChange),
      { initialProps: { totalRows: 1000, pageSize: 100 } }
    )

    act(() => {
      result.current.setPage(9)
    })
    expect(result.current.currentPage).toBe(9)

    // Shrink totalRows so the last valid page is now 2 (rows 0-299 -> 3 pages)
    rerender({ totalRows: 250, pageSize: 100 })
    expect(result.current.currentPage).toBe(2)
  })

  it('clamps the current page down when pageSize grows past it', () => {
    const onPageSizeChange = vi.fn()
    const { result, rerender } = renderHook(
      ({ totalRows, pageSize }) => usePagination(totalRows, pageSize, onPageSizeChange),
      { initialProps: { totalRows: 1000, pageSize: 100 } }
    )

    act(() => {
      result.current.setPage(9)
    })
    expect(result.current.currentPage).toBe(9)

    // Growing pageSize to 500 leaves only 2 pages (0 and 1)
    rerender({ totalRows: 1000, pageSize: 500 })
    expect(result.current.currentPage).toBe(1)
  })

  it('leaves the current page alone when it is still in range after a change', () => {
    const onPageSizeChange = vi.fn()
    const { result, rerender } = renderHook(
      ({ totalRows, pageSize }) => usePagination(totalRows, pageSize, onPageSizeChange),
      { initialProps: { totalRows: 1000, pageSize: 100 } }
    )

    act(() => {
      result.current.setPage(3)
    })
    expect(result.current.currentPage).toBe(3)

    // totalRows changes but page 3 is still valid (5 pages of 200 rows each)
    rerender({ totalRows: 900, pageSize: 100 })
    expect(result.current.currentPage).toBe(3)
  })
})
