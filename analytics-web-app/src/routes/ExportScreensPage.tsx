import { Suspense, useState, useEffect, useCallback, useMemo } from 'react'
import { usePageTitle } from '@/hooks/usePageTitle'
import { Download } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { ErrorBanner } from '@/components/ErrorBanner'
import { Button } from '@/components/ui/button'
import {
  listScreens,
  getScreenTypes,
  buildScreensExport,
  Screen,
  ScreenTypeInfo,
  ScreenTypeName,
  ScreenApiError,
} from '@/lib/screens-api'
import { renderIcon } from '@/lib/screen-type-utils'

function ExportScreensPageContent() {
  usePageTitle('Export Screens')
  const [screens, setScreens] = useState<Screen[]>([])
  const [screenTypes, setScreenTypes] = useState<ScreenTypeInfo[]>([])
  const [isLoading, setIsLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [search, setSearch] = useState('')

  const loadData = useCallback(async () => {
    setIsLoading(true)
    setError(null)
    try {
      const [screensData, typesData] = await Promise.all([listScreens(), getScreenTypes()])
      setScreens(screensData)
      setScreenTypes(typesData)
    } catch (err) {
      setError(err instanceof ScreenApiError ? err.message : 'Failed to load screens')
    } finally {
      setIsLoading(false)
    }
  }, [])

  useEffect(() => {
    loadData()
  }, [loadData])

  const screenTypeMap = useMemo(() => {
    const map = new Map<ScreenTypeName, ScreenTypeInfo>()
    for (const type of screenTypes) {
      map.set(type.name, type)
    }
    return map
  }, [screenTypes])

  const filteredScreens = useMemo(() => {
    if (!search.trim()) return screens
    const q = search.toLowerCase()
    return screens.filter((s) => s.name.toLowerCase().includes(q))
  }, [screens, search])

  const toggleScreen = (name: string) => {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(name)) {
        next.delete(name)
      } else {
        next.add(name)
      }
      return next
    })
  }

  const selectAll = () => {
    setSelected(new Set(filteredScreens.map((s) => s.name)))
  }

  const deselectAll = () => {
    const filteredNames = new Set(filteredScreens.map((s) => s.name))
    setSelected((prev) => {
      const next = new Set(prev)
      for (const name of filteredNames) {
        next.delete(name)
      }
      return next
    })
  }

  const allFilteredSelected = filteredScreens.length > 0 && filteredScreens.every((s) => selected.has(s.name))

  const toggleAll = () => {
    if (allFilteredSelected) {
      deselectAll()
    } else {
      selectAll()
    }
  }

  const selectedScreens = screens.filter((s) => selected.has(s.name))

  const typeCounts = useMemo(() => {
    const counts = new Map<string, number>()
    for (const s of selectedScreens) {
      const displayName = screenTypeMap.get(s.screen_type)?.display_name ?? s.screen_type
      counts.set(displayName, (counts.get(displayName) ?? 0) + 1)
    }
    return counts
  }, [selectedScreens, screenTypeMap])

  const handleDownload = () => {
    const json = buildScreensExport(selectedScreens)
    const blob = new Blob([json], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    const date = new Date().toISOString().slice(0, 10)
    a.download = `screens-export-${date}.json`
    document.body.appendChild(a)
    a.click()
    document.body.removeChild(a)
    URL.revokeObjectURL(url)
  }

  return (
    <AuthGuard>
      <PageLayout onRefresh={loadData}>
        <div className="p-6 flex flex-col h-full">
          {/* Breadcrumb */}
          <div className="flex items-center gap-1.5 text-sm text-theme-text-muted mb-4">
            <AppLink href="/admin" className="text-accent-link hover:underline">
              Admin
            </AppLink>
            <span>/</span>
            <span>Export Screens</span>
          </div>

          <div className="mb-6">
            <h1 className="text-2xl font-semibold text-theme-text-primary">Export Screens</h1>
            <p className="mt-1 text-theme-text-secondary">Select screens to export as a JSON file.</p>
          </div>

          {error && <ErrorBanner title="Failed to load screens" message={error} onRetry={loadData} />}

          {isLoading ? (
            <div className="flex-1 flex items-center justify-center">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
                <span className="text-theme-text-secondary">Loading screens...</span>
              </div>
            </div>
          ) : (
            <div className="flex-1 flex gap-5 min-h-0">
              {/* Table section */}
              <div className="flex-1 flex flex-col min-h-0">
                {/* Toolbar */}
                <div className="flex items-center gap-2 mb-4">
                  <input
                    type="text"
                    className="bg-app-panel border border-theme-border rounded-md px-3 py-1.5 text-sm text-theme-text-primary placeholder:text-theme-text-muted outline-none focus:border-accent-link w-64"
                    placeholder="Search screens..."
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                  />
                  <Button variant="ghost" size="sm" onClick={selectAll}>
                    Select All
                  </Button>
                  <Button variant="ghost" size="sm" onClick={deselectAll}>
                    Deselect All
                  </Button>
                  <span className="text-sm text-theme-text-muted">
                    <strong className="text-accent-link">{selected.size}</strong> of {screens.length} selected
                  </span>
                </div>

                {/* Table */}
                <div className="border border-theme-border rounded-lg overflow-hidden flex-1 overflow-y-auto">
                  <table className="w-full border-collapse">
                    <thead className="bg-app-panel sticky top-0">
                      <tr>
                        <th className="text-left p-2.5 px-4 w-10">
                          <input
                            type="checkbox"
                            className="accent-[var(--color-accent-link)] cursor-pointer"
                            checked={allFilteredSelected}
                            onChange={toggleAll}
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
                          Last Updated
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {filteredScreens.map((screen) => (
                        <tr
                          key={screen.name}
                          className="border-t border-theme-border hover:bg-accent-link/5 cursor-pointer"
                          onClick={() => toggleScreen(screen.name)}
                        >
                          <td className="p-2.5 px-4">
                            <input
                              type="checkbox"
                              className="accent-[var(--color-accent-link)] cursor-pointer"
                              checked={selected.has(screen.name)}
                              onChange={() => toggleScreen(screen.name)}
                              onClick={(e) => e.stopPropagation()}
                              aria-label={`Select ${screen.name}`}
                            />
                          </td>
                          <td className="p-2.5 px-4">
                            <span className="text-accent-link font-medium">{screen.name}</span>
                          </td>
                          <td className="p-2.5 px-4">
                            <span className="inline-flex items-center gap-1 px-2 py-0.5 bg-app-card rounded text-xs text-theme-text-muted">
                              {renderIcon(screenTypeMap.get(screen.screen_type)?.icon ?? 'file-text', 'w-3.5 h-3.5')}
                              {screenTypeMap.get(screen.screen_type)?.display_name ?? screen.screen_type}
                            </span>
                          </td>
                          <td className="p-2.5 px-4 text-sm text-theme-text-muted">
                            {new Date(screen.updated_at).toLocaleDateString(undefined, {
                              month: 'short',
                              day: 'numeric',
                              year: 'numeric',
                            })}
                          </td>
                        </tr>
                      ))}
                      {filteredScreens.length === 0 && (
                        <tr>
                          <td colSpan={4} className="p-8 text-center text-theme-text-muted">
                            {search ? 'No screens match your search.' : 'No screens available.'}
                          </td>
                        </tr>
                      )}
                    </tbody>
                  </table>
                </div>
              </div>

              {/* Summary panel */}
              <div className="w-72 flex-shrink-0">
                <div className="bg-app-panel border border-theme-border rounded-lg p-5 sticky top-0">
                  <h3 className="text-sm font-semibold text-theme-text-primary mb-3">Export Summary</h3>
                  <div className="flex justify-between text-sm py-1">
                    <span className="text-theme-text-muted">Selected screens</span>
                    <span className="text-theme-text-primary">{selected.size}</span>
                  </div>
                  {Array.from(typeCounts.entries()).map(([type, count]) => (
                    <div key={type} className="flex justify-between text-sm py-1">
                      <span className="text-theme-text-muted">{type}</span>
                      <span className="text-theme-text-primary">{count}</span>
                    </div>
                  ))}
                  <div className="border-t border-theme-border my-2" />
                  <div className="flex justify-between text-sm py-1">
                    <span className="text-theme-text-muted">Format</span>
                    <span className="text-theme-text-primary">JSON</span>
                  </div>
                  <Button
                    className="w-full mt-4 gap-1.5 justify-center"
                    disabled={selected.size === 0}
                    onClick={handleDownload}
                  >
                    <Download className="w-4 h-4" />
                    Download Export
                  </Button>
                </div>
              </div>
            </div>
          )}
        </div>
      </PageLayout>
    </AuthGuard>
  )
}

export default function ExportScreensPage() {
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
      <ExportScreensPageContent />
    </Suspense>
  )
}
