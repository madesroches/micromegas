import { Suspense, useCallback, useEffect, useState } from 'react'
import { usePageTitle } from '@/hooks/usePageTitle'
import { Ban, ShieldBan } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { ErrorBanner } from '@/components/ErrorBanner'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import { Button } from '@/components/ui/button'
import { DataSourceField } from '@/components/DataSourceSelector'
import { DocumentationLink, QUERY_AUDIT_LOG_URL, QUERY_DENY_LIST_FUNCTIONS_URL } from '@/components/DocumentationLink'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { useDataSourceState } from '@/hooks/useDataSourceState'
import {
  buildDenyQueriesSql,
  buildListQueryDenialsSql,
  buildRemoveQueryDenialSql,
  decodeQueryDenyRules,
  extractRemoveResult,
  extractRuleId,
  formatRelativeTime,
  type QueryDenyRule,
} from '@/lib/query-deny-list-api'

// Insert-chips for the common predicates the plan's examples build on (§3) -- a textarea rather
// than a field grid, since the expression *is* the rule and a grid can only ever express the
// AND-of-equalities subset. Each chip inserts a single-quoted fragment; the textarea holds the
// expression in its natural form, and `escapeSqlLiteral` (in the SQL builder) doubles those
// quotes on the way out.
const INSERT_CHIPS: { label: string; snippet: string }[] = [
  { label: 'sql_hash', snippet: "sql_hash = ''" },
  { label: 'user_id', snippet: "user_id = ''" },
  { label: 'email', snippet: "email = ''" },
  { label: 'client', snippet: "client = ''" },
  { label: 'entrypoint', snippet: "entrypoint = ''" },
  { label: 'notebook', snippet: "notebook = ''" },
  { label: 'client_ip', snippet: "client_ip = ''" },
  { label: 'sql LIKE', snippet: "sql LIKE '%%'" },
]

function formatDate(date: Date | null): string {
  if (!date) return '—'
  return date.toLocaleString()
}

function DenyQueryDialog({
  isOpen,
  onClose,
  onSubmit,
  isSubmitting,
  error,
}: {
  isOpen: boolean
  onClose: () => void
  onSubmit: (matchExpr: string, reason: string) => void
  isSubmitting: boolean
  error: string | null
}) {
  const [matchExpr, setMatchExpr] = useState('')
  const [reason, setReason] = useState('')

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (isOpen) {
        setMatchExpr('')
        setReason('')
      }
    })()
  }, [isOpen])

  if (!isOpen) return null

  const insertChip = (snippet: string) => {
    setMatchExpr((current) => (current.trim() ? `${current} AND ${snippet}` : snippet))
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/50" onClick={isSubmitting ? undefined : onClose} />
      <div className="relative w-full max-w-lg bg-app-panel border border-theme-border rounded-lg shadow-xl">
        <div className="px-4 py-3 border-b border-theme-border">
          <h2 className="text-lg font-medium text-theme-text-primary">Deny a Query</h2>
        </div>
        <div className="p-4 space-y-4">
          {error && (
            <div className="p-3 bg-accent-error/10 border border-accent-error/30 rounded-lg text-sm text-accent-error font-mono whitespace-pre-wrap">
              {error}
            </div>
          )}
          <div>
            <label className="block text-sm font-medium text-theme-text-secondary mb-1">
              Match expression
            </label>
            <textarea
              className="w-full h-24 bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm font-mono text-theme-text-primary placeholder:text-theme-text-muted outline-hidden focus:border-accent-link"
              placeholder="sql_hash = '9f2c41ab73de0155' AND entrypoint = 'grafana-alert'"
              value={matchExpr}
              onChange={(e) => setMatchExpr(e.target.value)}
              autoFocus
            />
            <div className="mt-2 flex flex-wrap gap-1.5">
              {INSERT_CHIPS.map((chip) => (
                <button
                  key={chip.label}
                  type="button"
                  onClick={() => insertChip(chip.snippet)}
                  className="px-2 py-1 text-xs rounded-sm border border-theme-border text-theme-text-secondary hover:bg-app-card transition-colors font-mono"
                >
                  {chip.label}
                </button>
              ))}
            </div>
            <DocumentationLink
              url={QUERY_DENY_LIST_FUNCTIONS_URL}
              label="match context & expression reference"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-theme-text-secondary mb-1">
              Reason (required)
            </label>
            <input
              type="text"
              className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm text-theme-text-primary placeholder:text-theme-text-muted outline-hidden focus:border-accent-link"
              placeholder="alert rule re-firing on failure; owner notified"
              value={reason}
              onChange={(e) => setReason(e.target.value)}
            />
          </div>
        </div>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-theme-border">
          <Button variant="outline" onClick={onClose} disabled={isSubmitting}>
            Cancel
          </Button>
          <Button
            onClick={() => onSubmit(matchExpr, reason)}
            disabled={isSubmitting || !matchExpr.trim() || !reason.trim()}
          >
            {isSubmitting ? (
              <span className="flex items-center gap-2">
                <span className="w-4 h-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
                Denying...
              </span>
            ) : (
              'Deny query'
            )}
          </Button>
        </div>
      </div>
    </div>
  )
}

