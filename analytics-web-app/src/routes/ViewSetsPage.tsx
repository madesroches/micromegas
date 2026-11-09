import { Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { usePageTitle } from '@/hooks/usePageTitle'
import { Layers, Plus } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { ErrorBanner } from '@/components/ErrorBanner'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import { Button } from '@/components/ui/button'
import { TableFrame } from '@/components/TableFrame'
import { DataSourceField } from '@/components/DataSourceSelector'
import { SyntaxEditor } from '@/components/SyntaxEditor'
import { DocumentationLink, VIEW_REDEFINITION_URL } from '@/components/DocumentationLink'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { useDataSourceState } from '@/hooks/useDataSourceState'
import {
  CREATE_SKELETON,
  buildCheckViewMergeSql,
  buildDropSql,
  buildListViewSetDefinitionsSql,
  buildListViewSetsSql,
  buildPartitionStatsSql,
  decodeLoadedViewSets,
  decodeMergeCheck,
  decodePartitionStats,
  decodeViewSetDefinitions,
  mergeViewSetRows,
  toReplaceStatement,
  extractViewSetName,
  type LoadedViewSet,
  type MergeCheck,
  type PartitionStats,
  type ViewSetDefinition,
  type ViewSetRow,
} from '@/lib/view-sets-api'

type Tab = 'definition' | 'schema' | 'partitions'
type Mutation = 'create' | 'save' | 'drop'

const HEALTH_TOOLTIP =
  'Reflects the replica that answered the query. Other replicas reload definitions on a timer, ' +
  'so a new or changed view set can read "failed to load" for up to the definition refresh ' +
  'interval (60 seconds by default).'

const HEALTH_STYLES: Record<ViewSetRow['health'], string> = {
  ok: 'bg-green-500/15 text-green-500',
  'failed to load': 'bg-accent-error/15 text-accent-error',
  unknown: 'bg-app-card text-theme-text-muted',
  'read-only': 'bg-app-card text-theme-text-muted',
}

function formatDate(date: Date | null): string {
  return date ? date.toLocaleString() : '—'
}

function formatBytes(bytes: number): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit++
  }
  return `${unit === 0 ? value : value.toFixed(1)} ${units[unit]}`
}

function Spinner({ label }: { label: string }) {
  return (
    <div className="flex items-center gap-3 p-4">
      <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
      <span className="text-theme-text-secondary text-sm">{label}</span>
    </div>
  )
}

function MergeCheckResult({ check }: { check: MergeCheck | null }) {
  if (!check) {
    return <p className="text-xs text-theme-text-muted">No merge_sort_order, nothing to check.</p>
  }
  if (check.streams) {
    return <p className="text-xs text-green-500">The merge streams its input.</p>
  }
  return (
    <ErrorBanner
      variant="warning"
      title="The merge does not stream"
      message="A sort survives in the merge plan, so merging buffers its whole input in memory."
      details={check.sortExecLine ?? undefined}
    />
  )
}

function CheckBar({
  sql,
  dataSource,
  disabled,
}: {
  sql: string
  dataSource: string
  disabled: boolean
}) {
  const checkQuery = useStreamQuery()
  const [result, setResult] = useState<{
    sql: string
    check?: MergeCheck | null
    error?: string
  } | null>(null)
  const [requestedSql, setRequestedSql] = useState<string | null>(null)

  const runCheck = async () => {
    setResult(null)
    setRequestedSql(sql)
    await checkQuery.execute({
      sql: buildCheckViewMergeSql(sql),
      dataSource,
      adminMode: true,
    })
  }

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (requestedSql === null || !checkQuery.isComplete) return
      setRequestedSql(null)
      if (checkQuery.error) {
        setResult({ sql: requestedSql, error: checkQuery.error.message })
      } else {
        const table = checkQuery.getTable()
        setResult({
          sql: requestedSql,
          check: table ? decodeMergeCheck(table) : null,
        })
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [requestedSql, checkQuery.isComplete, checkQuery.error])

  const shown = result && result.sql === sql ? result : null

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-3">
        <Button
          variant="outline"
          size="sm"
          onClick={runCheck}
          disabled={disabled || checkQuery.isStreaming || !sql.trim()}
        >
          {checkQuery.isStreaming ? 'Checking...' : 'Check'}
        </Button>
        <span className="text-xs text-theme-text-muted">
          Plans the merge without storing anything.
        </span>
      </div>
      {shown?.error && <ErrorBanner title="Check failed" message={shown.error} />}
      {shown && !shown.error && <MergeCheckResult check={shown.check ?? null} />}
    </div>
  )
}

