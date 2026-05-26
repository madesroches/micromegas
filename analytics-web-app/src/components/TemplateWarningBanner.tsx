import { AlertTriangle } from 'lucide-react'

interface TemplateWarningBannerProps {
  warnings: string[]
}

export function TemplateWarningBanner({ warnings }: TemplateWarningBannerProps) {
  if (warnings.length === 0) return null
  return (
    <div className="mb-2 flex gap-2 rounded border border-amber-500/50 bg-amber-500/10 px-3 py-2 text-xs text-amber-200">
      <AlertTriangle className="mt-0.5 h-3.5 w-3.5 flex-shrink-0 text-amber-300" />
      <ul className="space-y-0.5">
        {warnings.map((w, i) => (
          <li key={i}>{w}</li>
        ))}
      </ul>
    </div>
  )
}