function QueryDenyListPageContent() {
  usePageTitle('Query Deny List')

  const { dataSource, setDataSource, error: dataSourceError } = useDataSourceState()

  const [rules, setRules] = useState<QueryDenyRule[]>([])
  const [listError, setListError] = useState<string | null>(null)
  const [showDenyDialog, setShowDenyDialog] = useState(false)
  const [denyError, setDenyError] = useState<string | null>(null)
  // True only while a denyQuery.execute() call from *this* dialog session is outstanding or has
  // just completed and not yet been consumed -- guards the completion effect below against
  // reading denyQuery's leftover isComplete/error/getTable() from a previous, already-closed
  // submission when the dialog is reopened (see query_deny_list_plan.md review notes).
  const [denySubmitted, setDenySubmitted] = useState(false)
  const [removeTarget, setRemoveTarget] = useState<QueryDenyRule | null>(null)
  const [removeError, setRemoveError] = useState<string | null>(null)

  const listQuery = useStreamQuery()
  // Two independent instances -- deny and remove can each be mid-flight in their own dialog
  // without one's completion effect misreading the other's result.
  const denyQuery = useStreamQuery()
  const removeQuery = useStreamQuery()

  const loadRules = useCallback(() => {
    if (!dataSource) return
    setListError(null)
    listQuery.execute({ sql: buildListQueryDenialsSql(), dataSource })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dataSource])

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      loadRules()
    })()
  }, [loadRules])

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (listQuery.isComplete) {
        if (listQuery.error) {
          setListError(listQuery.error.message)
        } else {
          const table = listQuery.getTable()
          setRules(table ? decodeQueryDenyRules(table) : [])
        }
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [listQuery.isComplete, listQuery.error])

  const handleDeny = useCallback(
    async (matchExpr: string, reason: string) => {
      setDenyError(null)
      setDenySubmitted(true)
      await denyQuery.execute({
        sql: buildDenyQueriesSql(matchExpr, reason),
        dataSource,
      })
    },
    [denyQuery, dataSource]
  )

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (!showDenyDialog || !denySubmitted || !denyQuery.isComplete) return
      setDenySubmitted(false)
      if (denyQuery.error) {
        setDenyError(denyQuery.error.message)
      } else {
        const table = denyQuery.getTable()
        if (table && extractRuleId(table)) {
          setShowDenyDialog(false)
          loadRules()
        }
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showDenyDialog, denySubmitted, denyQuery.isComplete, denyQuery.error])

  const handleRemove = useCallback(async () => {
    if (!removeTarget) return
    setRemoveError(null)
    await removeQuery.execute({
      sql: buildRemoveQueryDenialSql(removeTarget.ruleId),
      dataSource,
    })
  }, [removeTarget, removeQuery, dataSource])

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (!removeTarget || !removeQuery.isComplete) return
      if (removeQuery.error) {
        setRemoveError(removeQuery.error.message)
        return
      }
      // remove_query_denial reports failure in-band, as a successful row whose value is
      // "ERROR: ..." rather than a stream error -- inspect it before treating the call as done.
      const table = removeQuery.getTable()
      const result = table ? extractRemoveResult(table) : null
      if (result && result.startsWith('SUCCESS')) {
        setRemoveTarget(null)
        loadRules()
      } else {
        setRemoveError(result ?? 'remove_query_denial returned no result')
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [removeQuery.isComplete, removeQuery.error])

  const openDenyDialog = () => {
    setDenyError(null)
    setShowDenyDialog(true)
  }

  const openRemoveDialog = (rule: QueryDenyRule) => {
    setRemoveError(null)
    setRemoveTarget(rule)
  }

  return (
    <AuthGuard requireAdmin>
      <PageLayout onRefresh={loadRules}>
        <div className="p-6 flex flex-col h-full">
          <div className="flex items-center gap-1.5 text-sm text-theme-text-muted mb-4">
            <AppLink href="/admin" className="text-accent-link hover:underline">
              Admin
            </AppLink>
            <span>/</span>
            <span>Query Deny List</span>
          </div>

          <div className="flex items-center justify-between mb-6">
            <div>
              <h1 className="text-2xl font-semibold text-theme-text-primary">Query Deny List</h1>
              <p className="mt-1 text-theme-text-secondary">
                Reject matching queries at the front of the FlightSQL service, before any work is
                spent on them.
              </p>
            </div>
            <Button onClick={openDenyDialog} className="gap-1.5">
              <Ban className="w-4 h-4" />
              Deny a Query
            </Button>
          </div>

          <DataSourceField value={dataSource} onChange={setDataSource} />

          {dataSourceError && <ErrorBanner title="Data source error" message={dataSourceError} />}
          {listError && (
            <ErrorBanner
              title="Failed to load rules"
              message={listError}
              onRetry={loadRules}
            />
          )}

          <DenyQueryDialog
            isOpen={showDenyDialog}
            onClose={() => setShowDenyDialog(false)}
            onSubmit={handleDeny}
            isSubmitting={denyQuery.isStreaming}
            error={denyError}
          />

          <ConfirmDialog
            isOpen={removeTarget !== null}
            onClose={() => {
              setRemoveTarget(null)
              setRemoveError(null)
            }}
            onConfirm={handleRemove}
            title="Remove Query Denial"
            message={`Remove the rule "${removeTarget?.reason ?? ''}"? Queries matching this expression will be allowed through again.`}
            confirmLabel="Remove"
            isLoading={removeQuery.isStreaming}
            variant="danger"
            error={removeError}
          />

          {listQuery.isStreaming && rules.length === 0 ? (
            <div className="flex-1 flex items-center justify-center">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
                <span className="text-theme-text-secondary">Loading rules...</span>
              </div>
            </div>
          ) : rules.length === 0 ? (
            <div className="flex-1 flex flex-col items-center justify-center text-center">
              <ShieldBan className="w-10 h-10 text-theme-text-muted opacity-40 mb-3" />
              <p className="text-theme-text-muted mb-2">No queries are currently denied.</p>
              <p className="text-theme-text-muted text-sm mb-4 max-w-md">
                To deny an offending query, find its fingerprint in the audit log first.
              </p>
              <DocumentationLink url={QUERY_AUDIT_LOG_URL} label="query audit log" />
            </div>
          ) : (
            <div className="border border-theme-border rounded-lg overflow-hidden overflow-x-auto">
              <table className="w-full border-collapse">
                <thead className="bg-app-panel">
                  <tr>
                    <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                      Match expression
                    </th>
                    <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                      Reason
                    </th>
                    <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                      Created by
                    </th>
                    <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                      Created
                    </th>
                    <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                      Last hit
                    </th>
                    <th className="text-right p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                      Actions
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {rules.map((rule) => (
                    <tr key={rule.ruleId} className="border-t border-theme-border hover:bg-accent-link/5">
                      <td className="p-2.5 px-4 font-mono text-xs text-theme-text-primary max-w-md break-all">
                        {rule.matchExpr}
                      </td>
                      <td className="p-2.5 px-4 text-theme-text-secondary text-sm">{rule.reason}</td>
                      <td className="p-2.5 px-4 text-theme-text-secondary text-sm">{rule.createdBy}</td>
                      <td className="p-2.5 px-4 text-theme-text-secondary text-sm whitespace-nowrap">
                        {formatDate(rule.createdAt)}
                      </td>
                      <td className="p-2.5 px-4 text-theme-text-secondary text-sm whitespace-nowrap">
                        {formatRelativeTime(rule.lastHitAt)}
                      </td>
                      <td className="p-2.5 px-4 text-right">
                        <button
                          onClick={() => openRemoveDialog(rule)}
                          className="p-1.5 rounded-sm text-theme-text-muted hover:text-red-400 hover:bg-red-400/10 transition-colors"
                          title="Remove"
                          aria-label={`Remove rule ${rule.ruleId}`}
                        >
                          <Ban className="w-4 h-4" />
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      </PageLayout>
    </AuthGuard>
  )
}

export default function QueryDenyListPage() {
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
      <QueryDenyListPageContent />
    </Suspense>
  )
}
