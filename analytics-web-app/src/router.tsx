import { Suspense, lazy } from 'react'
import { Routes, Route, Navigate } from 'react-router-dom'

// Lazy load route components for code splitting
const LoginPage = lazy(() => import('@/routes/LoginPage'))
const ProcessesPage = lazy(() => import('@/routes/ProcessesPage'))
const ProcessPage = lazy(() => import('@/routes/ProcessPage'))
const ProcessLogPage = lazy(() => import('@/routes/ProcessLogPage'))
const ProcessMetricsPage = lazy(() => import('@/routes/ProcessMetricsPage'))
const PerformanceAnalysisPage = lazy(() => import('@/routes/PerformanceAnalysisPage'))
const NotFoundPage = lazy(() => import('@/routes/NotFoundPage'))

function PageLoader() {
  return (
    <div className="min-h-screen bg-app-bg flex items-center justify-center">
      <div className="flex items-center gap-3">
        <div className="animate-spin rounded-full h-8 w-8 border-2 border-accent-link border-t-transparent" />
        <span className="text-theme-text-secondary">Loading...</span>
      </div>
    </div>
  )
}

export function AppRouter() {
  return (
    <Suspense fallback={<PageLoader />}>
      <Routes>
        <Route path="/" element={<Navigate to="/processes" replace />} />
        <Route path="/login" element={<LoginPage />} />
        <Route path="/processes" element={<ProcessesPage />} />
        <Route path="/process" element={<ProcessPage />} />
        <Route path="/process_log" element={<ProcessLogPage />} />
        <Route path="/process_metrics" element={<ProcessMetricsPage />} />
        <Route path="/performance_analysis" element={<PerformanceAnalysisPage />} />
        <Route path="*" element={<NotFoundPage />} />
      </Routes>
    </Suspense>
  )
}
