/**
 * The shared allowlist textarea used by both mint dialogs and the edit dialog
 * (`EditAllowlistDialog.tsx`). One component keeps the label/help-text wording and styling in a
 * single place instead of three copies.
 */
export function AllowedCidrsField({
  value,
  onChange,
  optional,
}: {
  value: string
  onChange: (value: string) => void
  /** Adds a muted "(optional)" suffix to the label — set by the mint dialogs, not the edit dialog. */
  optional?: boolean
}) {
  return (
    <div>
      <label className="block text-sm font-medium text-theme-text-secondary mb-1">
        Allowed IPs / CIDR ranges
        {optional && <span className="text-theme-text-muted"> (optional)</span>}
      </label>
      <textarea
        className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm font-mono text-theme-text-primary placeholder:text-theme-text-muted outline-hidden focus:border-accent-link"
        rows={3}
        placeholder={'203.0.113.0/24\n198.51.100.7'}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
      <p className="mt-1 text-xs text-theme-text-muted">
        One entry per line (commas also work). Bare IPs match that single address. Leave empty to
        allow this key from any IP.
      </p>
    </div>
  )
}
