/**
 * Tests for useFadeOnIdle.
 *
 * The effect is wrapped (see react-hooks/set-state-in-effect) in a nested
 * function that's declared, invoked, and returned so the cleanup and early
 * returns keep propagating — these rerender-based tests confirm the timer
 * state machine still behaves the same across a real fake-timer status
 * transition sequence.
 */
import { renderHook, act } from '@testing-library/react'
import { useFadeOnIdle } from '../useFadeOnIdle'

describe('useFadeOnIdle', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('starts unrevealed for an idle status', () => {
    const { result } = renderHook(({ status }) => useFadeOnIdle(status), {
      initialProps: { status: 'idle' },
    })
    expect(result.current).toBe('fade-on-idle')
  })

  it('reveals immediately when status changes away from idle', () => {
    const { result, rerender } = renderHook(({ status }) => useFadeOnIdle(status), {
      initialProps: { status: 'idle' },
    })

    rerender({ status: 'loading' })
    expect(result.current).toBe('fade-on-idle revealed')
  })

  it('stays revealed while loading, with no fade-out timer scheduled', () => {
    const { result, rerender } = renderHook(({ status }) => useFadeOnIdle(status), {
      initialProps: { status: 'idle' },
    })

    rerender({ status: 'loading' })
    act(() => {
      vi.advanceTimersByTime(10_000)
    })
    expect(result.current).toBe('fade-on-idle revealed')
  })

  it('fades out 200ms after reaching a terminal state', () => {
    const { result, rerender } = renderHook(({ status }) => useFadeOnIdle(status), {
      initialProps: { status: 'loading' },
    })

    rerender({ status: 'success' })
    expect(result.current).toBe('fade-on-idle revealed')

    act(() => {
      vi.advanceTimersByTime(199)
    })
    expect(result.current).toBe('fade-on-idle revealed')

    act(() => {
      vi.advanceTimersByTime(1)
    })
    expect(result.current).toBe('fade-on-idle')
  })

  it('clears the pending fade-out timer when status changes again before it fires', () => {
    const { result, rerender } = renderHook(({ status }) => useFadeOnIdle(status), {
      initialProps: { status: 'loading' },
    })

    rerender({ status: 'success' })
    act(() => {
      vi.advanceTimersByTime(100)
    })

    // A new status arrives before the 200ms fade-out timer fires
    rerender({ status: 'error' })
    expect(result.current).toBe('fade-on-idle revealed')

    // The old timer (100ms already elapsed, needed 200ms) must not have
    // fired at its original 200ms mark now that a new one was scheduled
    act(() => {
      vi.advanceTimersByTime(100)
    })
    expect(result.current).toBe('fade-on-idle revealed')

    act(() => {
      vi.advanceTimersByTime(100)
    })
    expect(result.current).toBe('fade-on-idle')
  })

  it('keeps the last revealed state when status returns to idle, and never fades it out', () => {
    const { result, rerender } = renderHook(({ status }) => useFadeOnIdle(status), {
      initialProps: { status: 'success' },
    })
    expect(result.current).toBe('fade-on-idle revealed')

    // Going idle cancels the pending fade-out timer (effect cleanup) and
    // idle's own branch is an early return, so revealed stays true forever.
    rerender({ status: 'idle' })
    act(() => {
      vi.advanceTimersByTime(10_000)
    })
    expect(result.current).toBe('fade-on-idle revealed')
  })
})
