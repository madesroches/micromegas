import { Suspense, useRef, useEffect, useState, useMemo, useCallback } from 'react'
import { Canvas, useThree, useFrame, ThreeEvent } from '@react-three/fiber'
import { useGLTF, Html, Grid, PerspectiveCamera } from '@react-three/drei'
import * as THREE from 'three'

export interface MapEvent {
  id: string
  time: Date
  processId: string
  x: number
  y: number
  z: number
  /** Generic key-value properties from query result columns beyond x/y/z/time/process_id */
  properties: Record<string, string>
}

export interface MapBounds {
  min: { x: number; y: number; z: number }
  max: { x: number; y: number; z: number }
}

interface MapViewerProps {
  mapUrl?: string
  events: MapEvent[]
  selectedEventId?: string
  onSelectEvent: (event: MapEvent | null) => void
  showHeatmap: boolean
  heatmapRadius: number
  heatmapIntensity: number
  fitToDataTrigger?: number
  onMapBoundsChange?: (bounds: MapBounds | null) => void
  markerColor?: string
  markerSize?: number
  groundSnap?: boolean
  groundSnapOffset?: number
  resetViewTrigger?: number
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
  onBoundsCalculated: (bounds: THREE.Box3) => void
  onSceneReady?: (scene: THREE.Object3D) => void
}

function MapModel({ url, onBoundsCalculated, onSceneReady }: MapModelProps) {
  const { scene } = useGLTF(url)
  const clonedScene = useMemo(() => scene.clone(), [scene])

  useEffect(() => {
    clonedScene.traverse((child) => {
      if (child instanceof THREE.Mesh) {
        child.receiveShadow = true
        child.castShadow = true
      }
    })

    const box = new THREE.Box3().setFromObject(clonedScene)
    onBoundsCalculated(box)
    onSceneReady?.(clonedScene)
  }, [clonedScene, onBoundsCalculated, onSceneReady])

  return <primitive object={clonedScene} />
}

interface InstancedMarkersProps {
  events: MapEvent[]
  selectedId?: string
  onSelect: (event: MapEvent | null) => void
  markerColor?: string
  markerSize?: number
  mapScene?: THREE.Object3D | null
  groundSnap?: boolean
  groundSnapOffset?: number
}

const DEFAULT_MARKER_COLOR = '#bf360c'
const COLOR_SELECTED = new THREE.Color('#ff6b6b')
const COLOR_HOVERED = new THREE.Color('#ff8a65')

const DEFAULT_MARKER_SIZE = 10

const DEFAULT_GROUND_SNAP_OFFSET = 5

