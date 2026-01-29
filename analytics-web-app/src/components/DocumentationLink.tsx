interface DocumentationLinkProps {
  url: string
  label: string
}

export function DocumentationLink({ url, label }: DocumentationLinkProps) {
  return (
    <div className="mt-4">
      <h4 className="text-xs font-semibold uppercase tracking-wide text-theme-text-muted mb-2">
        Documentation
      </h4>
      <a
        href={url}
        target="_blank"
        rel="noopener noreferrer"
        className="text-xs text-accent-link hover:underline"
      >
        {label}
      </a>
    </div>
  )
}

export const QUERY_GUIDE_URL = 'https://madesroches.github.io/micromegas/docs/query-guide/'
