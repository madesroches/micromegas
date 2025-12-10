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
  cachedConfig = {
    basePath: process.env.NODE_ENV === 'development' ? 'http://localhost:8000' : '',
  }
  return cachedConfig
}
