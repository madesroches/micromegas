import { useLocation, useNavigate } from 'react-router-dom'
import * as DropdownMenu from '@radix-ui/react-dropdown-menu'
import { ChevronDown, FileText, BarChart2, Zap, Info } from 'lucide-react'

type ViewType = 'log' | 'metrics' | 'performance' | 'process'

interface ViewConfig {
  path: string
  label: string
  icon: React.ReactNode
}

const VIEW_CONFIGS: Record<ViewType, ViewConfig> = {
  log: {
    path: '/process_log',
    label: 'Log',
    icon: <FileText className="w-4 h-4" />,
  },
  metrics: {
    path: '/process_metrics',
    label: 'Metrics',
    icon: <BarChart2 className="w-4 h-4" />,
  },
  performance: {
    path: '/performance_analysis',
    label: 'Performance Analytics',
    icon: <Zap className="w-4 h-4" />,
  },
  process: {
    path: '/process',
    label: 'Process Details',
    icon: <Info className="w-4 h-4" />,
  },
}

// Define primary and secondary actions for each view
const PIVOT_CONFIG: Record<ViewType, { primary: ViewType; secondary: ViewType[] }> = {
  metrics: { primary: 'log', secondary: ['performance', 'process'] },
  log: { primary: 'metrics', secondary: ['performance', 'process'] },
  performance: { primary: 'log', secondary: ['metrics', 'process'] },
  process: { primary: 'log', secondary: ['metrics', 'performance'] },
}

function detectCurrentView(pathname: string): ViewType | null {
  if (pathname.includes('process_log')) return 'log'
  if (pathname.includes('process_metrics')) return 'metrics'
  if (pathname.includes('performance_analysis')) return 'performance'
  // Check for /process but not /processes (list page)
  if (pathname.match(/\/process(?:\?|$)/)) return 'process'
  return null
}

export interface PivotButtonProps {
  /** Process ID to pivot on - when undefined, button is hidden */
  processId?: string
  /** Time range from value to preserve across pivots */
  timeRangeFrom?: string
  /** Time range to value to preserve across pivots */
  timeRangeTo?: string
}

export function PivotButton({ processId, timeRangeFrom, timeRangeTo }: PivotButtonProps) {
  const location = useLocation()
  const navigate = useNavigate()

  const currentView = detectCurrentView(location.pathname)

  // Only show when on a process view with a valid process_id
  if (!currentView || !processId) {
    return null
  }

  const config = PIVOT_CONFIG[currentView]
  const primaryView = VIEW_CONFIGS[config.primary]
  const secondaryViews = config.secondary.map((viewType) => VIEW_CONFIGS[viewType])

  const buildUrl = (view: ViewConfig): string => {
    const params = new URLSearchParams()
    params.set('process_id', processId)
    if (timeRangeFrom) params.set('from', timeRangeFrom)
    if (timeRangeTo) params.set('to', timeRangeTo)
    return `${view.path}?${params.toString()}`
  }

  const handleNavigate = (view: ViewConfig) => {
    navigate(buildUrl(view))
  }

  return (
    <div className="inline-flex rounded-md">
      {/* Primary button */}
      <button
        onClick={() => handleNavigate(primaryView)}
        className="flex items-center gap-2 px-3 py-1.5 bg-theme-border border border-theme-border-hover border-r-0 rounded-l-md text-theme-text-primary hover:bg-theme-border-hover transition-colors text-sm"
      >
        <span className="text-accent-link">{primaryView.icon}</span>
        <span>{primaryView.label}</span>
      </button>

      {/* Dropdown trigger */}
      <DropdownMenu.Root>
        <DropdownMenu.Trigger asChild>
          <button
            className="flex items-center px-2 py-1.5 bg-theme-border border border-theme-border-hover rounded-r-md text-theme-text-secondary hover:bg-theme-border-hover transition-colors"
            aria-label="More pivot options"
          >
            <ChevronDown className="w-3 h-3" />
          </button>
        </DropdownMenu.Trigger>

        <DropdownMenu.Portal>
          <DropdownMenu.Content
            align="end"
            sideOffset={4}
            className="min-w-[180px] bg-app-panel border border-theme-border rounded-md shadow-lg py-1 z-50"
          >
            {secondaryViews.map((view) => (
              <DropdownMenu.Item
                key={view.path}
                onClick={() => handleNavigate(view)}
                className="flex items-center gap-2 px-3 py-2 text-sm text-theme-text-primary hover:bg-theme-border/50 cursor-pointer outline-none"
              >
                <span className="text-theme-text-secondary">{view.icon}</span>
                {view.label}
              </DropdownMenu.Item>
            ))}
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
    </div>
  )
}
