import { Suspense, useRef, useEffect, useState, useMemo, useCallback } from 'react'
import { Canvas, useThree, ThreeEvent } from '@react-three/fiber'
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
  fitToDataTrigger?: number
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

interface InstancedDeathMarkersProps {
  events: DeathEvent[]
  selectedId?: string
  onSelect: (event: DeathEvent | null) => void
}

// Colors for marker states
const COLOR_NORMAL = new THREE.Color('#bf360c')
const COLOR_SELECTED = new THREE.Color('#ff6b6b')
const COLOR_HOVERED = new THREE.Color('#ff8a65')

function InstancedDeathMarkers({ events, selectedId, onSelect }: InstancedDeathMarkersProps) {
  const meshRef = useRef<THREE.InstancedMesh>(null)
  const [hoveredIndex, setHoveredIndex] = useState<number | null>(null)

  // Reusable objects for matrix calculations
  const tempObject = useMemo(() => new THREE.Object3D(), [])
  const tempColor = useMemo(() => new THREE.Color(), [])

  // Create geometry and material once
  const geometry = useMemo(() => new THREE.SphereGeometry(30, 16, 16), [])
  const material = useMemo(
    () =>
      new THREE.MeshStandardMaterial({
        vertexColors: true,
      }),
    []
  )

  // Find selected index
  const selectedIndex = useMemo(() => {
    if (!selectedId) return -1
    return events.findIndex((e) => e.id === selectedId)
  }, [events, selectedId])

  // Update instance matrices and colors
  useEffect(() => {
    if (!meshRef.current || events.length === 0) return

    const mesh = meshRef.current

    // Create color attribute if not present or size changed
    const colorArray = new Float32Array(events.length * 3)

    events.forEach((event, i) => {
      // Set position and scale
      const isSelected = i === selectedIndex
      const isHovered = i === hoveredIndex
      const scale = isSelected ? 1.5 : isHovered ? 1.2 : 1

      tempObject.position.set(event.x, event.z + 50, event.y)
      tempObject.scale.setScalar(scale)
      tempObject.updateMatrix()
      mesh.setMatrixAt(i, tempObject.matrix)

      // Set color
      if (isSelected) {
        tempColor.copy(COLOR_SELECTED)
      } else if (isHovered) {
        tempColor.copy(COLOR_HOVERED)
      } else {
        tempColor.copy(COLOR_NORMAL)
      }
      colorArray[i * 3] = tempColor.r
      colorArray[i * 3 + 1] = tempColor.g
      colorArray[i * 3 + 2] = tempColor.b
    })

    // Update instance matrix
    mesh.instanceMatrix.needsUpdate = true

    // Update colors via instance color attribute
    mesh.instanceColor = new THREE.InstancedBufferAttribute(colorArray, 3)
    mesh.instanceColor.needsUpdate = true
  }, [events, selectedIndex, hoveredIndex, tempObject, tempColor])

  // Cleanup
  useEffect(() => {
    return () => {
      geometry.dispose()
      material.dispose()
    }
  }, [geometry, material])

  const handleClick = useCallback(
    (e: ThreeEvent<MouseEvent>) => {
      e.stopPropagation()
      const instanceId = e.instanceId
      if (instanceId === undefined || instanceId < 0 || instanceId >= events.length) return

      const clickedEvent = events[instanceId]
      // Toggle selection
      if (clickedEvent.id === selectedId) {
        onSelect(null)
      } else {
        onSelect(clickedEvent)
      }
    },
    [events, selectedId, onSelect]
  )

  const handlePointerOver = useCallback(
    (e: ThreeEvent<PointerEvent>) => {
      e.stopPropagation()
      const instanceId = e.instanceId
      if (instanceId === undefined || instanceId < 0 || instanceId >= events.length) return

      setHoveredIndex(instanceId)
      document.body.style.cursor = 'pointer'
    },
    [events.length]
  )

  const handlePointerOut = useCallback(() => {
    setHoveredIndex(null)
    document.body.style.cursor = 'auto'
  }, [])

  if (events.length === 0) return null

  return (
    <instancedMesh
      ref={meshRef}
      args={[geometry, material, events.length]}
      onClick={handleClick}
      onPointerOver={handlePointerOver}
      onPointerOut={handlePointerOut}
    />
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

interface FitToDataControllerProps {
  events: DeathEvent[]
  trigger: number
}

function FitToDataController({ events, trigger }: FitToDataControllerProps) {
  const { camera, controls } = useThree()
  const prevTriggerRef = useRef(trigger)

  useEffect(() => {
    // Only fit when trigger changes (not on initial render)
    if (trigger === prevTriggerRef.current || events.length === 0) {
      prevTriggerRef.current = trigger
      return
    }
    prevTriggerRef.current = trigger

    // Calculate bounding box of all events
    const box = new THREE.Box3()
    events.forEach((event) => {
      box.expandByPoint(new THREE.Vector3(event.x, event.z + 50, event.y))
    })

    const center = box.getCenter(new THREE.Vector3())
    const size = box.getSize(new THREE.Vector3())

    // For orthographic camera, we need to adjust zoom based on the data extent
    if (camera instanceof THREE.OrthographicCamera) {
      const maxDim = Math.max(size.x, size.z)
      // Add some padding (20%)
      const paddedDim = maxDim * 1.2

      // Calculate zoom to fit the data
      // The orthographic camera's visible width at zoom=1 depends on the canvas size
      // We'll use a reasonable zoom value based on the data extent
      const targetZoom = Math.min(2, Math.max(0.01, 1000 / paddedDim))

      camera.position.set(center.x, 10000, center.z)
      camera.zoom = targetZoom
      camera.updateProjectionMatrix()
    }

    // Update controls target if available
    if (controls && 'target' in controls) {
      const mapControls = controls as { target: THREE.Vector3 }
      mapControls.target.set(center.x, 0, center.z)
    }
  }, [trigger, events, camera, controls])

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
  fitToDataTrigger = 0,
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
        <FitToDataController events={deathEvents} trigger={fitToDataTrigger} />

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

        <InstancedDeathMarkers
          events={deathEvents}
          selectedId={selectedEventId}
          onSelect={onSelectEvent}
        />
      </Canvas>
    </div>
  )
}
