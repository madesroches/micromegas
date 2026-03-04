import { useState } from 'react'
import { useAuth } from '@/lib/auth'
import { RefreshCw, ChevronDown, LogOut, ZoomIn, ZoomOut } from 'lucide-react'
import { AppLink } from '@/components/AppLink'
import { getConfig } from '@/lib/config'
import { zoomTimeRange } from '@/lib/time-range'
import { TimeRangePicker } from './TimeRangePicker'
import { PivotButton } from './PivotButton'
import { RefreshIntervalPicker } from './RefreshIntervalPicker'
import { MicromegasLogo } from '@/components/MicromegasLogo'
import type { TimeRangeControlProps } from './PageLayout'

interface HeaderProps {
  onRefresh?: () => void
  /** Time range control props - when provided, page controls time range */
  timeRangeControl?: TimeRangeControlProps
  /** Process ID for pivot button navigation */
  processId?: string
  /** Current auto-refresh interval in ms (0 = off) */
  refreshIntervalMs?: number
  /** Callback to change the auto-refresh interval */
  onRefreshIntervalChange?: (ms: number) => void
  /** Whether the screen is currently executing */
  isExecuting?: boolean
}

export function Header({ onRefresh, timeRangeControl, processId, refreshIntervalMs, onRefreshIntervalChange, isExecuting }: HeaderProps) {
  const { user, logout, status } = useAuth()
  const [isUserMenuOpen, setIsUserMenuOpen] = useState(false)
  const [isLoggingOut, setIsLoggingOut] = useState(false)

  const handleLogout = async () => {
    setIsLoggingOut(true)
    try {
      await logout()
      // Use full base path for raw browser navigation (not React Router)
      window.location.href = `${getConfig().basePath}/login`
    } catch (error) {
      console.error('Logout failed:', error)
      setIsLoggingOut(false)
    }
  }

  const displayName = user?.name || user?.email || user?.sub || ''
  const initials = displayName
    .split(' ')
    .map((n) => n[0])
    .join('')
    .toUpperCase()
    .slice(0, 2) || 'U'

  return (
    <header className="flex items-center justify-between px-3 sm:px-6 py-3 bg-app-header border-b border-theme-border">
      <div className="flex items-center gap-3 sm:gap-6">
        <AppLink href="/" className="hover:opacity-80 transition-opacity">
          <MicromegasLogo size="sm" />
        </AppLink>
      </div>

      <div className="flex items-center gap-2 sm:gap-4">
        {/* Pivot Button - navigate between process views (only shown with time range) */}
        {timeRangeControl && (
          <PivotButton
            processId={processId}
            timeRangeFrom={timeRangeControl.timeRangeFrom}
            timeRangeTo={timeRangeControl.timeRangeTo}
          />
        )}

        {/* Time Range Controls (only shown when page provides timeRangeControl) */}
        {timeRangeControl ? (
          <div className="flex items-stretch h-8">
            <TimeRangePicker
              from={timeRangeControl.timeRangeFrom}
              to={timeRangeControl.timeRangeTo}
              onChange={timeRangeControl.onTimeRangeChange}
            />
            <button
              onClick={() => {
                const zoomed = zoomTimeRange(timeRangeControl.timeRangeFrom, timeRangeControl.timeRangeTo, 'out')
                timeRangeControl.onTimeRangeChange(zoomed.from, zoomed.to)
              }}
              className="flex items-center justify-center px-2 bg-theme-border border-l border-theme-border-hover text-theme-text-primary hover:bg-theme-border-hover transition-colors"
              title="Zoom out"
            >
              <ZoomOut className="w-4 h-4" />
            </button>
            <button
              onClick={() => {
                const zoomed = zoomTimeRange(timeRangeControl.timeRangeFrom, timeRangeControl.timeRangeTo, 'in')
                timeRangeControl.onTimeRangeChange(zoomed.from, zoomed.to)
              }}
              className="flex items-center justify-center px-2 bg-theme-border border-l border-theme-border-hover text-theme-text-primary hover:bg-theme-border-hover transition-colors"
              title="Zoom in"
            >
              <ZoomIn className="w-4 h-4" />
            </button>
            {onRefresh && onRefreshIntervalChange ? (
              <RefreshIntervalPicker
                intervalMs={refreshIntervalMs ?? 0}
                onIntervalChange={onRefreshIntervalChange}
                onRefresh={onRefresh}
                isExecuting={isExecuting}
              />
            ) : onRefresh ? (
              <button
                onClick={onRefresh}
                className="flex items-center justify-center px-2 sm:px-2.5 bg-theme-border border-l border-theme-border-hover rounded-r-md text-theme-text-primary hover:bg-theme-border-hover transition-colors"
                title="Refresh"
              >
                <RefreshCw className="w-4 h-4" />
              </button>
            ) : null}
          </div>
        ) : onRefresh && onRefreshIntervalChange ? (
          <RefreshIntervalPicker
            intervalMs={refreshIntervalMs ?? 0}
            onIntervalChange={onRefreshIntervalChange}
            onRefresh={onRefresh}
            isExecuting={isExecuting}
            className="rounded-l-md"
          />
        ) : onRefresh ? (
          <button
            onClick={onRefresh}
            className="px-2.5 py-1.5 bg-theme-border rounded-md text-theme-text-primary hover:bg-theme-border-hover transition-colors"
            title="Refresh"
          >
            <RefreshCw className="w-4 h-4" />
          </button>
        ) : null}

        {/* User Menu */}
        {status === 'authenticated' && user && (
          <div className="relative">
            <button
              onClick={() => setIsUserMenuOpen(!isUserMenuOpen)}
              className="flex items-center gap-2 px-3 py-1.5 rounded-md hover:bg-theme-border transition-colors"
            >
              <div className="w-7 h-7 rounded-full bg-accent-link flex items-center justify-center text-xs font-semibold text-white">
                {initials}
              </div>
              <ChevronDown className="w-3 h-3 text-theme-text-secondary" />
            </button>

            {isUserMenuOpen && (
              <>
                <div
                  className="fixed inset-0 z-10"
                  onClick={() => setIsUserMenuOpen(false)}
                />
                <div className="absolute right-0 mt-2 w-56 bg-app-panel rounded-md shadow-lg border border-theme-border z-20">
                  <div className="py-1">
                    <div className="px-4 py-2 border-b border-theme-border">
                      <p className="text-sm font-medium text-theme-text-primary truncate">
                        {user.name || 'User'}
                      </p>
                      {user.email && (
                        <p className="text-xs text-theme-text-muted truncate">{user.email}</p>
                      )}
                    </div>
                    <button
                      onClick={handleLogout}
                      disabled={isLoggingOut}
                      className="w-full flex items-center px-4 py-2 text-sm text-theme-text-primary hover:bg-theme-border disabled:opacity-50"
                    >
                      <LogOut className="h-4 w-4 mr-2" />
                      {isLoggingOut ? 'Signing out...' : 'Sign out'}
                    </button>
                  </div>
                </div>
              </>
            )}
          </div>
        )}
      </div>
    </header>
  )
}
