import React from 'react'
import ReactDOM from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'
import { QueryProvider } from '@/components/QueryProvider'
import { AuthProvider } from '@/lib/auth'
import { Toaster } from '@/components/ui/toaster'
import { AppRouter } from '@/router'
import { getConfig } from '@/lib/config'
import './styles/globals.css'

// Filter benign Firefox dev-console warnings emitted by upstream R3F / three.js
// code we don't control:
//   - `THREE.Clock: This module has been deprecated`: R3F 9.x still constructs
//     `new THREE.Clock()` internally; three@0.183 warns from the Clock ctor.
//     R3F 10 canary already moved off Clock.
//   - `MouseEvent.mozPressure` / `MouseEvent.mozInputSource`: R3F's event
//     extractor does `for (prop in event)` over the native MouseEvent, which
//     accesses Firefox's deprecated non-standard properties.
const originalWarn = console.warn
console.warn = (...args: unknown[]) => {
  if (typeof args[0] === 'string') {
    const msg = args[0]
    if (msg.includes('Clock: This module has been deprecated')) return
    if (msg.includes('MouseEvent.mozPressure') || msg.includes('MouseEvent.mozInputSource')) return
  }
  originalWarn(...args)
}

// AbortController-driven cancellations are intentional. When fetch's body
// stream is cancelled by abort(), the underlying source's cleanup can produce
// an internal rejection that browsers surface even though our consumer-side
// awaits catch their own AbortError. Treat AbortError unhandled rejections as
// benign so they don't pollute the dev console.
window.addEventListener('unhandledrejection', (event) => {
  const reason = event.reason
  if (reason instanceof DOMException && reason.name === 'AbortError') {
    event.preventDefault()
  }
})

// Get base path for router - must match the proxy/deployment path
const basePath = getConfig().basePath

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <BrowserRouter
      basename={basePath}
      future={{
        v7_startTransition: true,
        v7_relativeSplatPath: true,
      }}
    >
      <AuthProvider>
        <QueryProvider>
          <AppRouter />
          <Toaster />
        </QueryProvider>
      </AuthProvider>
    </BrowserRouter>
  </React.StrictMode>
)