function InstancedMarkers({
  events,
  selectedId,
  onSelect,
  markerColor = DEFAULT_MARKER_COLOR,
  markerSize = DEFAULT_MARKER_SIZE,
  mapScene = null,
  groundSnap = false,
  groundSnapOffset = DEFAULT_GROUND_SNAP_OFFSET
}: InstancedMarkersProps) {
  const meshRef = useRef<THREE.InstancedMesh>(null)
  const [hoveredIndex, setHoveredIndex] = useState<number | null>(null)

  const tempObject = useMemo(() => new THREE.Object3D(), [])
  const tempColor = useMemo(() => new THREE.Color(), [])

  const raycaster = useMemo(() => new THREE.Raycaster(), [])
  const rayOrigin = useMemo(() => new THREE.Vector3(), [])
  const rayDirection = useMemo(() => new THREE.Vector3(0, -1, 0), [])

  const snappedPositions = useMemo(() => {
    if (!groundSnap || !mapScene || events.length === 0) {
      return null
    }

    const positions: { x: number; y: number; z: number }[] = []

    const mapBox = new THREE.Box3().setFromObject(mapScene)
    const rayStartHeight = mapBox.max.y + 1000

    events.forEach((event) => {
      rayOrigin.set(event.x, rayStartHeight, event.y)
      raycaster.set(rayOrigin, rayDirection)

      const intersects = raycaster.intersectObject(mapScene, true)

      if (intersects.length > 0) {
        const hit = intersects[0]
        positions.push({
          x: event.x,
          y: event.y,
          z: hit.point.y + groundSnapOffset
        })
      } else {
        positions.push({
          x: event.x,
          y: event.y,
          z: event.z
        })
      }
    })

    return positions
  }, [groundSnap, mapScene, events, raycaster, rayOrigin, rayDirection, groundSnapOffset])

  const geometry = useMemo(() => new THREE.SphereGeometry(1, 16, 16), [])
  const material = useMemo(
    () =>
      new THREE.MeshBasicMaterial({}),
    []
  )

  const selectedIndex = useMemo(() => {
    if (!selectedId) return -1
    return events.findIndex((e) => e.id === selectedId)
  }, [events, selectedId])

  useEffect(() => {
    if (!meshRef.current || events.length === 0) return

    const mesh = meshRef.current

    const colorArray = new Float32Array(events.length * 3)

    const normalColor = new THREE.Color(markerColor)

    events.forEach((event, i) => {
      const isSelected = i === selectedIndex
      const isHovered = i === hoveredIndex
      const scaleMultiplier = isSelected ? 1.5 : isHovered ? 1.2 : 1
      const finalScale = markerSize * scaleMultiplier

      const pos = snappedPositions ? snappedPositions[i] : { x: event.x, y: event.y, z: event.z + 50 }
      tempObject.position.set(pos.x, pos.z, pos.y)
      tempObject.scale.setScalar(finalScale)
      tempObject.updateMatrix()
      mesh.setMatrixAt(i, tempObject.matrix)

      if (isSelected) {
        tempColor.copy(COLOR_SELECTED)
      } else if (isHovered) {
        tempColor.copy(COLOR_HOVERED)
      } else {
        tempColor.copy(normalColor)
      }
      colorArray[i * 3] = tempColor.r
      colorArray[i * 3 + 1] = tempColor.g
      colorArray[i * 3 + 2] = tempColor.b
    })

    mesh.instanceMatrix.needsUpdate = true

    mesh.instanceColor = new THREE.InstancedBufferAttribute(colorArray, 3)
    mesh.instanceColor.needsUpdate = true
  }, [events, selectedIndex, hoveredIndex, tempObject, tempColor, markerColor, markerSize, snappedPositions])

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
  events: MapEvent[]
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

interface UnrealCameraControllerProps {
  mapBounds: THREE.Box3 | null
  fitToMapTrigger: number
  fitToDataTrigger: number
  events: MapEvent[]
  onSpeedChange?: (speed: number) => void
  resetViewTrigger: number
}

function UnrealCameraController({
  mapBounds,
  fitToMapTrigger,
  fitToDataTrigger,
  events,
  onSpeedChange,
  resetViewTrigger,
}: UnrealCameraControllerProps) {
  const { camera, gl } = useThree()
  const domElement = gl.domElement

  const targetRef = useRef(new THREE.Vector3(0, 0, 0))
  const sphericalRef = useRef(new THREE.Spherical(5000, Math.PI / 4, 0))

  const initialViewRef = useRef<{
    target: THREE.Vector3
    spherical: { radius: number; phi: number; theta: number }
  } | null>(null)

  const baseSpeedRef = useRef(2000)
  const MIN_SPEED = 10
  const MAX_SPEED = 50000

  const fitRadiusRef = useRef(5000)
  const zoomFactorRef = useRef(1.0)

  const isLeftMouseDownRef = useRef(false)
  const isLeftDraggingRef = useRef(false)
  const leftMouseStartRef = useRef({ x: 0, y: 0 })
  const isRightMouseDownRef = useRef(false)
  const isMiddleMouseDownRef = useRef(false)
  const lastMouseRef = useRef({ x: 0, y: 0 })
  const keysRef = useRef({
    w: false,
    a: false,
    s: false,
    d: false,
    q: false,
    e: false,
    shift: false,
  })

  useEffect(() => {
    onSpeedChange?.(baseSpeedRef.current)
  }, [onSpeedChange])

  const fitToBounds = useCallback(
    (box: THREE.Box3) => {
      const center = box.getCenter(new THREE.Vector3())
      const size = box.getSize(new THREE.Vector3())

      const perspCamera = camera as THREE.PerspectiveCamera
      const fovRad = perspCamera.fov * (Math.PI / 180)
      const aspect = perspCamera.aspect

      const distForZ = (size.z / 2) / Math.tan(fovRad / 2)
      const distForX = (size.x / 2) / (Math.tan(fovRad / 2) * aspect)
      const distance = Math.max(distForZ, distForX) * 1.05

      targetRef.current.copy(center)
      sphericalRef.current.radius = distance
      sphericalRef.current.phi = 0.001
      sphericalRef.current.theta = 0

      fitRadiusRef.current = distance
      zoomFactorRef.current = 1.0

      const offset = new THREE.Vector3()
      offset.setFromSpherical(sphericalRef.current)
      camera.position.copy(targetRef.current).add(offset)
      camera.lookAt(targetRef.current)
    },
    [camera]
  )

  const saveInitialView = useCallback(() => {
    initialViewRef.current = {
      target: targetRef.current.clone(),
      spherical: {
        radius: sphericalRef.current.radius,
        phi: sphericalRef.current.phi,
        theta: sphericalRef.current.theta,
      },
    }
  }, [])

  const prevFitToMapTriggerRef = useRef(fitToMapTrigger)
  useEffect(() => {
    if (mapBounds && fitToMapTrigger !== prevFitToMapTriggerRef.current) {
      prevFitToMapTriggerRef.current = fitToMapTrigger
      fitToBounds(mapBounds)
      saveInitialView()
    }
  }, [mapBounds, fitToMapTrigger, fitToBounds, saveInitialView])

  const hasAutoFitRef = useRef(false)
  useEffect(() => {
    if (mapBounds && !hasAutoFitRef.current) {
      hasAutoFitRef.current = true
      fitToBounds(mapBounds)
      saveInitialView()
    }
  }, [mapBounds, fitToBounds, saveInitialView])

  const prevFitToDataTriggerRef = useRef(fitToDataTrigger)
  useEffect(() => {
    if (fitToDataTrigger !== prevFitToDataTriggerRef.current && events.length > 0) {
      prevFitToDataTriggerRef.current = fitToDataTrigger
      const box = new THREE.Box3()
      events.forEach((event) => {
        box.expandByPoint(new THREE.Vector3(event.x, event.z + 50, event.y))
      })
      fitToBounds(box)
    }
  }, [fitToDataTrigger, events, fitToBounds])

  const prevResetViewTriggerRef = useRef(resetViewTrigger)
  useEffect(() => {
    if (resetViewTrigger !== prevResetViewTriggerRef.current && initialViewRef.current) {
      prevResetViewTriggerRef.current = resetViewTrigger
      targetRef.current.copy(initialViewRef.current.target)
      sphericalRef.current.radius = initialViewRef.current.spherical.radius
      sphericalRef.current.phi = initialViewRef.current.spherical.phi
      sphericalRef.current.theta = initialViewRef.current.spherical.theta
      zoomFactorRef.current = 1.0

      const offset = new THREE.Vector3()
      offset.setFromSpherical(sphericalRef.current)
      camera.position.copy(targetRef.current).add(offset)
      camera.lookAt(targetRef.current)
    }
  }, [resetViewTrigger, camera])

  useEffect(() => {
    const DRAG_THRESHOLD = 4

    const onMouseDown = (e: MouseEvent) => {
      if (e.button === 0) {
        isLeftMouseDownRef.current = true
        isLeftDraggingRef.current = false
        leftMouseStartRef.current = { x: e.clientX, y: e.clientY }
        lastMouseRef.current = { x: e.clientX, y: e.clientY }
      } else if (e.button === 2) {
        isRightMouseDownRef.current = true
        lastMouseRef.current = { x: e.clientX, y: e.clientY }
        domElement.style.cursor = 'grabbing'
      } else if (e.button === 1) {
        isMiddleMouseDownRef.current = true
        lastMouseRef.current = { x: e.clientX, y: e.clientY }
        domElement.style.cursor = 'move'
        e.preventDefault()
      }
    }

    const onMouseUp = (e: MouseEvent) => {
      if (e.button === 0) {
        isLeftMouseDownRef.current = false
        if (isLeftDraggingRef.current) {
          domElement.style.cursor = 'auto'
        }
        isLeftDraggingRef.current = false
      } else if (e.button === 2) {
        isRightMouseDownRef.current = false
        domElement.style.cursor = 'auto'
      } else if (e.button === 1) {
        isMiddleMouseDownRef.current = false
        domElement.style.cursor = 'auto'
      }
    }

    const panCamera = (deltaX: number, deltaY: number) => {
      const panSpeed = sphericalRef.current.radius * 0.001
      const right = new THREE.Vector3()
      const up = new THREE.Vector3(0, 1, 0)

      camera.getWorldDirection(right)
      right.crossVectors(up, right).normalize()

      const forward = new THREE.Vector3()
      camera.getWorldDirection(forward)
      forward.y = 0
      forward.normalize()

      targetRef.current.addScaledVector(right, deltaX * panSpeed)
      targetRef.current.addScaledVector(forward, deltaY * panSpeed)
    }

    const onMouseMove = (e: MouseEvent) => {
      const deltaX = e.clientX - lastMouseRef.current.x
      const deltaY = e.clientY - lastMouseRef.current.y
      lastMouseRef.current = { x: e.clientX, y: e.clientY }

      if (isLeftMouseDownRef.current) {
        if (!isLeftDraggingRef.current) {
          const dx = e.clientX - leftMouseStartRef.current.x
          const dy = e.clientY - leftMouseStartRef.current.y
          if (Math.sqrt(dx * dx + dy * dy) > DRAG_THRESHOLD) {
            isLeftDraggingRef.current = true
            domElement.style.cursor = 'grab'
          }
        }
        if (isLeftDraggingRef.current) {
          panCamera(deltaX, deltaY)
        }
      }

      if (isRightMouseDownRef.current) {
        const rotateSpeed = 0.005
        sphericalRef.current.theta -= deltaX * rotateSpeed
        sphericalRef.current.phi += deltaY * rotateSpeed

        sphericalRef.current.phi = Math.max(0.1, Math.min(Math.PI - 0.1, sphericalRef.current.phi))
      }

      if (isMiddleMouseDownRef.current) {
        panCamera(deltaX, deltaY)
      }
    }

    const onWheel = (e: WheelEvent) => {
      e.preventDefault()

      if (isRightMouseDownRef.current) {
        const speedMultiplier = e.deltaY > 0 ? 0.8 : 1.25
        baseSpeedRef.current = Math.max(
          MIN_SPEED,
          Math.min(MAX_SPEED, baseSpeedRef.current * speedMultiplier)
        )
        onSpeedChange?.(baseSpeedRef.current)
      } else {
        const zoomSpeed = 0.1
        const zoomMultiplier = e.deltaY > 0 ? (1 + zoomSpeed) : (1 - zoomSpeed)
        zoomFactorRef.current *= zoomMultiplier
        zoomFactorRef.current = Math.max(0.01, Math.min(1.0, zoomFactorRef.current))
        sphericalRef.current.radius = fitRadiusRef.current * zoomFactorRef.current
      }
    }

    const onContextMenu = (e: MouseEvent) => {
      e.preventDefault()
    }

    const onKeyDown = (e: KeyboardEvent) => {
      const key = e.key.toLowerCase()
      if (key in keysRef.current) {
        keysRef.current[key as keyof typeof keysRef.current] = true
      }
      if (e.key === 'Shift') {
        keysRef.current.shift = true
      }
    }

    const onKeyUp = (e: KeyboardEvent) => {
      const key = e.key.toLowerCase()
      if (key in keysRef.current) {
        keysRef.current[key as keyof typeof keysRef.current] = false
      }
      if (e.key === 'Shift') {
        keysRef.current.shift = false
      }
    }

    domElement.addEventListener('mousedown', onMouseDown)
    domElement.addEventListener('mouseup', onMouseUp)
    domElement.addEventListener('mousemove', onMouseMove)
    domElement.addEventListener('wheel', onWheel, { passive: false })
    domElement.addEventListener('contextmenu', onContextMenu)
    window.addEventListener('keydown', onKeyDown)
    window.addEventListener('keyup', onKeyUp)

    return () => {
      domElement.removeEventListener('mousedown', onMouseDown)
      domElement.removeEventListener('mouseup', onMouseUp)
      domElement.removeEventListener('mousemove', onMouseMove)
      domElement.removeEventListener('wheel', onWheel)
      domElement.removeEventListener('contextmenu', onContextMenu)
      window.removeEventListener('keydown', onKeyDown)
      window.removeEventListener('keyup', onKeyUp)
    }
  }, [camera, domElement])

  useFrame((_, delta) => {
    if (isRightMouseDownRef.current) {
      const speed = keysRef.current.shift ? baseSpeedRef.current * 2.5 : baseSpeedRef.current
      const moveSpeed = speed * delta

      const forward = new THREE.Vector3()
      camera.getWorldDirection(forward)
      forward.normalize()

      const right = new THREE.Vector3()
      right.crossVectors(forward, new THREE.Vector3(0, 1, 0)).normalize()

      if (keysRef.current.w) {
        targetRef.current.addScaledVector(forward, moveSpeed)
      }
      if (keysRef.current.s) {
        targetRef.current.addScaledVector(forward, -moveSpeed)
      }
      if (keysRef.current.a) {
        targetRef.current.addScaledVector(right, -moveSpeed)
      }
      if (keysRef.current.d) {
        targetRef.current.addScaledVector(right, moveSpeed)
      }
      if (keysRef.current.e) {
        targetRef.current.y += moveSpeed
      }
      if (keysRef.current.q) {
        targetRef.current.y -= moveSpeed
      }
    }

    const offset = new THREE.Vector3()
    offset.setFromSpherical(sphericalRef.current)
    camera.position.copy(targetRef.current).add(offset)
    camera.lookAt(targetRef.current)
  })

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
  events,
  selectedEventId,
  onSelectEvent,
  showHeatmap,
  heatmapRadius,
  heatmapIntensity,
  fitToDataTrigger = 0,
  onMapBoundsChange,
  markerColor,
  markerSize,
  groundSnap = false,
  groundSnapOffset,
  resetViewTrigger = 0,
}: MapViewerProps) {
  const [mapBounds, setMapBounds] = useState<THREE.Box3 | null>(null)
  const [mapScene, setMapScene] = useState<THREE.Object3D | null>(null)
  const [fitToMapTrigger, setFitToMapTrigger] = useState(0)
  const [currentSpeed, setCurrentSpeed] = useState(2000)

  const handleSpeedChange = useCallback((speed: number) => {
    setCurrentSpeed(speed)
  }, [])

  const handleBoundsCalculated = useCallback(
    (bounds: THREE.Box3) => {
      setMapBounds(bounds)
      setFitToMapTrigger((prev) => prev + 1)

      if (onMapBoundsChange) {
        onMapBoundsChange({
          min: { x: bounds.min.x, y: bounds.min.z, z: bounds.min.y },
          max: { x: bounds.max.x, y: bounds.max.z, z: bounds.max.y },
        })
      }
    },
    [onMapBoundsChange]
  )

  const handleSceneReady = useCallback((scene: THREE.Object3D) => {
    setMapScene(scene)
  }, [])

  useEffect(() => {
    if (!mapUrl) {
      setMapScene(null)
      if (onMapBoundsChange) {
        onMapBoundsChange(null)
      }
    }
  }, [mapUrl, onMapBoundsChange])

  return (
    <div className="w-full h-full">
      <Canvas gl={{ antialias: true, alpha: false }} onPointerMissed={() => onSelectEvent(null)}>
        <color attach="background" args={['#0a0a0f']} />

        <PerspectiveCamera makeDefault position={[0, 5000, 5000]} fov={60} near={1} far={100000} />

        <UnrealCameraController
          mapBounds={mapBounds}
          fitToMapTrigger={fitToMapTrigger}
          fitToDataTrigger={fitToDataTrigger}
          events={events}
          onSpeedChange={handleSpeedChange}
          resetViewTrigger={resetViewTrigger}
        />

        <ambientLight intensity={0.8} />
        <hemisphereLight args={['#ffffff', '#444444', 0.6]} />
        <directionalLight position={[5000, 10000, 5000]} intensity={1.0} />
        <directionalLight position={[-5000, 10000, -5000]} intensity={0.6} />
        <directionalLight position={[0, -10000, 0]} intensity={0.3} />

        <Suspense fallback={<LoadingIndicator />}>
          {mapUrl ? (
            <MapModel
              url={mapUrl}
              onBoundsCalculated={handleBoundsCalculated}
              onSceneReady={handleSceneReady}
            />
          ) : (
            <PlaceholderGrid />
          )}
        </Suspense>

        {showHeatmap && (
          <HeatmapLayer events={events} radius={heatmapRadius} intensity={heatmapIntensity} />
        )}

        <InstancedMarkers
          key={`markers-${markerColor}-${markerSize}-${groundSnap}`}
          events={events}
          selectedId={selectedEventId}
          onSelect={onSelectEvent}
          markerColor={markerColor}
          markerSize={markerSize}
          mapScene={mapScene}
          groundSnap={groundSnap}
          groundSnapOffset={groundSnapOffset}
        />
      </Canvas>

      <div className="absolute bottom-4 right-4 bg-app-bg border border-theme-border rounded-lg px-3 py-2 text-xs text-theme-text-muted">
        <div className="font-semibold text-theme-text-secondary mb-1">Controls</div>
        <div>Left-click + drag: Pan</div>
        <div>Right-click + drag: Rotate</div>
        <div>Scroll: Zoom</div>
        <div>Right-click + Scroll: Speed</div>
        <div>WASD + Right-click: Fly</div>
        <div>Q/E: Up/Down | Shift: Boost</div>
        <div className="mt-2 pt-2 border-t border-theme-border text-theme-text-secondary">
          Speed: <span className="font-mono text-accent-link">{currentSpeed.toLocaleString()}</span>
        </div>
      </div>
    </div>
  )
}
