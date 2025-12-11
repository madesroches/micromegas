import path from 'path'
import { fileURLToPath } from 'url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const isProduction = process.env.NODE_ENV === 'production'

// Base path for sub-path deployment (e.g., /micromegas)
// Must be set at build time for static assets to have correct URLs
const basePath = process.env.NEXT_PUBLIC_BASE_PATH || ''

/** @type {import('next').NextConfig} */
const nextConfig = {
  // Explicitly set the workspace root to the analytics-web-app directory
  // This prevents the warning about multiple lockfiles
  outputFileTracingRoot: __dirname,

  // Base path for deployment at a sub-path (e.g., /micromegas)
  // When set, all static assets will be prefixed with this path
  ...(basePath && { basePath }),

  // Static export for production (served by analytics-web-srv)
  // In development, frontend calls backend directly at http://localhost:8000 (via config.ts)
  ...(isProduction && { output: 'export' }),
}

export default nextConfig