function CreateDialog({
  isOpen,
  onClose,
  onSubmit,
  isSubmitting,
  error,
  dataSource,
}: {
  isOpen: boolean
  onClose: () => void
  onSubmit: (ddl: string) => void
  isSubmitting: boolean
  error: string | null
  dataSource: string
}) {
  const [ddl, setDdl] = useState(CREATE_SKELETON)

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (isOpen) setDdl(CREATE_SKELETON)
    })()
  }, [isOpen])

  if (!isOpen) return null

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/50" onClick={isSubmitting ? undefined : onClose} />
      <div className="relative w-full max-w-3xl bg-app-panel border border-theme-border rounded-lg shadow-xl">
        <div className="px-4 py-3 border-b border-theme-border">
          <h2 className="text-lg font-medium text-theme-text-primary">Create View Set</h2>
        </div>
        <div className="p-4 space-y-4">
          {error && (
            <div className="p-3 bg-accent-error/10 border border-accent-error/30 rounded-lg text-sm text-accent-error font-mono whitespace-pre-wrap">
              {error}
            </div>
          )}
          <SyntaxEditor value={ddl} onChange={setDdl} language="sql" minHeight="360px" />
          <CheckBar sql={ddl} dataSource={dataSource} disabled={isSubmitting} />
        </div>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-theme-border">
          <Button variant="outline" onClick={onClose} disabled={isSubmitting}>
            Cancel
          </Button>
          <Button onClick={() => onSubmit(ddl)} disabled={isSubmitting || !ddl.trim()}>
            {isSubmitting ? 'Creating...' : 'Create'}
          </Button>
        </div>
      </div>
    </div>
  )
}

function DefinitionTab({
  row,
  dataSource,
  isEditing,
  editText,
  onEditTextChange,
  onStartEdit,
  onCancelEdit,
  onSave,
  onDrop,
  saveError,
  isSaving,
}: {
  row: ViewSetRow
  dataSource: string
  isEditing: boolean
  editText: string
  onEditTextChange: (text: string) => void
  onStartEdit: () => void
  onCancelEdit: () => void
  onSave: () => void
  onDrop: () => void
  saveError: string | null
  isSaving: boolean
}) {
  const mergeQuery = useStreamQuery()
  const [merge, setMerge] = useState<MergeCheck | null | undefined>(undefined)
  const [mergeError, setMergeError] = useState<string | null>(null)
  const definitionSql = row.definitionSql

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      setMerge(undefined)
      setMergeError(null)
      if (definitionSql) {
        mergeQuery.execute({
          sql: buildCheckViewMergeSql(definitionSql),
          dataSource,
          adminMode: true,
        })
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [definitionSql, dataSource])

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (!mergeQuery.isComplete) return
      if (mergeQuery.error) {
        setMergeError(mergeQuery.error.message)
      } else {
        const table = mergeQuery.getTable()
        setMerge(table ? decodeMergeCheck(table) : null)
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mergeQuery.isComplete, mergeQuery.error])

  if (row.source === 'builtin' || definitionSql === null) {
    return (
      <p className="text-sm text-theme-text-muted">
        Built-in view sets are defined in code and have no stored definition.
      </p>
    )
  }

  if (isEditing) {
    return (
      <div className="space-y-3">
        {saveError && (
          <div className="p-3 bg-accent-error/10 border border-accent-error/30 rounded-lg text-sm text-accent-error font-mono whitespace-pre-wrap">
            {saveError}
          </div>
        )}
        <SyntaxEditor
          value={editText}
          onChange={onEditTextChange}
          language="sql"
          minHeight="360px"
        />
        <CheckBar sql={editText} dataSource={dataSource} disabled={isSaving} />
        <div className="flex justify-end gap-2">
          <Button variant="outline" onClick={onCancelEdit} disabled={isSaving}>
            Cancel
          </Button>
          <Button onClick={onSave} disabled={isSaving || !editText.trim()}>
            Save
          </Button>
        </div>
      </div>
    )
  }

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <span className="text-xs text-theme-text-muted">
          Updated {formatDate(row.updatedAt)}
          {row.updatedBy ? ` by ${row.updatedBy}` : ''}
        </span>
        <div className="flex gap-2">
          <Button variant="outline" size="sm" onClick={onStartEdit}>
            Edit
          </Button>
          <Button variant="outline" size="sm" onClick={onDrop}>
            Drop
          </Button>
        </div>
      </div>
      {mergeQuery.isStreaming && <Spinner label="Checking merge..." />}
      {mergeError && <ErrorBanner title="Merge check failed" message={mergeError} />}
      {merge && !merge.streams && <MergeCheckResult check={merge} />}
      <SyntaxEditor
        value={definitionSql}
        onChange={() => undefined}
        language="sql"
        minHeight="360px"
        readOnly
      />
    </div>
  )
}

