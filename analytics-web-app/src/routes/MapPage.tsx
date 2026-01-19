import { Suspense, useState, useCallback, useEffect, useRef } from 'react'
import { Map as MapIcon, Layers, Eye, EyeOff } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { MapViewer, DeathEvent } from '@/components/map/MapViewer'
import { DeathDetailPanel } from '@/components/map/DeathDetailPanel'
import { useMapData, MAP_VARIABLES } from '@/hooks/useMapData'
import { useTimeRange } from '@/hooks/useTimeRange'

const DEFAULT_SQL = `SELECT
  time,
  process_id,
  properties->>'x' as x,
  properties->>'y' as y,
  properties->>'z' as z,
  properties->>'player_name' as player_name,
  properties->>'death_cause' as death_cause
FROM spans
WHERE name = 'death_event'
  AND time BETWEEN $begin AND $end
ORDER BY time DESC
LIMIT 10000`

function MapPageContent() {
  const { parsed: timeRange, apiTimeRange } = useTimeRange()
  const mapData = useMapData()

  const [selectedEvent, setSelectedEvent] = useState<DeathEvent | null>(null)
  const [mapUrl, setMapUrl] = useState<string | undefined>(undefined)
  const [showHeatmap, setShowHeatmap] = useState(true)
  const [showMarkers, setShowMarkers] = useState(true)
  const [heatmapRadius, setHeatmapRadius] = useState(50)
  const [heatmapIntensity, setHeatmapIntensity] = useState(0.5)
  const [currentSql, setCurrentSql] = useState(DEFAULT_SQL)

  const executeRef = useRef(mapData.execute)
  executeRef.current = mapData.execute

  const currentSqlRef = useRef(currentSql)
  currentSqlRef.current = currentSql

  const loadData = useCallback((sql: string) => {
    setCurrentSql(sql)
    executeRef.current(sql)
  }, [])

  const queryKey = `${apiTimeRange.begin}-${apiTimeRange.end}`
  const prevQueryKeyRef = useRef<string | null>(null)

  useEffect(() => {
    if (prevQueryKeyRef.current !== queryKey) {
      const isInitialLoad = prevQueryKeyRef.current === null
      prevQueryKeyRef.current = queryKey
      loadData(isInitialLoad ? DEFAULT_SQL : currentSqlRef.current)
    }
  }, [queryKey, loadData])

  const handleRefresh = useCallback(() => {
    loadData(currentSqlRef.current)
  }, [loadData])

  const handleRunQuery = useCallback(
    (sql: string) => {
      loadData(sql)
    },
    [loadData]
  )

  const handleResetQuery = useCallback(() => {
    loadData(DEFAULT_SQL)
  }, [loadData])

  const handleSelectEvent = useCallback((event: DeathEvent | null) => {
    setSelectedEvent(event)
  }, [])

  const handleMapFileUpload = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0]
    if (file) {
      const url = URL.createObjectURL(file)
      setMapUrl(url)
    }
  }, [])

  const currentValues = {
    begin: apiTimeRange.begin,
    end: apiTimeRange.end,
  }

  const sqlPanel = (
    <QueryEditor
      defaultSql={DEFAULT_SQL}
      variables={MAP_VARIABLES}
      currentValues={currentValues}
      timeRangeLabel={timeRange.label}
      onRun={handleRunQuery}
      onReset={handleResetQuery}
      isLoading={mapData.isLoading}
      error={mapData.error}
    />
  )

  return (
    <AuthGuard>
      <PageLayout onRefresh={handleRefresh} rightPanel={sqlPanel}>
        <div className="p-6 flex flex-col h-full">
          <div className="mb-4 flex items-center justify-between">
            <div className="flex items-center gap-3">
              <MapIcon className="w-6 h-6 text-accent-link" />
              <h1 className="text-2xl font-semibold text-theme-text-primary">Map Explorer</h1>
            </div>

            <div className="flex items-center gap-4">
              <div className="flex items-center gap-2 text-sm text-theme-text-secondary">
                <span>{mapData.events.length} events</span>
              </div>
            </div>
          </div>

          <div className="mb-4 flex items-center gap-4 flex-wrap">
            <div className="flex items-center gap-2">
              <label className="text-sm text-theme-text-secondary">Map File:</label>
              <input
                type="file"
                accept=".glb,.gltf"
                onChange={handleMapFileUpload}
                className="text-sm text-theme-text-primary file:mr-3 file:py-1.5 file:px-3 file:rounded file:border-0 file:text-sm file:bg-app-card file:text-theme-text-primary hover:file:bg-theme-border file:cursor-pointer file:transition-colors"
              />
            </div>

            <div className="h-6 w-px bg-theme-border" />

            <button
              onClick={() => setShowMarkers(!showMarkers)}
              className={`flex items-center gap-2 px-3 py-1.5 rounded text-sm transition-colors ${
                showMarkers
                  ? 'bg-accent text-white'
                  : 'bg-app-card text-theme-text-secondary hover:text-theme-text-primary'
              }`}
            >
              {showMarkers ? <Eye className="w-4 h-4" /> : <EyeOff className="w-4 h-4" />}
              Markers
            </button>

            <button
              onClick={() => setShowHeatmap(!showHeatmap)}
              className={`flex items-center gap-2 px-3 py-1.5 rounded text-sm transition-colors ${
                showHeatmap
                  ? 'bg-accent text-white'
                  : 'bg-app-card text-theme-text-secondary hover:text-theme-text-primary'
              }`}
            >
              <Layers className="w-4 h-4" />
              Heatmap
            </button>

            {showHeatmap && (
              <>
                <div className="flex items-center gap-2">
                  <label className="text-xs text-theme-text-muted">Radius:</label>
                  <input
                    type="range"
                    min="20"
                    max="100"
                    value={heatmapRadius}
                    onChange={(e) => setHeatmapRadius(Number(e.target.value))}
                    className="w-20 accent-accent-link"
                  />
                </div>
                <div className="flex items-center gap-2">
                  <label className="text-xs text-theme-text-muted">Intensity:</label>
                  <input
                    type="range"
                    min="0.1"
                    max="1"
                    step="0.1"
                    value={heatmapIntensity}
                    onChange={(e) => setHeatmapIntensity(Number(e.target.value))}
                    className="w-20 accent-accent-link"
                  />
                </div>
              </>
            )}
          </div>

          {mapData.error && (
            <ErrorBanner
              title="Query execution failed"
              message={mapData.error}
              onRetry={handleRefresh}
            />
          )}

          <div className="flex-1 relative bg-app-panel border border-theme-border rounded-lg overflow-hidden">
            {mapData.isLoading && mapData.events.length === 0 ? (
              <div className="absolute inset-0 flex items-center justify-center">
                <div className="flex items-center gap-3">
                  <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
                  <span className="text-theme-text-secondary">Loading events...</span>
                </div>
              </div>
            ) : (
              <MapViewer
                mapUrl={mapUrl}
                deathEvents={showMarkers ? mapData.events : []}
                selectedEventId={selectedEvent?.id}
                onSelectEvent={handleSelectEvent}
                showHeatmap={showHeatmap}
                heatmapRadius={heatmapRadius}
                heatmapIntensity={heatmapIntensity}
              />
            )}

            {selectedEvent && (
              <DeathDetailPanel event={selectedEvent} onClose={() => setSelectedEvent(null)} />
            )}
          </div>
        </div>
      </PageLayout>
    </AuthGuard>
  )
}

export default function MapPage() {
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
      <MapPageContent />
    </Suspense>
  )
}
