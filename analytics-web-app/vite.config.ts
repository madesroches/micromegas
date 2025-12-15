import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import path from 'path'

export default defineConfig({
  plugins: [react()],
  appType: 'spa',
  base: './',
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
      '/api': {
        target: 'http://localhost:8000',
        rewrite: (path) => `/mmlocal${path}`,
      },
      '/auth': {
        target: 'http://localhost:8000',
        rewrite: (path) => `/mmlocal${path}`,
      },
      '/query': {
        target: 'http://localhost:8000',
        rewrite: (path) => `/mmlocal${path}`,
      },
      '/perfetto': {
        target: 'http://localhost:8000',
        rewrite: (path) => `/mmlocal${path}`,
      },
      '/health': {
        target: 'http://localhost:8000',
        rewrite: (path) => `/mmlocal${path}`,
      },
    },
  },
})
