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
  // In development, use `yarn dev` which runs Next.js dev server with rewrites
  ...(isProduction && { output: 'export' }),

  // Rewrites only work in development mode (not with static export)
  ...(!isProduction && {
    async rewrites() {
      return [
        {
          source: '/analyticsweb/:path*',
          destination: 'http://127.0.0.1:8000/analyticsweb/:path*',
        },
        {
          source: '/auth/:path*',
          destination: 'http://127.0.0.1:8000/auth/:path*',
        },
      ]
    },
  }),
}

export default nextConfig
