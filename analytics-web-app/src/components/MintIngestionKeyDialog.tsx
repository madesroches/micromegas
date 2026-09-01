import { useEffect, useRef, useState } from 'react'
import { X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { MintedKeyBanner } from '@/components/MintedKeyBanner'
import { AUDIENCE_PATTERN, type MyAudiences } from '@/lib/audience-grants-api'
import { mintIngestionApiKey } from '@/lib/ingestion-api-keys-api'
import type { MintApiKeyResponse } from '@/lib/api-keys-shared'

/**
 * The self-service ingestion-key mint dialog. Extracted from `AudienceAccessPage.tsx`'s local
 * `MintKeyDialog` (#1510) so `/admin/ingestion-keys`'s non-admin panel (#1544) can reuse the
 * exact same mint UI instead of a second copy.
 */
export function MintIngestionKeyDialog({
  open,
  prefillAudience,
  me,
  onClose,
  onMinted,
}: {
  open: boolean
  prefillAudience: string | null
  me: MyAudiences | null
  onClose: () => void
  onMinted: (response: MintApiKeyResponse) => void
}) {
  const [name, setName] = useState('')
  const [audienceChoice, setAudienceChoice] = useState<string>('__new__')
  const [newAudience, setNewAudience] = useState('')
  const [isMinting, setIsMinting] = useState(false)
  const [mintError, setMintError] = useState<string | null>(null)
  const [mintedKey, setMintedKey] = useState<MintApiKeyResponse | null>(null)
  const wasOpenRef = useRef(false)

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      // Reset only on the false->true transition of `open`, not on every render while the
      // dialog stays open -- `me` can get a fresh identity from a `loadMyAudiences()` refetch
      // (e.g. after a claimed mint) while this dialog is still showing the one-time key banner,
      // and that must not wipe `mintedKey`.
      const justOpened = open && !wasOpenRef.current
      wasOpenRef.current = open
      if (justOpened) {
        setName('')
        setMintError(null)
        setMintedKey(null)
        if (prefillAudience) {
          setAudienceChoice(prefillAudience)
          setNewAudience('')
        } else if (me?.audiences.length) {
          // Prefer an audience the caller personally holds a mint grant on over
          // `audiences[0]`: the seeded `('public','mint','*')` row puts `public` in
          // every non-admin's `audiences` list, so a plain lexicographic pick can
          // default to the wildcard-only shared pool instead of the caller's own
          // audience -- mirrors the CLI's `resolve_audience` `held_pairs` filter.
          const personal = me.audiences.find((a) => (me.held_pairs ?? []).includes(`${a}:mint`))
          setAudienceChoice(personal ?? me.audiences[0])
          setNewAudience('')
        } else {
          setAudienceChoice('__new__')
          setNewAudience('')
        }
      }
    })()
  }, [open, prefillAudience, me])

  if (!open) return null

  const isAdmin = me?.is_admin ?? false
  const prefix = !isAdmin ? me?.mint_prefix ?? null : null
  const composedNew = prefix ? `${prefix}${newAudience}` : newAudience
  const resolvedAudience = audienceChoice === '__new__' ? composedNew : audienceChoice
  const newAudienceValid = AUDIENCE_PATTERN.test(newAudience)
  const newAudienceInvalid = audienceChoice === '__new__' && !newAudienceValid

  const handleClose = () => {
    if (isMinting) return
    onClose()
  }

  const handleMint = async () => {
    setIsMinting(true)
    setMintError(null)
    try {
      const result = await mintIngestionApiKey(name.trim(), resolvedAudience || undefined)
      setMintedKey(result)
      onMinted(result)
    } catch (err) {
      setMintError(
        err instanceof Error ? err.message : 'Failed to mint ingestion key'
      )
    } finally {
      setIsMinting(false)
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/50" onClick={handleClose} />
      <div className="relative w-full max-w-md bg-app-panel border border-theme-border rounded-lg shadow-xl">
        <div className="px-4 py-3 border-b border-theme-border flex items-center justify-between">
          <h2 className="text-lg font-medium text-theme-text-primary">Mint ingestion key</h2>
          <button
            onClick={handleClose}
            className="p-1 text-theme-text-muted hover:text-theme-text-primary rounded-sm transition-colors"
          >
            <X className="w-5 h-5" />
          </button>
        </div>
        <div className="p-4 space-y-4">
          {mintError && (
            <div className="p-3 bg-accent-error/10 border border-accent-error/30 rounded-lg text-sm text-accent-error">
              {mintError}
            </div>
          )}

          {mintedKey ? (
            <MintedKeyBanner keyValue={mintedKey.key} onDismiss={handleClose}>
              {mintedKey.claimed && resolvedAudience && (
                <p className="mt-2 text-sm text-theme-text-secondary">
                  You claimed <code className="font-mono">{resolvedAudience}</code>; you now hold
                  read and mint on it.
                </p>
              )}
            </MintedKeyBanner>
          ) : (
            <>
              <div>
                <label className="block text-sm font-medium text-theme-text-secondary mb-1">
                  Name
                </label>
                <input
                  type="text"
                  className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm text-theme-text-primary placeholder:text-theme-text-muted outline-hidden focus:border-accent-link"
                  placeholder="my-laptop"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  autoFocus
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-theme-text-secondary mb-1">
                  Audience
                </label>
                <select
                  className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm text-theme-text-primary outline-hidden focus:border-accent-link"
                  value={audienceChoice}
                  onChange={(e) => setAudienceChoice(e.target.value)}
                >
                  {(me?.audiences ?? []).map((a) => (
                    <option key={a} value={a}>
                      {a}
                    </option>
                  ))}
                  <option value="__new__">New audience…</option>
                </select>
                {!isAdmin && (
                  <p className="mt-1 text-xs text-theme-text-muted">
                    <code className="font-mono">public</code> is readable by every authenticated
                    user. Pick <em>New audience…</em> to give this key&apos;s data its own
                    audience, with read access managed separately.
                  </p>
                )}
                {audienceChoice === '__new__' && (
                  <div className="mt-2">
                    <input
                      type="text"
                      className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm font-mono text-theme-text-primary placeholder:text-theme-text-muted outline-hidden focus:border-accent-link"
                      placeholder="myproj"
                      value={newAudience}
                      onChange={(e) => setNewAudience(e.target.value)}
                    />
                    {newAudience && !newAudienceValid && (
                      <p className="mt-1 text-xs text-accent-error">
                        Must match <code>[A-Za-z0-9_-]</code>, up to 255 characters.
                      </p>
                    )}
                    {!isAdmin && (
                      <p className="mt-1 text-xs font-mono text-theme-text-muted">
                        {newAudienceValid
                          ? `Will claim \`${composedNew}\` and grant you read + mint on it.`
                          : ''}
                      </p>
                    )}
                  </div>
                )}
              </div>
            </>
          )}
        </div>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-theme-border">
          <Button variant="outline" onClick={handleClose} disabled={isMinting}>
            {mintedKey ? 'Close' : 'Cancel'}
          </Button>
          {!mintedKey && (
            <Button
              onClick={handleMint}
              disabled={isMinting || !name.trim() || !resolvedAudience.trim() || newAudienceInvalid}
            >
              {isMinting ? (
                <span className="flex items-center gap-2">
                  <span className="w-4 h-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
                  Minting...
                </span>
              ) : (
                'Mint'
              )}
            </Button>
          )}
        </div>
      </div>
    </div>
  )
}
