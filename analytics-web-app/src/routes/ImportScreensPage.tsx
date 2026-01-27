import { Suspense, useState, useEffect, useCallback, useMemo, useRef } from 'react'
import { usePageTitle } from '@/hooks/usePageTitle'
import { Upload, Check, FileText } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { ErrorBanner } from '@/components/ErrorBanner'
import { Button } from '@/components/ui/button'
import { renderIcon } from '@/lib/screen-type-utils'
import {
  listScreens,
  getScreenTypes,
  parseScreensImportFile,
  importScreen,
  Screen,
  ScreenTypeInfo,
  ScreenTypeName,
  ExportedScreen,
  ImportConflictAction,
  ImportScreenResult,
  ScreenApiError,
} from '@/lib/screens-api'

type WizardStep = 1 | 2 | 3

interface ScreenImportEntry {
  screen: ExportedScreen
  selected: boolean
  isConflict: boolean
  conflictAction: ImportConflictAction
}

function WizardSteps({ current }: { current: WizardStep }) {
  const steps = [
    { num: 1, label: 'Upload File' },
    { num: 2, label: 'Review & Select' },
    { num: 3, label: 'Confirm' },
  ]

  return (
    <div className="flex gap-0 mb-8">
      {steps.map((step, i) => {
        const isDone = step.num < current
        const isActive = step.num === current

        return (
          <div key={step.num} className="flex items-center gap-2">
            {i > 0 && <div className="w-6 h-px bg-theme-border mx-1" />}
            <div
              className={`flex items-center gap-2 px-4 py-2.5 text-sm font-medium ${
                isActive
                  ? 'text-theme-text-primary'
                  : isDone
                    ? 'text-theme-text-muted'
                    : 'text-theme-text-muted'
              }`}
            >
              <span
                className={`w-6 h-6 rounded-full flex items-center justify-center text-xs font-bold border-2 ${
                  isActive
                    ? 'border-accent-link bg-accent-link text-white'
                    : isDone
                      ? 'border-green-500 bg-green-500 text-white'
                      : 'border-theme-border'
                }`}
              >
                {isDone ? <Check className="w-3 h-3" /> : step.num}
              </span>
              {step.label}
            </div>
          </div>
        )
      })}
    </div>
  )
}

