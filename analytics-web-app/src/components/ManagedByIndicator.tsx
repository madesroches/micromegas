import { useEffect, useRef, useState } from 'react'
import { AlertTriangle, ExternalLink } from 'lucide-react'

interface ManagedByIndicatorProps {
  managedBy: string
}

export function ManagedByIndicator({ managedBy }: ManagedByIndicatorProps) {
  const [open, setOpen] = useState(false)
  const containerRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false)
    }
    const onMouseDown = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    document.addEventListener('keydown', onKeyDown)
    document.addEventListener('mousedown', onMouseDown)
    return () => {
      document.removeEventListener('keydown', onKeyDown)
      document.removeEventListener('mousedown', onMouseDown)
    }
  }, [open])

  return (
    <div
      ref={containerRef}
      className="relative"
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={() => setOpen(false)}
      onBlur={(e) => {
        if (!containerRef.current?.contains(e.relatedTarget as Node | null)) setOpen(false)
      }}
    >
      <button
        type="button"
        aria-label="Managed by source control"
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen(true)}
        onFocus={() => setOpen(true)}
        className="p-1.5 rounded-md hover:bg-app-card"
        style={{ color: 'var(--accent-warning)' }}
      >
        <AlertTriangle className="w-4 h-4" />
      </button>
      {open && (
        <div
          role="dialog"
          aria-label="Source control notice"
          className="absolute right-0 top-full z-40 mt-1 w-72 rounded-md border bg-app-panel p-3 text-sm shadow-xl"
          style={{ borderColor: 'var(--accent-warning)' }}
        >
          <p style={{ color: 'var(--accent-warning)' }}>
            This screen is managed by source control. Edits made here may be overwritten on the next
            deployment.
          </p>
          <a
            href={managedBy}
            target="_blank"
            rel="noopener noreferrer"
            className="mt-2 inline-flex items-center gap-1 whitespace-nowrap text-accent-link hover:underline"
          >
            View source <ExternalLink className="w-3 h-3" />
          </a>
        </div>
      )}
    </div>
  )
}
