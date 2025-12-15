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
    basePath: import.meta.env.DEV ? (import.meta.env.VITE_BASE_PATH || '') : '',
  }
  return cachedConfig
}

/**
 * Get the base path for internal navigation links.
 * In development, this returns empty string (Vite handles routing).
 * In production, this returns the runtime base path from config injection.
 */
export function getLinkBasePath(): string {
  if (typeof window !== 'undefined' && window.__MICROMEGAS_CONFIG__) {
    return window.__MICROMEGAS_CONFIG__.basePath
  }
  // Development fallback - empty for Vite dev server
  return ''
}

/**
 * Prepend the runtime base path to an internal link.
 * Use this for all <Link href="..."> and navigate() calls.
 */
export function appLink(path: string): string {
  const base = getLinkBasePath()
  // Ensure path starts with /
  const normalizedPath = path.startsWith('/') ? path : `/${path}`
  return `${base}${normalizedPath}`
}
