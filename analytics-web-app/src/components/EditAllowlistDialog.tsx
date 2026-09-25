import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { AllowedCidrsField } from '@/components/AllowedCidrsField'
import { parseAllowlistInput } from '@/lib/api-keys-shared'

/**
 * Admin edit dialog for a key's IP allowlist, reached via the shield row action on
 * `ApiKeysAdminPage` (#1611). The parent mounts this only while it has a target and passes
 * `key={target.key_id}`, so the `useState` initializer below handles the prefill on open with no
 * reset-on-open effect needed.
 */
export function EditAllowlistDialog({
  keyName,
  initialCidrs,
  onSave,
  onClose,
}: {
  keyName: string
  initialCidrs: string[]
  onSave: (allowedCidrs: string[]) => Promise<unknown>
  onClose: () => void
}) {
  const [text, setText] = useState(initialCidrs.join('\n'))
  const [isSaving, setIsSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const handleClose = () => {
    if (isSaving) return
    onClose()
  }

  const handleSave = async () => {
    setIsSaving(true)
    setError(null)
    try {
      await onSave(parseAllowlistInput(text))
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to update IP allowlist')
    } finally {
      setIsSaving(false)
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/50" onClick={handleClose} />
      <div className="relative w-full max-w-md bg-app-panel border border-theme-border rounded-lg shadow-xl">
        <div className="px-4 py-3 border-b border-theme-border">
          <h2 className="text-lg font-medium text-theme-text-primary">
            IP allowlist — {keyName}
          </h2>
        </div>
        <div className="p-4 space-y-4">
          {error && (
            <div className="p-3 bg-accent-error/10 border border-accent-error/30 rounded-lg text-sm text-accent-error">
              {error}
            </div>
          )}
          <AllowedCidrsField value={text} onChange={setText} />
        </div>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-theme-border">
          <Button variant="outline" onClick={handleClose} disabled={isSaving}>
            Cancel
          </Button>
          <Button onClick={handleSave} disabled={isSaving}>
            {isSaving ? (
              <span className="flex items-center gap-2">
                <span className="w-4 h-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
                Saving...
              </span>
            ) : (
              'Save'
            )}
          </Button>
        </div>
      </div>
    </div>
  )
}
