import { X } from 'lucide-react'
import { AppLink } from '@/components/AppLink'
import type { MapEvent } from './MapViewer'

interface EventDetailPanelProps {
  event: MapEvent
  onClose: () => void
}

export function EventDetailPanel({ event, onClose }: EventDetailPanelProps) {
  const formatTime = (date: Date) => {
    return date.toLocaleString('en-US', {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      hour12: false,
    })
  }

  return (
    <div className="absolute bottom-4 left-4 w-80 bg-app-panel border border-theme-border rounded-lg shadow-lg z-10">
      <div className="flex items-center justify-between px-4 py-3 border-b border-theme-border">
        <h3 className="text-sm font-semibold text-theme-text-primary">Event Details</h3>
        <button
          onClick={onClose}
          className="p-1 rounded hover:bg-theme-border transition-colors"
          title="Close"
        >
          <X className="w-4 h-4 text-theme-text-muted" />
        </button>
      </div>

      <div className="p-4 space-y-3">
        <div>
          <div className="text-xs text-theme-text-muted uppercase tracking-wider mb-1">Time</div>
          <div className="text-sm font-mono text-theme-text-primary">{formatTime(event.time)}</div>
        </div>

        {Object.entries(event.properties).map(([key, value]) => (
          <div key={key}>
            <div className="text-xs text-theme-text-muted uppercase tracking-wider mb-1">
              {key.replace(/_/g, ' ')}
            </div>
            <div className="text-sm text-theme-text-primary">{value}</div>
          </div>
        ))}

        <div>
          <div className="text-xs text-theme-text-muted uppercase tracking-wider mb-1">
            Coordinates
          </div>
          <div className="text-sm font-mono text-theme-text-secondary">
            X: {event.x.toFixed(1)}, Y: {event.y.toFixed(1)}, Z: {event.z.toFixed(1)}
          </div>
        </div>

        {event.processId && (
          <div className="pt-2 border-t border-theme-border">
            <AppLink
              href={`/process?id=${event.processId}`}
              className="text-sm text-accent-link hover:underline"
            >
              View Process Logs
            </AppLink>
          </div>
        )}
      </div>
    </div>
  )
}
