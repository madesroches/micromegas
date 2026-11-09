import { useState } from 'react'
import { useAuth } from '@/lib/auth'
import { RefreshCw, ChevronDown, LogOut, ZoomIn, ZoomOut, Users, ShieldAlert } from 'lucide-react'
import { AppLink } from '@/components/AppLink'
import { ADMIN_COLOR, ADMIN_STRIPE_STYLE, useAdminMode } from '@/lib/admin-mode'
import { getConfig } from '@/lib/config'
import { navigateTo } from '@/lib/navigation'
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
  const adminMode = useAdminMode()
  const refreshPaused = adminMode.active && adminMode.kind === 'toggle'
  const canToggleAdminMode = !!user?.is_admin && adminMode.kind === 'toggle' && !!adminMode.setActive

  const handleLogout = async () => {
    setIsLoggingOut(true)
    try {
      await logout()
      // Use full base path for raw browser navigation (not React Router)
      navigateTo(`${getConfig().basePath}/login`)
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
    // Fixed h-16 (64px) instead of padding-driven height: AppShell's fixed
    // Sidebar wrapper is offset with `top-16` to sit just below this header
    // (see AppShell.tsx), so the two must be kept in sync.
    <header className="relative flex items-center justify-between px-3 sm:px-6 h-16 bg-app-header border-b border-theme-border">
      {adminMode.active && (
        <div
          data-testid="admin-mode-stripe"
          className="absolute top-0 left-0 right-0 h-1"
          style={ADMIN_STRIPE_STYLE}
        />
      )}
      <div className="flex items-center gap-3 sm:gap-6">
        <AppLink href="/" className="hover:opacity-80 transition-opacity">
          <MicromegasLogo size="sm" />
        </AppLink>
        {adminMode.active && (
          <div className="flex items-center gap-2">
            <span
              className="px-2 py-0.5 rounded text-xs font-bold tracking-wide text-white"
              style={{ backgroundColor: ADMIN_COLOR }}
            >
              {adminMode.kind === 'toggle' ? 'ADMIN MODE' : 'ADMIN'}
            </span>
            {adminMode.kind === 'toggle' && adminMode.setActive && (
              <button
                onClick={() => adminMode.setActive?.(false)}
                className="px-2 py-0.5 rounded text-xs border text-theme-text-primary hover:bg-theme-border"
                style={{ borderColor: ADMIN_COLOR }}
              >
                Turn off
              </button>
            )}
          </div>
        )}
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
                paused={refreshPaused}
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
            paused={refreshPaused}
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
                    <AppLink
                      href="/audiences"
                      onClick={() => setIsUserMenuOpen(false)}
                      className="w-full flex items-center px-4 py-2 text-sm text-theme-text-primary hover:bg-theme-border"
                    >
                      <Users className="h-4 w-4 mr-2" />
                      Audience access
                    </AppLink>
                    {canToggleAdminMode && (
                      <button
                        onClick={() => {
                          adminMode.setActive?.(!adminMode.active)
                          setIsUserMenuOpen(false)
                        }}
                        className="w-full flex items-start px-4 py-2 text-sm text-theme-text-primary hover:bg-theme-border text-left"
                      >
                        <ShieldAlert className="h-4 w-4 mr-2 mt-0.5 shrink-0" style={{ color: ADMIN_COLOR }} />
                        <span>
                          {adminMode.active ? 'Disable admin mode' : 'Enable admin mode'}
                          <span className="block text-xs text-theme-text-muted">
                            This page only. Lets cells modify the lakehouse.
                          </span>
                        </span>
                      </button>
                    )}
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
