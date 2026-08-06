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
})
