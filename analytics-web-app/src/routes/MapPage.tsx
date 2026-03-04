import { Suspense, useState, useCallback, useEffect, useRef, useMemo } from 'react'
import { Map as MapIcon, Layers, Eye, EyeOff, Focus, FlaskConical, Palette, Magnet, RotateCcw } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { MapViewer, DeathEvent, MapBounds } from '@/components/map/MapViewer'
import { DeathDetailPanel } from '@/components/map/DeathDetailPanel'
import { useMapData, MAP_VARIABLES } from '@/hooks/useMapData'
import { useScreenConfig } from '@/hooks/useScreenConfig'
import { parseTimeRange, getTimeRangeForApi } from '@/lib/time-range'

// Mock data generation
const MOCK_PLAYER_NAMES = [
  'PlayerOne',
  'ShadowHunter',
  'NightWolf',
  'DragonSlayer',
  'StormRider',
  'IronFist',
  'PhantomX',
  'BlazeMaster',
  'FrostBite',
  'ThunderStrike',
]

const MOCK_DEATH_CAUSES = [
  'Falling',
  'Explosion',
  'Enemy_Fire',
  'Collision',
  'Out_of_Bounds',
  'Drowning',
  'Crushed',
  'Environmental',
  'Friendly_Fire',
  'Unknown',
]

function generateMockEvents(count: number, seed: number = 42, mapBounds: MapBounds | null = null): DeathEvent[] {
  // Simple seeded random for reproducible results
  const seededRandom = (s: number) => {
    const x = Math.sin(s) * 10000
    return x - Math.floor(x)
  }

  const events: DeathEvent[] = []
  const now = new Date()

  // Derive hotspots and spread from map bounds if available
  const boundsMinX = mapBounds ? mapBounds.min.x : -5000
  const boundsMaxX = mapBounds ? mapBounds.max.x : 5000
  const boundsMinY = mapBounds ? mapBounds.min.y : -5000
  const boundsMaxY = mapBounds ? mapBounds.max.y : 5000
  const boundsMinZ = mapBounds ? mapBounds.min.z : 0
  const boundsMaxZ = mapBounds ? mapBounds.max.z : 200

  const centerX = (boundsMinX + boundsMaxX) / 2
  const centerY = (boundsMinY + boundsMaxY) / 2
  const rangeX = boundsMaxX - boundsMinX
  const rangeY = boundsMaxY - boundsMinY

  const hotspots = [
    { x: centerX, y: centerY },
    { x: centerX + rangeX * 0.3, y: centerY + rangeY * 0.2 },
    { x: centerX - rangeX * 0.2, y: centerY + rangeY * 0.3 },
    { x: centerX + rangeX * 0.15, y: centerY - rangeY * 0.25 },
    { x: centerX - rangeX * 0.3, y: centerY - rangeY * 0.1 },
  ]
  const spread = Math.max(rangeX, rangeY) * 0.2

  for (let i = 0; i < count; i++) {
    const r1 = seededRandom(seed + i * 3)
    const r2 = seededRandom(seed + i * 3 + 1)
    const r3 = seededRandom(seed + i * 3 + 2)
    const r4 = seededRandom(seed + i * 7)
    const r5 = seededRandom(seed + i * 11)
    const r6 = seededRandom(seed + i * 13)

    const hotspot = hotspots[Math.floor(r4 * hotspots.length)]

    events.push({
      id: `mock-${i}`,
      time: new Date(now.getTime() - r6 * 3600000 * 24),
      processId: `process-${Math.floor(r5 * 100)}`,
      x: hotspot.x + (r1 - 0.5) * spread,
      y: hotspot.y + (r2 - 0.5) * spread,
      z: boundsMinZ + r3 * (boundsMaxZ - boundsMinZ),
      playerName: MOCK_PLAYER_NAMES[Math.floor(r4 * MOCK_PLAYER_NAMES.length)],
      deathCause: MOCK_DEATH_CAUSES[Math.floor(r5 * MOCK_DEATH_CAUSES.length)],
    })
  }

  return events
}

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

// Config interface for MapPage
interface MapConfig {
  timeRangeFrom: string
  timeRangeTo: string
}

// Default config for MapPage
const DEFAULT_CONFIG: MapConfig = {
  timeRangeFrom: 'now-1h',
  timeRangeTo: 'now',
}

