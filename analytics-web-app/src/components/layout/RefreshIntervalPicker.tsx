import * as DropdownMenu from '@radix-ui/react-dropdown-menu'
import { RefreshCw, ChevronDown, Check } from 'lucide-react'

const PRESETS = [
  { label: 'Off', ms: 0 },
  { label: '5s', ms: 5_000 },
  { label: '10s', ms: 10_000 },
  { label: '30s', ms: 30_000 },
  { label: '1m', ms: 60_000 },
  { label: '5m', ms: 300_000 },
  { label: '15m', ms: 900_000 },
  { label: '30m', ms: 1_800_000 },
  { label: '1h', ms: 3_600_000 },
] as const

function labelForMs(ms: number): string | null {
  const preset = PRESETS.find((p) => p.ms === ms)
  return preset && preset.ms > 0 ? preset.label : null
}

interface RefreshIntervalPickerProps {
  intervalMs: number
  onIntervalChange: (ms: number) => void
  onRefresh: () => void
  /** Whether execution is in progress (spins the icon) */
  isExecuting?: boolean
  /** Extra classes on the refresh button (e.g. border rounding) */
  className?: string
}

export function RefreshIntervalPicker({
  intervalMs,
  onIntervalChange,
  onRefresh,
  isExecuting,
  className = '',
}: RefreshIntervalPickerProps) {
  const activeLabel = labelForMs(intervalMs)

  return (
    <div className="flex items-stretch h-full">
      <button
        onClick={onRefresh}
        className={`flex items-center justify-center px-2 sm:px-2.5 bg-theme-border border-l border-theme-border-hover text-theme-text-primary hover:bg-theme-border-hover transition-colors ${className}`}
        title={activeLabel ? `Auto-refreshing every ${activeLabel}` : 'Refresh'}
      >
        <RefreshCw className={`w-4 h-4 ${isExecuting ? 'animate-spin' : ''}`} />
      </button>
      {activeLabel && (
        <span className="flex items-center px-1.5 bg-theme-border border-l border-theme-border-hover text-xs text-theme-text-secondary select-none">
          {activeLabel}
        </span>
      )}
      <DropdownMenu.Root>
        <DropdownMenu.Trigger asChild>
          <button
            className="flex items-center justify-center px-1.5 bg-theme-border border-l border-theme-border-hover rounded-r-md text-theme-text-primary hover:bg-theme-border-hover transition-colors"
            title="Auto-refresh interval"
          >
            <ChevronDown className="w-3 h-3" />
          </button>
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content
            align="end"
            sideOffset={4}
            className="min-w-[120px] bg-app-panel border border-theme-border rounded-md shadow-lg py-1 z-50"
          >
            {PRESETS.map((preset) => (
              <DropdownMenu.Item
                key={preset.ms}
                onSelect={() => onIntervalChange(preset.ms)}
                className="flex items-center justify-between px-3 py-1.5 text-sm text-theme-text-primary hover:bg-theme-border cursor-pointer outline-none"
              >
                {preset.label}
                {preset.ms === intervalMs && <Check className="w-3.5 h-3.5 text-accent-link" />}
              </DropdownMenu.Item>
            ))}
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
    </div>
  )
}
