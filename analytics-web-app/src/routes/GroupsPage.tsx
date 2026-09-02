import { Suspense, useCallback, useEffect, useMemo, useState } from 'react'
import { usePageTitle } from '@/hooks/usePageTitle'
import { Users, Plus, X, ShieldAlert } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { ErrorBanner } from '@/components/ErrorBanner'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import { Button } from '@/components/ui/button'
import {
  GROUP_NAME_PATTERN,
  GroupsError,
  addGroupMember,
  createGroup,
  deleteGroup,
  fetchGroupMembers,
  fetchGroups,
  removeGroupMember,
  type GroupMember,
  type GroupSummary,
} from '@/lib/groups-api'

function formatDate(iso: string): string {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  return d.toLocaleString()
}

// ---------------------------------------------------------------------------
// New group dialog
// ---------------------------------------------------------------------------

interface NewGroupDialogProps {
  open: boolean
  onClose: () => void
  onCreated: (group: GroupSummary) => void
}

function NewGroupDialog({ open, onClose, onCreated }: NewGroupDialogProps) {
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [isSubmitting, setIsSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (open) {
        setName('')
        setDescription('')
        setError(null)
      }
    })()
  }, [open])

  if (!open) return null

  const nameValid = GROUP_NAME_PATTERN.test(name)
  const canSubmit = !isSubmitting && nameValid

  const handleSubmit = async () => {
    setIsSubmitting(true)
    setError(null)
    try {
      const group = await createGroup(name, description || undefined)
      onCreated(group)
      onClose()
    } catch (err) {
      setError(err instanceof GroupsError ? err.message : 'Failed to create group')
    } finally {
      setIsSubmitting(false)
    }
  }

  return (
    <div role="dialog" aria-modal="true" className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/50" onClick={isSubmitting ? undefined : onClose} />
      <div className="relative w-full max-w-md bg-app-panel border border-theme-border rounded-lg shadow-xl">
        <div className="px-4 py-3 border-b border-theme-border flex items-center justify-between">
          <h2 className="text-lg font-medium text-theme-text-primary font-mono">New group</h2>
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
          <div>
            <label className="block text-sm font-medium text-theme-text-secondary mb-1">
              Name
            </label>
            <input
              type="text"
              className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm font-mono text-theme-text-primary placeholder:text-theme-text-muted outline-hidden focus:border-accent-link"
              placeholder="eng"
              value={name}
              onChange={(e) => setName(e.target.value)}
              autoFocus
            />
            <p className="mt-1 text-xs text-theme-text-muted">
              Must match <code>[A-Za-z0-9_-]</code>, up to 255 characters.
            </p>
          </div>
          <div>
            <label className="block text-sm font-medium text-theme-text-secondary mb-1">
              Description <span className="text-theme-text-muted">(optional)</span>
            </label>
            <input
              type="text"
              className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm text-theme-text-primary placeholder:text-theme-text-muted outline-hidden focus:border-accent-link"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>
        </div>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-theme-border">
          <Button variant="outline" onClick={onClose} disabled={isSubmitting}>
            Cancel
          </Button>
          <Button onClick={handleSubmit} disabled={!canSubmit}>
            {isSubmitting ? 'Creating...' : 'Create group'}
          </Button>
        </div>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Add member dialog -- kind toggle mirrors AudienceAccessPage's GrantDialog.
// ---------------------------------------------------------------------------

type MemberKind = 'everyone' | 'user' | 'group'

interface AddMemberDialogProps {
  open: boolean
  groupName: string
  otherGroups: string[]
  onClose: () => void
  onAdded: () => void
}

function AddMemberDialog({ open, groupName, otherGroups, onClose, onAdded }: AddMemberDialogProps) {
  const [kind, setKind] = useState<MemberKind>('user')
  const [idInput, setIdInput] = useState('')
  const [isSubmitting, setIsSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (open) {
        setKind('user')
        setIdInput('')
        setError(null)
      }
    })()
  }, [open])

  if (!open) return null

  const member = kind === 'everyone' ? '*' : `${kind}:${idInput}`
  const canSubmit = !isSubmitting && (kind === 'everyone' || idInput.length > 0)

  const handleSubmit = async () => {
    setIsSubmitting(true)
    setError(null)
    try {
      await addGroupMember(groupName, member)
      onAdded()
      onClose()
    } catch (err) {
      if (err instanceof GroupsError && err.status === 404) {
        setError(`Group "${idInput}" does not exist.`)
      } else if (err instanceof GroupsError && err.status === 409) {
        setError('Adding this member would create a cycle.')
      } else {
        setError(err instanceof GroupsError ? err.message : 'Failed to add member')
      }
    } finally {
      setIsSubmitting(false)
    }
  }

  return (
    <div role="dialog" aria-modal="true" className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/50" onClick={isSubmitting ? undefined : onClose} />
      <div className="relative w-full max-w-md bg-app-panel border border-theme-border rounded-lg shadow-xl">
        <div className="px-4 py-3 border-b border-theme-border flex items-center justify-between">
          <h2 className="text-lg font-medium text-theme-text-primary font-mono">
            Add member to `{groupName}`
          </h2>
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
          <div>
            <label className="block text-sm font-medium text-theme-text-secondary mb-1">
              Kind
            </label>
            <div className="inline-flex rounded-md border border-theme-border overflow-hidden">
              {(['everyone', 'user', 'group'] as MemberKind[]).map((k) => (
                <button
                  key={k}
                  type="button"
                  onClick={() => {
                    setKind(k)
                    setIdInput('')
                  }}
                  className={`px-3 py-1.5 text-sm capitalize ${
                    kind === k
                      ? 'bg-accent-link text-white'
                      : 'bg-app-bg text-theme-text-secondary hover:bg-app-card'
                  }`}
                >
                  {k}
                </button>
              ))}
            </div>
          </div>
          {kind === 'user' && (
            <div>
              <label className="block text-sm font-medium text-theme-text-secondary mb-1">
                Email
              </label>
              <input
                type="text"
                className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm font-mono text-theme-text-primary placeholder:text-theme-text-muted outline-hidden focus:border-accent-link"
                placeholder="alice@example.com"
                value={idInput}
                onChange={(e) => setIdInput(e.target.value)}
                autoFocus
              />
            </div>
          )}
          {kind === 'group' && (
            <div>
              <label className="block text-sm font-medium text-theme-text-secondary mb-1">
                Nested group
              </label>
              <select
                className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm font-mono text-theme-text-primary outline-hidden focus:border-accent-link"
                value={idInput}
                onChange={(e) => setIdInput(e.target.value)}
              >
                <option value="">Select a group…</option>
                {otherGroups.map((g) => (
                  <option key={g} value={g}>
                    {g}
                  </option>
                ))}
              </select>
              <p className="mt-1 text-xs text-theme-text-muted">
                Every member of the selected group becomes a member of `{groupName}`.
              </p>
            </div>
          )}
          <p className="text-xs font-mono text-theme-text-secondary">{member}</p>
        </div>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-theme-border">
          <Button variant="outline" onClick={onClose} disabled={isSubmitting}>
            Cancel
          </Button>
          <Button onClick={handleSubmit} disabled={!canSubmit}>
            {isSubmitting ? 'Adding...' : 'Add member'}
          </Button>
        </div>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// GroupsPage
// ---------------------------------------------------------------------------

function GroupsPageContent() {
  usePageTitle('Groups')

  const [groups, setGroups] = useState<GroupSummary[]>([])
  const [isLoading, setIsLoading] = useState(true)
  const [listError, setListError] = useState<string | null>(null)

  const [selected, setSelected] = useState<string | null>(null)
  const [members, setMembers] = useState<GroupMember[]>([])
  const [membersLoading, setMembersLoading] = useState(false)
  const [membersError, setMembersError] = useState<string | null>(null)

  const [newGroupOpen, setNewGroupOpen] = useState(false)
  const [addMemberOpen, setAddMemberOpen] = useState(false)
  const [deleteGroupTarget, setDeleteGroupTarget] = useState<string | null>(null)
  const [isDeletingGroup, setIsDeletingGroup] = useState(false)
  const [deleteGroupError, setDeleteGroupError] = useState<string | null>(null)
  const [removeMemberTarget, setRemoveMemberTarget] = useState<string | null>(null)
  const [isRemovingMember, setIsRemovingMember] = useState(false)
  const [removeMemberError, setRemoveMemberError] = useState<string | null>(null)

  const loadGroups = useCallback(async () => {
    setIsLoading(true)
    setListError(null)
    try {
      const rows = await fetchGroups()
      setGroups(rows)
    } catch (err) {
      setListError(err instanceof GroupsError ? err.message : 'Failed to load groups')
    } finally {
      setIsLoading(false)
    }
  }, [])

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (async () => {
      await loadGroups()
    })()
  }, [loadGroups])

  const loadMembers = useCallback(async (name: string) => {
    setMembersLoading(true)
    setMembersError(null)
    try {
      const rows = await fetchGroupMembers(name)
      setMembers(rows)
    } catch (err) {
      setMembersError(err instanceof GroupsError ? err.message : 'Failed to load members')
    } finally {
      setMembersLoading(false)
    }
  }, [])

  const selectGroup = useCallback(
    (name: string) => {
      setSelected(name)
      void loadMembers(name)
    },
    [loadMembers]
  )

  // The wildcard warning: unmissable while the `admins` group's own members include `*`.
  const admins = groups.find((g) => g.name === 'admins')
  const [wildcardAdmin, setWildcardAdmin] = useState(false)
  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (async () => {
      if (!admins) {
        setWildcardAdmin(false)
        return
      }
      try {
        const rows = await fetchGroupMembers('admins')
        setWildcardAdmin(rows.some((r) => r.member === '*'))
      } catch {
        setWildcardAdmin(false)
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [admins?.member_count])

  const otherGroups = useMemo(
    () => groups.map((g) => g.name).filter((n) => n !== selected),
    [groups, selected]
  )

  const handleDeleteGroup = async () => {
    if (!deleteGroupTarget) return
    setIsDeletingGroup(true)
    setDeleteGroupError(null)
    try {
      await deleteGroup(deleteGroupTarget)
      setDeleteGroupTarget(null)
      if (selected === deleteGroupTarget) {
        setSelected(null)
        setMembers([])
      }
      void loadGroups()
    } catch (err) {
      setDeleteGroupError(err instanceof GroupsError ? err.message : 'Failed to delete group')
    } finally {
      setIsDeletingGroup(false)
    }
  }

  const handleRemoveMember = async () => {
    if (!selected || !removeMemberTarget) return
    setIsRemovingMember(true)
    setRemoveMemberError(null)
    try {
      await removeGroupMember(selected, removeMemberTarget)
      setRemoveMemberTarget(null)
      void loadMembers(selected)
      void loadGroups()
    } catch (err) {
      setRemoveMemberError(err instanceof GroupsError ? err.message : 'Failed to remove member')
    } finally {
      setIsRemovingMember(false)
    }
  }

  return (
    <AuthGuard requireAdmin>
      <PageLayout onRefresh={loadGroups}>
        <div className="p-6 flex flex-col h-full">
          <div className="flex items-center gap-1.5 text-sm text-theme-text-muted mb-4">
            <AppLink href="/admin" className="text-accent-link hover:underline">
              Admin
            </AppLink>
            <span>/</span>
            <span>Groups</span>
          </div>

          <div className="flex items-center justify-between mb-6 gap-4 flex-wrap">
            <div>
              <h1 className="text-2xl font-semibold text-theme-text-primary">Groups</h1>
              <p className="mt-1 text-theme-text-secondary">
                Local group membership: who is nested in each group, and who is an admin.
              </p>
            </div>
            <Button onClick={() => setNewGroupOpen(true)} className="gap-1.5">
              <Plus className="w-4 h-4" />
              New group
            </Button>
          </div>

          {wildcardAdmin && (
            <ErrorBanner
              variant="warning"
              title="Every authenticated caller is an admin"
              message={
                'The `admins` group currently includes `*` (everyone). Add a `user:` member for ' +
                'yourself, then remove `*`, to restrict admin access.'
              }
            />
          )}

          {listError && (
            <ErrorBanner title="Failed to load groups" message={listError} onRetry={loadGroups} />
          )}

          <NewGroupDialog
            open={newGroupOpen}
            onClose={() => setNewGroupOpen(false)}
            onCreated={() => void loadGroups()}
          />

          {selected && (
            <AddMemberDialog
              open={addMemberOpen}
              groupName={selected}
              otherGroups={otherGroups}
              onClose={() => setAddMemberOpen(false)}
              onAdded={() => {
                void loadMembers(selected)
                void loadGroups()
              }}
            />
          )}

          <ConfirmDialog
            isOpen={deleteGroupTarget !== null}
            onClose={() => {
              setDeleteGroupTarget(null)
              setDeleteGroupError(null)
            }}
            onConfirm={handleDeleteGroup}
            title="Delete group"
            message={`Delete group \`${deleteGroupTarget}\`? This only succeeds if it is not referenced by any nested membership or audience grant.`}
            confirmLabel="Delete"
            isLoading={isDeletingGroup}
            variant="danger"
            error={deleteGroupError}
          />

          <ConfirmDialog
            isOpen={removeMemberTarget !== null}
            onClose={() => {
              setRemoveMemberTarget(null)
              setRemoveMemberError(null)
            }}
            onConfirm={handleRemoveMember}
            title="Remove member"
            message={`Remove \`${removeMemberTarget}\` from \`${selected}\`?`}
            confirmLabel="Remove"
            isLoading={isRemovingMember}
            variant="danger"
            error={removeMemberError}
          />

          {isLoading ? (
            <div className="flex-1 flex items-center justify-center">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
                <span className="text-theme-text-secondary">Loading groups…</span>
              </div>
            </div>
          ) : groups.length === 0 ? (
            <div className="flex-1 flex flex-col items-center justify-center text-center">
              <Users className="w-10 h-10 text-theme-text-muted opacity-40 mb-3" />
              <p className="text-theme-text-muted mb-4 max-w-md">No groups yet.</p>
            </div>
          ) : (
            <div className="grid grid-cols-1 lg:grid-cols-2 gap-4 flex-1 min-h-0">
              <div className="border border-theme-border rounded-lg overflow-y-auto">
                <table className="w-full border-collapse">
                  <thead className="bg-app-panel sticky top-0">
                    <tr>
                      <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                        Name
                      </th>
                      <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                        Members
                      </th>
                      <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                        Created
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {groups.map((g) => (
                      <tr
                        key={g.name}
                        onClick={() => selectGroup(g.name)}
                        className={`border-t border-theme-border cursor-pointer hover:bg-accent-link/5 ${
                          selected === g.name ? 'bg-accent-link/10' : ''
                        }`}
                      >
                        <td className="p-2.5 px-4">
                          <span className="font-mono text-sm text-theme-text-primary">
                            {g.name}
                          </span>
                          {g.name === 'admins' && (
                            <ShieldAlert className="inline-block w-3.5 h-3.5 ml-1.5 text-accent-warning" />
                          )}
                          {g.description && (
                            <div className="text-xs text-theme-text-muted">{g.description}</div>
                          )}
                        </td>
                        <td className="p-2.5 px-4 text-sm text-theme-text-secondary">
                          {g.member_count}
                        </td>
                        <td className="p-2.5 px-4 text-xs text-theme-text-muted">
                          {formatDate(g.created_at)} · {g.created_by}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>

              <div className="border border-theme-border rounded-lg overflow-y-auto p-4">
                {!selected ? (
                  <p className="text-theme-text-muted text-sm">Select a group to view members.</p>
                ) : (
                  <>
                    <div className="flex items-center justify-between mb-3">
                      <h2 className="font-mono text-sm text-theme-text-primary">{selected}</h2>
                      <div className="flex gap-2">
                        <Button
                          size="sm"
                          variant="outline"
                          onClick={() => setAddMemberOpen(true)}
                          className="gap-1"
                        >
                          <Plus className="w-3.5 h-3.5" />
                          Add member
                        </Button>
                        {selected !== 'admins' && (
                          <Button
                            size="sm"
                            variant="outline"
                            onClick={() => setDeleteGroupTarget(selected)}
                          >
                            Delete group
                          </Button>
                        )}
                      </div>
                    </div>
                    {membersError && (
                      <ErrorBanner
                        title="Failed to load members"
                        message={membersError}
                        onRetry={() => void loadMembers(selected)}
                      />
                    )}
                    {membersLoading ? (
                      <div className="flex items-center gap-3 py-4">
                        <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
                        <span className="text-sm text-theme-text-secondary">
                          Loading members…
                        </span>
                      </div>
                    ) : members.length === 0 ? (
                      <p className="text-theme-text-muted text-sm">No members yet.</p>
                    ) : (
                      <div className="flex flex-wrap gap-2">
                        {members.map((m) => {
                          const isStar = m.member === '*'
                          const nestedGroup = m.member.startsWith('group:')
                            ? m.member.slice('group:'.length)
                            : null
                          return (
                            <div
                              key={m.member}
                              className={`px-2.5 py-1.5 rounded-md border text-xs ${
                                isStar
                                  ? 'border-red-500/50 bg-red-500/5'
                                  : 'border-theme-border bg-app-bg'
                              }`}
                            >
                              <div className="flex items-center gap-1.5">
                                {nestedGroup ? (
                                  <button
                                    onClick={() => selectGroup(nestedGroup)}
                                    className="font-mono text-accent-link hover:underline"
                                  >
                                    {m.member}
                                  </button>
                                ) : (
                                  <span className="font-mono text-theme-text-primary">
                                    {m.member}
                                  </span>
                                )}
                                {isStar && (
                                  <span className="text-red-400">any authenticated principal</span>
                                )}
                                <button
                                  onClick={() => setRemoveMemberTarget(m.member)}
                                  aria-label={`Remove ${m.member} from ${selected}`}
                                  className="ml-1 text-theme-text-muted hover:text-red-400"
                                >
                                  ×
                                </button>
                              </div>
                              <div className="text-theme-text-muted mt-0.5">
                                {m.created_by} · {formatDate(m.created_at)}
                              </div>
                            </div>
                          )
                        })}
                      </div>
                    )}
                  </>
                )}
              </div>
            </div>
          )}
        </div>
      </PageLayout>
    </AuthGuard>
  )
}

export default function GroupsPage() {
  return (
    <Suspense
      fallback={
        <AuthGuard requireAdmin>
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
      <GroupsPageContent />
    </Suspense>
  )
}
