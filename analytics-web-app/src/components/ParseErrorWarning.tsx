interface ParseErrorWarningProps {
  errors: string[]
  className?: string
}

export function ParseErrorWarning({ errors, className = 'mb-4' }: ParseErrorWarningProps) {
  if (errors.length === 0) return null

  return (
    <div className={`px-3 py-2 bg-amber-500/10 border border-amber-500/30 rounded text-amber-400 text-xs ${className}`}>
      <span className="font-medium">Warning:</span> {errors.length} row(s) had invalid JSON properties and were skipped.
      <details className="mt-1">
        <summary className="cursor-pointer hover:text-amber-300">Show details</summary>
        <ul className="mt-1 ml-4 list-disc text-amber-400/80">
          {errors.slice(0, 5).map((err, i) => <li key={i}>{err}</li>)}
          {errors.length > 5 && <li>...and {errors.length - 5} more</li>}
        </ul>
      </details>
    </div>
  )
}
