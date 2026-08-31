import { Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { usePageTitle } from '@/hooks/usePageTitle'
import { Users, Plus, Share2, X, KeyRound } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { ErrorBanner } from '@/components/ErrorBanner'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import { Button } from '@/components/ui/button'
import { MintedKeyBanner } from '@/components/MintedKeyBanner'
import { useAuth } from '@/lib/auth'
import {
  AUDIENCE_PATTERN,
  AudienceGrantError,
  createAudienceGrant,
  deleteAudienceGrant,
  fetchMyAudiences,
  fetchVisibleGrants,
  validateSelector,
  type AudienceGrant,
  type GrantAxis,
  type MyAudiences,
} from '@/lib/audience-grants-api'
import { mintIngestionApiKey } from '@/lib/ingestion-api-keys-api'
import type { MintApiKeyResponse } from '@/lib/api-keys-shared'

// ---------------------------------------------------------------------------
// useVisibleGrants -- wraps fetchVisibleGrants() in useStreamQuery's minimal
// { isComplete, error } shape so the page's completion effect needs no special-casing versus
// the streamed pages, but calls REST, not SQL/flight-SQL: this page's writes are fixed to this
// deployment's own store, so its read has to be too (see the "Reading through this deployment"
// standing note below).
// ---------------------------------------------------------------------------

interface UseVisibleGrantsReturn {
  execute: () => Promise<void>
  isComplete: boolean
  isStreaming: boolean
  error: { message: string; code?: string } | null
  getGrants: () => AudienceGrant[]
}

function useVisibleGrants(): UseVisibleGrantsReturn {
  const [isComplete, setIsComplete] = useState(false)
  const [isStreaming, setIsStreaming] = useState(false)
  const [error, setError] = useState<{ message: string; code?: string } | null>(null)
  const [grants, setGrants] = useState<AudienceGrant[]>([])

  const execute = useCallback(async () => {
    setIsComplete(false)
    setIsStreaming(true)
    setError(null)
    try {
      const rows = await fetchVisibleGrants()
      setGrants(rows)
    } catch (err) {
      setError({
        message: err instanceof AudienceGrantError ? err.message : 'Failed to load grants',
        code: err instanceof AudienceGrantError ? err.code : undefined,
      })
    } finally {
      setIsStreaming(false)
      setIsComplete(true)
    }
  }, [])

  return { execute, isComplete, isStreaming, error, getGrants: () => grants }
}

// ---------------------------------------------------------------------------
// Grouping
// ---------------------------------------------------------------------------

interface AudienceGroup {
  audience: string
  read: AudienceGrant[]
  mint: AudienceGrant[]
  count: number
}

function selectorSortKey(selector: string): string {
  // `*` sorts first within an axis, then alphabetically.
  return selector === '*' ? '' : selector
}

function groupGrants(grants: AudienceGrant[]): AudienceGroup[] {
  const byAudience = new Map<string, AudienceGrant[]>()
  for (const grant of grants) {
    const list = byAudience.get(grant.audience)
    if (list) {
      list.push(grant)
    } else {
      byAudience.set(grant.audience, [grant])
    }
  }
  const groups: AudienceGroup[] = []
  for (const [audience, rows] of byAudience) {
    const read = rows
      .filter((r) => r.axis === 'read')
      .sort((a, b) => selectorSortKey(a.selector).localeCompare(selectorSortKey(b.selector)))
    const mint = rows
      .filter((r) => r.axis === 'mint')
      .sort((a, b) => selectorSortKey(a.selector).localeCompare(selectorSortKey(b.selector)))
    groups.push({ audience, read, mint, count: rows.length })
  }
  groups.sort((a, b) => a.audience.localeCompare(b.audience))
  return groups
}

function formatDate(date: Date | null): string {
  if (!date) return '—'
  return date.toLocaleString()
}

// ---------------------------------------------------------------------------
// GrantDialog -- "Add grant" (admin, from the header) and "Share" (anyone, from a card row).
// ---------------------------------------------------------------------------

type SelectorKind = 'everyone' | 'user' | 'group'

interface GrantDialogProps {
  /** `null` closes the dialog. `'add'` is the admin header action (audience/axis editable);
   *  a `{ audience, axis }` pair is the Share action from a card row (both fixed/displayed). */
  target: 'add' | { audience: string; axis: GrantAxis } | null
  onClose: () => void
  onSubmit: (audience: string, axis: GrantAxis, selector: string) => Promise<void>
  isSubmitting: boolean
  error: string | null
  alreadyExistedNote: string | null
}

function GrantDialog({
  target,
  onClose,
  onSubmit,
  isSubmitting,
  error,
  alreadyExistedNote,
}: GrantDialogProps) {
  const isAdd = target === 'add'
  const fixedAudience = isAdd || target === null ? null : target.audience
  const fixedAxis = isAdd || target === null ? null : target.axis

  const [audienceInput, setAudienceInput] = useState('')
  const [axisInput, setAxisInput] = useState<GrantAxis>('read')
  const [selectorKind, setSelectorKind] = useState<SelectorKind>('user')
  const [idInput, setIdInput] = useState('')

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (target !== null) {
        setAudienceInput(isAdd ? '' : (target as { audience: string }).audience)
        setAxisInput(isAdd ? 'read' : (target as { axis: GrantAxis }).axis)
        setSelectorKind(isAdd ? 'everyone' : 'user')
        setIdInput('')
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [target])

  if (target === null) return null

  const audience = fixedAudience ?? audienceInput
  const axis = fixedAxis ?? axisInput
  const selector = selectorKind === 'everyone' ? '*' : `${selectorKind}:${idInput}`

  const audienceValid = isAdd ? AUDIENCE_PATTERN.test(audienceInput) : true
  const selectorError = validateSelector(selector)

  const canSubmit = !isSubmitting && audienceValid && selectorError === null

  const title = isAdd ? 'Add audience grant' : `Share ${axis} access to \`${audience}\``

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/50" onClick={isSubmitting ? undefined : onClose} />
      <div className="relative w-full max-w-lg bg-app-panel border border-theme-border rounded-lg shadow-xl">
        <div className="px-4 py-3 border-b border-theme-border flex items-center justify-between">
          <h2 className="text-lg font-medium text-theme-text-primary font-mono">{title}</h2>
          <button
            onClick={onClose}
            disabled={isSubmitting}
            className="p-1 text-theme-text-muted hover:text-theme-text-primary rounded-sm transition-colors disabled:opacity-50"
          >
            <X className="w-5 h-5" />
          </button>
        </div>
        <div className="p-4 space-y-4">
          {error && (
            <div className="p-3 bg-accent-error/10 border border-accent-error/30 rounded-lg text-sm text-accent-error">
              {error}
            </div>
          )}
          {alreadyExistedNote && (
            <div className="p-3 bg-app-card border border-theme-border rounded-lg text-sm text-theme-text-secondary">
              {alreadyExistedNote}
            </div>
          )}

          {isAdd ? (
            <div>
              <label className="block text-sm font-medium text-theme-text-secondary mb-1">
                Audience
              </label>
              <input
                type="text"
                className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm font-mono text-theme-text-primary placeholder:text-theme-text-muted outline-hidden focus:border-accent-link"
                placeholder="team-alpha"
                value={audienceInput}
                onChange={(e) => setAudienceInput(e.target.value)}
                autoFocus
              />
              <p className="mt-1 text-xs text-theme-text-muted">
                Must match <code>[A-Za-z0-9_-]</code>, up to 255 characters.
              </p>
            </div>
          ) : (
            <div>
              <div className="text-sm font-medium text-theme-text-secondary mb-1">Audience</div>
              <div className="font-mono text-sm text-theme-text-primary">{audience}</div>
            </div>
          )}

          {isAdd ? (
            <div>
              <label className="block text-sm font-medium text-theme-text-secondary mb-1">
                Axis
              </label>
              <div className="inline-flex rounded-md border border-theme-border overflow-hidden">
                {(['read', 'mint'] as GrantAxis[]).map((a) => (
                  <button
                    key={a}
                    type="button"
                    onClick={() => setAxisInput(a)}
                    className={`px-3 py-1.5 text-sm capitalize ${
                      axisInput === a
                        ? 'bg-accent-link text-white'
                        : 'bg-app-bg text-theme-text-secondary hover:bg-app-card'
                    }`}
                  >
                    {a}
                  </button>
                ))}
              </div>
              <p className="mt-1 text-xs text-theme-text-muted">
                Read: may query data stamped with this audience. Mint: may issue ingestion keys
                stamped with it. A read grant never confers mint.
              </p>
            </div>
          ) : (
            <div>
              <div className="text-sm font-medium text-theme-text-secondary mb-1">Axis</div>
              <div className="font-mono text-sm text-theme-text-primary capitalize">{axis}</div>
            </div>
          )}

          <div>
            <label className="block text-sm font-medium text-theme-text-secondary mb-1">
              Selector
            </label>
            <div className="inline-flex rounded-md border border-theme-border overflow-hidden mb-2">
              {(
                isAdd
                  ? (['everyone', 'user', 'group'] as SelectorKind[])
                  : (['user', 'group'] as SelectorKind[])
              ).map((kind) => (
                <button
                  key={kind}
                  type="button"
                  onClick={() => setSelectorKind(kind)}
                  className={`px-3 py-1.5 text-sm capitalize ${
                    selectorKind === kind
                      ? 'bg-accent-link text-white'
                      : 'bg-app-bg text-theme-text-secondary hover:bg-app-card'
                  }`}
                >
                  {kind}
                </button>
              ))}
            </div>
            {selectorKind !== 'everyone' && (
              <input
                type="text"
                className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm font-mono text-theme-text-primary placeholder:text-theme-text-muted outline-hidden focus:border-accent-link"
                placeholder={selectorKind === 'user' ? 'alice@example.com' : 'eng'}
                value={idInput}
                onChange={(e) => setIdInput(e.target.value)}
              />
            )}
            <p className="mt-2 text-xs font-mono text-theme-text-secondary">{selector}</p>
            <p className="mt-1 text-xs text-theme-text-muted">
              Matched against the caller's OIDC <code>email</code> / <code>groups</code> claim.
              There is no user directory here — enter the claim value verbatim.
            </p>
          </div>
        </div>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-theme-border">
          <Button variant="outline" onClick={onClose} disabled={isSubmitting}>
            Cancel
          </Button>
          <Button onClick={() => onSubmit(audience, axis, selector)} disabled={!canSubmit}>
            {isSubmitting ? (
              <span className="flex items-center gap-2">
                <span className="w-4 h-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
                Saving...
              </span>
            ) : isAdd ? (
              'Add grant'
            ) : (
              'Share'
            )}
          </Button>
        </div>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// MintKeyDialog
// ---------------------------------------------------------------------------

function MintKeyDialog({
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
          const personal = me.audiences.find((a) => me.held_pairs.includes(`${a}:mint`))
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

// ---------------------------------------------------------------------------
// AudienceAccessPage
// ---------------------------------------------------------------------------

function AudienceAccessPageContent() {
  usePageTitle('Audience Access')
  const { user } = useAuth()

  const listQuery = useVisibleGrants()
  const [grants, setGrants] = useState<AudienceGrant[]>([])
  const [listError, setListError] = useState<string | null>(null)
  const [me, setMe] = useState<MyAudiences | null>(null)
  const [selfServiceOff, setSelfServiceOff] = useState(false)
  const [myAudiencesError, setMyAudiencesError] = useState<string | null>(null)
  // Set from either `/visible` or `/my-audiences` when the server reports `AUTH_DISABLED`
  // (`--disable-auth`'s `key_management_disabled_router` answers every `/api/audience-grants*`
  // route with a fixed 503, but `/auth/me` still reports `is_admin: true` in that mode, so
  // without this the page would otherwise render its normal admin/non-admin body against writes
  // that all 503). Once set, the whole page body is replaced by a single explanatory panel.
  const [authDisabled, setAuthDisabled] = useState(false)

  const [axisFilter, setAxisFilter] = useState<GrantAxis | null>(null)
  const [findText, setFindText] = useState('')

  const [dialogTarget, setDialogTarget] = useState<'add' | { audience: string; axis: GrantAxis } | null>(
    null
  )
  const [shareError, setShareError] = useState<string | null>(null)
  const [isSharing, setIsSharing] = useState(false)
  const [alreadyExistedNote, setAlreadyExistedNote] = useState<string | null>(null)

  const [deleteTarget, setDeleteTarget] = useState<AudienceGrant | null>(null)
  const [isDeleting, setIsDeleting] = useState(false)
  const [deleteError, setDeleteError] = useState<string | null>(null)

  const [mintOpen, setMintOpen] = useState(false)
  const [mintPrefillAudience, setMintPrefillAudience] = useState<string | null>(null)

  const isAdmin = user?.is_admin ?? false
  const myEmail = me?.email ?? user?.email ?? null

  const loadGrants = useCallback(() => {
    void listQuery.execute()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const loadMyAudiences = useCallback(async () => {
    try {
      const result = await fetchMyAudiences()
      setMe(result)
      setSelfServiceOff(false)
      setMyAudiencesError(null)
    } catch (err) {
      if (err instanceof AudienceGrantError && err.code === 'AUTH_DISABLED') {
        setAuthDisabled(true)
      } else if (err instanceof AudienceGrantError && err.status === 403) {
        setMe(null)
        setSelfServiceOff(true)
        setMyAudiencesError(null)
      } else {
        // Genuine failure (500, network, parse) -- keep `me` at whatever it was (most likely
        // still null) and surface a retryable error instead of silently leaving the Mint
        // affordances gated open on a null identity.
        setMyAudiencesError(
          err instanceof AudienceGrantError ? err.message : 'Failed to load your audiences'
        )
      }
    }
  }, [])

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      loadGrants()
      void loadMyAudiences()
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (listQuery.isComplete) {
        if (listQuery.error) {
          if (listQuery.error.code === 'AUTH_DISABLED') {
            setAuthDisabled(true)
          }
          setListError(listQuery.error.message)
        } else {
          setListError(null)
          setGrants(listQuery.getGrants())
        }
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [listQuery.isComplete, listQuery.error])

  const findLower = findText.trim().toLowerCase()
  const groups = useMemo(() => {
    const filteredByAxis =
      axisFilter === null
        ? grants
        : grants.filter((g) => g.axis === axisFilter)
    const all = groupGrants(filteredByAxis)
    if (!findLower) return all
    return all.filter(
      (g) =>
        g.audience.toLowerCase().includes(findLower) ||
        g.read.some((r) => r.selector.toLowerCase().includes(findLower)) ||
        g.mint.some((r) => r.selector.toLowerCase().includes(findLower))
    )
  }, [grants, axisFilter, findLower])

  // Unfiltered-by-axis lookup, keyed by audience: used to tell "this row is genuinely empty"
  // apart from "this row is only empty because the Axis filter stripped it" (`groups` above
  // reflects the axis-filtered set, so group.read/group.mint can't answer that on their own).
  const unfilteredByAudience = useMemo(() => {
    const map = new Map<string, AudienceGroup>()
    for (const g of groupGrants(grants)) map.set(g.audience, g)
    return map
  }, [grants])

  const totalCount = grants.length
  const totalAudiences = new Set(grants.map((g) => g.audience)).size

  // Ground truth for "can I share this pair" is `me.held_pairs`, not anything visible in
  // `grants`: `/visible` returns every row on a pair the caller can merely *see* (wider than
  // what they *hold* -- it includes pairs visible only via a `*` row, or a `group:` row the
  // caller isn't actually a member of), and the client has no group-membership info of its own
  // to tell those apart. Scanning selector prefixes on the row would just guess.
  const heldPairs = useMemo(() => new Set(me?.held_pairs ?? []), [me])

  const canShareRow = (audience: string, axis: GrantAxis): boolean => {
    if (isAdmin) return true
    if (selfServiceOff) return false
    return heldPairs.has(`${audience}:${axis}`)
  }

  const canDeleteChip = (chip: AudienceGrant): boolean => {
    if (isAdmin) return true
    if (selfServiceOff) return false
    // The server refuses this specific deletion even for the caller's own row: it's the
    // self-service claim marker `max_claims_per_caller` counts from (`ingestion_keys.rs`), and a
    // non-admin deleting it would let them dodge that bound by claiming, deleting, re-claiming.
    // Matched on shape, not provenance, so it also covers an admin-granted `mint` row on the
    // caller themselves -- same as the server-side check.
    if (chip.axis === 'mint' && chip.selector === `user:${myEmail}`) return false
    return chip.selector === `user:${myEmail}` || chip.createdBy === myEmail
  }

  const openAddDialog = () => {
    setShareError(null)
    setAlreadyExistedNote(null)
    setDialogTarget('add')
  }

  const openShareDialog = (audience: string, axis: GrantAxis) => {
    setShareError(null)
    setAlreadyExistedNote(null)
    setDialogTarget({ audience, axis })
  }

  const handleGrantSubmit = async (audience: string, axis: GrantAxis, selector: string) => {
    setIsSharing(true)
    setShareError(null)
    setAlreadyExistedNote(null)
    try {
      const { grant, created } = await createAudienceGrant(audience, axis, selector)
      if (created) {
        setDialogTarget(null)
      } else {
        setAlreadyExistedNote(
          `That grant already existed (created ${formatDate(new Date(grant.created_at))} by ${grant.created_by}).`
        )
      }
      loadGrants()
    } catch (err) {
      setShareError(err instanceof AudienceGrantError ? err.message : 'Failed to create grant')
    } finally {
      setIsSharing(false)
    }
  }

  const openDeleteDialog = (grant: AudienceGrant) => {
    setDeleteError(null)
    setDeleteTarget(grant)
  }

  const isOwnRow = (grant: AudienceGrant) => grant.selector === `user:${myEmail}`

  const handleDelete = async () => {
    if (!deleteTarget) return
    setIsDeleting(true)
    setDeleteError(null)
    try {
      await deleteAudienceGrant(deleteTarget.audience, deleteTarget.axis, deleteTarget.selector)
      setDeleteTarget(null)
      loadGrants()
    } catch (err) {
      setDeleteError(err instanceof AudienceGrantError ? err.message : 'Failed to delete grant')
      loadGrants()
    } finally {
      setIsDeleting(false)
    }
  }

  const deleteMessage = deleteTarget
    ? isAdmin || !isOwnRow(deleteTarget)
      ? deleteTarget.axis === 'mint'
        ? `Delete the mint grant on \`${deleteTarget.audience}\` for \`${deleteTarget.selector}\`? Principals matching this selector lose access immediately.`
        : `Delete the read grant on \`${deleteTarget.audience}\` for \`${deleteTarget.selector}\`? Principals matching this selector lose access once the grant cache expires (default 60 s).`
      : `Remove your direct ${deleteTarget.axis} grant on \`${deleteTarget.audience}\`? Unless a group or everyone grant also covers you, you lose access to this audience and cannot restore it yourself — an admin or someone who holds it would have to share it again.`
    : ''

  const openMintDialog = (prefillAudience?: string) => {
    setMintPrefillAudience(prefillAudience ?? null)
    setMintOpen(true)
  }

  const handleMinted = (response: MintApiKeyResponse) => {
    loadGrants()
    if (response.claimed) {
      void loadMyAudiences()
    }
  }

  // `me !== null` guards against a genuine my-audiences fetch failure (myAudiencesError):
  // without it, Mint controls would stay active with a null identity -- wrong prefix, no
  // audience options in the dialog's <select>. `me` being legitimately still-loading also
  // reads as null here, which is fine: Mint just doesn't show yet.
  const showMintButton = me !== null && (isAdmin || !selfServiceOff)

  // `--disable-auth` reports `is_admin: true` from `/auth/me` while every
  // `/api/audience-grants*` route 503s (`AUTH_DISABLED`) -- render one explanatory panel instead
  // of the normal admin/non-admin body, since every write control below would also 503.
  if (authDisabled) {
    return (
      <AuthGuard>
        <PageLayout>
          <div className="p-6 flex flex-col h-full items-center justify-center text-center">
            <Users className="w-10 h-10 text-theme-text-muted opacity-40 mb-3" />
            <p className="text-theme-text-muted max-w-md">
              Audience grant management is unavailable when authentication is disabled.
            </p>
          </div>
        </PageLayout>
      </AuthGuard>
    )
  }

  return (
    <AuthGuard>
      <PageLayout onRefresh={loadGrants}>
        <div className="p-6 flex flex-col h-full">
          <div className="flex items-center gap-1.5 text-sm text-theme-text-muted mb-4">
            {isAdmin && (
              <>
                <AppLink href="/admin" className="text-accent-link hover:underline">
                  Admin
                </AppLink>
                <span>/</span>
              </>
            )}
            <span>Audience Access</span>
          </div>

          <div className="flex items-center justify-between mb-6 gap-4 flex-wrap">
            <div>
              <h1 className="text-2xl font-semibold text-theme-text-primary">Audience Access</h1>
              <p className="mt-1 text-theme-text-secondary">
                {isAdmin
                  ? 'Who can read from, and mint into, each audience.'
                  : 'The audiences you can read from and mint into, and who shares them with you.'}
              </p>
            </div>
            <div className="flex items-center gap-2">
              {showMintButton && (
                <Button variant="outline" onClick={() => openMintDialog()} className="gap-1.5">
                  <KeyRound className="w-4 h-4" />
                  Mint ingestion key
                </Button>
              )}
              {isAdmin && (
                <Button onClick={openAddDialog} className="gap-1.5">
                  <Plus className="w-4 h-4" />
                  Add grant
                </Button>
              )}
            </div>
          </div>

          <div className="flex items-center gap-3 mb-4 flex-wrap">
            <input
              type="text"
              className="w-64 bg-app-bg border border-theme-border rounded-md px-3 py-1.5 text-sm text-theme-text-primary placeholder:text-theme-text-muted outline-hidden focus:border-accent-link"
              placeholder="Find audience or selector"
              value={findText}
              onChange={(e) => setFindText(e.target.value)}
            />
            <div className="inline-flex rounded-md border border-theme-border overflow-hidden">
              {([null, 'read', 'mint'] as (GrantAxis | null)[]).map((a) => (
                <button
                  key={a ?? 'both'}
                  type="button"
                  onClick={() => setAxisFilter(a)}
                  className={`px-3 py-1.5 text-sm capitalize ${
                    axisFilter === a
                      ? 'bg-accent-link text-white'
                      : 'bg-app-bg text-theme-text-secondary hover:bg-app-card'
                  }`}
                >
                  {a ?? 'Both'}
                </button>
              ))}
            </div>
            <span className="text-sm text-theme-text-muted">
              {totalCount} grants across {totalAudiences} audiences
            </span>
          </div>

          <div className="mb-4 space-y-1.5 text-xs text-theme-text-muted">
            <p>
              <strong className="text-theme-text-secondary">Propagation:</strong> read grants
              take effect within <code>MICROMEGAS_AUDIENCE_GRANT_CACHE_TTL_SECONDS</code>{' '}
              (default 60 s) because reads are served from a whole-table snapshot; mint grants
              and the rows on this page are live.
            </p>
            <p>
              <strong className="text-theme-text-secondary">Defaults:</strong>{' '}
              <code>public</code> ships with Read and Mint grants for everyone (attributed to{' '}
              <code>default</code> on the admin view). They are ordinary grants: removing the
              Read row stops public data from being universally readable; removing the Mint row
              limits minting into <code>public</code> to admins.
            </p>
            <p>
              <strong className="text-theme-text-secondary">Env-map grants:</strong> read access
              may also come from the <code>MICROMEGAS_AUDIENCE_GRANTS</code> startup map or a
              per-key <code>read_audiences</code> list; neither is shown here and neither can be
              shared from here.
            </p>
            <p>
              <strong className="text-theme-text-secondary">Reading through this deployment:</strong>{' '}
              Share, Remove, Revoke, and Mint always call this deployment's own store — there is
              no data-source picker here, unlike Query Deny List, since a flight-SQL data source
              is not guaranteed to be the same Postgres this page's writes land on.
              <code className="ml-1">list_audience_grants()</code> is still the way to audit this
              store from SQL.
            </p>
            {!isAdmin && selfServiceOff && (
              <p>
                Self-service is disabled on this deployment. You can see your grants here; ask an
                admin to change them.
              </p>
            )}
          </div>

          {listError && <ErrorBanner title="Failed to load grants" message={listError} onRetry={loadGrants} />}
          {myAudiencesError && (
            <ErrorBanner
              title="Failed to load your audiences"
              message={myAudiencesError}
              onRetry={() => void loadMyAudiences()}
            />
          )}
          {(shareError || deleteError) && !dialogTarget && !deleteTarget && (
            <ErrorBanner title="Error" message={(shareError || deleteError) ?? ''} />
          )}

          <GrantDialog
            target={dialogTarget}
            onClose={() => {
              setDialogTarget(null)
              setShareError(null)
              setAlreadyExistedNote(null)
            }}
            onSubmit={handleGrantSubmit}
            isSubmitting={isSharing}
            error={shareError}
            alreadyExistedNote={alreadyExistedNote}
          />

          <ConfirmDialog
            isOpen={deleteTarget !== null}
            onClose={() => {
              setDeleteTarget(null)
              setDeleteError(null)
            }}
            onConfirm={handleDelete}
            title={
              deleteTarget && !isAdmin && isOwnRow(deleteTarget) ? 'Remove my access' : 'Delete grant'
            }
            message={deleteMessage}
            confirmLabel={deleteTarget && !isAdmin && isOwnRow(deleteTarget) ? 'Remove' : 'Delete'}
            isLoading={isDeleting}
            variant="danger"
            error={deleteError}
          />

          <MintKeyDialog
            open={mintOpen && me !== null}
            prefillAudience={mintPrefillAudience}
            me={me}
            onClose={() => setMintOpen(false)}
            onMinted={handleMinted}
          />

          {listQuery.isStreaming && grants.length === 0 ? (
            <div className="flex-1 flex items-center justify-center">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
                <span className="text-theme-text-secondary">Loading grants…</span>
              </div>
            </div>
          ) : groups.length === 0 && grants.length === 0 ? (
            <div className="flex-1 flex flex-col items-center justify-center text-center">
              <Users className="w-10 h-10 text-theme-text-muted opacity-40 mb-3" />
              {isAdmin ? (
                <>
                  <p className="text-theme-text-muted mb-4 max-w-md">
                    No audience grants yet. With none at all, nothing is readable or mintable by
                    a non-admin; add a grant to open up an audience.
                  </p>
                  <Button onClick={openAddDialog} className="gap-1.5">
                    <Plus className="w-4 h-4" />
                    Add grant
                  </Button>
                </>
              ) : (
                <>
                  <p className="text-theme-text-muted mb-4 max-w-md">
                    You hold no audience grants of your own. You can read <code>public</code> via
                    its Read grant. Mint an ingestion key into a new audience to claim one, or ask
                    an admin for a grant.
                  </p>
                  {showMintButton && (
                    <Button onClick={() => openMintDialog()} className="gap-1.5">
                      <KeyRound className="w-4 h-4" />
                      Mint ingestion key
                    </Button>
                  )}
                </>
              )}
            </div>
          ) : groups.length === 0 ? (
            <div className="flex-1 flex items-center justify-center text-center">
              <p className="text-theme-text-muted">No grants match this filter.</p>
            </div>
          ) : (
            <div className="space-y-4">
              {groups.map((group) => (
                <div
                  key={group.audience}
                  className="border border-theme-border rounded-lg overflow-hidden"
                >
                  <div className="px-4 py-2.5 bg-app-panel flex items-center justify-between">
                    <span className="font-mono text-sm text-theme-text-primary">
                      {group.audience}
                    </span>
                    <span className="text-xs text-theme-text-muted">{group.count} grants</span>
                  </div>
                  <div className="p-4 space-y-4">
                    {(['read', 'mint'] as GrantAxis[]).map((axis) => {
                      const rows = axis === 'read' ? group.read : group.mint
                      if (rows.length === 0) {
                        const trueRows = axis === 'read'
                          ? unfilteredByAudience.get(group.audience)?.read ?? []
                          : unfilteredByAudience.get(group.audience)?.mint ?? []
                        // Emptied only by the Axis filter (grants exist pre-filter) -- render
                        // nothing for this row rather than a misleading empty-state sentence.
                        if (trueRows.length > 0) return null
                        if (!isAdmin) return null
                        return (
                          <div key={axis} className="text-sm text-theme-text-muted">
                            <span className="inline-block px-2 py-0.5 rounded-sm bg-app-card text-xs uppercase mr-2">
                              {axis}
                            </span>
                            {axis === 'mint' ? (
                              <>
                                No mint grants — nobody can issue ingestion keys stamped with
                                this audience.
                              </>
                            ) : (
                              <>
                                No read grants — this audience is unreadable except through the{' '}
                                <code>MICROMEGAS_AUDIENCE_GRANTS</code> env map or a per-key{' '}
                                <code>read_audiences</code> list, neither shown here.
                              </>
                            )}
                          </div>
                        )
                      }
                      const shareable = canShareRow(group.audience, axis)
                      return (
                        <div key={axis}>
                          <div className="flex items-center gap-2 mb-2">
                            <span className="inline-block px-2 py-0.5 rounded-sm bg-app-card text-xs uppercase">
                              {axis}
                            </span>
                            {shareable && (
                              <button
                                onClick={() => openShareDialog(group.audience, axis)}
                                className="text-xs text-accent-link hover:underline inline-flex items-center gap-1"
                              >
                                <Share2 className="w-3 h-3" />+ Share {axis} access
                              </button>
                            )}
                            {axis === 'mint' && !isAdmin && showMintButton && (
                              <button
                                onClick={() => openMintDialog(group.audience)}
                                className="text-xs text-accent-link hover:underline inline-flex items-center gap-1"
                              >
                                <KeyRound className="w-3 h-3" />
                                Mint into this audience
                              </button>
                            )}
                          </div>
                          <div className="flex flex-wrap gap-2">
                            {rows.map((row) => {
                              const isStar = row.selector === '*'
                              const isMine = row.selector === `user:${myEmail}`
                              const canDelete = canDeleteChip(row)
                              return (
                                <div
                                  key={`${row.audience} ${row.axis} ${row.selector}`}
                                  className={`px-2.5 py-1.5 rounded-md border text-xs ${
                                    isStar
                                      ? 'border-red-500/50 bg-red-500/5'
                                      : 'border-theme-border bg-app-bg'
                                  }`}
                                >
                                  <div className="flex items-center gap-1.5">
                                    <span className="font-mono text-theme-text-primary">
                                      {row.selector}
                                    </span>
                                    {isStar && (
                                      <span className="text-red-400">
                                        any authenticated principal
                                      </span>
                                    )}
                                    {isMine && (
                                      <span className="px-1 rounded-sm bg-accent-link/20 text-accent-link">
                                        you
                                      </span>
                                    )}
                                    {canDelete && (
                                      <button
                                        onClick={() => openDeleteDialog(row)}
                                        aria-label={`${
                                          isMine ? 'Remove my access' : 'Revoke'
                                        } ${row.audience} ${row.axis} ${row.selector}`}
                                        className="ml-1 text-theme-text-muted hover:text-red-400"
                                      >
                                        ×
                                      </button>
                                    )}
                                  </div>
                                  <div className="text-theme-text-muted mt-0.5">
                                    {row.createdBy} · {formatDate(row.createdAt)}
                                  </div>
                                </div>
                              )
                            })}
                          </div>
                        </div>
                      )
                    })}
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      </PageLayout>
    </AuthGuard>
  )
}

export default function AudienceAccessPage() {
  return (
    <Suspense
      fallback={
        <AuthGuard>
          <PageLayout>
            <div className="p-6">
              <div className="flex items-center justify-center h-64">
                <div className="animate-spin rounded-full h-8 w-8 border-2 border-accent-link border-t-transparent" />
              </div>
            </div>
          </PageLayout>
        </AuthGuard>
      }
    >
      <AudienceAccessPageContent />
    </Suspense>
  )
}
