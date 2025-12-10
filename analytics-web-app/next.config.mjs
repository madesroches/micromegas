import path from 'path'
import { fileURLToPath } from 'url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const isProduction = process.env.NODE_ENV === 'production'

/** @type {import('next').NextConfig} */
const nextConfig = {
  // Explicitly set the workspace root to the analytics-web-app directory
  // This prevents the warning about multiple lockfiles
  outputFileTracingRoot: __dirname,

  // Static export for production (served by analytics-web-srv)
  // In development, frontend calls backend directly at http://localhost:8000 (via config.ts)
  ...(isProduction && { output: 'export' }),
}

export default nextConfig
