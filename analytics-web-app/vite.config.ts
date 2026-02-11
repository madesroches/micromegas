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
    plugins: [
      react(),
      {
        name: 'wasm-content-type',
        configureServer(server) {
          server.middlewares.use((req, res, next) => {
            if (req.url?.endsWith('.wasm')) {
              res.setHeader('Content-Type', 'application/wasm')
            }
            next()
          })
        },
      },
      {
        name: 'log-base-path',
        configureServer(server) {
          server.httpServer?.once('listening', () => {
            if (basePath) {
              console.log(`\n  ➜  App URL:  \x1b[36mhttp://localhost:${frontendPort}${basePath}/\x1b[0m\n`)
            }
          })
        },
      },
    ],
    appType: 'spa',
    base: './',
    // Expose base path to frontend via import.meta.env
    define: {
      'import.meta.env.VITE_BASE_PATH': JSON.stringify(basePath),
    },
    resolve: {
      alias: {
        '@': path.resolve(__dirname, './src'),
        'datafusion-wasm': path.resolve(__dirname, './src/lib/datafusion-wasm'),
      },
    },
    optimizeDeps: {
      exclude: ['datafusion-wasm'],
    },
    build: {
      outDir: 'dist',
      sourcemap: mode === 'development',
    },
    server: {
      port: frontendPort,
      proxy: {
        // API endpoints under /api
        [`${basePath}/api`]: {
          target: backendUrl,
        },
        // Auth endpoints stay at /auth (not /api/auth) for OAuth callback compatibility
        [`${basePath}/auth`]: {
          target: backendUrl,
        },
      },
    },
  }
})
