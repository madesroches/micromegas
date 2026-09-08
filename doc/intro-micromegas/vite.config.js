import { defineConfig } from 'vite'

export default defineConfig({
  root: './',
  base: './', // Use relative paths for assets
  server: {
    port: 5173,
    open: true,
    host: true
  },
  build: {
    outDir: 'dist',
    assetsDir: 'assets',
    sourcemap: true,
    // Presentation is built as a single inline HTML file; chunk-splitting doesn't apply.
    chunkSizeWarningLimit: 10000,
    rollupOptions: {
      input: {
        main: './index.html'
      }
    }
  },
  publicDir: 'media',
  resolve: {
    alias: {
      '@': '/src'
    }
  }
})
