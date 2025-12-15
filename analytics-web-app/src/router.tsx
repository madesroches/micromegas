import { Routes, Route, Navigate } from 'react-router-dom'
import LoginPage from '@/routes/LoginPage'
import ProcessesPage from '@/routes/ProcessesPage'
import ProcessPage from '@/routes/ProcessPage'
import ProcessLogPage from '@/routes/ProcessLogPage'
import ProcessMetricsPage from '@/routes/ProcessMetricsPage'
import ProcessTracePage from '@/routes/ProcessTracePage'
import PerformanceAnalysisPage from '@/routes/PerformanceAnalysisPage'

export function AppRouter() {
  return (
    <Routes>
      <Route path="/" element={<Navigate to="/processes" replace />} />
      <Route path="/login" element={<LoginPage />} />
      <Route path="/processes" element={<ProcessesPage />} />
      <Route path="/process" element={<ProcessPage />} />
      <Route path="/process_log" element={<ProcessLogPage />} />
      <Route path="/process_metrics" element={<ProcessMetricsPage />} />
      <Route path="/process_trace" element={<ProcessTracePage />} />
      <Route path="/performance_analysis" element={<PerformanceAnalysisPage />} />
    </Routes>
  )
}
