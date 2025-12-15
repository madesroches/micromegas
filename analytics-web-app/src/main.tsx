import React from 'react'
import ReactDOM from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'
import { QueryProvider } from '@/components/QueryProvider'
import { AuthProvider } from '@/lib/auth'
import { Toaster } from '@/components/ui/toaster'
import { AppRouter } from '@/router'
import { getConfig } from '@/lib/config'
import './styles/globals.css'

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
