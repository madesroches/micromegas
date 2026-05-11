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

export interface MMAmbientLight {
  color: [number, number, number]
  intensity: number
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
  resetViewTrigger?: number
  heightOffset?: number
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

export interface MapLoadPayload {
  scene: THREE.Object3D
  bounds: THREE.Box3
  glbCamera: THREE.PerspectiveCamera | null
  ambientLight: MMAmbientLight | null
}

interface MapModelProps {
  url: string
  onLoaded: (payload: MapLoadPayload) => void
}

function MapModel({ url, onLoaded }: MapModelProps) {
  const gltf = useGLTF(url)
  const clonedScene = useMemo(() => gltf.scene.clone(), [gltf.scene])

  useEffect(() => {
    clonedScene.traverse((child) => {
      if (child instanceof THREE.Mesh) {
        child.receiveShadow = true
        child.castShadow = true
      }
    })

    // Camera world transform lives on the parent node; refresh matrices before reading.
    gltf.scene.updateMatrixWorld(true)

    const cam = gltf.cameras[0]
    const glbCamera = cam instanceof THREE.PerspectiveCamera ? cam : null

    const ambientExt = gltf.parser.json.extensions?.MM_ambient_light as
      | { color?: unknown; intensity?: unknown }
      | undefined
    const color = ambientExt?.color
    const intensity = ambientExt?.intensity
    const ambientLight: MMAmbientLight | null =
      ambientExt &&
      Array.isArray(color) &&
      color.length === 3 &&
      color.every((c) => typeof c === 'number') &&
      typeof intensity === 'number'
        ? { color: [color[0], color[1], color[2]], intensity }
        : null

    const bounds = new THREE.Box3().setFromObject(clonedScene)
    onLoaded({ scene: clonedScene, bounds, glbCamera, ambientLight })
  }, [clonedScene, gltf, onLoaded])

  return <primitive object={clonedScene} />
}

interface InstancedMarkersProps {
  events: MapEvent[]
  selectedId?: string
  onSelect: (event: MapEvent | null) => void
  markerColor?: string
  markerSize?: number
  heightOffset?: number
  mapScene?: THREE.Object3D | null
  groundSnap?: boolean
}

const DEFAULT_MARKER_COLOR = '#bf360c'
const COLOR_SELECTED = new THREE.Color('#ff6b6b')
const COLOR_HOVERED = new THREE.Color('#ff8a65')

const DEFAULT_MARKER_SIZE = 10

function InstancedMarkers({
  events,
  selectedId,
  onSelect,
  markerColor = DEFAULT_MARKER_COLOR,
  markerSize = DEFAULT_MARKER_SIZE,
  heightOffset = 0,
  mapScene = null,
  groundSnap = false,
}: InstancedMarkersProps) {
  const meshRef = useRef<THREE.InstancedMesh>(null)
  const [hoveredIndex, setHoveredIndex] = useState<number | null>(null)

  const tempObject = useMemo(() => new THREE.Object3D(), [])
  const tempColor = useMemo(() => new THREE.Color(), [])

  const raycaster = useMemo(() => new THREE.Raycaster(), [])
  const rayOrigin = useMemo(() => new THREE.Vector3(), [])
  const rayDirection = useMemo(() => new THREE.Vector3(0, 0, -1), [])

  const snappedPositions = useMemo(() => {
    if (!groundSnap || !mapScene || events.length === 0) {
      return null
    }

    const positions: { x: number; y: number; z: number }[] = []

    const mapBox = new THREE.Box3().setFromObject(mapScene)
    const rayStartHeight = mapBox.max.z + 1000

    events.forEach((event) => {
      rayOrigin.set(event.x, event.y, rayStartHeight)
      raycaster.set(rayOrigin, rayDirection)

      const intersects = raycaster.intersectObject(mapScene, true)

      if (intersects.length > 0) {
        const hit = intersects[0]
        positions.push({
          x: event.x,
          y: event.y,
          z: hit.point.z + heightOffset
        })
      } else {
        positions.push({
          x: event.x,
          y: event.y,
          z: event.z + heightOffset
        })
      }
    })

    return positions
  }, [groundSnap, mapScene, events, raycaster, rayOrigin, rayDirection, heightOffset])

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

  const colorAttrRef = useRef<THREE.InstancedBufferAttribute | null>(null)

  useEffect(() => {
    if (!meshRef.current || events.length === 0) return

    const mesh = meshRef.current

    const normalColor = new THREE.Color(markerColor)

    events.forEach((event, i) => {
      const isSelected = i === selectedIndex
      const isHovered = i === hoveredIndex
      const scaleMultiplier = isSelected ? 1.5 : isHovered ? 1.2 : 1
      const finalScale = markerSize * scaleMultiplier

      const pos = snappedPositions ? snappedPositions[i] : { x: event.x, y: event.y, z: event.z + heightOffset }
      tempObject.position.set(pos.x, pos.y, pos.z)
      tempObject.scale.setScalar(finalScale)
      tempObject.updateMatrix()
      mesh.setMatrixAt(i, tempObject.matrix)
    })

    mesh.instanceMatrix.needsUpdate = true

    if (!colorAttrRef.current || colorAttrRef.current.count !== events.length) {
      const colorArray = new Float32Array(events.length * 3)
      colorAttrRef.current = new THREE.InstancedBufferAttribute(colorArray, 3)
      mesh.instanceColor = colorAttrRef.current
    }

    const attr = colorAttrRef.current
    events.forEach((_event, i) => {
      if (i === selectedIndex) {
        tempColor.copy(COLOR_SELECTED)
      } else if (i === hoveredIndex) {
        tempColor.copy(COLOR_HOVERED)
      } else {
        tempColor.copy(normalColor)
      }
      attr.setXYZ(i, tempColor.r, tempColor.g, tempColor.b)
    })
    attr.needsUpdate = true
  }, [events, selectedIndex, hoveredIndex, tempObject, tempColor, markerColor, markerSize, heightOffset, snappedPositions])

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

const HEATMAP_PADDING = 1000

function HeatmapLayer({ events, radius, intensity }: HeatmapLayerProps) {
  const meshRef = useRef<THREE.Mesh>(null)
  const [texture, setTexture] = useState<THREE.CanvasTexture | null>(null)

  // Single pass over events; spread-based Math.min(...arr) blows the stack at large counts.
  const bounds = useMemo(() => {
    if (events.length === 0) return null
    let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity
    for (const e of events) {
      if (e.x < minX) minX = e.x
      if (e.x > maxX) maxX = e.x
      if (e.y < minY) minY = e.y
      if (e.y > maxY) maxY = e.y
    }
    return { minX, maxX, minY, maxY }
  }, [events])

  useEffect(() => {
    if (!bounds) {
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

    const rangeX = bounds.maxX - bounds.minX + HEATMAP_PADDING * 2
    const rangeY = bounds.maxY - bounds.minY + HEATMAP_PADDING * 2

    events.forEach((event) => {
      const canvasX = ((event.x - bounds.minX + HEATMAP_PADDING) / rangeX) * size
      const canvasY = ((event.y - bounds.minY + HEATMAP_PADDING) / rangeY) * size

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
  }, [events, bounds, radius, intensity])

  if (!texture || !bounds) return null

  const width = bounds.maxX - bounds.minX + HEATMAP_PADDING * 2
  const height = bounds.maxY - bounds.minY + HEATMAP_PADDING * 2
  const centerX = (bounds.minX + bounds.maxX) / 2
  const centerY = (bounds.minY + bounds.maxY) / 2

  return (
    <mesh ref={meshRef} position={[centerX, centerY, 10]}>
      <planeGeometry args={[width, height]} />
      <meshBasicMaterial map={texture} transparent opacity={0.7} depthWrite={false} />
    </mesh>
  )
}

/**
 * Reinterpret a Y-up offset (decoded from THREE.Spherical) as a Z-up world offset.
 * THREE.Spherical is hard-coded Y-up: phi=0 places the offset along +Y. In a Z-up
 * scene we want phi=0 to mean "world up" (+Z), so permute (x, y, z) → (x, -z, y).
 */
function sphericalToZUpOffset(spherical: THREE.Spherical, out: THREE.Vector3): THREE.Vector3 {
  out.setFromSpherical(spherical)
  const tmp = out.y
  out.y = -out.z
  out.z = tmp
  return out
}

/**
 * Inverse of sphericalToZUpOffset: convert a Z-up world offset back into a vector
 * suitable for THREE.Spherical.setFromVector3 (which expects Y-up). Permute
 * (x, y, z) → (x, z, -y).
 */
function zUpOffsetToSphericalInput(offset: THREE.Vector3, out: THREE.Vector3): THREE.Vector3 {
  out.set(offset.x, offset.z, -offset.y)
  return out
}

interface UnrealCameraControllerProps {
  mapBounds: THREE.Box3 | null
  mapScene: THREE.Object3D | null
  fitToDataTrigger: number
  events: MapEvent[]
  onSpeedChange?: (speed: number) => void
  resetViewTrigger: number
  glbCamera: THREE.PerspectiveCamera | null
}

function UnrealCameraController({
  mapBounds,
  mapScene,
  fitToDataTrigger,
  events,
  onSpeedChange,
  resetViewTrigger,
  glbCamera,
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

  // Track latest mapScene through a ref so mousedown can raycast against it
  // without rebinding the DOM event handlers when the scene changes.
  const mapSceneRef = useRef<THREE.Object3D | null>(mapScene)
  useEffect(() => {
    mapSceneRef.current = mapScene
  }, [mapScene])

  const fitToBounds = useCallback(
    (box: THREE.Box3) => {
      const center = box.getCenter(new THREE.Vector3())
      const size = box.getSize(new THREE.Vector3())

      const perspCamera = camera as THREE.PerspectiveCamera
      const fovRad = perspCamera.fov * (Math.PI / 180)
      const aspect = perspCamera.aspect

      // Z-up scene with overhead camera: screen-vertical maps to world Y, screen-horizontal to world X.
      const distForY = (size.y / 2) / Math.tan(fovRad / 2)
      const distForX = (size.x / 2) / (Math.tan(fovRad / 2) * aspect)
      const distance = Math.max(distForY, distForX) * 1.05

      targetRef.current.copy(center)
      sphericalRef.current.radius = distance
      sphericalRef.current.phi = 0.001
      sphericalRef.current.theta = 0

      fitRadiusRef.current = distance
      zoomFactorRef.current = 1.0

      const offset = new THREE.Vector3()
      sphericalToZUpOffset(sphericalRef.current, offset)
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

  const prevFitToDataTriggerRef = useRef(fitToDataTrigger)
  useEffect(() => {
    if (fitToDataTrigger !== prevFitToDataTriggerRef.current && events.length > 0) {
      prevFitToDataTriggerRef.current = fitToDataTrigger
      const box = new THREE.Box3()
      events.forEach((event) => {
        box.expandByPoint(new THREE.Vector3(event.x, event.y, event.z + 50))
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
      sphericalToZUpOffset(sphericalRef.current, offset)
      camera.position.copy(targetRef.current).add(offset)
      camera.lookAt(targetRef.current)
    }
  }, [resetViewTrigger, camera])

  // Seed orbit state from the GLB's embedded camera once it resolves.
  const seededGlbCameraRef = useRef<THREE.PerspectiveCamera | null>(null)
  useEffect(() => {
    if (!glbCamera || seededGlbCameraRef.current === glbCamera) return
    seededGlbCameraRef.current = glbCamera

    const cameraPos = glbCamera.getWorldPosition(new THREE.Vector3())
    const worldQuat = glbCamera.getWorldQuaternion(new THREE.Quaternion())
    const forward = new THREE.Vector3(0, 0, -1).applyQuaternion(worldQuat)

    let radius: number
    if (mapBounds) {
      const sphere = mapBounds.getBoundingSphere(new THREE.Sphere())
      radius = sphere.radius * 2
    } else {
      radius = Math.max(cameraPos.length(), 1000)
    }

    const target = cameraPos.clone().addScaledVector(forward, radius)
    targetRef.current.copy(target)

    const worldOffset = cameraPos.clone().sub(target)
    const sphericalInput = zUpOffsetToSphericalInput(worldOffset, new THREE.Vector3())
    sphericalRef.current.setFromVector3(sphericalInput)

    fitRadiusRef.current = sphericalRef.current.radius
    zoomFactorRef.current = 1.0

    // Copy intrinsics onto the scene camera.
    const perspCamera = camera as THREE.PerspectiveCamera
    perspCamera.fov = glbCamera.fov
    perspCamera.near = glbCamera.near
    perspCamera.far = glbCamera.far
    perspCamera.updateProjectionMatrix()

    const offset = new THREE.Vector3()
    sphericalToZUpOffset(sphericalRef.current, offset)
    camera.position.copy(targetRef.current).add(offset)
    camera.lookAt(targetRef.current)

    saveInitialView()
  }, [glbCamera, mapBounds, camera, saveInitialView])

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

        // Re-anchor the orbit pivot to whatever the camera is currently
        // looking at, so right-drag rotates around the visible POI rather
        // than a stale target left over from a previous fit/fly.
        const scene = mapSceneRef.current
        if (scene) {
          const rc = new THREE.Raycaster()
          rc.setFromCamera(new THREE.Vector2(0, 0), camera)
          const hits = rc.intersectObject(scene, true)
          if (hits.length > 0) {
            const newTarget = hits[0].point
            const offset = new THREE.Vector3().copy(camera.position).sub(newTarget)
            const sphericalInput = zUpOffsetToSphericalInput(offset, new THREE.Vector3())
            sphericalRef.current.setFromVector3(sphericalInput)
            // Keep zoom math (radius = fitRadius * zoomFactor) consistent
            // by absorbing the new radius into fitRadius.
            fitRadiusRef.current = sphericalRef.current.radius / zoomFactorRef.current
            targetRef.current.copy(newTarget)
          }
        }
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
      const up = new THREE.Vector3(0, 0, 1)

      camera.getWorldDirection(right)
      right.crossVectors(up, right).normalize()

      const forward = new THREE.Vector3()
      camera.getWorldDirection(forward)
      forward.z = 0
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
        sphericalRef.current.theta += deltaX * rotateSpeed
        sphericalRef.current.phi += deltaY * rotateSpeed

        // Keep the camera above the horizon — flipping below the map is
        // disorienting and rarely useful for a top-down scene.
        sphericalRef.current.phi = Math.max(
          0.05,
          Math.min(Math.PI / 2 - 0.05, sphericalRef.current.phi)
        )
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
      right.crossVectors(forward, new THREE.Vector3(0, 0, 1)).normalize()

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
        targetRef.current.z += moveSpeed
      }
      if (keysRef.current.q) {
        targetRef.current.z -= moveSpeed
      }
    }

    const offset = new THREE.Vector3()
    sphericalToZUpOffset(sphericalRef.current, offset)
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

function SceneSetup() {
  const { scene, camera } = useThree()
  useEffect(() => {
    scene.up.set(0, 0, 1)
    camera.up.set(0, 0, 1)
  }, [scene, camera])
  return null
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
  resetViewTrigger = 0,
  heightOffset: heightOffsetProp,
}: MapViewerProps) {
  const [mapBounds, setMapBounds] = useState<THREE.Box3 | null>(null)
  const [mapScene, setMapScene] = useState<THREE.Object3D | null>(null)
  const [glbCamera, setGlbCamera] = useState<THREE.PerspectiveCamera | null>(null)
  const [ambientLight, setAmbientLight] = useState<MMAmbientLight | null>(null)
  const [currentSpeed, setCurrentSpeed] = useState(2000)

  const handleSpeedChange = useCallback((speed: number) => {
    setCurrentSpeed(speed)
  }, [])

  const handleMapLoaded = useCallback(
    (payload: MapLoadPayload) => {
      setMapBounds(payload.bounds)
      setMapScene(payload.scene)
      setGlbCamera(payload.glbCamera)
      setAmbientLight(payload.ambientLight)

      if (onMapBoundsChange) {
        onMapBoundsChange({
          min: { x: payload.bounds.min.x, y: payload.bounds.min.y, z: payload.bounds.min.z },
          max: { x: payload.bounds.max.x, y: payload.bounds.max.y, z: payload.bounds.max.z },
        })
      }

      if (payload.ambientLight === null) {
        console.error(
          `[MapViewer] GLB ${mapUrl} is missing MM_ambient_light extension; ambient lighting will be absent`
        )
      }
      if (payload.glbCamera === null) {
        console.error(
          `[MapViewer] GLB ${mapUrl} has no perspective camera; initial framing may be wrong`
        )
      }
    },
    [onMapBoundsChange, mapUrl]
  )

  const effectiveMarkerSize = useMemo(() => {
    if (mapBounds) {
      const size = mapBounds.getSize(new THREE.Vector3())
      const extent = Math.max(size.x, size.y)
      return (markerSize ?? DEFAULT_MARKER_SIZE) * extent * 0.00025
    }
    return markerSize ?? DEFAULT_MARKER_SIZE
  }, [markerSize, mapBounds])

  const heightOffset = useMemo(() => {
    if (mapBounds) {
      const size = mapBounds.getSize(new THREE.Vector3())
      const extent = Math.max(size.x, size.y)
      if (heightOffsetProp !== undefined) {
        return heightOffsetProp * extent * 0.01
      }
      return extent * 0.005
    }
    return heightOffsetProp ?? 50
  }, [mapBounds, heightOffsetProp])

  useEffect(() => {
    if (!mapUrl) {
      setMapBounds(null)
      setMapScene(null)
      setGlbCamera(null)
      setAmbientLight(null)
      if (onMapBoundsChange) {
        onMapBoundsChange(null)
      }
    }
  }, [mapUrl, onMapBoundsChange])

  return (
    <div className="w-full h-full">
      <Canvas
        gl={{
          antialias: true,
          alpha: false,
          toneMapping: THREE.NeutralToneMapping,
          toneMappingExposure: 1.3,
        }}
        onPointerMissed={() => onSelectEvent(null)}
      >
        <SceneSetup />
        <color attach="background" args={['#0a0a0f']} />

        <PerspectiveCamera makeDefault position={[0, 5000, 5000]} fov={60} near={1} far={100000} />

        <UnrealCameraController
          mapBounds={mapBounds}
          mapScene={mapScene}
          fitToDataTrigger={fitToDataTrigger}
          events={events}
          onSpeedChange={handleSpeedChange}
          resetViewTrigger={resetViewTrigger}
          glbCamera={glbCamera}
        />

        {ambientLight && (
          <ambientLight
            color={new THREE.Color(ambientLight.color[0], ambientLight.color[1], ambientLight.color[2])}
            intensity={ambientLight.intensity}
          />
        )}

        <Suspense fallback={<LoadingIndicator />}>
          {mapUrl ? (
            <MapModel url={mapUrl} onLoaded={handleMapLoaded} />
          ) : (
            <PlaceholderGrid />
          )}
        </Suspense>

        {showHeatmap && (
          <HeatmapLayer events={events} radius={heatmapRadius} intensity={heatmapIntensity} />
        )}

        <InstancedMarkers
          key={`markers-${markerColor}-${effectiveMarkerSize}-${groundSnap}`}
          events={events}
          selectedId={selectedEventId}
          onSelect={onSelectEvent}
          markerColor={markerColor}
          markerSize={effectiveMarkerSize}
          heightOffset={heightOffset}
          mapScene={mapScene}
          groundSnap={groundSnap}
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