// URL builder for MapPage - builds query string from config
const buildUrl = (cfg: MapConfig): string => {
  const params = new URLSearchParams()
  if (cfg.timeRangeFrom && cfg.timeRangeFrom !== DEFAULT_CONFIG.timeRangeFrom) {
    params.set('from', cfg.timeRangeFrom)
  }
  if (cfg.timeRangeTo && cfg.timeRangeTo !== DEFAULT_CONFIG.timeRangeTo) {
    params.set('to', cfg.timeRangeTo)
  }
  const qs = params.toString()
  return qs ? `?${qs}` : ''
}

function MapPageContent() {
  // Use the new config-driven pattern
  const { config, updateConfig } = useScreenConfig(DEFAULT_CONFIG, buildUrl)

  // Compute API time range from config
  const apiTimeRange = useMemo(() => {
    try {
      return getTimeRangeForApi(config.timeRangeFrom ?? 'now-1h', config.timeRangeTo ?? 'now')
    } catch {
      return getTimeRangeForApi('now-1h', 'now')
    }
  }, [config.timeRangeFrom, config.timeRangeTo])

  // Compute display label for time range
  const timeRangeLabel = useMemo(() => {
    try {
      return parseTimeRange(config.timeRangeFrom ?? 'now-1h', config.timeRangeTo ?? 'now').label
    } catch {
      return 'Last 1 hour'
    }
  }, [config.timeRangeFrom, config.timeRangeTo])

  const mapData = useMapData({ apiTimeRange })

  const [selectedEvent, setSelectedEvent] = useState<DeathEvent | null>(null)
  const [mapUrl, setMapUrl] = useState<string | undefined>(undefined)
  const [showHeatmap, setShowHeatmap] = useState(true)
  const [showMarkers, setShowMarkers] = useState(true)
  const [heatmapRadius, setHeatmapRadius] = useState(50)
  const [heatmapIntensity, setHeatmapIntensity] = useState(0.5)
  const [currentSql, setCurrentSql] = useState(DEFAULT_SQL)
  const [fitToDataTrigger, setFitToDataTrigger] = useState(0)
  const [useMockData, setUseMockData] = useState(false)
  const [mockEventCount, setMockEventCount] = useState(1000)
  const [mapBounds, setMapBounds] = useState<MapBounds | null>(null)
  const [resetViewTrigger, setResetViewTrigger] = useState(0)
  const [markerColor, setMarkerColor] = useState('#bf360c')
  const [markerSize, setMarkerSize] = useState(10)
  const [groundSnap, setGroundSnap] = useState(false)

  // Generate mock events (memoized to avoid regenerating on every render)
  const mockEvents = useMemo(
    () => (useMockData ? generateMockEvents(mockEventCount, 42, mapBounds) : []),
    [useMockData, mockEventCount, mapBounds]
  )

  // Use mock data or real data based on toggle
  const displayEvents = useMockData ? mockEvents : mapData.events

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

  // Time range changes create history entries (navigational)
  const handleTimeRangeChange = useCallback(
    (from: string, to: string) => {
      updateConfig({ timeRangeFrom: from, timeRangeTo: to })
    },
    [updateConfig]
  )

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

  const handleFitToData = useCallback(() => {
    setFitToDataTrigger((prev) => prev + 1)
  }, [])

  const handleMapBoundsChange = useCallback((bounds: MapBounds | null) => {
    setMapBounds(bounds)
  }, [])

  const handleResetView = useCallback(() => {
    setResetViewTrigger((prev) => prev + 1)
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
      timeRangeLabel={timeRangeLabel}
      onRun={handleRunQuery}
      onReset={handleResetQuery}
      isLoading={mapData.isLoading}
      error={mapData.error}
    />
  )

  return (
    <AuthGuard>
      <PageLayout
        onRefresh={handleRefresh}
        rightPanel={sqlPanel}
        timeRangeControl={{
          timeRangeFrom: config.timeRangeFrom ?? 'now-1h',
          timeRangeTo: config.timeRangeTo ?? 'now',
          onTimeRangeChange: handleTimeRangeChange,
        }}
      >
        <div className="p-6 flex flex-col h-full">
          <div className="mb-4 flex items-center justify-between">
            <div className="flex items-center gap-3">
              <MapIcon className="w-6 h-6 text-accent-link" />
              <h1 className="text-2xl font-semibold text-theme-text-primary">Map Explorer</h1>
            </div>

            <div className="flex items-center gap-4">
              <div className="flex items-center gap-2 text-sm text-theme-text-secondary">
                <span>
                  {displayEvents.length.toLocaleString()} events
                  {useMockData && ' (mock)'}
                </span>
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

            <div className="h-6 w-px bg-theme-border" />

            <button
              onClick={handleFitToData}
              disabled={displayEvents.length === 0}
              className="flex items-center gap-2 px-3 py-1.5 rounded text-sm transition-colors bg-app-card text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-border disabled:opacity-50 disabled:cursor-not-allowed"
              title="Fit camera to show all data points"
            >
              <Focus className="w-4 h-4" />
              Fit to Data
            </button>

            <div className="h-6 w-px bg-theme-border" />

            <div className="flex items-center gap-2">
              <Palette className="w-4 h-4 text-theme-text-muted" />
              <input
                type="color"
                value={markerColor}
                onChange={(e) => setMarkerColor(e.target.value)}
                className="w-7 h-7 rounded cursor-pointer border border-theme-border bg-transparent"
                title="Marker color"
              />
            </div>

            <div className="flex items-center gap-2">
              <label className="text-xs text-theme-text-muted">Size:</label>
              <input
                type="range"
                min="1"
                max="50"
                value={markerSize}
                onChange={(e) => setMarkerSize(Number(e.target.value))}
                className="w-20 accent-accent-link"
              />
              <span className="text-xs text-theme-text-muted w-5">{markerSize}</span>
            </div>

            <button
              onClick={() => setGroundSnap(!groundSnap)}
              disabled={!mapBounds}
              className={`flex items-center gap-2 px-3 py-1.5 rounded text-sm transition-colors ${
                groundSnap
                  ? 'bg-accent text-white'
                  : 'bg-app-card text-theme-text-secondary hover:text-theme-text-primary'
              } disabled:opacity-50 disabled:cursor-not-allowed`}
              title="Snap markers to ground surface"
            >
              <Magnet className="w-4 h-4" />
              Ground Snap
            </button>

            <button
              onClick={handleResetView}
              className="flex items-center gap-2 px-3 py-1.5 rounded text-sm transition-colors bg-app-card text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-border"
              title="Reset camera to initial view"
            >
              <RotateCcw className="w-4 h-4" />
              Reset View
            </button>

            <div className="h-6 w-px bg-theme-border" />

            <button
              onClick={() => setUseMockData(!useMockData)}
              className={`flex items-center gap-2 px-3 py-1.5 rounded text-sm transition-colors ${
                useMockData
                  ? 'bg-amber-600 text-white'
                  : 'bg-app-card text-theme-text-secondary hover:text-theme-text-primary'
              }`}
              title="Toggle mock data for testing without backend"
            >
              <FlaskConical className="w-4 h-4" />
              Mock Data
            </button>

            {useMockData && (
              <div className="flex items-center gap-2">
                <label className="text-xs text-theme-text-muted">Count:</label>
                <select
                  value={mockEventCount}
                  onChange={(e) => setMockEventCount(Number(e.target.value))}
                  className="bg-app-card text-theme-text-primary text-sm rounded px-2 py-1 border border-theme-border"
                >
                  <option value={100}>100</option>
                  <option value={500}>500</option>
                  <option value={1000}>1,000</option>
                  <option value={5000}>5,000</option>
                  <option value={10000}>10,000</option>
                  <option value={50000}>50,000</option>
                  <option value={100000}>100,000</option>
                </select>
              </div>
            )}

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
            {!useMockData && mapData.isLoading && mapData.events.length === 0 ? (
              <div className="absolute inset-0 flex items-center justify-center">
                <div className="flex items-center gap-3">
                  <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
                  <span className="text-theme-text-secondary">Loading events...</span>
                </div>
              </div>
            ) : (
              <MapViewer
                mapUrl={mapUrl}
                deathEvents={showMarkers ? displayEvents : []}
                selectedEventId={selectedEvent?.id}
                onSelectEvent={handleSelectEvent}
                showHeatmap={showHeatmap}
                heatmapRadius={heatmapRadius}
                heatmapIntensity={heatmapIntensity}
                fitToDataTrigger={fitToDataTrigger}
                onMapBoundsChange={handleMapBoundsChange}
                markerColor={markerColor}
                markerSize={markerSize}
                groundSnap={groundSnap}
                resetViewTrigger={resetViewTrigger}
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
