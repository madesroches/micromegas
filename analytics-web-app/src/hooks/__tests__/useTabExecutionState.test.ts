/**
 * Tests for useTabExecutionState.
 *
 * Mounts with a `<link rel="icon">` present in jsdom (mirroring index.html) and
 * asserts the hook swaps its `href` between the idle/busy/error favicon variants,
 * caches the idle href on the element (`dataset.idleHref`) so repeated toggles
 * don't drift, and reverts to idle on unmount.
 */
import { renderHook } from '@testing-library/react'
import { useTabExecutionState } from '../useTabExecutionState'

describe('useTabExecutionState', () => {
  let link: HTMLLinkElement

  beforeEach(() => {
    link = document.createElement('link')
    link.rel = 'icon'
    link.href = './icon.svg'
    document.head.appendChild(link)
  })

  afterEach(() => {
    link.remove()
  })

  it('leaves the favicon untouched for idle', () => {
    const idleHref = link.href
    renderHook(({ state }) => useTabExecutionState(state), {
      initialProps: { state: 'idle' as const },
    })
    expect(link.href).toBe(idleHref)
  })

  it('swaps to the busy favicon', () => {
    const idleHref = link.href
    renderHook(({ state }) => useTabExecutionState(state), {
      initialProps: { state: 'busy' as const },
    })
    expect(link.href).toBe(idleHref.replace(/icon\.svg$/, 'icon-busy.svg'))
  })

  it('swaps to the error favicon', () => {
    const idleHref = link.href
    renderHook(({ state }) => useTabExecutionState(state), {
      initialProps: { state: 'error' as const },
    })
    expect(link.href).toBe(idleHref.replace(/icon\.svg$/, 'icon-error.svg'))
  })

  it('is stable under repeated toggles (idleHref caching does not drift)', () => {
    const idleHref = link.href
    const { rerender } = renderHook(({ state }) => useTabExecutionState(state), {
      initialProps: { state: 'idle' as const },
    })

    rerender({ state: 'busy' })
    expect(link.href).toBe(idleHref.replace(/icon\.svg$/, 'icon-busy.svg'))

    rerender({ state: 'error' })
    expect(link.href).toBe(idleHref.replace(/icon\.svg$/, 'icon-error.svg'))

    rerender({ state: 'idle' })
    expect(link.href).toBe(idleHref)

    rerender({ state: 'busy' })
    expect(link.href).toBe(idleHref.replace(/icon\.svg$/, 'icon-busy.svg'))
  })

  it('reverts to idleHref on unmount while busy', () => {
    const idleHref = link.href
    const { unmount } = renderHook(({ state }) => useTabExecutionState(state), {
      initialProps: { state: 'busy' as const },
    })
    expect(link.href).toBe(idleHref.replace(/icon\.svg$/, 'icon-busy.svg'))

    unmount()
    expect(link.href).toBe(idleHref)
  })

  it('reverts to idleHref on unmount while in error', () => {
    const idleHref = link.href
    const { unmount } = renderHook(({ state }) => useTabExecutionState(state), {
      initialProps: { state: 'error' as const },
    })
    expect(link.href).toBe(idleHref.replace(/icon\.svg$/, 'icon-error.svg'))

    unmount()
    expect(link.href).toBe(idleHref)
  })

  it('does nothing (and does not throw) when no icon link is present', () => {
    link.remove()
    expect(() => {
      renderHook(({ state }) => useTabExecutionState(state), {
        initialProps: { state: 'busy' as const },
      })
    }).not.toThrow()
  })
})
