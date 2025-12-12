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
  // In dev, the backend runs on port 8000, frontend on port 3000
  // Include the base path if set via NEXT_PUBLIC_BASE_PATH
  const devBasePath = process.env.NEXT_PUBLIC_BASE_PATH || ''
  cachedConfig = {
    basePath: process.env.NODE_ENV === 'development' ? `http://localhost:8000${devBasePath}` : '',
  }
  return cachedConfig
}

/**
 * Get the base path for internal navigation links.
 * In development, this returns just the path portion (no host).
 * In production, this returns the runtime base path.
 */
export function getLinkBasePath(): string {
  if (typeof window !== 'undefined' && window.__MICROMEGAS_CONFIG__) {
    return window.__MICROMEGAS_CONFIG__.basePath
  }
  // Development fallback - just use the path, not the full URL
  return process.env.NEXT_PUBLIC_BASE_PATH || ''
}

/**
 * Prepend the runtime base path to an internal link.
 * Use this for all <Link href="..."> and router.push() calls.
 */
export function appLink(path: string): string {
  const base = getLinkBasePath()
  // Ensure path starts with /
  const normalizedPath = path.startsWith('/') ? path : `/${path}`
  return `${base}${normalizedPath}`
}
