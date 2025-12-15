// Runtime configuration for the analytics web app
// The backend injects __MICROMEGAS_CONFIG__ into index.html at serve time,
// allowing the same build to work with different base paths.

interface RuntimeConfig {
  basePath: string
}

declare global {
  interface Window {
    __MICROMEGAS_CONFIG__?: RuntimeConfig
  }
}

// Cache the config after first access
let cachedConfig: RuntimeConfig | null = null

export function getConfig(): RuntimeConfig {
  if (cachedConfig) {
    return cachedConfig
  }

  if (typeof window !== 'undefined' && window.__MICROMEGAS_CONFIG__) {
    cachedConfig = window.__MICROMEGAS_CONFIG__
    return cachedConfig
  }

  // Development fallback (no injection needed in dev mode)
  // In dev mode, Vite injects VITE_BASE_PATH from MICROMEGAS_BASE_PATH env var
  // Must use the base path so browser URL matches cookie path for auth to work
  cachedConfig = {
    basePath: import.meta.env.DEV ? (import.meta.env?.VITE_BASE_PATH || '') : '',
  }
  return cachedConfig
}

/**
 * Normalize an internal link path (ensures leading slash).
 * BrowserRouter basename handles the base path automatically.
 */
export function appLink(path: string): string {
  return path.startsWith('/') ? path : `/${path}`
}
