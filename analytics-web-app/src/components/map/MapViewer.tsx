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

export interface MMAmbientLight {
  color: [number, number, number]
  intensity: number
}

interface MapViewerProps {
  mapUrl?: string
  events: MapEvent[]
  selectedEventId?: string
  onSelectEvent: (event: MapEvent | null) => void
  markerColor?: string
  markerSize?: number
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
}: InstancedMarkersProps) {
  const meshRef = useRef<THREE.InstancedMesh>(null)
  const [hoveredIndex, setHoveredIndex] = useState<number | null>(null)

  const tempObject = useMemo(() => new THREE.Object3D(), [])

  const geometry = useMemo(() => new THREE.SphereGeometry(1, 16, 16), [])
  // Overlay semantics: markers should remain visible regardless of where
  // they sit in Z relative to the map. Skip depth test/write and render
  // after the map.
  const material = useMemo(
    () =>
      new THREE.MeshBasicMaterial({ depthTest: false, depthWrite: false }),
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

    if (!colorAttrRef.current || colorAttrRef.current.count !== events.length) {
      const colorArray = new Float32Array(events.length * 3)
      colorAttrRef.current = new THREE.InstancedBufferAttribute(colorArray, 3)
      mesh.instanceColor = colorAttrRef.current
    }
    const attr = colorAttrRef.current

    for (let i = 0; i < events.length; i++) {
      const isSelected = i === selectedIndex
      const isHovered = i === hoveredIndex
      const scaleMultiplier = isSelected ? 1.5 : isHovered ? 1.2 : 1
      const finalScale = markerSize * scaleMultiplier

      const event = events[i]
      tempObject.position.set(event.x, event.y, event.z)
      tempObject.scale.setScalar(finalScale)
      tempObject.updateMatrix()
      mesh.setMatrixAt(i, tempObject.matrix)

      const c = isSelected ? COLOR_SELECTED : isHovered ? COLOR_HOVERED : normalColor
      attr.setXYZ(i, c.r, c.g, c.b)
    }

    // Required for correct raycasting and frustum culling on InstancedMesh:
    // the default bounding sphere comes from the unit geometry at origin, so
    // raycasts that miss the origin (i.e. most of them) skip every instance.
    mesh.computeBoundingSphere()
    mesh.instanceMatrix.needsUpdate = true
    attr.needsUpdate = true
  }, [events, selectedIndex, hoveredIndex, tempObject, markerColor, markerSize])

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
      renderOrder={10}
      onClick={handleClick}
      onPointerOver={handlePointerOver}
      onPointerOut={handlePointerOut}
    />
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
  resetViewTrigger: number
  glbCamera: THREE.PerspectiveCamera | null
}

