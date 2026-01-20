import { useState } from 'react'
import { useAuth } from '@/lib/auth'
import { RefreshCw, ChevronDown, LogOut } from 'lucide-react'
import { AppLink } from '@/components/AppLink'
import { getConfig } from '@/lib/config'
import { TimeRangePicker } from './TimeRangePicker'
import { PivotButton } from './PivotButton'
import { MicromegasLogo } from '@/components/MicromegasLogo'
import type { TimeRangeControlProps } from './PageLayout'

interface HeaderProps {
  onRefresh?: () => void
  /** Time range control props - when provided, page controls time range */
  timeRangeControl?: TimeRangeControlProps
  /** Process ID for pivot button navigation */
  processId?: string
}

export function Header({ onRefresh, timeRangeControl, processId }: HeaderProps) {
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
          <div className="flex items-center">
            <TimeRangePicker
              from={timeRangeControl.timeRangeFrom}
              to={timeRangeControl.timeRangeTo}
              onChange={timeRangeControl.onTimeRangeChange}
            />
            <button
              onClick={onRefresh}
              className="px-2 sm:px-2.5 py-1.5 bg-theme-border border-l border-theme-border-hover rounded-r-md text-theme-text-primary hover:bg-theme-border-hover transition-colors"
              title="Refresh"
            >
              <RefreshCw className="w-4 h-4" />
            </button>
          </div>
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
