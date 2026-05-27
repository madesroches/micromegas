import { Suspense, useEffect, useLayoutEffect, useState, useMemo, useCallback } from 'react'
import { Canvas, useThree } from '@react-three/fiber'
import { useGLTF, Html, PerspectiveCamera } from '@react-three/drei'
import * as THREE from 'three'
import type { Overlay, OverlayConstants, Shape } from './overlay'
import { MapCameraController } from './MapCamera'
import { MapInstancedMarkers } from './MapInstancedMarkers'

export interface MMAmbientLight {
  color: [number, number, number]
  intensity: number
}

interface MapViewerProps {
  mapUrl: string
  overlay: Overlay
  constants: OverlayConstants
  shape: Shape
  selectedRowIndex: number | null
  onSelect: (rowIndex: number | null) => void
  onHover?: (rowIndex: number | null, clientX: number, clientY: number) => void
  resetViewTrigger?: number
}

// Override calculatePosition so drei doesn't project the 3D origin while the
// camera is still in its identity state — that projection produces NaN, which
// drei then writes into `transform:translate3d(NaNpx, NaNpx, 0)` and Firefox
// reports as a CSS parse error.
const centerOfViewport = (_el: THREE.Object3D, _camera: THREE.Camera, size: { width: number; height: number }): [number, number] =>
  [size.width / 2, size.height / 2]

function LoadingIndicator() {
  return (
    <Html calculatePosition={centerOfViewport} center>
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

  useLayoutEffect(() => {
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

function SceneSetup() {
  // scene.up only — camera.up is recomputed every frame from theta in
  // MapCameraController's useFrame, so setting it here would be dead.
  const { scene } = useThree()
  useEffect(() => {
    scene.up.set(0, 0, 1)
  }, [scene])
  return null
}

export function MapViewer({
  mapUrl,
  overlay,
  constants,
  shape,
  selectedRowIndex,
  onSelect,
  onHover,
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

  // Clear loaded-GLB state whenever mapUrl changes (including the A→B case
  // where both are truthy), so transient consumers (MapCameraController)
  // don't see stale scene state during the Suspense gap.
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

  // Gate marker/camera/ambient mounting until the GLB payload has propagated.
  // `mapScene` is set by handleMapLoaded together with mapBounds/glbCamera/
  // ambientLight, and cleared by the URL-change block above — it's the single
  // source of truth for "GLB payload arrived". Until it's set, the
  // <LoadingIndicator> fallback covers the canvas; nothing renders against the
  // default camera orbit, so no flash/snap.
  const ready = mapScene !== null

  return (
    <div className="w-full h-full">
      <Canvas
        gl={{
          antialias: true,
          alpha: false,
          toneMapping: THREE.NeutralToneMapping,
          toneMappingExposure: 1.3,
        }}
        onPointerMissed={() => onSelect(null)}
      >
        <SceneSetup />
        <color attach="background" args={['#0a0a0f']} />

        {/* Stays outside the ready gate so r3f always has a default camera
            registered, avoiding "no default camera" warnings during the
            not-ready window. Position and orientation are owned by
            MapCameraController (overwritten every frame from sphericalRef +
            targetRef). FOV/near/far are the seed for GLB-cameraless contracts;
            the GLB-camera effect copies intrinsics onto this camera when a
            conforming GLB loads. */}
        <PerspectiveCamera makeDefault fov={60} near={1} far={100000} />

        <Suspense fallback={<LoadingIndicator />}>
          <MapModel url={mapUrl} onLoaded={handleMapLoaded} />
        </Suspense>

        {ready && (
          <>
            <MapCameraController
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

            <MapInstancedMarkers
              overlay={overlay}
              constants={constants}
              shape={shape}
              selectedRowIndex={selectedRowIndex}
              onSelect={onSelect}
              onHover={onHover}
            />
          </>
        )}
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
        <div>Ctrl + Scroll: Zoom</div>
        <div>W/S: Up / Down</div>
        <div>A/D: Strafe</div>
        <div>Q/E: Forward / Back</div>
        <div>Z: Reset view</div>
      </div>
    </div>
  )
}