function ImportScreensPageContent() {
  usePageTitle('Import Screens')
  const [step, setStep] = useState<WizardStep>(1)
  const [error, setError] = useState<string | null>(null)
  const [fileName, setFileName] = useState<string | null>(null)
  const [entries, setEntries] = useState<ScreenImportEntry[]>([])
  const [existingScreens, setExistingScreens] = useState<Screen[]>([])
  const [screenTypes, setScreenTypes] = useState<ScreenTypeInfo[]>([])
  const [isImporting, setIsImporting] = useState(false)
  const [results, setResults] = useState<ImportScreenResult[] | null>(null)
  const fileInputRef = useRef<HTMLInputElement>(null)
  const [isDragOver, setIsDragOver] = useState(false)

  const loadExistingScreens = useCallback(async () => {
    try {
      const [screensData, typesData] = await Promise.all([listScreens(), getScreenTypes()])
      setExistingScreens(screensData)
      setScreenTypes(typesData)
    } catch (err) {
      setError(err instanceof ScreenApiError ? err.message : 'Failed to load existing screens')
    }
  }, [])

  useEffect(() => {
    loadExistingScreens()
  }, [loadExistingScreens])

  const screenTypeMap = useMemo(() => {
    const map = new Map<ScreenTypeName, ScreenTypeInfo>()
    for (const type of screenTypes) {
      map.set(type.name, type)
    }
    return map
  }, [screenTypes])

  const existingNameSet = useMemo(() => new Set(existingScreens.map((s) => s.name)), [existingScreens])

  const handleFile = useCallback(
    (file: File) => {
      setError(null)
      setFileName(file.name)

      const reader = new FileReader()
      reader.onload = (e) => {
        try {
          const parsed = parseScreensImportFile(e.target?.result as string)
          const importEntries: ScreenImportEntry[] = parsed.screens.map((screen) => ({
            screen,
            selected: true,
            isConflict: existingNameSet.has(screen.name),
            conflictAction: 'rename' as ImportConflictAction,
          }))
          setEntries(importEntries)
          setStep(2)
        } catch (err) {
          setError(err instanceof Error ? err.message : 'Failed to parse file')
        }
      }
      reader.readAsText(file)
    },
    [existingNameSet]
  )

  const handleFileInput = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0]
    if (file) handleFile(file)
  }

  const handleDrop = (e: React.DragEvent) => {
    e.preventDefault()
    setIsDragOver(false)
    const file = e.dataTransfer.files[0]
    if (file) handleFile(file)
  }

  const handleDragOver = (e: React.DragEvent) => {
    e.preventDefault()
    setIsDragOver(true)
  }

  const handleDragLeave = () => {
    setIsDragOver(false)
  }

  const toggleEntry = (index: number) => {
    setEntries((prev) =>
      prev.map((entry, i) => (i === index ? { ...entry, selected: !entry.selected } : entry))
    )
  }

  const setConflictAction = (index: number, action: ImportConflictAction) => {
    setEntries((prev) =>
      prev.map((entry, i) => (i === index ? { ...entry, conflictAction: action } : entry))
    )
  }

  const selectAllEntries = () => {
    setEntries((prev) => prev.map((entry) => ({ ...entry, selected: true })))
  }

  const deselectAllEntries = () => {
    setEntries((prev) => prev.map((entry) => ({ ...entry, selected: false })))
  }

  const selectedEntries = entries.filter((e) => e.selected)
  const conflictCount = entries.filter((e) => e.isConflict).length
  const selectedNewCount = selectedEntries.filter((e) => !e.isConflict).length
  const selectedConflictEntries = selectedEntries.filter((e) => e.isConflict)

  const allSelected = entries.length > 0 && entries.every((e) => e.selected)

  const handleImport = async () => {
    setIsImporting(true)
    setError(null)
    const importResults: ImportScreenResult[] = []
    // Build a mutable copy of existing names for rename tracking
    const namesInUse = new Set(existingNameSet)

    for (const entry of selectedEntries) {
      try {
        const result = await importScreen(entry.screen, entry.conflictAction, namesInUse)
        importResults.push(result)
        if (result.status === 'created' || result.status === 'renamed') {
          namesInUse.add(result.finalName ?? result.name)
        }
      } catch (err) {
        importResults.push({
          name: entry.screen.name,
          status: 'error',
          error: err instanceof Error ? err.message : 'Unknown error',
        })
      }
    }

    setResults(importResults)
    setIsImporting(false)
  }

  const renderStep1 = () => (
    <>
      <WizardSteps current={1} />
      <div
        className={`border-2 border-dashed rounded-xl p-12 text-center transition-colors cursor-pointer ${
          isDragOver
            ? 'border-accent-link bg-accent-link/5'
            : 'border-theme-border hover:border-accent-link hover:bg-accent-link/5'
        }`}
        role="button"
        aria-label="Drop a screens export file here or click to browse"
        onDrop={handleDrop}
        onDragOver={handleDragOver}
        onDragLeave={handleDragLeave}
        onClick={() => fileInputRef.current?.click()}
      >
        <Upload className="w-10 h-10 mx-auto mb-3 text-theme-text-muted opacity-50" />
        <p className="text-base font-medium text-theme-text-primary mb-1.5">
          Drop a screens export file here
        </p>
        <p className="text-sm text-theme-text-muted">
          or <span className="text-accent-link underline">browse your files</span>
        </p>
        <p className="text-xs text-theme-text-muted mt-3">Accepts .json files exported from Micromegas.</p>
        <input
          ref={fileInputRef}
          type="file"
          accept=".json"
          className="hidden"
          onChange={handleFileInput}
          aria-label="Choose screens export file"
        />
      </div>
    </>
  )

  const renderStep2 = () => (
    <>
      <WizardSteps current={2} />

      {/* File info bar */}
      <div className="flex items-center gap-3 px-4 py-3 bg-app-panel border border-theme-border rounded-lg mb-4">
        <FileText className="w-4.5 h-4.5 text-theme-text-muted" />
        <span className="text-sm">{fileName}</span>
        <span className="text-xs text-theme-text-muted">
          {entries.length} screens found &middot; {conflictCount} conflict{conflictCount !== 1 ? 's' : ''}
        </span>
      </div>

      {/* Toolbar */}
      <div className="flex items-center gap-2 mb-4">
        <Button variant="ghost" size="sm" onClick={selectAllEntries}>
          Select All
        </Button>
        <Button variant="ghost" size="sm" onClick={deselectAllEntries}>
          Deselect All
        </Button>
        <span className="text-sm text-theme-text-muted">
          <strong className="text-accent-link">{selectedEntries.length}</strong> of {entries.length} selected
        </span>
      </div>

      {/* Table */}
      <div className="border border-theme-border rounded-lg overflow-hidden">
        <table className="w-full border-collapse">
          <thead className="bg-app-panel">
            <tr>
              <th className="text-left p-2.5 px-4 w-10">
                <input
                  type="checkbox"
                  className="accent-[var(--color-accent-link)] cursor-pointer"
                  checked={allSelected}
                  onChange={() => (allSelected ? deselectAllEntries() : selectAllEntries())}
                  aria-label="Select all screens"
                />
              </th>
              <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                Screen Name
              </th>
              <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                Type
              </th>
              <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                Status
              </th>
              <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                Action on Conflict
              </th>
            </tr>
          </thead>
          <tbody>
            {entries.map((entry, i) => (
              <tr
                key={entry.screen.name}
                className="border-t border-theme-border hover:bg-accent-link/5"
              >
                <td className="p-2.5 px-4">
                  <input
                    type="checkbox"
                    className="accent-[var(--color-accent-link)] cursor-pointer"
                    checked={entry.selected}
                    onChange={() => toggleEntry(i)}
                    aria-label={`Select ${entry.screen.name}`}
                  />
                </td>
                <td className="p-2.5 px-4">
                  <span className="text-accent-link font-medium">{entry.screen.name}</span>
                </td>
                <td className="p-2.5 px-4">
                  <span className="inline-flex items-center gap-1 px-2 py-0.5 bg-app-card rounded text-xs text-theme-text-muted">
                    {renderIcon(
                      screenTypeMap.get(entry.screen.screen_type)?.icon ?? 'file-text',
                      'w-3.5 h-3.5'
                    )}
                    {screenTypeMap.get(entry.screen.screen_type)?.display_name ?? entry.screen.screen_type}
                  </span>
                </td>
                <td className="p-2.5 px-4">
                  {entry.isConflict ? (
                    <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs font-semibold bg-yellow-500/15 text-yellow-500">
                      Exists
                    </span>
                  ) : (
                    <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs font-semibold bg-green-500/15 text-green-500">
                      New
                    </span>
                  )}
                </td>
                <td className="p-2.5 px-4">
                  {entry.isConflict ? (
                    <select
                      className="bg-app-panel border border-theme-border rounded px-2 py-1 text-xs text-theme-text-primary outline-none"
                      value={entry.conflictAction}
                      onChange={(e) => setConflictAction(i, e.target.value as ImportConflictAction)}
                      aria-label={`Conflict action for ${entry.screen.name}`}
                    >
                      <option value="skip">Skip</option>
                      <option value="overwrite">Overwrite</option>
                      <option value="rename">Rename</option>
                    </select>
                  ) : (
                    <span className="text-xs text-theme-text-muted">&mdash;</span>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {/* Footer */}
      <div className="flex justify-between mt-6 pt-4 border-t border-theme-border">
        <Button
          variant="outline"
          onClick={() => {
            setStep(1)
            setEntries([])
            setFileName(null)
            setError(null)
          }}
        >
          Back
        </Button>
        <Button disabled={selectedEntries.length === 0} onClick={() => setStep(3)}>
          Continue
        </Button>
      </div>
    </>
  )

  const renderStep3 = () => {
    if (results) {
      const successes = results.filter((r) => !r.error)
      const errors = results.filter((r) => r.error)
      return (
        <>
          <WizardSteps current={3} />
          <div className="bg-app-panel border border-theme-border rounded-lg p-5 max-w-lg">
            <h3 className="text-sm font-semibold text-theme-text-primary mb-3">Import Complete</h3>
            <div className="flex justify-between text-sm py-1">
              <span className="text-theme-text-muted">Successful</span>
              <span className="text-green-500">{successes.length}</span>
            </div>
            {errors.length > 0 && (
              <div className="flex justify-between text-sm py-1">
                <span className="text-theme-text-muted">Failed</span>
                <span className="text-red-400">{errors.length}</span>
              </div>
            )}
            <div className="border-t border-theme-border my-2" />
            {results.map((r) => (
              <div key={r.name} className="flex justify-between text-sm py-1">
                <span className="text-theme-text-primary">
                  {r.name}
                  {r.finalName && (
                    <span className="text-theme-text-muted"> &rarr; {r.finalName}</span>
                  )}
                </span>
                <span
                  className={
                    r.error
                      ? 'text-red-400'
                      : r.status === 'skipped'
                        ? 'text-theme-text-muted'
                        : 'text-green-500'
                  }
                >
                  {r.error ? 'Error' : r.status}
                </span>
              </div>
            ))}
          </div>
          <div className="flex gap-3 mt-6">
            <AppLink href="/admin">
              <Button variant="outline">Back to Admin</Button>
            </AppLink>
            <AppLink href="/screens">
              <Button>View Screens</Button>
            </AppLink>
          </div>
        </>
      )
    }

    return (
      <>
        <WizardSteps current={3} />
        <div className="bg-app-panel border border-theme-border rounded-lg p-5 max-w-lg">
          <h3 className="text-sm font-semibold text-theme-text-primary mb-3">Import Summary</h3>
          <div className="flex justify-between text-sm py-1">
            <span className="text-theme-text-muted">Source file</span>
            <span className="text-theme-text-primary">{fileName}</span>
          </div>
          <div className="border-t border-theme-border my-2" />
          <div className="flex justify-between text-sm py-1">
            <span className="text-theme-text-muted">Screens to import</span>
            <span className="text-theme-text-primary">{selectedEntries.length}</span>
          </div>
          <div className="flex justify-between text-sm py-1">
            <span className="text-theme-text-muted">New screens</span>
            <span className="text-green-500">{selectedNewCount}</span>
          </div>
          {selectedConflictEntries.length > 0 && (
            <>
              {['skip', 'overwrite', 'rename'].map((action) => {
                const count = selectedConflictEntries.filter((e) => e.conflictAction === action).length
                if (count === 0) return null
                return (
                  <div key={action} className="flex justify-between text-sm py-1">
                    <span className="text-theme-text-muted">
                      Conflicts ({action})
                    </span>
                    <span className="text-yellow-500">{count}</span>
                  </div>
                )
              })}
            </>
          )}
          <div className="border-t border-theme-border my-2" />
          <p className="text-xs text-theme-text-muted">
            This action will create or update the screens listed above.
          </p>
        </div>

        <div className="flex justify-between mt-6 pt-4 border-t border-theme-border">
          <Button variant="outline" onClick={() => setStep(2)} disabled={isImporting}>
            Back
          </Button>
          <Button onClick={handleImport} disabled={isImporting} className="gap-1.5">
            {isImporting ? (
              <>
                <div className="w-4 h-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
                Importing...
              </>
            ) : (
              <>
                <Upload className="w-4 h-4" />
                Import Now
              </>
            )}
          </Button>
        </div>
      </>
    )
  }

  return (
    <AuthGuard>
      <PageLayout>
        <div className="p-6 flex flex-col h-full">
          {/* Breadcrumb */}
          <div className="flex items-center gap-1.5 text-sm text-theme-text-muted mb-4">
            <AppLink href="/admin" className="text-accent-link hover:underline">
              Admin
            </AppLink>
            <span>/</span>
            <span>Import Screens</span>
          </div>

          <div className="mb-6">
            <h1 className="text-2xl font-semibold text-theme-text-primary">Import Screens</h1>
            <p className="mt-1 text-theme-text-secondary">
              Upload and import screen configurations from a JSON export file.
            </p>
          </div>

          {error && <ErrorBanner title="Import error" message={error} />}

          {step === 1 && renderStep1()}
          {step === 2 && renderStep2()}
          {step === 3 && renderStep3()}
        </div>
      </PageLayout>
    </AuthGuard>
  )
}

export default function ImportScreensPage() {
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
      <ImportScreensPageContent />
    </Suspense>
  )
}
