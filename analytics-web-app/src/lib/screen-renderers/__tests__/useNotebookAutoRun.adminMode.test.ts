import { renderHook, act } from '@testing-library/react'
import { useNotebookAutoRun } from '../useNotebookAutoRun'

describe('useNotebookAutoRun admin mode', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  function setup(adminMode: boolean) {
    const executeFrom = vi.fn().mockResolvedValue(undefined)
    const executeAfter = vi.fn().mockResolvedValue(undefined)
    const hook = renderHook(
      ({ adminMode }) =>
        useNotebookAutoRun({
          executeFromCellByName: executeFrom,
          executeAfterCellByName: executeAfter,
          adminMode,
        }),
      { initialProps: { adminMode } }
    )
    return { ...hook, executeFrom, executeAfter }
  }

  it('runs normally when off', () => {
    const { result, executeFrom, executeAfter } = setup(false)
    act(() => {
      result.current.scheduleAutoRun('a')
      result.current.triggerAutoRun('a', true)
      vi.advanceTimersByTime(300)
    })
    expect(executeFrom).toHaveBeenCalledWith('a')
    expect(executeAfter).toHaveBeenCalledWith('a')
  })

  it('does not execute when on', () => {
    const { result, executeFrom, executeAfter } = setup(true)
    act(() => {
      result.current.scheduleAutoRun('a')
      result.current.triggerAutoRun('a', true)
      vi.advanceTimersByTime(1000)
    })
    expect(executeFrom).not.toHaveBeenCalled()
    expect(executeAfter).not.toHaveBeenCalled()
  })

  it('drops a run scheduled before the mode was turned on', () => {
    const { result, rerender, executeFrom } = setup(false)
    act(() => {
      result.current.scheduleAutoRun('a')
    })
    rerender({ adminMode: true })
    act(() => {
      vi.advanceTimersByTime(1000)
    })
    expect(executeFrom).not.toHaveBeenCalled()
  })
})
