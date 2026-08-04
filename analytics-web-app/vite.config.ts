/// <reference types="vitest/config" />
import { defineConfig, loadEnv } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
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
      tailwindcss(),
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
        'micromegas-datafusion-wasm': path.resolve(__dirname, './src/lib/datafusion-wasm'),
      },
    },
    optimizeDeps: {
      exclude: ['micromegas-datafusion-wasm'],
    },
    build: {
      outDir: 'dist',
      sourcemap: mode === 'development',
      // ScreenPage pulls in three.js + perfetto and is a lazy-loaded route chunk;
      // the default 500 kB warning isn't actionable here.
      chunkSizeWarningLimit: 2000,
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
    test: {
      globals: true,
      environment: 'jsdom',
      environmentOptions: {
        jsdom: { url: 'http://localhost:3000' },
      },
      setupFiles: ['./src/test-setup.ts'],
      exclude: ['**/node_modules/**', '**/.git/**', '**/dist/**'],
      alias: {
        'react-markdown': path.resolve(__dirname, './src/__mocks__/react-markdown.tsx'),
        'remark-gfm': path.resolve(__dirname, './src/__mocks__/remark-gfm.ts'),
        '@radix-ui/react-dropdown-menu': path.resolve(
          __dirname,
          './src/__mocks__/@radix-ui/react-dropdown-menu.tsx'
        ),
        'micromegas-datafusion-wasm': path.resolve(
          __dirname,
          './src/__mocks__/micromegas-datafusion-wasm.ts'
        ),
      },
      coverage: {
        provider: 'v8',
        include: ['src/**/*.{ts,tsx}'],
        exclude: [
          'src/**/*.d.ts',
          'src/**/__mocks__/**',
          'src/**/__tests__/**',
          'src/**/__test-utils__/**',
          'src/lib/datafusion-wasm/**',
          'src/main.tsx',
          'src/router.tsx',
          'src/components/layout/TimeRangePicker/types.ts',
        ],
      },
    },
  }
})
