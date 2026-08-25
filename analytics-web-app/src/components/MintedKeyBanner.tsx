import { useState } from 'react'
import { Copy, Check } from 'lucide-react'
import { Button } from '@/components/ui/button'

/**
 * The one-time cleartext-key banner: shown exactly once right after a mint, never persisted,
 * never refetchable. Lifted out of `ApiKeysAdminPage.tsx` (#1510) so `AudienceAccessPage`'s
 * `MintKeyDialog` can reuse the same chrome + copy-to-clipboard behavior instead of duplicating
 * it.
 */
export function MintedKeyBanner({
  keyValue,
  onDismiss,
  children,
}: {
  keyValue: string
  onDismiss: () => void
  /** Extra content rendered below the key, e.g. the "you claimed <audience>" note. */
  children?: React.ReactNode
}) {
  const [copied, setCopied] = useState(false)

  const handleCopyKey = async () => {
    try {
      await navigator.clipboard.writeText(keyValue)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      // Clipboard access can fail (permissions, insecure context) — the key
      // is still visible and selectable, so this is a soft failure.
    }
  }

  return (
    <div className="mb-4 p-4 rounded-lg border border-accent-warning/40 bg-accent-warning/10">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="text-sm font-semibold text-theme-text-primary mb-1">
            Key minted — copy it now, it won't be shown again
          </div>
          <code className="block break-all text-sm font-mono text-theme-text-primary bg-app-bg px-2.5 py-1.5 rounded-sm border border-theme-border">
            {keyValue}
          </code>
          {children}
        </div>
        <button
          onClick={onDismiss}
          className="shrink-0 p-1.5 rounded-sm text-theme-text-muted hover:text-theme-text-primary hover:bg-theme-border transition-colors"
          aria-label="Dismiss"
        >
          ×
        </button>
      </div>
      <Button variant="outline" onClick={handleCopyKey} className="mt-3 gap-1.5">
        {copied ? <Check className="w-4 h-4 text-green-500" /> : <Copy className="w-4 h-4" />}
        {copied ? 'Copied' : 'Copy key'}
      </Button>
    </div>
  )
}
