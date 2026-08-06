import { renderHook } from '@testing-library/react'
import { usePageTitle } from '../usePageTitle'

describe('usePageTitle', () => {
  it('falls back to the app name when title is null/undefined', () => {
    renderHook(({ title }) => usePageTitle(title), { initialProps: { title: null } })
    expect(document.title).toBe('Micromegas')
  })

  it('sets "{title} - Micromegas" when a title is given', () => {
    renderHook(({ title }) => usePageTitle(title), { initialProps: { title: 'My Screen' } })
    expect(document.title).toBe('My Screen - Micromegas')
  })

  it('prefixes the title with "[*] " when busy is true', () => {
    renderHook(({ title, busy }) => usePageTitle(title, busy), {
      initialProps: { title: 'My Screen', busy: true },
    })
    expect(document.title).toBe('[*] My Screen - Micromegas')
  })

  it('prefixes the fallback app name with "[*] " when busy and title is null', () => {
    renderHook(({ title, busy }) => usePageTitle(title, busy), {
      initialProps: { title: null as string | null, busy: true },
    })
    expect(document.title).toBe('[*] Micromegas')
  })

  it('removes the busy prefix again once busy goes back to false', () => {
    const { rerender } = renderHook(({ title, busy }) => usePageTitle(title, busy), {
      initialProps: { title: 'My Screen', busy: true },
    })
    expect(document.title).toBe('[*] My Screen - Micromegas')

    rerender({ title: 'My Screen', busy: false })
    expect(document.title).toBe('My Screen - Micromegas')
  })
})
