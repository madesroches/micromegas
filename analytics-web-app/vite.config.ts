import { defineConfig, loadEnv } from 'vite'
import react from '@vitejs/plugin-react'
import path from 'path'

export default defineConfig(({ mode }) => {
  // Load env variables (MICROMEGAS_BASE_PATH, etc.)
  // Base path must be set via environment - use start_analytics_web.py or set manually
  const env = loadEnv(mode, process.cwd(), '')
  const basePath = env.MICROMEGAS_BASE_PATH

  return {
    plugins: [react()],
    appType: 'spa',
    base: './',
    // Expose base path to frontend via import.meta.env
    define: {
      'import.meta.env.VITE_BASE_PATH': JSON.stringify(basePath),
    },
    resolve: {
      alias: {
        '@': path.resolve(__dirname, './src'),
      },
    },
    build: {
      outDir: 'dist',
      sourcemap: true,
    },
    server: {
      port: 3000,
      proxy: {
        // Proxy API endpoints to backend without rewriting
        // This ensures browser URL path matches cookie path for auth to work
        // Only proxy specific API paths, not frontend routes
        [`${basePath}/api`]: {
          target: 'http://localhost:8000',
        },
        [`${basePath}/auth`]: {
          target: 'http://localhost:8000',
        },
        [`${basePath}/query`]: {
          target: 'http://localhost:8000',
        },
        [`${basePath}/perfetto`]: {
          target: 'http://localhost:8000',
        },
        [`${basePath}/health`]: {
          target: 'http://localhost:8000',
        },
      },
    },
  }
})
