/**
 * Tests for useScreenConfig hook
 */
import { renderHook, act } from '@testing-library/react'
import { ReactNode } from 'react'
import { MemoryRouter } from 'react-router-dom'

// Mock react-router-dom's useNavigate
const mockNavigate = jest.fn()
jest.mock('react-router-dom', () => ({
  ...jest.requireActual('react-router-dom'),
  useNavigate: () => mockNavigate,
}))

import { useScreenConfig } from '../useScreenConfig'
import type { BaseScreenConfig } from '@/lib/screen-config'

interface TestConfig extends BaseScreenConfig {
  processId?: string
  search?: string
  count?: number
}

const DEFAULT_CONFIG: TestConfig = {
  timeRangeFrom: 'now-1h',
  timeRangeTo: 'now',
  processId: '',
  search: '',
}

const buildUrl = (cfg: TestConfig): string => {
  const params = new URLSearchParams()
  if (cfg.processId) params.set('process_id', cfg.processId)
  if (cfg.timeRangeFrom && cfg.timeRangeFrom !== 'now-1h') params.set('from', cfg.timeRangeFrom)
  if (cfg.timeRangeTo && cfg.timeRangeTo !== 'now') params.set('to', cfg.timeRangeTo)
  if (cfg.search) params.set('search', cfg.search)
  if (cfg.count !== undefined) params.set('count', String(cfg.count))
  const qs = params.toString()
  return qs ? `?${qs}` : ''
}

// Helper to create wrapper with initial URL
function createWrapper(initialEntries: string[] = ['/']) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <MemoryRouter
        initialEntries={initialEntries}
        future={{ v7_startTransition: true, v7_relativeSplatPath: true }}
      >
        {children}
      </MemoryRouter>
    )
  }
}

describe('useScreenConfig', () => {
  beforeEach(() => {
    mockNavigate.mockClear()
  })

  describe('initialization', () => {
    it('should initialize with defaults when URL is empty', () => {
      const { result } = renderHook(() => useScreenConfig(DEFAULT_CONFIG, buildUrl), {
        wrapper: createWrapper(['/test']),
      })

      expect(result.current.config).toEqual(DEFAULT_CONFIG)
    })

    it('should initialize from URL params', () => {
      const { result } = renderHook(() => useScreenConfig(DEFAULT_CONFIG, buildUrl), {
        wrapper: createWrapper(['/test?process_id=abc-123&from=now-24h&search=hello']),
      })

      expect(result.current.config.processId).toBe('abc-123')
      expect(result.current.config.timeRangeFrom).toBe('now-24h')
      expect(result.current.config.search).toBe('hello')
      // Defaults still apply for missing params
      expect(result.current.config.timeRangeTo).toBe('now')
    })

    it('should merge URL params over defaults', () => {
      const customDefaults: TestConfig = {
        timeRangeFrom: 'now-7d',
        timeRangeTo: 'now',
        processId: 'default-process',
        search: 'default-search',
      }

      const { result } = renderHook(() => useScreenConfig(customDefaults, buildUrl), {
        wrapper: createWrapper(['/test?search=override']),
      })

      expect(result.current.config.search).toBe('override')
      expect(result.current.config.processId).toBe('default-process')
      expect(result.current.config.timeRangeFrom).toBe('now-7d')
    })
  })

  describe('updateConfig', () => {
    it('should update config state', async () => {
      const { result } = renderHook(() => useScreenConfig(DEFAULT_CONFIG, buildUrl), {
        wrapper: createWrapper(['/test']),
      })

      await act(async () => {
        result.current.updateConfig({ search: 'new-search' })
        // Wait for microtask (queueMicrotask in implementation)
        await new Promise((resolve) => setTimeout(resolve, 0))
      })

      expect(result.current.config.search).toBe('new-search')
    })

    it('should call navigate with built URL', async () => {
      const { result } = renderHook(() => useScreenConfig(DEFAULT_CONFIG, buildUrl), {
        wrapper: createWrapper(['/test']),
      })

      await act(async () => {
        result.current.updateConfig({ search: 'test-query' })
        await new Promise((resolve) => setTimeout(resolve, 0))
      })

      expect(mockNavigate).toHaveBeenCalledWith('?search=test-query', { replace: undefined })
    })

    it('should use replace mode when specified', async () => {
      const { result } = renderHook(() => useScreenConfig(DEFAULT_CONFIG, buildUrl), {
        wrapper: createWrapper(['/test']),
      })

      await act(async () => {
        result.current.updateConfig({ search: 'replaced' }, { replace: true })
        await new Promise((resolve) => setTimeout(resolve, 0))
      })

      expect(mockNavigate).toHaveBeenCalledWith('?search=replaced', { replace: true })
    })

    it('should merge partial updates with existing config', async () => {
      const { result } = renderHook(() => useScreenConfig(DEFAULT_CONFIG, buildUrl), {
        wrapper: createWrapper(['/test?process_id=existing']),
      })

      expect(result.current.config.processId).toBe('existing')

      await act(async () => {
        result.current.updateConfig({ search: 'added' })
        await new Promise((resolve) => setTimeout(resolve, 0))
      })

      expect(result.current.config.processId).toBe('existing')
      expect(result.current.config.search).toBe('added')
    })

    it('should handle multiple rapid updates', async () => {
      const { result } = renderHook(() => useScreenConfig(DEFAULT_CONFIG, buildUrl), {
        wrapper: createWrapper(['/test']),
      })

      await act(async () => {
        result.current.updateConfig({ search: 'first' })
        result.current.updateConfig({ search: 'second' })
        result.current.updateConfig({ search: 'third' })
        await new Promise((resolve) => setTimeout(resolve, 0))
      })

      expect(result.current.config.search).toBe('third')
    })
  })

  describe('popstate handling', () => {
    it('should restore config on popstate event', async () => {
      const { result } = renderHook(() => useScreenConfig(DEFAULT_CONFIG, buildUrl), {
        wrapper: createWrapper(['/test?search=initial']),
      })

      expect(result.current.config.search).toBe('initial')

      // Simulate browser back/forward by changing window.location and firing popstate
      await act(async () => {
        // Mock window.location.search for popstate handler
        Object.defineProperty(window, 'location', {
          value: { search: '?search=from-history' },
          writable: true,
        })
        window.dispatchEvent(new PopStateEvent('popstate'))
      })

      expect(result.current.config.search).toBe('from-history')
    })
  })

  describe('buildUrl integration', () => {
    it('should omit default values from URL', async () => {
      const { result } = renderHook(() => useScreenConfig(DEFAULT_CONFIG, buildUrl), {
        wrapper: createWrapper(['/test']),
      })

      await act(async () => {
        // Update with default time range values (should not appear in URL)
        result.current.updateConfig({ timeRangeFrom: 'now-1h', timeRangeTo: 'now' })
        await new Promise((resolve) => setTimeout(resolve, 0))
      })

      // buildUrl should produce empty string for all defaults
      expect(mockNavigate).toHaveBeenCalledWith('', { replace: undefined })
    })

    it('should include non-default values in URL', async () => {
      const { result } = renderHook(() => useScreenConfig(DEFAULT_CONFIG, buildUrl), {
        wrapper: createWrapper(['/test']),
      })

      await act(async () => {
        result.current.updateConfig({
          processId: 'proc-1',
          timeRangeFrom: 'now-24h',
          search: 'query',
        })
        await new Promise((resolve) => setTimeout(resolve, 0))
      })

      expect(mockNavigate).toHaveBeenCalledWith(
        '?process_id=proc-1&from=now-24h&search=query',
        { replace: undefined }
      )
    })
  })
})
