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

// Documentation URLs
const DOCS_BASE_URL = 'https://madesroches.github.io/micromegas/docs'
const SCHEMA_REF_URL = `${DOCS_BASE_URL}/query-guide/schema-reference`

export const QUERY_GUIDE_URL = `${DOCS_BASE_URL}/query-guide/`
export const LOG_ENTRIES_SCHEMA_URL = `${SCHEMA_REF_URL}/#log_entries`
export const PROCESSES_SCHEMA_URL = `${SCHEMA_REF_URL}/#processes`
export const MEASURES_SCHEMA_URL = `${SCHEMA_REF_URL}/#measures`
