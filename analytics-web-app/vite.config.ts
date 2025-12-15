import { defineConfig, loadEnv } from 'vite'
import react from '@vitejs/plugin-react'
import path from 'path'

export default defineConfig(({ mode }) => {
  // Load env variables - use start_analytics_web.py or set manually
  const env = loadEnv(mode, process.cwd(), '')
  const basePath = env.MICROMEGAS_BASE_PATH
  const backendUrl = env.MICROMEGAS_BACKEND_URL || `http://localhost:${env.MICROMEGAS_BACKEND_PORT || '8000'}`
  const frontendPort = parseInt(env.MICROMEGAS_FRONTEND_PORT || '3000', 10)

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
      port: frontendPort,
      proxy: {
        // Proxy API endpoints to backend without rewriting
        // This ensures browser URL path matches cookie path for auth to work
        // Only proxy specific API paths, not frontend routes
        [`${basePath}/api`]: {
          target: backendUrl,
        },
        [`${basePath}/auth`]: {
          target: backendUrl,
        },
        [`${basePath}/query`]: {
          target: backendUrl,
        },
        [`${basePath}/perfetto`]: {
          target: backendUrl,
        },
        [`${basePath}/health`]: {
          target: backendUrl,
        },
      },
    },
  }
})