function UnrealCameraController({
  mapBounds,
  mapScene,
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

  // WASD speed derives from the current orbit radius so flying feels the same
  // at every zoom level — one camera-to-target distance per ~2 seconds.
  const SPEED_PER_RADIUS = 0.5

  const fitRadiusRef = useRef(5000)
  const zoomFactorRef = useRef(1.0)

  const isLeftMouseDownRef = useRef(false)
  const isLeftDraggingRef = useRef(false)
  const leftMouseStartRef = useRef({ x: 0, y: 0 })
  const isRightMouseDownRef = useRef(false)
  const isHoveredRef = useRef(false)
  const lastMouseRef = useRef({ x: 0, y: 0 })
  const keysRef = useRef({
    w: false,
    a: false,
    s: false,
    d: false,
  })

  // Track latest mapScene through a ref so mousedown can raycast against it
  // without rebinding the DOM event handlers when the scene changes.
  const mapSceneRef = useRef<THREE.Object3D | null>(mapScene)
  useEffect(() => {
    mapSceneRef.current = mapScene
  }, [mapScene])

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

  const prevResetViewTriggerRef = useRef(resetViewTrigger)
  useEffect(() => {
    if (resetViewTrigger !== prevResetViewTriggerRef.current && initialViewRef.current) {
      prevResetViewTriggerRef.current = resetViewTrigger
      targetRef.current.copy(initialViewRef.current.target)
      sphericalRef.current.radius = initialViewRef.current.spherical.radius
      sphericalRef.current.phi = initialViewRef.current.spherical.phi
      sphericalRef.current.theta = initialViewRef.current.spherical.theta
      zoomFactorRef.current = 1.0
      // Restore fitRadius alongside the spherical state. The GLB seed set
      // fitRadius equal to the initial spherical.radius; without restoring
      // it, a stale fitRadius left over from a prior re-anchor would
      // cause the next wheel event to snap radius to fitRadius * zoomFactor.
      fitRadiusRef.current = initialViewRef.current.spherical.radius

      const offset = new THREE.Vector3()
      sphericalToZUpOffset(sphericalRef.current, offset)
      camera.position.copy(targetRef.current).add(offset)
      // Theta-driven camera.up: at phi=0 the spherical offset is parallel to
      // world-up, so a static (0,0,1) up makes lookAt degenerate. Deriving up
      // from theta keeps lookAt well-defined at every phi and makes theta
      // rotate the screen orientation when looking straight down.
      camera.up.set(-Math.sin(sphericalRef.current.theta), Math.cos(sphericalRef.current.theta), 0)
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
    camera.up.set(-Math.sin(sphericalRef.current.theta), Math.cos(sphericalRef.current.theta), 0)
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
            // Recompute zoomFactor against the existing scene-sized fitRadius,
            // preserving the invariant `radius = fitRadius * zoomFactor`. We
            // deliberately do not shrink fitRadius here: if we did, the new
            // zoomFactor would land at the upper cap and wheel-out would be
            // blocked when the re-anchor target is close to the camera.
            zoomFactorRef.current = sphericalRef.current.radius / fitRadiusRef.current
            targetRef.current.copy(newTarget)
          }
        }
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
      }
    }

    const panCamera = (deltaX: number, deltaY: number) => {
      const panSpeed = sphericalRef.current.radius * 0.001
      // Derive the horizontal pan basis from theta directly, not from
      // camera.getWorldDirection: at phi=0 the camera-forward is parallel to
      // worldUp, and cross(worldUp, cameraForward) collapses to zero, silently
      // dropping the input. Theta is well-defined at every phi.
      const theta = sphericalRef.current.theta
      const sinTheta = Math.sin(theta)
      const cosTheta = Math.cos(theta)
      const right = new THREE.Vector3(-cosTheta, -sinTheta, 0)
      const forward = new THREE.Vector3(-sinTheta, cosTheta, 0)

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
        // disorienting and rarely useful for a top-down scene. phi=0
        // (straight down) is allowed since the theta-driven camera.up keeps
        // lookAt well-defined there.
        sphericalRef.current.phi = Math.max(
          0,
          Math.min(Math.PI / 2 - 0.05, sphericalRef.current.phi)
        )
      }

    }

    const onWheel = (e: WheelEvent) => {
      e.preventDefault()

      const zoomSpeed = 0.1
      const zoomMultiplier = e.deltaY > 0 ? (1 + zoomSpeed) : (1 - zoomSpeed)
      // Wide clamp range so re-anchored zoomFactors (which can land far
      // outside [0.01, 1.0] when the new orbit pivot is close to the camera)
      // don't cause a single-step snap on the next wheel event, and so the
      // user can zoom out past the initial scene fit if they want.
      const newZoomFactor = Math.max(
        0.001,
        Math.min(10.0, zoomFactorRef.current * zoomMultiplier)
      )
      const oldRadius = sphericalRef.current.radius
      const newRadius = fitRadiusRef.current * newZoomFactor
      zoomFactorRef.current = newZoomFactor
      sphericalRef.current.radius = newRadius

      // Cursor-anchored zoom. Scale the orbit target around the world point
      // under the cursor; the camera follows in useFrame as
      // `target + sphericalOffset`. Because we only scaled the spherical
      // radius by s = newRadius/oldRadius, the cursor's world hit point
      // stays at the same screen location across the zoom step.
      const scene = mapSceneRef.current
      if (!scene || oldRadius === 0) return
      const rect = domElement.getBoundingClientRect()
      if (rect.width === 0 || rect.height === 0) return

      const ndc = new THREE.Vector2(
        ((e.clientX - rect.left) / rect.width) * 2 - 1,
        -((e.clientY - rect.top) / rect.height) * 2 + 1
      )
      const rc = new THREE.Raycaster()
      rc.setFromCamera(ndc, camera)
      const hits = rc.intersectObject(scene, true)
      if (hits.length === 0) return

      const anchor = hits[0].point
      const s = newRadius / oldRadius
      targetRef.current.sub(anchor).multiplyScalar(s).add(anchor)
    }

    const onContextMenu = (e: MouseEvent) => {
      e.preventDefault()
    }

    const isFormTarget = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null
      return !!t?.matches('input, textarea, select, [contenteditable="true"]')
    }

    const onKeyDown = (e: KeyboardEvent) => {
      if (isFormTarget(e)) return
      const key = e.key.toLowerCase()
      if (key in keysRef.current) {
        keysRef.current[key as keyof typeof keysRef.current] = true
      }
    }

    const onKeyUp = (e: KeyboardEvent) => {
      // Don't filter form targets on keyup — we need to release any key that
      // got pressed (e.g., focus moved into an input mid-hold).
      const key = e.key.toLowerCase()
      if (key in keysRef.current) {
        keysRef.current[key as keyof typeof keysRef.current] = false
      }
    }

    // Hover gates WASD/QE so multiple Map cells on a page don't all fly
    // together and so flying stops when the pointer leaves the canvas.
    const onMouseEnter = () => {
      isHoveredRef.current = true
    }
    const onMouseLeave = () => {
      isHoveredRef.current = false
      keysRef.current.w = false
      keysRef.current.a = false
      keysRef.current.s = false
      keysRef.current.d = false
    }

    // Safety net: if the browser/tab loses focus mid-drag (alt-tab, OS dialog),
    // the eventual mouseup may never reach us — clear all drag state so the
    // next interaction starts clean.
    const onWindowBlur = () => {
      if (isLeftDraggingRef.current || isRightMouseDownRef.current) {
        domElement.style.cursor = 'auto'
      }
      isLeftMouseDownRef.current = false
      isLeftDraggingRef.current = false
      isRightMouseDownRef.current = false
    }

    // mousedown stays on the canvas (drags only start over the map), but
    // mousemove/mouseup live on the window so a drag that sweeps off the
    // canvas keeps tracking and a release-outside isn't lost.
    domElement.addEventListener('mousedown', onMouseDown)
    window.addEventListener('mouseup', onMouseUp)
    window.addEventListener('mousemove', onMouseMove)
    domElement.addEventListener('wheel', onWheel, { passive: false })
    domElement.addEventListener('contextmenu', onContextMenu)
    domElement.addEventListener('mouseenter', onMouseEnter)
    domElement.addEventListener('mouseleave', onMouseLeave)
    window.addEventListener('keydown', onKeyDown)
    window.addEventListener('keyup', onKeyUp)
    window.addEventListener('blur', onWindowBlur)

    return () => {
      domElement.removeEventListener('mousedown', onMouseDown)
      window.removeEventListener('mouseup', onMouseUp)
      window.removeEventListener('mousemove', onMouseMove)
      domElement.removeEventListener('wheel', onWheel)
      domElement.removeEventListener('contextmenu', onContextMenu)
      domElement.removeEventListener('mouseenter', onMouseEnter)
      domElement.removeEventListener('mouseleave', onMouseLeave)
      window.removeEventListener('keydown', onKeyDown)
      window.removeEventListener('keyup', onKeyUp)
      window.removeEventListener('blur', onWindowBlur)
    }
  }, [camera, domElement])

  useFrame((_, delta) => {
    if (isHoveredRef.current) {
      const moveSpeed = sphericalRef.current.radius * SPEED_PER_RADIUS * delta

      const forward = new THREE.Vector3()
      camera.getWorldDirection(forward)
      forward.normalize()

      // Theta-based right, same reason as panCamera: cross(cameraForward, worldUp)
      // collapses to zero at phi=0 and silently drops A/D strafe.
      const theta = sphericalRef.current.theta
      const right = new THREE.Vector3(Math.cos(theta), Math.sin(theta), 0)

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
    }

    const offset = new THREE.Vector3()
    sphericalToZUpOffset(sphericalRef.current, offset)
    camera.position.copy(targetRef.current).add(offset)
    camera.up.set(-Math.sin(sphericalRef.current.theta), Math.cos(sphericalRef.current.theta), 0)
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
  // scene.up only — camera.up is recomputed every frame from theta in
  // UnrealCameraController's useFrame, so setting it here would be dead.
  const { scene } = useThree()
  useEffect(() => {
    scene.up.set(0, 0, 1)
  }, [scene])
  return null
}

export function MapViewer({
  mapUrl,
  events,
  selectedEventId,
  onSelectEvent,
  markerColor,
  markerSize,
  resetViewTrigger = 0,
}: MapViewerProps) {
  const [mapBounds, setMapBounds] = useState<THREE.Box3 | null>(null)
  const [mapScene, setMapScene] = useState<THREE.Object3D | null>(null)
  const [glbCamera, setGlbCamera] = useState<THREE.PerspectiveCamera | null>(null)
  const [ambientLight, setAmbientLight] = useState<MMAmbientLight | null>(null)
  const [contractErrors, setContractErrors] = useState<string[]>([])

  const handleMapLoaded = useCallback(
    (payload: MapLoadPayload) => {
      setMapBounds(payload.bounds)
      setMapScene(payload.scene)
      setGlbCamera(payload.glbCamera)
      setAmbientLight(payload.ambientLight)

      const errors: string[] = []
      if (payload.glbCamera === null) {
        errors.push('No perspective camera in GLB — initial framing is the default seed, and Reset View will not work.')
      }
      if (payload.ambientLight === null) {
        errors.push('No MM_ambient_light extension in GLB — scene will render without ambient illumination.')
      }
      setContractErrors(errors)

      for (const msg of errors) {
        console.error(`[MapViewer] ${mapUrl}: ${msg}`)
      }
    },
    [mapUrl]
  )

  const effectiveMarkerSize = useMemo(() => {
    if (mapBounds) {
      const size = mapBounds.getSize(new THREE.Vector3())
      const extent = Math.max(size.x, size.y)
      return (markerSize ?? DEFAULT_MARKER_SIZE) * extent * 0.00025
    }
    return markerSize ?? DEFAULT_MARKER_SIZE
  }, [markerSize, mapBounds])

  // Clear loaded-GLB state whenever mapUrl changes (including the A→B case
  // where both are truthy), so transient consumers (UnrealCameraController,
  // marker sizing from mapBounds) don't see stale scene state during the
  // Suspense gap.
  //
  // Done as a render-phase state derivation rather than an effect: an
  // effect-based clear races against MapModel's load effect when the new GLB
  // is already in drei's useGLTF cache (no Suspense). useEffects fire
  // child-first then parent, so the child's onLoaded → setState(payload) runs
  // before the parent's setState(null), and React 18 auto-batches both into
  // one commit where the parent's null wins. The render-phase form forces
  // the clear before MapModel re-renders, so the child effect runs against
  // already-cleared state.
  const [clearedForUrl, setClearedForUrl] = useState(mapUrl)
  if (clearedForUrl !== mapUrl) {
    setClearedForUrl(mapUrl)
    setMapBounds(null)
    setMapScene(null)
    setGlbCamera(null)
    setAmbientLight(null)
    setContractErrors([])
  }

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

        {/* Position and orientation are owned by UnrealCameraController (overwritten
            every frame from sphericalRef + targetRef). FOV/near/far are the seed
            for GLB-cameraless contracts; the GLB-camera effect copies intrinsics
            onto this camera when a conforming GLB loads. */}
        <PerspectiveCamera makeDefault fov={60} near={1} far={100000} />

        <UnrealCameraController
          mapBounds={mapBounds}
          mapScene={mapScene}
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

        <InstancedMarkers
          events={events}
          selectedId={selectedEventId}
          onSelect={onSelectEvent}
          markerColor={markerColor}
          markerSize={effectiveMarkerSize}
        />
      </Canvas>

      {contractErrors.length > 0 && (
        <div className="absolute top-4 left-1/2 -translate-x-1/2 max-w-2xl bg-red-900/90 border border-red-500 rounded-lg px-4 py-3 text-sm text-red-50 shadow-lg">
          <div className="font-semibold text-base mb-1">GLB does not satisfy renderer contract</div>
          <div className="text-xs text-red-200 mb-2 font-mono break-all">{mapUrl}</div>
          <ul className="list-disc pl-5 space-y-1">
            {contractErrors.map((msg, i) => (
              <li key={i}>{msg}</li>
            ))}
          </ul>
        </div>
      )}

      <div className="absolute bottom-4 right-4 bg-app-bg border border-theme-border rounded-lg px-3 py-2 text-xs text-theme-text-muted">
        <div className="font-semibold text-theme-text-secondary mb-1">Controls</div>
        <div>Left-click + drag: Pan</div>
        <div>Right-click + drag: Rotate</div>
        <div>Scroll: Zoom</div>
        <div>WASD: Fly</div>
        <div>Z: Reset view</div>
      </div>
    </div>
  )
}