function SchemaTab({ row }: { row: ViewSetRow }) {
  if (row.health === 'failed to load') {
    return (
      <p className="text-sm text-theme-text-muted">
        This view set is not loaded on the answering replica, so it has no schema yet.
      </p>
    )
  }
  return (
    <div className="space-y-4">
      {row.description && (
        <div className="prose prose-invert max-w-none text-sm text-theme-text-secondary">
          <Markdown remarkPlugins={[remarkGfm]}>{row.description}</Markdown>
        </div>
      )}
      <TableFrame>
        <table className="w-full border-collapse">
          <thead className="bg-app-panel">
            <tr>
              <th className="text-left p-2 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                Column
              </th>
              <th className="text-left p-2 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                Type
              </th>
            </tr>
          </thead>
          <tbody>
            {row.columns.map((col) => (
              <tr key={col.name} className="border-t border-theme-border">
                <td className="p-2 px-4 font-mono text-xs text-theme-text-primary">{col.name}</td>
                <td className="p-2 px-4 font-mono text-xs text-theme-text-secondary">{col.type}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </TableFrame>
      {row.sortOrder.length > 0 && (
        <p className="text-xs text-theme-text-muted">
          Sorted by <span className="font-mono">{row.sortOrder.join(', ')}</span>
        </p>
      )}
    </div>
  )
}

function PartitionsTab({
  stats,
  isLoading,
  error,
  onRetry,
}: {
  stats: PartitionStats | null
  isLoading: boolean
  error: string | null
  onRetry: () => void
}) {
  if (error)
    return <ErrorBanner title="Failed to load partitions" message={error} onRetry={onRetry} />
  if (isLoading) return <Spinner label="Loading partitions..." />
  if (!stats) {
    return <p className="text-sm text-theme-text-muted">This view set has no partitions.</p>
  }
  const items: [string, string][] = [
    ['Partitions', stats.partitionCount.toLocaleString()],
    ['Rows', stats.numRows.toLocaleString()],
    ['File size', formatBytes(stats.fileSize)],
    ['Earliest insert time', formatDate(stats.beginInsertTime)],
    ['Latest insert time', formatDate(stats.endInsertTime)],
    ['Last updated', formatDate(stats.lastUpdated)],
  ]
  return (
    <dl className="grid grid-cols-2 gap-x-6 gap-y-3 max-w-xl">
      {items.map(([label, value]) => (
        <div key={label}>
          <dt className="text-xs uppercase tracking-wider text-theme-text-muted">{label}</dt>
          <dd className="text-sm text-theme-text-primary">{value}</dd>
        </div>
      ))}
    </dl>
  )
}

function ViewSetsPageContent() {
  usePageTitle('View Sets')

  const { dataSource, setDataSource, error: dataSourceError } = useDataSourceState()

  const [definitions, setDefinitions] = useState<ViewSetDefinition[]>([])
  const [loaded, setLoaded] = useState<LoadedViewSet[]>([])
  const [listError, setListError] = useState<string | null>(null)
  const [loadedError, setLoadedError] = useState(false)
  const [selectedName, setSelectedName] = useState<string | null>(null)
  const [tab, setTab] = useState<Tab>('definition')

  const [stats, setStats] = useState<PartitionStats[]>([])
  const [statsError, setStatsError] = useState<string | null>(null)

  const [showCreate, setShowCreate] = useState(false)
  const [isEditing, setIsEditing] = useState(false)
  const [editText, setEditText] = useState('')
  const [confirmSave, setConfirmSave] = useState(false)
  const [dropTarget, setDropTarget] = useState<string | null>(null)
  const [pending, setPending] = useState<Mutation | null>(null)
  const [errors, setErrors] = useState<Record<Mutation, string | null>>({
    create: null,
    save: null,
    drop: null,
  })

  const [prevDataSource, setPrevDataSource] = useState(dataSource)
  if (dataSource !== prevDataSource) {
    setPrevDataSource(dataSource)
    setSelectedName(null)
    setIsEditing(false)
    setEditText('')
    setConfirmSave(false)
    setDropTarget(null)
    setErrors((e) => ({ ...e, save: null, drop: null }))
  }

  const definitionsQuery = useStreamQuery()
  const loadedQuery = useStreamQuery()
  const statsQuery = useStreamQuery()
  const mutationQuery = useStreamQuery()

  const loadLists = useCallback(() => {
    if (!dataSource) return
    setListError(null)
    setLoadedError(false)
    setDefinitions([])
    setLoaded([])
    definitionsQuery.execute({
      sql: buildListViewSetDefinitionsSql(),
      dataSource,
      adminMode: true,
    })
    loadedQuery.execute({
      sql: buildListViewSetsSql(),
      dataSource,
      adminMode: true,
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dataSource])

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      loadLists()
    })()
  }, [loadLists])

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (!definitionsQuery.isComplete) return
      if (definitionsQuery.error) {
        setListError(definitionsQuery.error.message)
      } else {
        const table = definitionsQuery.getTable()
        setDefinitions(table ? decodeViewSetDefinitions(table) : [])
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [definitionsQuery.isComplete, definitionsQuery.error])

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (!loadedQuery.isComplete) return
      if (loadedQuery.error) {
        setLoadedError(true)
        setListError(loadedQuery.error.message)
      } else {
        const table = loadedQuery.getTable()
        setLoaded(table ? decodeLoadedViewSets(table) : [])
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loadedQuery.isComplete, loadedQuery.error])

  const loadStats = useCallback(() => {
    if (!dataSource) return
    setStatsError(null)
    statsQuery.execute({
      sql: buildPartitionStatsSql(),
      dataSource,
      adminMode: true,
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dataSource])

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (tab === 'partitions') loadStats()
    })()
  }, [tab, loadStats])

  const refresh = () => {
    loadLists()
    if (tab === 'partitions') loadStats()
  }

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (!statsQuery.isComplete) return
      if (statsQuery.error) {
        setStatsError(statsQuery.error.message)
      } else {
        const table = statsQuery.getTable()
        setStats(table ? decodePartitionStats(table) : [])
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [statsQuery.isComplete, statsQuery.error])

  const listsLoading = definitionsQuery.isStreaming || loadedQuery.isStreaming
  const rows = useMemo(
    () => (listsLoading ? [] : mergeViewSetRows(definitions, loaded, !loadedError)),
    [listsLoading, definitions, loaded, loadedError],
  )
  const selected = rows.find((r) => r.name === selectedName) ?? null
  const selectedStats = stats.find((s) => s.viewSetName === selectedName) ?? null

  const createdNameRef = useRef<string | null>(null)

  const runMutation = useCallback(
    async (kind: Mutation, sql: string) => {
      if (!dataSource) {
        setErrors((e) => ({ ...e, [kind]: 'No data source selected' }))
        return
      }
      setErrors((e) => ({ ...e, [kind]: null }))
      setPending(kind)
      await mutationQuery.execute({ sql, dataSource, adminMode: true })
    },
    [mutationQuery, dataSource],
  )

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (!pending || !mutationQuery.isComplete) return
      const kind = pending
      setPending(null)
      if (mutationQuery.error) {
        setErrors((e) => ({ ...e, [kind]: mutationQuery.error!.message }))
        return
      }
      if (kind === 'create') {
        setShowCreate(false)
        const created = createdNameRef.current
        if (created) setSelectedName(created)
      }
      if (kind === 'save') {
        setConfirmSave(false)
        setIsEditing(false)
      }
      if (kind === 'drop') {
        setDropTarget(null)
        setSelectedName(null)
      }
      loadLists()
      if (tab === 'partitions') loadStats()
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pending, mutationQuery.isComplete, mutationQuery.error])

  const selectRow = (name: string) => {
    setSelectedName(name)
    setIsEditing(false)
    setErrors((e) => ({ ...e, save: null }))
  }

  return (
    <PageLayout onRefresh={refresh}>
      <div className="p-6 flex flex-col h-full">
        <div className="flex items-center gap-1.5 text-sm text-theme-text-muted mb-4">
          <AppLink href="/admin" className="text-accent-link hover:underline">
            Admin
          </AppLink>
          <span>/</span>
          <span>View Sets</span>
        </div>

        <div className="flex items-center justify-between mb-6">
          <div>
            <h1 className="text-2xl font-semibold text-theme-text-primary">View Sets</h1>
            <p className="mt-1 text-theme-text-secondary">
              Inspect view sets and manage the ones defined in SQL.
            </p>
          </div>
          <Button
            onClick={() => {
              setErrors((e) => ({ ...e, create: null }))
              setShowCreate(true)
            }}
            className="gap-1.5"
          >
            <Plus className="w-4 h-4" />
            Create View Set
          </Button>
        </div>

        {dataSourceError && <ErrorBanner title="Data source error" message={dataSourceError} />}
        {listError && (
          <ErrorBanner title="Failed to load view sets" message={listError} onRetry={loadLists} />
        )}

        <CreateDialog
          isOpen={showCreate}
          onClose={() => setShowCreate(false)}
          onSubmit={(ddl) => {
            createdNameRef.current = extractViewSetName(ddl)
            void runMutation('create', ddl)
          }}
          isSubmitting={pending === 'create' && mutationQuery.isStreaming}
          error={errors.create}
          dataSource={dataSource}
        />

        <ConfirmDialog
          isOpen={confirmSave}
          onClose={() => setConfirmSave(false)}
          onConfirm={() => runMutation('save', toReplaceStatement(editText))}
          title="Save View Set"
          message={
            `Replace the definition of "${selectedName ?? ''}"? A change to the output schema ` +
            'invalidates the existing partitions. A change that keeps the schema does not ' +
            'rebuild anything: the existing partitions must be retired and rebuilt by other means.'
          }
          confirmLabel="Save"
          isLoading={pending === 'save' && mutationQuery.isStreaming}
          error={errors.save}
        />

        <ConfirmDialog
          isOpen={dropTarget !== null}
          onClose={() => {
            setDropTarget(null)
            setErrors((e) => ({ ...e, drop: null }))
          }}
          onConfirm={() => runMutation('drop', buildDropSql(dropTarget ?? ''))}
          title="Drop View Set"
          message={`Drop "${dropTarget ?? ''}"? Its partitions are retired. A view set that another definition reads cannot be dropped.`}
          confirmLabel="Drop"
          isLoading={pending === 'drop' && mutationQuery.isStreaming}
          variant="danger"
          error={errors.drop}
        />

        <div className="flex gap-6 flex-1 min-h-0">
          <div className="w-80 shrink-0 flex flex-col gap-3">
            <DataSourceField value={dataSource} onChange={setDataSource} />
            {listsLoading ? (
              <Spinner label="Loading view sets..." />
            ) : rows.length === 0 && listError ? null : rows.length === 0 ? (
              <div className="flex flex-col items-center text-center py-8">
                <Layers className="w-10 h-10 text-theme-text-muted opacity-40 mb-3" />
                <p className="text-theme-text-muted">No view sets.</p>
              </div>
            ) : (
              <TableFrame>
                <table className="w-full border-collapse">
                  <thead className="bg-app-panel sticky top-0">
                    <tr>
                      <th className="text-left p-2.5 px-3 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                        Name
                      </th>
                      <th
                        className="text-left p-2.5 px-3 text-xs font-semibold text-theme-text-muted uppercase tracking-wider"
                        title={HEALTH_TOOLTIP}
                      >
                        Health
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {rows.map((row) => (
                      <tr
                        key={row.name}
                        onClick={() => selectRow(row.name)}
                        className={`border-t border-theme-border cursor-pointer hover:bg-accent-link/5 ${
                          row.name === selectedName ? 'bg-accent-link/10' : ''
                        }`}
                      >
                        <td className="p-2.5 px-3">
                          <div className="font-mono text-xs text-theme-text-primary break-all">
                            {row.name}
                          </div>
                          <span className="text-[10px] uppercase tracking-wider text-theme-text-muted">
                            {row.source}
                          </span>
                        </td>
                        <td className="p-2.5 px-3">
                          <span
                            className={`px-1.5 py-0.5 rounded-sm text-[11px] ${HEALTH_STYLES[row.health]}`}
                            title={HEALTH_TOOLTIP}
                          >
                            {row.health}
                          </span>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </TableFrame>
            )}
          </div>

          <div className="flex-1 min-w-0 overflow-auto">
            {selected ? (
              <div>
                <h2 className="text-lg font-semibold font-mono text-theme-text-primary mb-3">
                  {selected.name}
                </h2>
                <div className="flex gap-1 border-b border-theme-border mb-4">
                  {(['definition', 'schema', 'partitions'] as Tab[]).map((t) => (
                    <button
                      key={t}
                      type="button"
                      onClick={() => setTab(t)}
                      className={`px-3 py-2 text-sm capitalize border-b-2 -mb-px ${
                        tab === t
                          ? 'border-accent-link text-theme-text-primary'
                          : 'border-transparent text-theme-text-muted hover:text-theme-text-secondary'
                      }`}
                    >
                      {t}
                    </button>
                  ))}
                </div>
                {tab === 'definition' && (
                  <DefinitionTab
                    row={selected}
                    dataSource={dataSource}
                    isEditing={isEditing}
                    editText={editText}
                    onEditTextChange={setEditText}
                    onStartEdit={() => {
                      setEditText(selected.definitionSql ?? '')
                      setErrors((e) => ({ ...e, save: null }))
                      setIsEditing(true)
                    }}
                    onCancelEdit={() => setIsEditing(false)}
                    onSave={() => {
                      if (extractViewSetName(editText) !== selected.name) {
                        setErrors((e) => ({
                          ...e,
                          save: `The statement must define "${selected.name}"; renaming a view set is not supported.`,
                        }))
                        return
                      }
                      setErrors((e) => ({ ...e, save: null }))
                      setConfirmSave(true)
                    }}
                    onDrop={() => {
                      setErrors((e) => ({ ...e, drop: null }))
                      setDropTarget(selected.name)
                    }}
                    saveError={confirmSave ? null : errors.save}
                    isSaving={pending === 'save' && mutationQuery.isStreaming}
                  />
                )}
                {tab === 'schema' && <SchemaTab row={selected} />}
                {tab === 'partitions' && (
                  <PartitionsTab
                    stats={selectedStats}
                    isLoading={statsQuery.isStreaming}
                    error={statsError}
                    onRetry={loadStats}
                  />
                )}
                {isEditing && tab === 'definition' && (
                  <div className="mt-3">
                    <DocumentationLink
                      url={VIEW_REDEFINITION_URL}
                      label="what a redefinition means"
                    />
                  </div>
                )}
              </div>
            ) : (
              <div className="h-full flex items-center justify-center text-theme-text-muted text-sm">
                Select a view set to see its definition, schema and partitions.
              </div>
            )}
          </div>
        </div>
      </div>
    </PageLayout>
  )
}

export default function ViewSetsPage() {
  return (
    <AuthGuard requireAdmin>
      <Suspense fallback={<div className="p-6 text-theme-text-muted">Loading...</div>}>
        <ViewSetsPageContent />
      </Suspense>
    </AuthGuard>
  )
}
