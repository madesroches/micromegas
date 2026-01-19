import { Suspense, useRef, useEffect, useState } from 'react'
import { Canvas, useThree, useFrame } from '@react-three/fiber'
import { OrthographicCamera, MapControls, useGLTF, Html, Grid } from '@react-three/drei'
import * as THREE from 'three'

export interface DeathEvent {
  id: string
  time: Date
  processId: string
  x: number
  y: number
  z: number
  playerName?: string
  deathCause?: string
}

interface MapViewerProps {
  mapUrl?: string
  deathEvents: DeathEvent[]
  selectedEventId?: string
  onSelectEvent: (event: DeathEvent | null) => void
  showHeatmap: boolean
  heatmapRadius: number
  heatmapIntensity: number
}

function LoadingIndicator() {
  return (
    <Html center>
      <div className="flex items-center gap-2 bg-app-panel px-4 py-2 rounded-lg border border-theme-border">
        <div className="animate-spin rounded-full h-4 w-4 border-2 border-accent-link border-t-transparent" />
        <span className="text-sm text-theme-text-secondary">Loading map...</span>
      </div>
    </Html>
  )
}

interface MapModelProps {
  url: string
}

function MapModel({ url }: MapModelProps) {
  const { scene } = useGLTF(url)
  const clonedScene = scene.clone()

  useEffect(() => {
    clonedScene.traverse((child) => {
      if (child instanceof THREE.Mesh) {
        child.receiveShadow = true
      }
    })
  }, [clonedScene])

  return <primitive object={clonedScene} />
}

interface DeathMarkersProps {
  events: DeathEvent[]
  selectedId?: string
  onSelect: (event: DeathEvent | null) => void
}

function DeathMarkers({ events, selectedId, onSelect }: DeathMarkersProps) {
  const [hoveredId, setHoveredId] = useState<string | null>(null)

  if (events.length === 0) return null

  return (
    <group>
      {events.map((event) => {
        const isSelected = event.id === selectedId
        const isHovered = event.id === hoveredId
        const scale = isSelected ? 1.5 : isHovered ? 1.2 : 1

        return (
          <mesh
            key={event.id}
            position={[event.x, event.z + 50, event.y]}
            onClick={(e) => {
              e.stopPropagation()
              onSelect(isSelected ? null : event)
            }}
            onPointerOver={(e) => {
              e.stopPropagation()
              setHoveredId(event.id)
              document.body.style.cursor = 'pointer'
            }}
            onPointerOut={() => {
              setHoveredId(null)
              document.body.style.cursor = 'auto'
            }}
            scale={scale}
          >
            <sphereGeometry args={[30, 16, 16]} />
            <meshStandardMaterial
              color={isSelected ? '#ff6b6b' : '#bf360c'}
              emissive={isSelected ? '#ff6b6b' : isHovered ? '#bf360c' : '#000000'}
              emissiveIntensity={isSelected ? 0.5 : isHovered ? 0.3 : 0}
            />
          </mesh>
        )
      })}
    </group>
  )
}

interface HeatmapLayerProps {
  events: DeathEvent[]
  radius: number
  intensity: number
}

function HeatmapLayer({ events, radius, intensity }: HeatmapLayerProps) {
  const meshRef = useRef<THREE.Mesh>(null)
  const [texture, setTexture] = useState<THREE.CanvasTexture | null>(null)

  useEffect(() => {
    if (events.length === 0) {
      setTexture(null)
      return
    }

    const canvas = document.createElement('canvas')
    const size = 1024
    canvas.width = size
    canvas.height = size
    const ctx = canvas.getContext('2d')
    if (!ctx) return

    ctx.clearRect(0, 0, size, size)

    const minX = Math.min(...events.map((e) => e.x))
    const maxX = Math.max(...events.map((e) => e.x))
    const minY = Math.min(...events.map((e) => e.y))
    const maxY = Math.max(...events.map((e) => e.y))
    const padding = 1000
    const rangeX = maxX - minX + padding * 2
    const rangeY = maxY - minY + padding * 2

    events.forEach((event) => {
      const canvasX = ((event.x - minX + padding) / rangeX) * size
      const canvasY = ((event.y - minY + padding) / rangeY) * size

      const gradient = ctx.createRadialGradient(canvasX, canvasY, 0, canvasX, canvasY, radius)
      gradient.addColorStop(0, `rgba(191, 54, 12, ${intensity})`)
      gradient.addColorStop(0.5, `rgba(191, 54, 12, ${intensity * 0.5})`)
      gradient.addColorStop(1, 'rgba(191, 54, 12, 0)')

      ctx.fillStyle = gradient
      ctx.fillRect(canvasX - radius, canvasY - radius, radius * 2, radius * 2)
    })

    const tex = new THREE.CanvasTexture(canvas)
    tex.needsUpdate = true
    setTexture(tex)

    return () => {
      tex.dispose()
    }
  }, [events, radius, intensity])

  if (!texture || events.length === 0) return null

  const minX = Math.min(...events.map((e) => e.x))
  const maxX = Math.max(...events.map((e) => e.x))
  const minY = Math.min(...events.map((e) => e.y))
  const maxY = Math.max(...events.map((e) => e.y))
  const padding = 1000
  const width = maxX - minX + padding * 2
  const height = maxY - minY + padding * 2
  const centerX = (minX + maxX) / 2
  const centerY = (minY + maxY) / 2

  return (
    <mesh ref={meshRef} position={[centerX, 10, centerY]} rotation={[-Math.PI / 2, 0, 0]}>
      <planeGeometry args={[width, height]} />
      <meshBasicMaterial map={texture} transparent opacity={0.7} depthWrite={false} />
    </mesh>
  )
}

function CameraController() {
  const { camera } = useThree()

  useEffect(() => {
    camera.position.set(0, 10000, 0)
    camera.lookAt(0, 0, 0)
  }, [camera])

  return null
}

function PlaceholderGrid() {
  return (
    <Grid
      infiniteGrid
      cellSize={100}
      cellThickness={0.5}
      cellColor="#2a2a35"
      sectionSize={1000}
      sectionThickness={1}
      sectionColor="#3a3a45"
      fadeDistance={50000}
    />
  )
}

export function MapViewer({
  mapUrl,
  deathEvents,
  selectedEventId,
  onSelectEvent,
  showHeatmap,
  heatmapRadius,
  heatmapIntensity,
}: MapViewerProps) {
  return (
    <div className="w-full h-full">
      <Canvas
        gl={{ antialias: true, alpha: false }}
        onPointerMissed={() => onSelectEvent(null)}
      >
        <color attach="background" args={['#0a0a0f']} />

        <OrthographicCamera
          makeDefault
          position={[0, 10000, 0]}
          zoom={0.1}
          near={1}
          far={100000}
        />

        <CameraController />

        <MapControls
          enableRotate={false}
          enableDamping
          dampingFactor={0.1}
          minZoom={0.01}
          maxZoom={2}
          screenSpacePanning
        />

        <ambientLight intensity={0.6} />
        <directionalLight position={[5000, 10000, 5000]} intensity={0.8} />

        <Suspense fallback={<LoadingIndicator />}>
          {mapUrl ? <MapModel url={mapUrl} /> : <PlaceholderGrid />}
        </Suspense>

        {showHeatmap && (
          <HeatmapLayer events={deathEvents} radius={heatmapRadius} intensity={heatmapIntensity} />
        )}

        <DeathMarkers events={deathEvents} selectedId={selectedEventId} onSelect={onSelectEvent} />
      </Canvas>
    </div>
  )
}
