/**
 * Hook for managing screen config state with URL synchronization.
 *
 * This hook implements the MVC pattern for built-in pages:
 * - Config is the source of truth (Model)
 * - Components receive config as props (View)
 * - Page decides navigation strategy (Controller)
 *
 * Key behaviors:
 * - Initializes config from URL on mount
 * - Handles browser back/forward via popstate
 * - Combined state + URL update prevents drift
 * - Supports push (navigational) and replace (editing) modes
 */

import { useState, useCallback, useEffect, useRef } from 'react'
import { useNavigate, useLocation } from 'react-router-dom'
import { parseUrlParams } from '@/lib/url-params'
import type { BaseScreenConfig } from '@/lib/screen-config'

/**
 * Options for updateConfig.
 */
export interface UpdateOptions {
  /** Use replaceState instead of pushState (default: false) */
  replace?: boolean
}

/**
 * Return type of useScreenConfig hook.
 */
export interface UseScreenConfigResult<T extends BaseScreenConfig> {
  /** Current config state */
  config: T
  /** Update config and sync to URL atomically */
  updateConfig: (partial: Partial<T>, options?: UpdateOptions) => void
}

/**
 * Hook for managing screen config with URL synchronization.
 *
 * @param defaults - Default config values (captured on mount)
 * @param buildUrl - Function to build URL from config (must be stable)
 * @returns Config state and update function
 *
 * @example
 * ```tsx
 * const DEFAULT_CONFIG: ProcessesConfig = {
 *   timeRangeFrom: 'now-1h',
 *   timeRangeTo: 'now',
 * }
 *
 * const buildUrl = (cfg: ProcessesConfig) => {
 *   const params = new URLSearchParams()
 *   if (cfg.search) params.set('search', cfg.search)
 *   return `?${params.toString()}`
 * }
 *
 * function ProcessesContent() {
 *   const { config, updateConfig } = useScreenConfig(DEFAULT_CONFIG, buildUrl)
 *
 *   // Time range creates history entry
 *   const handleTimeRangeChange = (from: string, to: string) => {
 *     updateConfig({ timeRangeFrom: from, timeRangeTo: to })
 *   }
 *
 *   // Search replaces current entry
 *   const handleSearchChange = (search: string) => {
 *     updateConfig({ search }, { replace: true })
 *   }
 * }
 * ```
 */
export function useScreenConfig<T extends BaseScreenConfig>(
  defaults: T,
  buildUrl: (config: T) => string
): UseScreenConfigResult<T> {
  const location = useLocation()
  const navigate = useNavigate()

  // Capture defaults and initial search on first render
  const defaultsRef = useRef(defaults)
  const initialSearchRef = useRef(location.search)

  // Initialize from URL on mount
  const [config, setConfig] = useState<T>(() => {
    const fromUrl = parseUrlParams(new URLSearchParams(initialSearchRef.current))
    return { ...defaultsRef.current, ...fromUrl } as T
  })

  // Handle browser back/forward - restore config from defaults + URL
  // Note: popstate only fires for browser navigation (back/forward), not for
  // pushState/replaceState, so this won't conflict with our navigate() calls.
  // We use window.location here because it's guaranteed to be updated when
  // popstate fires, whereas React Router's location updates asynchronously.
  useEffect(() => {
    const handlePopstate = () => {
      const fromUrl = parseUrlParams(new URLSearchParams(window.location.search))
      setConfig({ ...defaultsRef.current, ...fromUrl } as T)
    }
    window.addEventListener('popstate', handlePopstate)
    return () => window.removeEventListener('popstate', handlePopstate)
  }, [])

  // Combined update: updates state AND syncs URL atomically
  // This prevents state/URL drift that can occur with separate calls
  const updateConfig = useCallback(
    (partial: Partial<T>, options?: UpdateOptions) => {
      setConfig((prev) => {
        const newConfig = { ...prev, ...partial }
        // Schedule navigation as microtask to avoid "Cannot update BrowserRouter
        // while rendering" warning. The microtask runs immediately after the
        // current synchronous code, keeping state and URL effectively atomic.
        queueMicrotask(() => {
          navigate(buildUrl(newConfig), { replace: options?.replace })
        })
        return newConfig
      })
    },
    [navigate, buildUrl]
  )

  return { config, updateConfig }
}
